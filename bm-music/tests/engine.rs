//! Engine tests on synthetic audio (no game files needed): state machine, crossfades, loops, layers,
//! pause, ducking, stingers, playlists, read-ahead thread and integration with the headless mixer.

use bm_music::*;
use bm_sound::{AudioEngine, MixerEngine};
use std::sync::Arc;

const RATE: u32 = 48_000;

fn tone(rate: u32, freq: f32, secs: f32, amp: f32) -> MemSource {
    let n = (secs * rate as f32) as usize;
    let mut v = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * amp;
        v.push(s);
        v.push(s);
    }
    MemSource::from_f32(rate, 2, v)
}

/// Goertzel magnitude (amplitude) of `freq` in the left channel of interleaved stereo `x`.
fn tone_amp(x: &[f32], rate: u32, freq: f32) -> f32 {
    let n = x.len() / 2;
    let w = 2.0 * std::f32::consts::PI * freq / rate as f32;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for i in 0..n {
        re += x[i * 2] as f64 * (w as f64 * i as f64).cos();
        im += x[i * 2] as f64 * (w as f64 * i as f64).sin();
    }
    (2.0 * (re * re + im * im).sqrt() / n as f64) as f32
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn max_step(x: &[f32]) -> f32 {
    let n = x.len() / 2;
    (1..n).map(|i| (x[i * 2] - x[(i - 1) * 2]).abs().max((x[i * 2 + 1] - x[(i - 1) * 2 + 1]).abs())).fold(0.0, f32::max)
}

fn secs(x: &[f32], a: f32, b: f32) -> &[f32] {
    &x[(a * RATE as f32) as usize * 2..(b * RATE as f32) as usize * 2]
}

fn two_state_def() -> (MusicDef, Arc<MemoryResolver>) {
    let r = Arc::new(MemoryResolver::new());
    r.insert("a", tone(RATE, 440.0, 1.0, 0.4));
    r.insert("b", tone(RATE, 660.0, 1.0, 0.4));
    let mut d = MusicDef::default();
    d.tracks.insert("a".into(), TrackDef::bank("a"));
    d.tracks.insert("b".into(), TrackDef::bank("b"));
    d.cues.insert("calm".into(), CueDef::single("a"));
    d.cues.insert("fight".into(), CueDef::single("b"));
    d.states.insert("calm".into(), StateDef::cue("calm"));
    d.states.insert("fight".into(), StateDef::cue("fight"));
    d.initial_state = "calm".into();
    d.default_transition = TransitionRule::crossfade(0.5);
    (d, r)
}

fn sys(d: MusicDef, r: Arc<MemoryResolver>) -> MusicSystem {
    MusicSystem::new(d, r, MusicOptions::offline(RATE)).unwrap()
}

#[test]
fn plays_initial_state_and_loops_seamlessly() {
    let (d, r) = two_state_def();
    let s = sys(d, r);
    let out = s.render(RATE as usize * 3);
    // 3 s from a 1 s loop of a whole number of cycles (440 Hz * 1 s): amplitude 0.4, no clicks
    let body = secs(&out, 0.2, 3.0);
    assert!((tone_amp(body, RATE, 440.0) - 0.4).abs() < 0.01);
    let ideal = 0.4 * 2.0 * std::f32::consts::PI * 440.0 / RATE as f32;
    assert!(max_step(&out) < ideal * 1.05, "step {} ideal {}", max_step(&out), ideal);
    assert_eq!(s.stats().underruns, 0);
}

#[test]
fn crossfade_is_click_free_and_equal_power() {
    let (d, r) = two_state_def();
    let s = sys(d, r);
    let mut out = s.render(RATE as usize); // 1 s of calm
    assert!(s.set_state("fight"));
    out.extend(s.render(RATE as usize * 2));
    let bound = (0.4 * 2.0 * std::f32::consts::PI * (440.0 + 660.0) / RATE as f32) * 1.2;
    assert!(max_step(&out) < bound, "click: {} vs {}", max_step(&out), bound);
    // before: only 440, after the 0.5 s fade only 660
    assert!(tone_amp(secs(&out, 0.3, 0.9), RATE, 660.0) < 0.005);
    let after = secs(&out, 1.7, 2.9);
    assert!(tone_amp(after, RATE, 440.0) < 0.005);
    assert!((tone_amp(after, RATE, 660.0) - 0.4).abs() < 0.01);
    // equal-power: the level in the middle of the fade stays close to the level on either side
    let mid = secs(&out, 1.2, 1.3);
    let r_mid = rms(mid);
    let r_edge = rms(secs(&out, 0.5, 0.9));
    assert!(r_mid > r_edge * 0.85 && r_mid < r_edge * 1.35, "mid {r_mid} edge {r_edge}");
    let ev = s.poll_events();
    assert!(ev.iter().any(|e| matches!(e, MusicEvent::StateChanged { to, .. } if to == "fight")));
    assert!(ev.iter().any(|e| matches!(e, MusicEvent::CueStarted { cue } if cue == "fight")));
}

#[test]
fn curves_change_the_crossfade_shape() {
    let (mut d, r) = two_state_def();
    d.default_transition = TransitionRule { fade_out: FadeDef::new(1.0, Curve::Linear), fade_in: FadeDef::new(1.0, Curve::Linear), ..Default::default() };
    let s = sys(d, r);
    s.render(RATE as usize / 2);
    s.set_state("fight");
    let out = s.render(RATE as usize * 2);
    // linear: at 50 % both tones have amplitude 0.2
    let mid = secs(&out, 0.45, 0.55);
    assert!((tone_amp(mid, RATE, 440.0) - 0.2).abs() < 0.03, "{}", tone_amp(mid, RATE, 440.0));
    assert!((tone_amp(mid, RATE, 660.0) - 0.2).abs() < 0.03);
}

#[test]
fn loop_crossfade_hides_a_bad_wrap() {
    let r = Arc::new(MemoryResolver::new());
    r.insert("x", tone(RATE, 437.3, 0.5, 0.4)); // not a whole number of cycles
    let mut d = MusicDef::default();
    d.tracks.insert("x".into(), TrackDef { loop_crossfade_secs: 0.05, ..TrackDef::bank("x") });
    d.cues.insert("c".into(), CueDef::single("x"));
    d.states.insert("s".into(), StateDef::cue("c"));
    d.initial_state = "s".into();
    let out = sys(d.clone(), r.clone()).render(RATE as usize * 3);
    let ideal = 0.4 * 2.0 * std::f32::consts::PI * 437.3 / RATE as f32;
    assert!(max_step(&out) < ideal * 1.3, "{} vs {}", max_step(&out), ideal);
    // without the crossfade the wrap clicks
    d.tracks.get_mut("x").unwrap().loop_crossfade_secs = 0.0;
    let out = sys(d, r).render(RATE as usize * 3);
    assert!(max_step(&out) > ideal * 1.5);
}

#[test]
fn loop_points_play_attack_once_then_loop_the_region() {
    let r = Arc::new(MemoryResolver::new());
    // 1 s of 300 Hz (attack) + 1 s of 500 Hz (loop body)
    let mut v = Vec::new();
    for i in 0..RATE as usize * 2 {
        let f = if i < RATE as usize { 300.0 } else { 500.0 };
        let s = (2.0 * std::f32::consts::PI * f * i as f32 / RATE as f32).sin() * 0.4;
        v.push(s);
        v.push(s);
    }
    r.insert("x", MemSource::from_f32(RATE, 2, v));
    let mut d = MusicDef::default();
    d.tracks.insert("x".into(), TrackDef { loop_start_secs: Some(1.0), ..TrackDef::bank("x") });
    d.cues.insert("c".into(), CueDef::single("x"));
    d.states.insert("s".into(), StateDef::cue("c"));
    d.initial_state = "s".into();
    let out = sys(d, r).render(RATE as usize * 4);
    assert!(tone_amp(secs(&out, 0.1, 0.9), RATE, 300.0) > 0.35);
    assert!(tone_amp(secs(&out, 1.1, 1.9), RATE, 500.0) > 0.35);
    assert!(tone_amp(secs(&out, 2.1, 3.9), RATE, 300.0) < 0.01, "attack must not repeat");
    assert!(tone_amp(secs(&out, 2.1, 3.9), RATE, 500.0) > 0.35);
}

#[test]
fn sync_to_loop_end_waits_for_the_boundary() {
    let (mut d, r) = two_state_def();
    d.default_transition = TransitionRule { sync: SyncMode::LoopEnd, max_wait_secs: 10.0, ..TransitionRule::crossfade(0.1) };
    let s = sys(d, r);
    s.render(RATE as usize * 3 / 10); // 0.3 s into the 1 s loop
    assert!(s.set_state("fight"));
    let ev = s.poll_events();
    let in_secs = ev.iter().find_map(|e| if let MusicEvent::TransitionScheduled { in_secs, .. } = e { Some(*in_secs) } else { None }).unwrap();
    assert!((in_secs - 0.7).abs() < 0.01, "in_secs {in_secs}");
    assert_eq!(s.state(), "calm");
    assert_eq!(s.pending_state().as_deref(), Some("fight"));
    let out = s.render(RATE as usize * 2);
    assert_eq!(s.state(), "fight");
    // still pure 440 until the boundary at 0.7 s
    assert!(tone_amp(secs(&out, 0.0, 0.65), RATE, 660.0) < 0.005);
    assert!(tone_amp(secs(&out, 0.9, 1.9), RATE, 660.0) > 0.38);
}

#[test]
fn max_wait_caps_the_sync_delay() {
    let (mut d, r) = two_state_def();
    d.default_transition = TransitionRule { sync: SyncMode::LoopEnd, max_wait_secs: 0.2, ..TransitionRule::crossfade(0.1) };
    let s = sys(d, r);
    s.render(RATE as usize / 10);
    s.set_state("fight");
    let ev = s.poll_events();
    let in_secs = ev.iter().find_map(|e| if let MusicEvent::TransitionScheduled { in_secs, .. } = e { Some(*in_secs) } else { None }).unwrap();
    assert!((in_secs - 0.2).abs() < 0.01);
}

#[test]
fn beat_sync_fires_on_the_beat() {
    let r = Arc::new(MemoryResolver::new());
    r.insert("a", tone(RATE, 440.0, 4.0, 0.4));
    r.insert("b", tone(RATE, 660.0, 4.0, 0.4));
    let mut d = MusicDef::default();
    d.tracks.insert("a".into(), TrackDef { bpm: Some(120.0), ..TrackDef::bank("a") });
    d.tracks.insert("b".into(), TrackDef::bank("b"));
    d.cues.insert("calm".into(), CueDef::single("a"));
    d.cues.insert("fight".into(), CueDef::single("b"));
    d.states.insert("calm".into(), StateDef::cue("calm"));
    d.states.insert("fight".into(), StateDef::cue("fight"));
    d.initial_state = "calm".into();
    d.default_transition = TransitionRule { sync: SyncMode::Beat, ..TransitionRule::crossfade(0.05) };
    let s = sys(d, r);
    s.render(RATE as usize * 3 / 10); // 0.3 s; beats every 0.5 s
    s.set_state("fight");
    let ev = s.poll_events();
    let in_secs = ev.iter().find_map(|e| if let MusicEvent::TransitionScheduled { in_secs, .. } = e { Some(*in_secs) } else { None }).unwrap();
    assert!((in_secs - 0.2).abs() < 0.01, "{in_secs}");
}

#[test]
fn min_hold_defers_and_cancel_returns_to_current() {
    let (mut d, r) = two_state_def();
    d.states.get_mut("calm").unwrap().min_hold_secs = 1.0;
    let s = sys(d, r);
    s.render(RATE as usize / 5);
    s.set_state("fight");
    let in_secs = s.poll_events().iter().find_map(|e| if let MusicEvent::TransitionScheduled { in_secs, .. } = e { Some(*in_secs) } else { None }).unwrap();
    assert!((in_secs - 0.8).abs() < 0.01, "{in_secs}");
    // asking for the current state cancels the pending move
    s.set_state("calm");
    assert_eq!(s.pending_state(), None);
    s.render(RATE as usize * 2);
    assert_eq!(s.state(), "calm");
    // force ignores the hold
    let (mut d, r) = two_state_def();
    d.states.get_mut("calm").unwrap().min_hold_secs = 5.0;
    let s = sys(d, r);
    s.force_state("fight");
    s.render(RATE as usize / 10);
    assert_eq!(s.state(), "fight");
}

#[test]
fn unknown_names_are_rejected_without_side_effects() {
    let (d, r) = two_state_def();
    let s = sys(d, r);
    assert!(!s.set_state("nope"));
    assert!(!s.set_zone(77));
    assert!(!s.event("nothing"));
    assert!(!s.trigger_stinger("none"));
    assert_eq!(s.state(), "calm");
    assert!(s.poll_events().iter().filter(|e| matches!(e, MusicEvent::Ignored { .. })).count() >= 4);
}

#[test]
fn invalid_definitions_fail_fast() {
    let (mut d, r) = two_state_def();
    d.states.insert("bad".into(), StateDef::cue("missing"));
    assert!(matches!(MusicSystem::new(d, r, MusicOptions::offline(RATE)), Err(MusicError::Invalid(_))));
}

#[test]
fn intensity_layers_fade_in_smoothly() {
    let r = Arc::new(MemoryResolver::new());
    r.insert("base", tone(RATE, 220.0, 1.0, 0.3));
    r.insert("drums", tone(RATE, 880.0, 1.0, 0.3));
    let mut d = MusicDef::default();
    d.tracks.insert("base".into(), TrackDef::bank("base"));
    d.tracks.insert("drums".into(), TrackDef::bank("drums"));
    d.cues.insert(
        "c".into(),
        CueDef {
            stems: vec![
                StemDef { name: "base".into(), track: "base".into(), ..Default::default() },
                StemDef { name: "drums".into(), track: "drums".into(), layer: Some(LayerDef { fade_in: (0.4, 0.8), ..Default::default() }), ..Default::default() },
            ],
            ..Default::default()
        },
    );
    d.states.insert("s".into(), StateDef::cue("c"));
    d.initial_state = "s".into();
    d.intensity = Some(IntensityDef { smooth_secs: 0.5, initial: 0.0 });
    let s = sys(d, r);
    let mut out = s.render(RATE as usize);
    assert!(tone_amp(secs(&out, 0.2, 0.9), RATE, 880.0) < 0.005);
    assert!((tone_amp(secs(&out, 0.2, 0.9), RATE, 220.0) - 0.3).abs() < 0.01);
    s.set_intensity(1.0);
    out.extend(s.render(RATE as usize * 2));
    assert!((tone_amp(secs(&out, 2.0, 2.9), RATE, 880.0) - 0.3).abs() < 0.01);
    assert!((tone_amp(secs(&out, 2.0, 2.9), RATE, 220.0) - 0.3).abs() < 0.01);
    let bound = 0.3 * 2.0 * std::f32::consts::PI * (220.0 + 880.0) / RATE as f32 * 1.15;
    assert!(max_step(&out) < bound, "{} vs {}", max_step(&out), bound);
    assert!((s.intensity() - 1.0).abs() < 1e-3);
}

#[test]
fn state_stem_gains_rebalance_a_cue_without_restarting_it() {
    let r = Arc::new(MemoryResolver::new());
    r.insert("base", tone(RATE, 220.0, 1.0, 0.3));
    r.insert("drums", tone(RATE, 880.0, 1.0, 0.3));
    let mut d = MusicDef::default();
    d.tracks.insert("base".into(), TrackDef::bank("base"));
    d.tracks.insert("drums".into(), TrackDef::bank("drums"));
    d.cues.insert(
        "c".into(),
        CueDef {
            stems: vec![
                StemDef { name: "base".into(), track: "base".into(), ..Default::default() },
                StemDef { name: "drums".into(), track: "drums".into(), ..Default::default() },
            ],
            ..Default::default()
        },
    );
    let mut quiet = StateDef::cue("c");
    quiet.stem_gains.insert("drums".into(), 0.0);
    d.states.insert("quiet".into(), quiet);
    d.states.insert("loud".into(), StateDef::cue("c"));
    d.initial_state = "quiet".into();
    d.default_transition = TransitionRule::crossfade(0.3);
    let s = sys(d, r);
    let mut out = s.render(RATE as usize);
    s.set_state("loud");
    out.extend(s.render(RATE as usize));
    assert!(tone_amp(secs(&out, 0.2, 0.9), RATE, 880.0) < 0.005);
    assert!((tone_amp(secs(&out, 1.5, 1.95), RATE, 880.0) - 0.3).abs() < 0.01);
    // same cue: no second CueStarted
    let ev = s.poll_events();
    assert_eq!(ev.iter().filter(|e| matches!(e, MusicEvent::CueStarted { .. })).count(), 1);
    assert!(max_step(&out) < 0.3 * 2.0 * std::f32::consts::PI * 1100.0 / RATE as f32 * 1.15);
}

#[test]
fn pause_fades_freezes_and_resumes_exactly() {
    let (d, r) = two_state_def();
    let s = sys(d, r);
    let mut out = s.render(RATE as usize / 2);
    s.pause(0.1);
    out.extend(s.render(RATE as usize / 2)); // fades out within the first 0.1 s then silence
    let silent = secs(&out, 0.7, 0.99);
    assert!(rms(silent) < 1e-6);
    let before = s.status().clock_secs;
    s.render(RATE as usize / 2);
    assert!((s.status().clock_secs - before).abs() < 1e-6, "clock must freeze while paused");
    assert!(s.is_paused());
    s.resume(0.1);
    out.extend(s.render(RATE as usize));
    let bound = 0.4 * 2.0 * std::f32::consts::PI * 440.0 / RATE as f32 * 1.1;
    assert!(max_step(&out) < bound, "{} vs {}", max_step(&out), bound);
    assert!((tone_amp(secs(&out, 1.4, 1.9), RATE, 440.0) - 0.4).abs() < 0.01);
}

#[test]
fn manual_duck_attenuates_and_releases() {
    let (d, r) = two_state_def();
    let s = sys(d, r);
    let base = rms(secs(&s.render(RATE as usize), 0.5, 1.0));
    s.duck(0.25, 0.05, 0.2);
    let ducked = s.render(RATE as usize);
    let d_rms = rms(secs(&ducked, 0.5, 1.0));
    assert!((d_rms / base - 0.25).abs() < 0.03, "{}", d_rms / base);
    s.unduck();
    let rel = s.render(RATE as usize);
    assert!((rms(secs(&rel, 0.6, 1.0)) / base - 1.0).abs() < 0.03);
    assert!(max_step(&ducked) < 0.4 * 2.0 * std::f32::consts::PI * 440.0 / RATE as f32 * 1.1);
}

#[test]
fn stinger_plays_once_and_ducks_the_music() {
    let (mut d, r) = two_state_def();
    r.insert("hit", tone(RATE, 1000.0, 0.5, 0.3));
    d.tracks.insert("hit".into(), TrackDef::bank("hit"));
    d.stingers.insert("hit".into(), StingerDef { track: "hit".into(), duck_depth: 0.3, duck_attack_secs: 0.02, duck_release_secs: 0.2, ..Default::default() });
    let s = sys(d, r);
    s.render(RATE as usize / 2);
    assert!(s.trigger_stinger("hit"));
    let out = s.render(RATE as usize * 2);
    assert!(tone_amp(secs(&out, 0.1, 0.4), RATE, 1000.0) > 0.28, "stinger audible");
    assert!(tone_amp(secs(&out, 0.1, 0.4), RATE, 440.0) < 0.4 * 0.35, "music ducked while it plays");
    assert!(tone_amp(secs(&out, 1.2, 1.9), RATE, 1000.0) < 0.005, "stinger is one-shot");
    assert!((tone_amp(secs(&out, 1.2, 1.9), RATE, 440.0) - 0.4).abs() < 0.01, "music recovers");
    assert!(s.poll_events().iter().any(|e| matches!(e, MusicEvent::StingerStarted { stinger } if stinger == "hit")));
}

#[test]
fn playlist_advances_gaplessly_and_wraps() {
    let r = Arc::new(MemoryResolver::new());
    r.insert("a", tone(RATE, 300.0, 0.5, 0.4));
    r.insert("b", tone(RATE, 500.0, 0.5, 0.4));
    let mut d = MusicDef::default();
    d.tracks.insert("a".into(), TrackDef { looping: false, ..TrackDef::bank("a") });
    d.tracks.insert("b".into(), TrackDef { looping: false, ..TrackDef::bank("b") });
    d.cues.insert("a".into(), CueDef::single("a"));
    d.cues.insert("b".into(), CueDef::single("b"));
    d.states.insert("p".into(), StateDef { cue: Some("a".into()), playlist: vec!["b".into()], playlist_fade_secs: 0.0, ..Default::default() });
    d.initial_state = "p".into();
    let s = sys(d, r);
    let out = s.render(RATE as usize * 3 / 2);
    assert!(tone_amp(secs(&out, 0.1, 0.45), RATE, 300.0) > 0.38);
    assert!(tone_amp(secs(&out, 0.55, 0.95), RATE, 500.0) > 0.38);
    assert!(tone_amp(secs(&out, 1.05, 1.45), RATE, 300.0) > 0.38, "wraps to the first entry");
    let ev = s.poll_events();
    assert!(ev.iter().filter(|e| matches!(e, MusicEvent::CueFinished { .. })).count() >= 2);
}

#[test]
fn playlist_overlap_crossfades_before_the_end() {
    let r = Arc::new(MemoryResolver::new());
    r.insert("a", tone(RATE, 300.0, 1.0, 0.4));
    r.insert("b", tone(RATE, 500.0, 1.0, 0.4));
    let mut d = MusicDef::default();
    d.tracks.insert("a".into(), TrackDef { looping: false, ..TrackDef::bank("a") });
    d.tracks.insert("b".into(), TrackDef { looping: false, ..TrackDef::bank("b") });
    d.cues.insert("a".into(), CueDef::single("a"));
    d.cues.insert("b".into(), CueDef::single("b"));
    d.states.insert("p".into(), StateDef { cue: Some("a".into()), playlist: vec!["b".into()], playlist_overlap_secs: 0.4, ..Default::default() });
    d.initial_state = "p".into();
    let s = sys(d, r);
    let out = s.render(RATE as usize * 2);
    // 500 Hz is already present 0.3 s before the 1 s end of the first track
    assert!(tone_amp(secs(&out, 0.75, 0.95), RATE, 500.0) > 0.05);
    assert!(tone_amp(secs(&out, 0.75, 0.95), RATE, 300.0) > 0.05);
    let bound = 0.4 * 2.0 * std::f32::consts::PI * 800.0 / RATE as f32 * 1.2;
    assert!(max_step(&out) < bound);
}

#[test]
fn on_finish_chains_states_and_events_map_to_states() {
    let r = Arc::new(MemoryResolver::new());
    r.insert("a", tone(RATE, 300.0, 0.5, 0.4));
    r.insert("b", tone(RATE, 500.0, 2.0, 0.4));
    let mut d = MusicDef::default();
    d.tracks.insert("a".into(), TrackDef { looping: false, ..TrackDef::bank("a") });
    d.tracks.insert("b".into(), TrackDef::bank("b"));
    d.cues.insert("a".into(), CueDef::single("a"));
    d.cues.insert("b".into(), CueDef::single("b"));
    d.states.insert("intro".into(), StateDef { cue: Some("a".into()), on_finish: Some("main".into()), ..Default::default() });
    d.states.insert("main".into(), StateDef::cue("b"));
    d.initial_state = "intro".into();
    d.default_transition = TransitionRule::crossfade(0.05);
    d.events.insert("again".into(), EventAction { state: Some("intro".into()), ..Default::default() });
    let s = sys(d, r);
    let out = s.render(RATE as usize * 3 / 2);
    assert_eq!(s.state(), "main");
    assert!(tone_amp(secs(&out, 0.8, 1.4), RATE, 500.0) > 0.38);
    assert!(s.event("again"));
    s.render(RATE as usize / 4);
    assert_eq!(s.state(), "intro");
}

#[test]
fn zones_follow_and_lock() {
    let (mut d, r) = two_state_def();
    r.insert("c", tone(RATE, 880.0, 1.0, 0.4));
    d.tracks.insert("c".into(), TrackDef::bank("c"));
    d.cues.insert("party".into(), CueDef::single("c"));
    d.states.insert("party".into(), StateDef::cue("party"));
    d.states.get_mut("calm").unwrap().follow_zone = true;
    d.states.get_mut("fight").unwrap().zone_locked = true;
    d.zones.insert(1, "calm".into());
    d.zones.insert(2, "party".into());
    d.default_transition = TransitionRule::crossfade(0.05);
    let s = sys(d, r);
    s.render(RATE as usize / 10);
    assert!(s.set_zone(2));
    s.render(RATE as usize / 5);
    assert_eq!(s.state(), "party");
    s.set_state("fight");
    s.render(RATE as usize / 5);
    assert_eq!(s.state(), "fight");
    s.set_zone(1); // ignored while locked, remembered
    s.render(RATE as usize / 5);
    assert_eq!(s.state(), "fight");
    s.set_zone(2);
    s.render(RATE as usize / 10);
    assert_eq!(s.state(), "fight");
    // calm follows the remembered zone (party)
    s.set_state("calm");
    s.render(RATE as usize / 5);
    assert_eq!(s.state(), "party");
}

#[test]
fn missing_track_degrades_to_silence_and_reports() {
    let (d, _) = two_state_def();
    let empty = Arc::new(MemoryResolver::new());
    let s = sys(d, empty);
    let out = s.render(RATE as usize / 2);
    assert!(rms(&out) < 1e-6);
    assert!(s.poll_events().iter().any(|e| matches!(e, MusicEvent::TrackError { .. })));
}

#[test]
fn different_source_rate_is_resampled_without_artifacts() {
    let r = Arc::new(MemoryResolver::new());
    r.insert("a", tone(44_100, 441.0, 1.0, 0.4)); // exactly 441 cycles per second
    let mut d = MusicDef::default();
    d.tracks.insert("a".into(), TrackDef::bank("a"));
    d.cues.insert("c".into(), CueDef::single("a"));
    d.states.insert("s".into(), StateDef::cue("c"));
    d.initial_state = "s".into();
    let out = sys(d, r).render(RATE as usize * 3);
    let ideal = 0.4 * 2.0 * std::f32::consts::PI * 441.0 / RATE as f32;
    assert!(max_step(&secs(&out, 0.0, 3.0).to_vec()) < ideal * 1.1, "{}", max_step(&out));
    assert!((tone_amp(secs(&out, 0.5, 2.5), RATE, 441.0) - 0.4).abs() < 0.01);
}

#[test]
fn threaded_prefetch_streams_without_underruns() {
    let (d, r) = two_state_def();
    let s = MusicSystem::new(d, r, MusicOptions { sample_rate: RATE, threaded: true, ..Default::default() }).unwrap();
    // let the worker prime the rings, then pull in small blocks like an audio callback would
    std::thread::sleep(std::time::Duration::from_millis(150));
    let mut out = Vec::new();
    for i in 0..200 {
        out.extend(s.render(512));
        if i == 60 {
            s.set_state("fight");
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(s.stats().underruns, 0);
    let bound = 0.4 * 2.0 * std::f32::consts::PI * 1100.0 / RATE as f32 * 1.2;
    assert!(max_step(&out) < bound, "{} vs {}", max_step(&out), bound);
    assert!(tone_amp(secs(&out, 1.8, 2.1), RATE, 660.0) > 0.38);
}

#[test]
fn attached_to_the_headless_mixer_stays_continuous() {
    let (d, r) = two_state_def();
    let mut engine = MixerEngine::headless(RATE);
    let mut s = MusicSystem::new(d, r, MusicOptions::offline(RATE)).unwrap();
    let h = s.attach(&mut engine).unwrap();
    assert!(engine.is_playing(h));
    let out = engine.render_offline(3.0, 480, |t, _| {
        if (1.0..1.0 + 0.011).contains(&t) {
            s.set_state("fight");
        }
    });
    let n = out.len() / 2;
    assert_eq!(n, 3 * RATE as usize);
    // mixer: soft limiter + bus gains are smooth, so the bound only grows slightly
    let bound = 0.4 * 2.0 * std::f32::consts::PI * 1100.0 / RATE as f32 * 1.3;
    assert!(max_step(&out) < bound, "{} vs {}", max_step(&out), bound);
    assert!(rms(secs(&out, 0.3, 0.9)) > 0.1);
    assert!(tone_amp(secs(&out, 2.0, 2.9), RATE, 660.0) > 0.3);
    s.detach(&mut engine, 0.0);
}

#[test]
fn mission_events_drive_states() {
    let (mut d, r) = two_state_def();
    d.events.insert(MissionEvent::Combat.name().into(), EventAction { state: Some("fight".into()), ..Default::default() });
    d.events.insert(MissionEvent::Calm.name().into(), EventAction { state: Some("calm".into()), ..Default::default() });
    let s = sys(d, r);
    s.render(RATE as usize / 10);
    assert!(s.mission_event(MissionEvent::Combat));
    s.render(RATE as usize);
    assert_eq!(s.state(), "fight");
    assert!(s.mission_event(MissionEvent::Calm));
    s.render(RATE as usize);
    assert_eq!(s.state(), "calm");
    s.mission_event(MissionEvent::Paused);
    s.render(RATE as usize);
    assert!(s.is_paused());
    s.mission_event(MissionEvent::Resumed);
    assert!(!s.is_paused());
}
