//! Offline 3D mix of a level: scene emitters + scripted one-shots around a moving listener, rendered
//! to a stereo WAV without any audio device, followed by a numeric sanity report (no clipping, expected
//! length, non-silence, left/right movement).
//!
//! `render_mix [scene] [out.wav] [seconds]` (defaults: `Scenes/M01/M01_main`, `render_mix.wav`, 12)
use bm_sound::pcm::AudioStats;
use bm_sound::scene::SoundScene;
use bm_sound::snd_db::SndKind;
use bm_sound::{AudioEngine, Bus, Listener, MixerEngine, PlayOptions, ReverbParams, SoundPlayer};
use std::sync::Arc;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let scene_path = a.get(1).cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let out_path = a.get(2).cloned().unwrap_or_else(|| "render_mix.wav".into());
    let seconds: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(12.0);

    let vfs = Arc::new(bm_vfs::Vfs::from_env().expect("game dir (BM_GAME_DIR)"));
    let lib = Arc::new(bm_sound::SoundLibrary::open_level(vfs.clone(), &scene_path).expect("bank"));
    let scene = bm_models::Scene::load(&vfs, &scene_path).expect("scene");
    let db = lib.snd(0).expect(".SND");
    let ss = SoundScene::from_scene(&scene, db);

    // Emitters whose sound object has waves in this bank; pick the densest cluster (50 m radius).
    let playable: Vec<_> = ss
        .emitters()
        .filter(|e| e.sound.as_ref().map_or(false, |s| s.kind == SndKind::Sound && db.find(&s.path, &SndKind::Sound).map_or(false, |o| !db.waves_of(o).is_empty())))
        .collect();
    println!("{} ZSNDOBJ emitters, {} playable sound emitters", ss.emitters().count(), playable.len());
    let centre = playable
        .iter()
        .max_by_key(|e| playable.iter().filter(|o| (0..3).map(|i| (o.position[i] - e.position[i]).powi(2)).sum::<f32>() < 5000.0f32.powi(2)).count())
        .map(|e| e.position)
        .unwrap_or([0.0; 3]);
    println!("listener start near {:?}", centre);

    let mut engine = MixerEngine::headless(48000);
    engine.set_bus_volume(Bus::Ambient, 0.8);
    // room reverb of the first matching room (documented EAX preset -> reverb send)
    if let Some(r) = db.room_effects.first() {
        engine.set_reverb(ReverbParams::from_eax(r));
        println!("reverb preset {} (decay {:.2}s)", r.name, r.decay_time);
    }
    let mut player = SoundPlayer::new(lib.clone(), Box::new(engine.clone()));
    player.seed(7);

    // looping ambience from the scene's emitters within 80 m of the start
    let mut started = 0;
    for e in &playable {
        let d = (0..3).map(|i| (e.position[i] - centre[i]).powi(2)).sum::<f32>().sqrt();
        if d > 8000.0 || started >= 12 {
            continue;
        }
        let path = &e.sound.as_ref().unwrap().path;
        let o = PlayOptions { position: Some(e.position), looping: true, bus: Some(Bus::Ambient), min_max_m: Some((6.0, 60.0)), reverb_send: Some(0.2), ..Default::default() };
        if let Ok(Some(_)) = player.play_object(0, path, &o) {
            started += 1;
        }
    }
    println!("started {started} looping emitters");

    // scripted events relative to the listener path
    let walk = |t: f32| [centre[0] + 150.0 * t, centre[1], centre[2] + 40.0 * t]; // ~1.5 m/s
    let events: Vec<(f32, &str, [f32; 3])> = vec![
        (1.0, "weapons/firearms/sniperrifle_sako_trg/shot", [centre[0] + 2500.0, centre[1], centre[2] + 500.0]),
        (3.0, "explosions/explosion_bomb_remote/explosion_bomb_remote_002", [centre[0] + 400.0, centre[1], centre[2] + 4000.0]),
        (6.0, "weapons/firearms/sniperrifle_sako_trg/shot", [centre[0] - 3000.0, centre[1], centre[2] + 800.0]),
    ];
    let dialogue = lib.iter().find(|i| i.streamed && i.name.to_ascii_lowercase().starts_with("dialog") && i.duration > 1.0 && i.duration < 4.0).map(|i| i.id);
    let mut fired = vec![false; events.len() + 1];
    let mut rendered = 0usize;
    let mix = engine.clone().render_offline(seconds, 512, |t, eng| {
        let t = t as f32;
        // listener walks forward; yaw turns 90 degrees to the right over the last third
        let turn = ((t / seconds - 0.66) * 3.0).clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
        eng.set_listener(Listener { position: walk(t), forward: [turn.sin(), 0.0, turn.cos()], up: [0.0, 1.0, 0.0] });
        for (i, (when, obj, pos)) in events.iter().enumerate() {
            if !fired[i] && t >= *when {
                fired[i] = true;
                let o = PlayOptions { position: Some(*pos), min_max_m: Some((15.0, 150.0)), reverb_send: Some(0.3), ..Default::default() };
                if let Err(e) = player.play_object(0, obj, &o) {
                    eprintln!("skip {obj}: {e}");
                }
            }
        }
        if !fired[events.len()] && t >= 4.0 {
            fired[events.len()] = true;
            if let Some(id) = dialogue {
                let p = walk(t);
                let o = PlayOptions { position: Some([p[0] - 300.0, p[1], p[2] + 300.0]), min_max_m: Some((5.0, 40.0)), ..Default::default() };
                let _ = player.play_id(id, &o);
            }
        }
        rendered += 1;
    });
    drop(player);
    MixerEngine::write_wav(&out_path, &mix, 48000).unwrap();

    let st = AudioStats::of_f32(&mix, 2, 48000);
    let l: f32 = mix.iter().step_by(2).map(|s| s * s).sum::<f32>();
    let r: f32 = mix.iter().skip(1).step_by(2).map(|s| s * s).sum::<f32>();
    let engine_stats = engine.stats();
    println!("wrote {out_path}: {} frames ({:.2}s), {} blocks", st.frames, st.frames as f32 / 48000.0, rendered);
    println!("peak {:.3} rms {:.4} clipped {:.5} active10ms {:.2} dc {:.5} L/R energy {:.2}", st.peak, st.rms, st.clipped_fraction, st.active_fraction, st.dc_offset, l / r.max(1e-9));
    println!("voices at end {} (stolen {}, rejected {})", engine_stats.active_voices, engine_stats.stolen, engine_stats.rejected);
    let mut ok = true;
    let mut check = |name: &str, c: bool| {
        println!("  [{}] {name}", if c { "ok" } else { "FAIL" });
        ok &= c;
    };
    check("length matches request", st.frames == (seconds * 48000.0) as usize);
    check("no clipping (peak < 1.0)", st.peak < 1.0 && st.clipped_fraction == 0.0);
    check("not silent (rms > 0.001)", st.rms > 0.001);
    check("mostly active (>60% of 10 ms windows)", st.active_fraction > 0.6);
    check("no DC offset", st.dc_offset.abs() < 0.01);
    if !ok {
        std::process::exit(1);
    }
}
