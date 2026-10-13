//! Research tool: per-slot loudness / loop-seam statistics of a level's music table.
//! Usage: track_analyze [scene-base]   (default Scenes/M01/M01_main)
use bm_music::{AudioSource, GameResolver, LevelMusic, OpenContext, TrackDef, TrackResolver};
use bm_sound::SoundLibrary;
use bm_vfs::Vfs;
use std::sync::Arc;

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn rms(src: &mut dyn AudioSource, start: u64, frames: usize) -> f32 {
    let ch = src.channels() as usize;
    let mut buf = vec![0.0f32; frames * ch];
    let n = src.read(start, &mut buf);
    if n == 0 {
        return 0.0;
    }
    (buf[..n * ch].iter().map(|v| v * v).sum::<f32>() / (n * ch) as f32).sqrt()
}

fn main() {
    let base = std::env::args().nth(1).unwrap_or("Scenes/M01/M01_main".into());
    let vfs = Arc::new(Vfs::from_env().expect("game dir"));
    let lib = Arc::new(SoundLibrary::open_level(vfs.clone(), &base).expect("bank"));
    let lm = LevelMusic::load(&vfs, &lib, &base, false).unwrap();
    let res = GameResolver::new(vfs.clone(), lib.clone(), &lm.level, lm.slot_files());
    println!("slot  pct mode   dur   rate ch  rms_dB trimmed_dB  peak  head_dB tail_dB  seam/med  file");
    for s in lm.slots.iter().flatten() {
        let Some(file) = &s.file else { println!("{:>3}  (no bank file) {}", s.slot, s.sound); continue };
        let t = TrackDef::bank(file);
        let ctx = OpenContext { cue: "x".into(), stem: "main".into(), primary: true, track_key: "x".into() };
        let mut src = match res.open(&ctx, &t) { Ok(s) => s, Err(e) => { println!("{:>3} {e}", s.slot); continue } };
        let len = src.len_frames();
        let rate = src.sample_rate();
        let ch = src.channels() as usize;
        // mean rms over 24 windows of 0.5 s
        let win = (rate / 2) as usize;
        let mut acc = 0.0;
        let mut cnt = 0;
        for k in 0..24u64 {
            let st = (len.saturating_sub(win as u64)) * k / 23;
            let r = rms(&mut *src, st, win);
            acc += r * r;
            cnt += 1;
        }
        let mean = (acc / cnt as f32).sqrt();
        let mut peak = 0.0f32;
        let chunk = rate as usize * 4;
        let mut buf = vec![0.0f32; chunk * ch];
        let mut p = 0u64;
        // peak over a sparse sample (every 8th second)
        while p < len {
            let n = src.read(p, &mut buf[..(rate as usize / 2) * ch]);
            peak = buf[..n * ch].iter().fold(peak, |m, v| m.max(v.abs()));
            p += rate as u64 * 8;
        }
        let head = rms(&mut *src, 0, win);
        let tail = rms(&mut *src, len.saturating_sub(win as u64), win);
        // seam: |last - first| vs median |dx| in the first second
        let mut a = vec![0.0f32; 2048 * ch];
        let mut z = vec![0.0f32; 2048 * ch];
        let na = src.read(0, &mut a);
        let nz = src.read(len - 2048, &mut z);
        let mut d: Vec<f32> = (1..na).map(|i| (a[i * ch] - a[(i - 1) * ch]).abs()).collect();
        d.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let med = d[d.len() / 2].max(1e-6);
        let seam = (a[0] - z[(nz - 1) * ch]).abs();
        println!(
            "{:>3} {:>4} {:>3} {:>6.1} {:>6} {} {:>7.1} {:>8.1} {:>7.2} {:>7.1} {:>7.1} {:>9.1}  {}",
            s.slot, s.volume_pct, s.mode, len as f32 / rate as f32, rate, ch, db(mean), db(mean * s.volume_pct as f32 / 100.0), peak, db(head), db(tail), seam / med, file
        );
    }
}
