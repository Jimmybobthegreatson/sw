//! List / inspect / dump / play sounds of a level bank.
//!
//! ```text
//! play_sound list   [scene] [filter]          list bank entries (case-insensitive substring filter)
//! play_sound info   <scene> <name|#index>     details, SND wave record, lip sync summary
//! play_sound dump   <scene> <out_dir> <name|#index>...   decode to 16-bit WAV files + numeric sanity report
//! play_sound sample <scene> <out_dir> [n]     dump n evenly spaced sounds (default 6) incl. one streamed line
//! play_sound play   <scene> <name|#index>     play on the default device (needs --features device)
//! ```
//! `scene` defaults to `Scenes/M01/M01_main`. Mods are honoured: set `BM_MOD_DIR` to a directory.
use bm_sound::pcm::AudioStats;
use bm_sound::{SoundId, SoundLibrary};
use std::path::PathBuf;
use std::sync::Arc;

fn open(scene: &str) -> SoundLibrary {
    let mut vfs = bm_vfs::Vfs::from_env().expect("game dir (BM_GAME_DIR)");
    if let Some(m) = std::env::var_os("BM_MOD_DIR") {
        vfs.add_mod_dir(PathBuf::from(m)).expect("mod dir");
    }
    SoundLibrary::open_level(Arc::new(vfs), scene).expect("sound bank")
}

fn resolve(lib: &SoundLibrary, key: &str) -> SoundId {
    if let Some(i) = key.strip_prefix('#') {
        return SoundId { bank: 0, index: i.parse().expect("index") };
    }
    lib.find(key).unwrap_or_else(|| {
        let hits = lib.search(key);
        match hits.first() {
            Some(h) => {
                eprintln!("no exact match, using {}", h.name);
                h.id
            }
            None => panic!("no sound named {key}"),
        }
    })
}

fn report(lib: &SoundLibrary, id: SoundId, out: Option<&std::path::Path>) {
    let info = lib.info(id).unwrap();
    let pcm = match lib.pcm(id) {
        Ok(p) => p,
        Err(e) => {
            println!("#{:<5} {:<60} ERROR {e}", id.index, info.name);
            return;
        }
    };
    let st = AudioStats::of_pcm(&pcm);
    println!(
        "#{:<5} {:<60} {:>6} Hz x{} {:>7.2}s frames {:>8} (bank says {:>8}) peak {:.3} rms {:.3} clip {:.4} active {:.2}{}",
        id.index,
        info.name,
        pcm.sample_rate,
        pcm.channels,
        pcm.duration_secs(),
        pcm.frames(),
        info.frames,
        st.peak,
        st.rms,
        st.clipped_fraction,
        st.active_fraction,
        if info.streamed { "  [stream]" } else { "" }
    );
    if let Some(dir) = out {
        std::fs::create_dir_all(dir).unwrap();
        let fname = info.name.replace(['/', '\\'], "__");
        let p = dir.join(format!("{}.wav", fname.trim_end_matches(".wav")));
        std::fs::write(&p, pcm.to_wav()).unwrap();
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let cmd = a.get(1).map(String::as_str).unwrap_or("help");
    let scene = a.get(2).cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    match cmd {
        "list" => {
            let lib = open(&scene);
            let filter = a.get(3).cloned().unwrap_or_default();
            let mut n = 0;
            for i in lib.search(&filter) {
                println!("#{:<5} {:<64} {:>6} Hz x{} {:>8.2}s{}", i.id.index, i.name, i.sample_rate, i.channels, i.duration, if i.streamed { " [stream]" } else { "" });
                n += 1;
            }
            println!("{n} sounds");
        }
        "info" => {
            let lib = open(&scene);
            let id = resolve(&lib, a.get(3).expect("name"));
            let info = lib.info(id).unwrap();
            println!("{info:#?}");
            report(&lib, id, None);
            if let Some(l) = lib.lip_sync(id).unwrap() {
                println!("lip sync: {} channels, {} keys, last key {:.2}s", l.channels.len(), l.channels.iter().map(|c| c.keys.len()).sum::<usize>(), l.duration_secs());
                for c in &l.channels {
                    println!("  channel {:>2}: {} keys, max weight {}", c.id, c.keys.len(), c.keys.iter().map(|k| k.weight).max().unwrap_or(0));
                }
            }
            if let Some(db) = lib.snd(0) {
                if let Some(w) = db.waves.iter().find(|w| w.file.eq_ignore_ascii_case(&info.name)) {
                    let s = &db.objects.iter().find(|o| o.name_off == w.sound_name_off).unwrap();
                    println!("SND: wave record duration {:.3}s, owned by sound '{}'", w.duration, s.path);
                    if let Some(p) = db.sound_params(s) {
                        println!("     sound params {p:?}");
                    }
                }
            }
        }
        "dump" => {
            let lib = open(&scene);
            let out = PathBuf::from(a.get(3).expect("out dir"));
            for k in &a[4..] {
                report(&lib, resolve(&lib, k), Some(&out));
            }
        }
        "sample" => {
            let lib = open(&scene);
            let out = PathBuf::from(a.get(3).expect("out dir"));
            let n: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(6);
            let all: Vec<_> = lib.iter().collect();
            let step = (all.len() / n.max(1)).max(1);
            for i in all.iter().step_by(step).take(n) {
                report(&lib, i.id, Some(&out));
            }
            if let Some(d) = all.iter().find(|i| i.name.to_ascii_lowercase().starts_with("dialog") && i.streamed) {
                report(&lib, d.id, Some(&out));
            }
        }
        #[cfg(feature = "device")]
        "play" => {
            let lib = Arc::new(open(&scene));
            let id = resolve(&lib, a.get(3).expect("name"));
            let (engine, _dev) = bm_sound::device::DeviceOutput::start_default().expect("audio device");
            let mut player = bm_sound::SoundPlayer::new(lib.clone(), Box::new(engine));
            let h = player.play_id(id, &Default::default()).unwrap().expect("voice");
            while bm_sound::AudioEngine::is_playing(&*player.engine, h) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        #[cfg(not(feature = "device"))]
        "play" => eprintln!("rebuild with `--features device` to play on an output device; use `dump` to write WAV files"),
        _ => eprintln!("usage: play_sound list|info|dump|sample|play ... (see the file header)"),
    }
}
