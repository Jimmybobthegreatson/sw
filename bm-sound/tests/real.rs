//! Real-file tests; they skip (pass) when the game install is absent.
use bm_sound::pcm::AudioStats;
use bm_sound::snd_db::SndKind;
use bm_sound::{SoundId, SoundLibrary, SoundScene};
use std::sync::Arc;

fn lib(scene: &str) -> Option<SoundLibrary> {
    let dir = bm_vfs::game_dir_if_present().or_else(|| {
        eprintln!("game dir missing, skipping");
        None
    })?;
    let vfs = Arc::new(bm_vfs::Vfs::new(dir).ok()?);
    SoundLibrary::open_level(vfs, scene).ok()
}

#[test]
fn resident_sounds_decode_to_declared_length_without_clipping() {
    let Some(lib) = lib("Scenes/M01/M01_main") else { return };
    let mut checked = 0;
    let mut silent = 0;
    let mut clip_sum = 0.0;
    for info in lib.iter().filter(|i| !i.streamed).step_by(7).take(200) {
        let pcm = lib.pcm(info.id).unwrap();
        assert_eq!(pcm.frames() as u32, info.frames, "{}", info.name);
        assert_eq!(pcm.sample_rate, info.sample_rate);
        let st = AudioStats::of_pcm(&pcm);
        // some UI/effect sounds are mastered hot; only pathological clipping indicates a decode error
        assert!(st.clipped_fraction < 0.3, "{} clipped {}", info.name, st.clipped_fraction);
        clip_sum += st.clipped_fraction;
        assert!(st.dc_offset.abs() < 0.5, "{} dc {}", info.name, st.dc_offset);
        if st.peak < 1e-4 {
            silent += 1;
        }
        checked += 1;
    }
    assert!(checked > 50, "only {checked} checked");
    assert!(clip_sum / (checked as f32) < 0.01, "mean clipped fraction {}", clip_sum / checked as f32);
    assert!(silent * 10 < checked, "{silent} of {checked} silent");
    // cache works
    let id = lib.iter().find(|i| !i.streamed).unwrap().id;
    let a = lib.pcm(id).unwrap();
    let b = lib.pcm(id).unwrap();
    assert!(Arc::ptr_eq(&a.samples, &b.samples));
}

#[test]
fn streamed_dialogue_decodes_and_lip_sync_parses() {
    let Some(lib) = lib("Scenes/M01/M01_main") else { return };
    let mut n = 0;
    for info in lib.iter().filter(|i| i.streamed && i.name.to_ascii_lowercase().starts_with("dialog")).step_by(40).take(25) {
        let pcm = lib.pcm(info.id).unwrap();
        // decoder may emit up to one block more than the declared length
        assert!((pcm.frames() as i64 - info.frames as i64).abs() <= 2041, "{} decoded {} declared {}", info.name, pcm.frames(), info.frames);
        let st = AudioStats::of_pcm(&pcm);
        assert!(st.peak > 0.01 && st.clipped_fraction < 0.05, "{} {st:?}", info.name);
        // speech is low-pass dominated; a broken ADPCM decode would sound like white noise (rate ~0.5)
        assert!(pcm.zero_crossing_rate() < 0.3, "{} zcr {}", info.name, pcm.zero_crossing_rate());
        if let Some(l) = lib.lip_sync(info.id).unwrap() {
            // last key lies within ~1.5 s of the audio length (1/24 s ticks)
            assert!(l.duration_secs() <= info.duration + 1.5, "{} lip {} audio {}", info.name, l.duration_secs(), info.duration);
            assert!(!l.channels.is_empty());
            n += 1;
        }
    }
    assert!(n > 10);
}

#[test]
fn all_lip_chunks_of_a_level_parse() {
    let Some(lib) = lib("Scenes/M01/M01_main") else { return };
    let mut ok = 0;
    let mut tot = 0;
    for info in lib.iter().filter(|i| i.streamed) {
        if lib.lip_raw(info.id).unwrap().is_some() {
            tot += 1;
            if lib.lip_sync(info.id).unwrap().is_some() {
                ok += 1;
            }
        }
    }
    assert!(tot > 100);
    assert_eq!(ok, tot);
}

#[test]
fn snd_waves_join_the_bank_and_objects_resolve() {
    let Some(lib) = lib("Scenes/M01/M01_main") else { return };
    let db = lib.snd(0).unwrap();
    assert!(db.waves.len() > 1000);
    let mut joined = 0;
    for w in &db.waves {
        if let Some(id) = lib.find(&w.file) {
            let info = lib.info(id).unwrap();
            if (info.duration - w.duration).abs() < 0.01 {
                joined += 1;
            }
        }
    }
    // names join and durations (wave record +0x18) agree with the bank for essentially all waves
    assert!(joined * 100 >= db.waves.len() * 98, "{joined}/{}", db.waves.len());
    let obj = db.find("explosions/explosion_bomb_remote/explosion_bomb_remote_002", &SndKind::Sound).unwrap();
    assert!(!db.waves_of(obj).is_empty());
    let p = db.sound_params(obj).unwrap();
    assert_eq!(p.volume_pct, 100);
    assert_eq!(p.cone_inner, 360);
    assert!(!db.room_effects.is_empty());
    for r in &db.room_effects {
        assert!(r.decay_time > 0.05 && r.decay_time < 100.0, "{r:?}");
    }
}

#[test]
fn scene_emitters_resolve_to_sound_objects() {
    let Some(l) = lib("Scenes/M01/M01_main") else { return };
    let vfs = l.vfs().clone();
    let scene = bm_models::Scene::load(&vfs, "Scenes/M01/M01_main").unwrap();
    let db = l.snd(0).unwrap();
    let ss = SoundScene::from_scene(&scene, db);
    let em: Vec<_> = ss.emitters().collect();
    assert!(em.len() > 30, "{} emitters", em.len());
    let resolved = em.iter().filter(|e| e.sound.is_some()).count();
    assert!(resolved * 10 >= em.len() * 8, "{resolved}/{}", em.len());
    // a known ambient emitter
    let wf = em.iter().find(|e| e.name.ends_with("Waterfall")).expect("waterfall emitter");
    assert_eq!(wf.sound.as_ref().unwrap().path, "levelsetup/m01/water/waterfall");
    assert!(wf.position.iter().all(|c| c.is_finite()) && wf.position != [0.0; 3]);
    // room reverb lookup by room node name
    let matched = ss.rooms.iter().filter(|(_, n)| ss.room_effect(db, n).is_some()).count();
    assert!(matched >= 5, "matched rooms {matched}");
}

#[test]
fn bank_ids_are_stable() {
    let Some(lib) = lib("Scenes/M01/M01_main") else { return };
    let id = lib.find("Action/Wipe_Table/Wipe_Table_01.wav").unwrap();
    assert_eq!(id, SoundId { bank: 0, index: 0 });
    let info = lib.info(id).unwrap();
    assert_eq!(info.sample_rate, 16054);
    assert_eq!(info.frames, 15845);
}

#[test]
fn loose_file_overrides_bank_sound_by_name_and_by_id() {
    let Some(dir) = bm_vfs::game_dir_if_present() else { return };
    let md = std::env::temp_dir().join(format!("bm_sound_real_mod_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&md);
    std::fs::create_dir_all(md.join("Sounds/M01_main/Action/Wipe_Table")).unwrap();
    let tone: Vec<i16> = (0..8000).map(|i| ((i as f32 * 0.3).sin() * 20000.0) as i16).collect();
    std::fs::write(md.join("Sounds/M01_main/Action/Wipe_Table/Wipe_Table_01.wav"), bm_sound::pcm::wav_i16(&tone, 1, 8000)).unwrap();
    std::fs::write(md.join("Sounds/M01_main/id_1.wav"), bm_sound::pcm::wav_i16(&tone[..4000], 1, 8000)).unwrap();
    let mut vfs = bm_vfs::Vfs::new(dir).unwrap();
    vfs.add_mod_dir(&md).unwrap();
    let lib = SoundLibrary::open_level(Arc::new(vfs), "Scenes/M01/M01_main").unwrap();
    let by_name = lib.pcm(lib.find("Action/Wipe_Table/Wipe_Table_01.wav").unwrap()).unwrap();
    assert_eq!((by_name.frames(), by_name.sample_rate), (8000, 8000));
    let by_id = lib.pcm(SoundId { bank: 0, index: 1 }).unwrap();
    assert_eq!(by_id.frames(), 4000);
    // untouched entries still come from the bank
    let other = lib.pcm(SoundId { bank: 0, index: 2 }).unwrap();
    assert_ne!(other.frames(), 8000);
    let _ = std::fs::remove_dir_all(md);
}
