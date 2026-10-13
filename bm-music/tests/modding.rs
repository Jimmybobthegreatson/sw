//! Loose-file music modding on a synthetic game dir + mod dirs (no game files needed).
use bm_music::*;
use bm_sound::pcm::wav_i16;
use bm_sound::SoundLibrary;
use bm_vfs::Vfs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const RATE: u32 = 48_000;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bm_music_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn tone_wav(freq: f32, secs: f32, amp: f32) -> Vec<u8> {
    let n = (secs * RATE as f32) as usize;
    let mut v = Vec::new();
    for i in 0..n {
        let s = ((2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin() * amp * 32767.0) as i16;
        v.push(s);
        v.push(s);
    }
    wav_i16(&v, 2, RATE)
}

fn amp(x: &[f32], freq: f32) -> f32 {
    let n = x.len() / 2;
    let w = 2.0 * std::f64::consts::PI * freq as f64 / RATE as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for i in 0..n {
        re += x[i * 2] as f64 * (w * i as f64).cos();
        im += x[i * 2] as f64 * (w * i as f64).sin();
    }
    (2.0 * (re * re + im * im).sqrt() / n as f64) as f32
}

fn vfs_with(game: &Path, mods: &[&Path]) -> Arc<Vfs> {
    let mut v = Vfs::new(game).unwrap();
    for m in mods {
        v.add_mod_dir(m).unwrap();
    }
    Arc::new(v)
}

fn base_def() -> MusicDef {
    let mut d = MusicDef::default();
    // the original files do not exist in this synthetic setup: only loose files can satisfy them
    d.tracks.insert("calm_t".into(), TrackDef::bank("Music/Calm.wav"));
    d.tracks.insert("fight_t".into(), TrackDef::bank("Music/Fight.wav"));
    d.cues.insert("calm".into(), CueDef::single("calm_t"));
    d.cues.insert(
        "fight".into(),
        CueDef {
            stems: vec![
                StemDef { name: "main".into(), track: "fight_t".into(), ..Default::default() },
                StemDef { name: "drums".into(), track: "fight_t".into(), ..Default::default() },
            ],
            ..Default::default()
        },
    );
    d.states.insert("calm".into(), StateDef::cue("calm"));
    d.states.insert("fight".into(), StateDef::cue("fight"));
    d.initial_state = "calm".into();
    d.default_transition = TransitionRule::crossfade(0.05);
    d
}

fn resolver(vfs: &Arc<Vfs>) -> Arc<GameResolver> {
    Arc::new(GameResolver::new(vfs.clone(), Arc::new(SoundLibrary::new(vfs.clone())), "M01", vec![]))
}

#[test]
fn cue_replacement_by_cue_name_and_stem_name() {
    let game = tmpdir("g1");
    let m = tmpdir("m1");
    write(&m, "Music/M01/calm.wav", &tone_wav(440.0, 1.0, 0.5));
    write(&m, "Music/M01/fight.wav", &tone_wav(660.0, 1.0, 0.5)); // primary stem "main"
    write(&m, "Music/M01/fight.drums.wav", &tone_wav(990.0, 1.0, 0.25));
    let vfs = vfs_with(&game, &[&m]);
    let sys = MusicSystem::new(base_def(), resolver(&vfs), MusicOptions::offline(RATE)).unwrap();
    let out = sys.render(RATE as usize / 2);
    assert!((amp(&out[RATE as usize / 5 * 2..], 440.0) - 0.5).abs() < 0.02);
    sys.set_state("fight");
    sys.render(RATE as usize / 5);
    let out = sys.render(RATE as usize / 2);
    assert!((amp(&out, 660.0) - 0.5).abs() < 0.02, "{}", amp(&out, 660.0));
    assert!((amp(&out, 990.0) - 0.25).abs() < 0.02, "{}", amp(&out, 990.0));
    assert!(sys.poll_events().iter().all(|e| !matches!(e, MusicEvent::TrackError { .. })));
    let _ = (std::fs::remove_dir_all(game), std::fs::remove_dir_all(m));
}

#[test]
fn level_specific_wins_over_global_and_later_mod_wins() {
    let game = tmpdir("g2");
    let a = tmpdir("a2");
    let b = tmpdir("b2");
    write(&a, "Music/_global/calm_t.wav", &tone_wav(300.0, 1.0, 0.5)); // by track key, any level
    write(&a, "Music/M01/calm_t.wav", &tone_wav(400.0, 1.0, 0.5)); // level specific
    write(&b, "Music/M01/calm_t.wav", &tone_wav(500.0, 1.0, 0.5)); // later mod overrides earlier
    let vfs = vfs_with(&game, &[&a, &b]);
    let sys = MusicSystem::new(base_def(), resolver(&vfs), MusicOptions::offline(RATE)).unwrap();
    let out = sys.render(RATE as usize / 2);
    assert!((amp(&out[RATE as usize / 5 * 2..], 500.0) - 0.5).abs() < 0.02);
    // only the global file: used for any level
    let vfs = vfs_with(&game, &[&a]);
    let sys = MusicSystem::new(base_def(), Arc::new(GameResolver::new(vfs.clone(), Arc::new(SoundLibrary::new(vfs.clone())), "M05", vec![])), MusicOptions::offline(RATE)).unwrap();
    let out = sys.render(RATE as usize / 2);
    assert!((amp(&out[RATE as usize / 5 * 2..], 300.0) - 0.5).abs() < 0.02);
    for d in [game, a, b] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn file_sources_and_custom_definition_files_are_merged() {
    let game = tmpdir("g3");
    let m = tmpdir("m3");
    write(&m, "Music/MyMod/theme.wav", &tone_wav(777.0, 1.0, 0.4));
    write(
        &m,
        "Music/M01/10_extra.ron",
        br#"(
            tracks: { "theme": (source: File("Music/MyMod/theme.wav")) },
            cues: { "theme": (stems: [(track: "theme")]) },
            states: { "theme": (cue: "theme") },
            zones: { 9: "theme" },
            events: { "my_event": (state: "theme") },
        )"#,
    );
    write(
        &m,
        "Music/M01/20_more.json",
        br#"{ "transitions": [ { "to": ["theme"], "rule": { "fade_in": { "secs": 0.01 }, "fade_out": { "secs": 0.01 } } } ] }"#,
    );
    write(&m, "Music/_global/00_first.ron", br#"( stingers: {} , master_gain: 1.0 )"#);
    // broken files are reported and skipped, they must not break the others
    write(&m, "Music/M01/30_garbage.ron", b"this is ( not ron");
    write(&m, "Music/M01/40_dangling.json", br#"{ "states": { "x": { "cue": "nope" } } }"#);
    let vfs = vfs_with(&game, &[&m]);
    let (def, merged, warnings) = merge_mod_definitions(&vfs, "M01", base_def());
    assert_eq!(merged.len(), 3, "{merged:?}");
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("30_garbage")));
    assert!(warnings.iter().any(|w| w.contains("40_dangling")));
    assert_eq!(merged[0], "Music/_global/00_first.ron", "global files merge first");
    def.validate().unwrap();
    assert_eq!(def.zones[&9], "theme");
    assert_eq!(def.rule_for("calm", "theme").fade_in.secs, 0.01);
    // other levels do not see the M01 files
    let (def5, merged5, _) = merge_mod_definitions(&vfs, "M05", base_def());
    assert_eq!(merged5.len(), 1);
    assert!(!def5.states.contains_key("theme"));

    let sys = MusicSystem::new(def, resolver(&vfs), MusicOptions::offline(RATE)).unwrap();
    sys.render(RATE as usize / 5);
    assert!(sys.set_zone(9));
    sys.render(RATE as usize / 5);
    let out = sys.render(RATE as usize / 2);
    assert_eq!(sys.state(), "theme");
    assert!((amp(&out, 777.0) - 0.4).abs() < 0.02, "{}", amp(&out, 777.0));
    let _ = (std::fs::remove_dir_all(game), std::fs::remove_dir_all(m));
}

#[test]
fn undecodable_loose_file_is_reported_as_track_error() {
    let game = tmpdir("g4");
    let m = tmpdir("m4");
    write(&m, "Music/M01/calm.ogg", b"definitely not an ogg stream");
    let vfs = vfs_with(&game, &[&m]);
    let sys = MusicSystem::new(base_def(), resolver(&vfs), MusicOptions::offline(RATE)).unwrap();
    let out = sys.render(RATE as usize / 5);
    assert!(out.iter().all(|v| *v == 0.0));
    assert!(sys.poll_events().iter().any(|e| matches!(e, MusicEvent::TrackError { .. })));
    let _ = (std::fs::remove_dir_all(game), std::fs::remove_dir_all(m));
}

#[test]
fn shipped_example_files_parse_merge_and_validate() {
    let mut d = base_def();
    d.states.insert("explore".into(), StateDef::cue("calm"));
    for (name, text) in [
        ("mod_minimal.ron", include_str!("../examples/data/mod_minimal.ron")),
        ("mod_layered.ron", include_str!("../examples/data/mod_layered.ron")),
    ] {
        let over = MusicDef::parse(text, Format::Ron).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut m = d.clone();
        m.merge(over);
        m.validate().unwrap_or_else(|e| panic!("{name}: {e:?}"));
        // and they survive a RON / JSON round trip
        assert_eq!(MusicDef::parse(&m.to_ron(), Format::Ron).unwrap(), m, "{name}");
        assert_eq!(MusicDef::parse(&m.to_json(), Format::Json).unwrap(), m, "{name}");
    }
    let layered = MusicDef::parse(include_str!("../examples/data/mod_layered.ron"), Format::Ron).unwrap();
    assert_eq!(layered.cues["dynamic"].stems.len(), 3);
    assert_eq!(layered.states["combat"].stem_gains["bed"], 0.8);
}
