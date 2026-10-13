//! Tests against the user's own game install. They skip (pass with a note) when it is missing.
use bm_music::analysis::{basic_stats, check_boundaries};
use bm_music::*;
use bm_sound::pcm::wav_i16;
use bm_sound::{MixerEngine, SoundLibrary};
use bm_vfs::Vfs;
use std::sync::Arc;

const RATE: u32 = 48_000;

fn game() -> Option<Arc<Vfs>> {
    bm_vfs::game_dir_if_present()?;
    Vfs::from_env().ok().map(Arc::new)
}

fn open(vfs: &Arc<Vfs>, scene: &str) -> LevelMusicSet {
    let lib = Arc::new(SoundLibrary::open_level(vfs.clone(), scene).unwrap());
    load_level_def(vfs.clone(), lib, scene).unwrap()
}

#[test]
fn every_main_level_has_a_music_table_and_a_valid_playable_default() {
    let Some(vfs) = game() else { return eprintln!("game dir missing, skipping") };
    let mut mains: Vec<String> = vfs.list("Scenes/M*/M*_main.WHD").into_iter().map(|e| e.path[..e.path.len() - 4].to_string()).collect();
    mains.sort();
    assert!(mains.len() >= 12, "{mains:?}");
    for scene in mains {
        let set = open(&vfs, &scene);
        let files = set.level.slots.iter().flatten().filter(|s| s.file.is_some()).count();
        assert!(files >= 40, "{scene}: only {files} slot files");
        // the stable slots
        assert!(set.level.slot(2).and_then(|s| s.file.clone()).unwrap().contains("Exploration"), "{scene}");
        set.def.validate().unwrap_or_else(|e| panic!("{scene}: {e:?}"));
        assert!(set.warnings.is_empty(), "{:?}", set.warnings);
        assert!(set.def.states.contains_key("explore") && set.def.states.contains_key("combat"), "{scene}");
        // every generated track opens and has audio
        for (key, t) in &set.def.tracks {
            let ctx = OpenContext { cue: "probe".into(), stem: "main".into(), primary: true, track_key: key.clone() };
            let src = set.resolver.open(&ctx, t).unwrap_or_else(|e| panic!("{scene} {key}: {e}"));
            assert!(src.len_frames() > 44_100, "{scene} {key}");
        }
        // every zone of the scene maps to a state
        for z in &set.level.zones {
            assert!(set.def.zones.contains_key(&z.id), "{scene} zone {}", z.id);
        }
    }
}

#[test]
fn streamed_source_is_identical_to_the_sound_library_decode() {
    let Some(vfs) = game() else { return eprintln!("game dir missing, skipping") };
    let lib = Arc::new(SoundLibrary::open_level(vfs.clone(), "Scenes/M01/M01_main").unwrap());
    let set = load_level_def(vfs.clone(), lib.clone(), "Scenes/M01/M01_main").unwrap();
    let id = lib.find("Music/Exploration.wav").unwrap();
    let pcm = lib.pcm(id).unwrap();
    let ctx = OpenContext { cue: "c".into(), stem: "main".into(), primary: true, track_key: "exploration".into() };
    let mut src = set.resolver.open(&ctx, &set.def.tracks["exploration"]).unwrap();
    assert_eq!(src.len_frames() as usize, pcm.frames());
    assert_eq!(src.sample_rate(), 44_100);
    let mut buf = vec![0.0f32; 4096 * 2];
    for start in [0usize, 1_000_000, pcm.frames() - 4096] {
        assert_eq!(src.read(start as u64, &mut buf), 4096);
        for i in 0..4096 {
            assert!((buf[i * 2] - pcm.sample_f32(start + i, 0)).abs() < 1e-6);
            assert!((buf[i * 2 + 1] - pcm.sample_f32(start + i, 1)).abs() < 1e-6);
        }
    }
}

#[test]
fn scripted_session_renders_without_clicks_or_underruns() {
    let Some(vfs) = game() else { return eprintln!("game dir missing, skipping") };
    let set = open(&vfs, "Scenes/M01/M01_main");
    let mut engine = MixerEngine::headless(RATE);
    let mut sys = set.system(MusicOptions::offline(RATE)).unwrap();
    sys.attach(&mut engine).unwrap();
    let script = [(5.0, MissionEvent::Combat), (15.0, MissionEvent::Calm), (22.0, MissionEvent::Suspicious), (30.0, MissionEvent::Alert)];
    let mut next = 0;
    let mut changes = Vec::new();
    let out = engine.render_offline(40.0, 480, |t, _| {
        while next < script.len() && t >= script[next].0 {
            assert!(sys.mission_event(script[next].1));
            next += 1;
        }
        for e in sys.poll_events() {
            if matches!(e, MusicEvent::StateChanged { .. }) {
                changes.push((t * RATE as f64) as usize);
            }
        }
    });
    let (peak, _dc, bad) = basic_stats(&out);
    assert!(!bad && peak < 1.0 && peak > 0.05, "peak {peak}");
    assert_eq!(sys.stats().underruns, 0);
    assert!(changes.len() >= 5, "{changes:?}");
    for r in check_boundaries(&out, &changes, (0.3 * RATE as f32) as usize, 0.999, 3.0) {
        assert!(r.ok, "click at {:.2}s: {r:?}", r.frame as f64 / RATE as f64);
    }
    assert_eq!(sys.state(), "alert");
}

#[test]
fn looping_party_tracks_wrap_without_a_click() {
    let Some(vfs) = game() else { return eprintln!("game dir missing, skipping") };
    let set = open(&vfs, "Scenes/M06/M06_main");
    for key in ["m06_salsa_bomba", "m06_rock_white_noise", "m06_blues_double_trouble", "sniperloopslow"] {
        let t = set.def.tracks[key].clone();
        assert!(t.looping, "{key}");
        let ctx = OpenContext { cue: "p".into(), stem: "main".into(), primary: true, track_key: key.into() };
        let src = set.resolver.open(&ctx, &t).unwrap();
        let len = src.len_frames() as f64 / src.sample_rate() as f64;
        let start = len - 3.0;
        let mut d = MusicDef::default();
        d.tracks.insert(key.into(), t.clone());
        d.cues.insert("c".into(), CueDef { start_secs: start, ..CueDef::single(key) });
        d.states.insert("s".into(), StateDef::cue("c"));
        d.initial_state = "s".into();
        let sys = MusicSystem::new(d, set.resolver.clone(), MusicOptions::offline(RATE)).unwrap();
        let out = sys.render(RATE as usize * 8);
        let wrap = ((len - start - t.loop_crossfade_secs as f64) * RATE as f64) as usize;
        let r = &check_boundaries(&out, &[wrap], (0.02 * RATE as f32) as usize, 0.9999, 3.0)[0];
        assert!(r.ok, "{key}: {r:?}");
    }
}

#[test]
fn threaded_streaming_of_real_music_keeps_up() {
    let Some(vfs) = game() else { return eprintln!("game dir missing, skipping") };
    let set = open(&vfs, "Scenes/M01/M01_main");
    let sys = set.system(MusicOptions { sample_rate: RATE, threaded: true, ..Default::default() }).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(400));
    let mut out = Vec::new();
    for i in 0..300 {
        out.extend(sys.render(512));
        if i == 100 {
            sys.mission_event(MissionEvent::Combat);
        }
        std::thread::sleep(std::time::Duration::from_millis(4)); // faster than real time, far slower than the reader
    }
    assert_eq!(sys.stats().underruns, 0, "audio thread starved");
    let (peak, _, bad) = basic_stats(&out);
    assert!(!bad && peak > 0.02);
    assert_eq!(sys.state(), "combat");
}

#[test]
fn loose_file_replaces_an_original_track_on_a_real_level() {
    let Some(base) = game() else { return eprintln!("game dir missing, skipping") };
    let modd = std::env::temp_dir().join(format!("bm_music_realmod_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&modd);
    std::fs::create_dir_all(modd.join("Music/M01")).unwrap();
    let tone: Vec<i16> = (0..RATE as usize * 10).flat_map(|i| { let s = ((i as f32 * 2.0 * std::f32::consts::PI * 523.25 / RATE as f32).sin() * 12000.0) as i16; [s, s] }).collect();
    std::fs::write(modd.join("Music/M01/exploration.wav"), wav_i16(&tone, 2, RATE)).unwrap();
    std::fs::write(modd.join("Music/M01/zz.ron"), b"( states: { \"mine\": (cue: \"exploration\") } )").unwrap();
    let mut vfs = Vfs::new(base.game_dir()).unwrap();
    vfs.add_mod_dir(&modd).unwrap();
    let set = open(&Arc::new(vfs), "Scenes/M01/M01_main");
    assert_eq!(set.merged_files, vec!["Music/M01/zz.ron".to_string()]);
    assert!(set.def.states.contains_key("mine"));
    let sys = set.system(MusicOptions::offline(RATE)).unwrap();
    let out = sys.render(RATE as usize);
    let n = RATE as usize / 2;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    let w = 2.0 * std::f64::consts::PI * 523.25 / RATE as f64;
    for i in 0..n {
        let x = out[(RATE as usize / 4 + i) * 2] as f64;
        re += x * (w * (RATE as usize / 4 + i) as f64).cos();
        im += x * (w * (RATE as usize / 4 + i) as f64).sin();
    }
    let a = 2.0 * (re * re + im * im).sqrt() / n as f64;

    // the original trim (8/39) must not be applied to a replacement: amplitude = 12000/32768
    assert!((a - 12000.0 / 32768.0).abs() < 0.03, "replacement tone amplitude {a}");
    let _ = std::fs::remove_dir_all(modd);
}

#[test]
fn scenes_without_a_music_table_fall_back_to_their_bank_music() {
    let Some(vfs) = game() else { return eprintln!("game dir missing, skipping") };
    let set = open(&vfs, "Scenes/HitmanBloodMoney");
    assert!(set.level.def_name.is_none());
    assert_eq!(set.def.initial_state, "47_main_theme");
    assert!(set.def.states.contains_key("13_schubert_ave_maria"));
    let sys = set.system(MusicOptions::offline(RATE)).unwrap();
    let out = sys.render(RATE as usize * 2);
    assert!(basic_stats(&out).0 > 0.05);
    // and every scene of the install yields a valid definition
    let mut n = 0;
    for e in vfs.list("Scenes/**/*.WHD") {
        let base = e.path[..e.path.len() - 4].to_string();
        let Ok(lib) = SoundLibrary::open_level(vfs.clone(), &base) else { continue };
        let set = load_level_def(vfs.clone(), Arc::new(lib), &base).unwrap_or_else(|e| panic!("{base}: {e}"));
        set.def.validate().unwrap_or_else(|e| panic!("{base}: {e:?}"));
        n += 1;
    }
    assert!(n >= 50, "only {n} scenes");
}
