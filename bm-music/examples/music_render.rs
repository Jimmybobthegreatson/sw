//! Renders a scripted music session of a level offscreen through the headless mixer, writes a WAV and
//! numerically checks continuity at every state transition (no clicks) plus loudness sanity.
//!
//! ```text
//! cargo run --release -p bm-music --example music_render -- [scene] [out.wav] [seconds] [--mod <dir>]
//! cargo run --release -p bm-music --example music_render -- --loops [scene]     # check loop wraps of all looping tracks
//! ```
//! Default script (all event names come from the level's generated definition): explore -> combat ->
//! calm -> suspicious -> alert -> calm, a pause/resume, and a stinger. Exit code 1 on a failed check.
use bm_music::analysis::{basic_stats, check_boundaries};
use bm_music::{load_level_def, CueDef, MissionEvent, MusicDef, MusicEvent, MusicOptions, MusicSystem, StateDef, TrackDef};
use bm_sound::{MixerEngine, SoundLibrary};
use bm_vfs::Vfs;
use std::sync::Arc;

const RATE: u32 = 48_000;

fn main() {
    let mut scene = "Scenes/M01/M01_main".to_string();
    let mut out_path = "music_render.wav".to_string();
    let mut seconds = 70.0f32;
    let mut loops = false;
    let mut vfs = Vfs::from_env().expect("game dir");
    let mut pos = 0;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--loops" => loops = true,
            "--mod" => vfs.add_mod_dir(it.next().expect("--mod <dir>")).expect("mod dir"),
            s => {
                match pos {
                    0 => scene = s.to_string(),
                    1 => out_path = s.to_string(),
                    _ => seconds = s.parse().expect("seconds"),
                }
                pos += 1;
            }
        }
    }
    let vfs = Arc::new(vfs);
    let lib = Arc::new(SoundLibrary::open_level(vfs.clone(), &scene).expect("sound bank"));
    let set = load_level_def(vfs, lib, &scene).expect("level music");
    for w in &set.warnings {
        eprintln!("warning: {w}");
    }
    let ok = if loops { check_loops(&set) } else { run_script(&set, &out_path, seconds) };
    std::process::exit(if ok { 0 } else { 1 });
}

fn run_script(set: &bm_music::LevelMusicSet, out_path: &str, seconds: f32) -> bool {
    let mut engine = MixerEngine::headless(RATE);
    let mut sys = set.system(MusicOptions::offline(RATE)).expect("music system");
    sys.attach(&mut engine).expect("attach");

    // (time, action)
    let script: Vec<(f64, MissionEvent)> = vec![
        (10.0, MissionEvent::Combat),
        (28.0, MissionEvent::Calm),
        (38.0, MissionEvent::Suspicious),
        (48.0, MissionEvent::Alert),
        (58.0, MissionEvent::Calm),
    ];
    let mut next = 0;
    let mut paused_at = false;
    let mut resumed = false;
    let mut events: Vec<(f64, MusicEvent)> = Vec::new();
    let t0 = std::time::Instant::now();
    let out = engine.render_offline(seconds, 480, |t, _| {
        while next < script.len() && t >= script[next].0 {
            sys.mission_event(script[next].1);
            next += 1;
        }
        if !paused_at && t >= 63.0 {
            sys.pause(0.3);
            paused_at = true;
        }
        if !resumed && t >= 65.0 {
            sys.resume(0.3);
            resumed = true;
        }
        for e in sys.poll_events() {
            events.push((t, e));
        }
    });
    let wall = t0.elapsed().as_secs_f32();
    MixerEngine::write_wav(out_path, &out, RATE).expect("write wav");

    println!("scene {} -> {out_path}: {:.1}s audio rendered in {wall:.2}s ({:.0}x realtime)", set.level.scene_base, seconds, seconds / wall.max(1e-3));
    let mut change_frames = Vec::new();
    for (t, e) in &events {
        match e {
            MusicEvent::StateChanged { from, to } => {
                println!("  {t:7.2}s state {from:?} -> {to:?}");
                change_frames.push((*t * RATE as f64) as usize);
            }
            MusicEvent::CueStarted { cue } => println!("  {t:7.2}s cue started {cue}"),
            MusicEvent::CueFinished { cue } => println!("  {t:7.2}s cue finished {cue}"),
            MusicEvent::StingerStarted { stinger } => println!("  {t:7.2}s stinger {stinger}"),
            MusicEvent::TrackError { cue, stem, message } => println!("  {t:7.2}s TRACK ERROR {cue}/{stem}: {message}"),
            MusicEvent::Ignored { what } => println!("  {t:7.2}s ignored {what}"),
            MusicEvent::TransitionScheduled { .. } => {}
        }
    }
    let (peak, dc, bad) = basic_stats(&out);
    let st = sys.stats();
    println!("peak {peak:.3}  dc {dc:+.5}  non-finite {bad}  underruns {}", st.underruns);

    // transitions fire at the first block after the event; check +-0.3 s around each, fades are 1..5 s
    let mut ok = !bad && peak < 1.0 && st.underruns == 0;
    let reports = check_boundaries(&out, &change_frames, (0.3 * RATE as f32) as usize, 0.999, 3.0);
    for r in &reports {
        println!("  boundary @ {:6.2}s  max step {:.4}  ref(p99.9) {:.4}  ratio {:.2}  {}", r.frame as f64 / RATE as f64, r.max_step, r.reference, r.ratio, if r.ok { "ok" } else { "CLICK" });
        ok &= r.ok;
    }
    // the final second must be silent only if we paused
    println!("{}", if ok { "PASS" } else { "FAIL" });
    ok
}

/// Renders the last/first seconds around the loop wrap of every looping track and checks the seam.
fn check_loops(set: &bm_music::LevelMusicSet) -> bool {
    let mut all_ok = true;
    for (key, t) in &set.def.tracks {
        if !t.looping {
            continue;
        }
        // use the real length through the resolver
        let ctx = bm_music::OpenContext { cue: "probe".into(), stem: "main".into(), primary: true, track_key: key.clone() };
        let src = match bm_music::TrackResolver::open(&*set.resolver, &ctx, t) {
            Ok(s) => s,
            Err(e) => {
                println!("{key:<32} cannot open: {e}");
                continue;
            }
        };
        let len = src.len_frames() as f64 / src.sample_rate() as f64;
        let start = (len - 3.0).max(0.0);
        let mut d = MusicDef::default();
        d.tracks.insert(key.clone(), TrackDef { gain: 1.0, ..t.clone() });
        d.cues.insert("c".into(), CueDef { start_secs: start, ..CueDef::single(key) });
        d.states.insert("s".into(), StateDef::cue("c"));
        d.initial_state = "s".into();
        let sys = MusicSystem::new(d, set.resolver.clone(), MusicOptions::offline(RATE)).unwrap();
        let out = sys.render(RATE as usize * 8);
        // the wrap happens (len - start - xfade) after the start
        let wrap = ((len - start - t.loop_crossfade_secs as f64) * RATE as f64) as usize;
        let r = &check_boundaries(&out, &[wrap], (0.02 * RATE as f32) as usize, 0.9999, 3.0)[0];
        println!("{key:<32} len {len:7.1}s xfade {:.1}s  wrap step {:.4}  ref {:.4}  ratio {:5.2}  {}", t.loop_crossfade_secs, r.max_step, r.reference, r.ratio, if r.ok { "ok" } else { "CLICK" });
        all_ok &= r.ok;
    }
    println!("{}", if all_ok { "PASS" } else { "FAIL" });
    all_ok
}
