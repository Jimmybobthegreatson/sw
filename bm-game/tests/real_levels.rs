//! Headless simulation of the real Hacienda (M01) and the hideout. Skipped when the game directory is absent.

use bm_game::*;
use std::sync::Arc;
use std::time::Instant;

fn vfs() -> Option<Arc<bm_vfs::Vfs>> {
    bm_vfs::game_dir_if_present()?;
    Some(Arc::new(bm_vfs::Vfs::from_env().ok()?))
}

/// A scripted player: walk forward, turn, run, stop, look around.
fn script(tick: usize) -> PlayerInput {
    let phase = (tick / 180) % 6;
    match phase {
        0 => PlayerInput { move_y: 1.0, ..Default::default() },
        1 => PlayerInput { move_y: 1.0, look_dx: 0.02, run: true, ..Default::default() },
        2 => PlayerInput { move_x: 1.0, ..Default::default() },
        3 => PlayerInput { move_y: -1.0, look_dx: -0.015, ..Default::default() },
        4 => PlayerInput { move_y: 1.0, run: true, look_dx: 0.01, ..Default::default() },
        _ => PlayerInput::default(),
    }
}

fn fingerprint(g: &Game) -> Vec<(u64, [i32; 3])> {
    // quantised to 1/10 cm so the comparison is robust against nothing but real divergence
    g.world.transforms.iter().map(|(i, t)| (i.0, [(t.pos.x * 10.0) as i32, (t.pos.y * 10.0) as i32, (t.pos.z * 10.0) as i32])).collect()
}

fn run(vfs: Arc<bm_vfs::Vfs>, base: &str, seconds: usize, seed: u64) -> Game {
    let t0 = Instant::now();
    let mut g = Game::load(vfs, base, &LoadOptions { seed, ..Default::default() }).expect("level loads");
    eprintln!("{base}: loaded in {:.2}s {:?}", t0.elapsed().as_secs_f32(), g.stats);
    let pid = g.world.player.expect("player");
    let start = g.world.pos(pid).unwrap();
    let t1 = Instant::now();
    let mut min_y = f32::MAX;
    for i in 0..seconds * 60 {
        g.tick(&script(i));
        let p = g.world.pos(pid).unwrap();
        assert!(p.is_finite(), "player position finite");
        min_y = min_y.min(p.y);
        assert!(p.y > g.world.meta.kill_y, "player fell out of the world at tick {i}: {p:?}");
    }
    eprintln!("{base}: {seconds}s simulated in {:.2}s, player {start:?} -> {:?}, min_y {min_y}", t1.elapsed().as_secs_f32(), g.world.pos(pid).unwrap());
    g
}

#[test]
fn m01_hacienda_runs_headless() {
    let Some(vfs) = vfs() else { return };
    let g = run(vfs, "Scenes/M01/M01_main", 20, 1);
    // level content
    assert!(g.stats.collision_triangles > 50_000, "collision built: {:?}", g.stats);
    assert!(g.stats.npcs >= 60, "NPCs spawned: {:?}", g.stats);
    assert!(g.stats.doors >= 10, "doors found: {:?}", g.stats);
    assert!(g.stats.lights > 100 && g.stats.markers > 10);
    assert!(g.stats.npcs_with_routes > 10, "patrol routes resolved: {:?}", g.stats);
    assert_eq!(g.world.count_kind(Kind::Npc), g.stats.npcs);
    assert!(g.world.find("Characters!Special_characters!DonChilean").is_some());
    // mission data
    assert_eq!(g.world.mission.status, MissionStatus::Active);
    // objectives decoded from the LOC: two targets + the exit
    assert_eq!(g.world.mission.objectives.len(), 3);
    assert_eq!(g.world.mission.title(), "A VINTAGE YEAR");
    // the player stays on walkable ground
    let pid = g.world.player.unwrap();
    let body = g.world.bodies.get(pid).unwrap();
    assert!(body.grounded, "player grounded after the walk");
    // NPCs are standing on the ground too (not fallen through): everyone above the kill plane
    for (id, b) in g.world.bodies.iter() {
        assert!(b.pos.y > g.world.meta.kill_y, "{} fell: {:?}", g.world.infos.get(id).unwrap().name, b.pos);
    }
    let grounded = g.world.bodies.iter().filter(|(_, b)| b.grounded).count();
    assert!(grounded as f32 > g.world.bodies.len() as f32 * 0.8, "{grounded}/{} bodies grounded", g.world.bodies.len());
    // events: level load + spawns were raised
    let names: Vec<_> = g.events().iter().map(|e| e.event.name.as_str()).collect();
    assert!(names.contains(&"on_level_load") && names.contains(&"on_entity_spawn"));
}

#[test]
fn m01_is_deterministic() {
    let Some(vfs) = vfs() else { return };
    let a = run(vfs.clone(), "Scenes/M01/M01_main", 8, 42);
    let b = run(vfs, "Scenes/M01/M01_main", 8, 42);
    assert_eq!(fingerprint(&a), fingerprint(&b));
    assert_eq!(a.world.rng, b.world.rng);
}

#[test]
fn m01_player_can_shoot_a_guard_and_render_state_has_poses() {
    let Some(vfs) = vfs() else { return };
    let mut g = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
    // put the player 6 m in front of a guard, looking at it
    let guard = g.world.find("Characters!Guards!Outside!1_Sentries!Guard_ReceptionEntry").expect("guard");
    let gp = g.world.pos(guard).unwrap();
    let gy = g.world.transforms.get(guard).unwrap().yaw;
    let stand = gp + math::forward(gy) * 600.0;
    assert!(g.run_command(&format!("tp {} {} {}", stand.x, stand.y + 5.0, stand.z)).starts_with("ok"));
    for _ in 0..60 {
        g.tick(&PlayerInput::default());
    }
    let rs = g.render_state();
    let actors: Vec<_> = rs.entities.iter().filter(|e| e.kind == Kind::Npc).collect();
    assert!(actors.len() >= 60);
    let posed = actors.iter().filter(|e| e.skinning.as_ref().is_some_and(|s| s.len() > 50)).count();
    assert!(posed >= 50, "{posed} NPCs have skinning matrices");
    assert!(rs.entities.iter().any(|e| e.kind == Kind::Door));
    assert!(!rs.lights.is_empty());
    assert!(rs.camera.is_some());
    // damage through the normal pipeline
    let hp0 = g.world.healths.get(guard).unwrap().hp;
    g.world.requests.push(Request::Damage { victim: guard, attacker: g.world.player, amount: 20.0, weapon: Some("test".into()), zone: None, point: gp, dir: Vec3::UP });
    g.tick(&PlayerInput::default());
    assert!(g.world.healths.get(guard).map_or(true, |h| h.hp < hp0));
    assert!(g.events().iter().any(|e| e.event.name == "on_damage"));
}

#[test]
fn hideout_runs_headless() {
    let Some(vfs) = vfs() else { return };
    let g = run(vfs, "Scenes/hideout/hideout_main", 15, 1);
    assert!(g.stats.collision_triangles > 10_000, "{:?}", g.stats);
    assert!(g.world.player.is_some());
    assert_eq!(g.world.mission.status, MissionStatus::Active);
}

#[test]
fn m01_audio_music_and_particles_react_to_gameplay() {
    let Some(vfs) = vfs() else { return };
    let mut g = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
    let engine = bm_sound::MixerEngine::headless(48000);
    g.enable_audio(Box::new(engine.clone()), true).expect("audio");
    assert!(g.audio.as_ref().unwrap().music.is_some(), "music system created");
    // walk + draw + fire a few shots at the wall in front of the player
    let mut frames = 0;
    for i in 0..(60 * 6) {
        let input = PlayerInput { move_y: if i < 120 { 1.0 } else { 0.0 }, draw: i == 130, fire: i > 140 && i % 15 == 0, ..Default::default() };
        g.frame(1.0 / 60.0, &input);
        frames += 1;
        let _ = engine.render(800);
    }
    assert!(frames > 0);
    let a = g.audio.as_ref().unwrap();
    assert!(a.stats.played > 0, "sounds were started (footsteps / shots): {:?} failed {}", a.stats.played, a.stats.failed);
    let (spawned, ambient) = {
        let fx = g.fx.as_ref().unwrap();
        (fx.spawned, fx.ambient_count())
    };
    assert!(spawned > 0, "gunfire / impact effects were spawned");
    let rs = g.render_state();
    let alive = g.fx.as_ref().unwrap().system.alive_count();
    assert!(rs.particles.instances.len() + alive > 0 || ambient > 0);
}

#[test]
fn m01_bodies_are_not_embedded_in_walls() {
    let Some(vfs) = vfs() else { return };
    let g = run(vfs, "Scenes/M01/M01_main", 30, 3);
    let mut bad = Vec::new();
    for (id, b) in g.world.bodies.iter() {
        let mut out = Vec::new();
        for cy in [60.0f32, 100.0, 150.0] {
            g.world.collision.sphere_contacts(b.pos + v3(0.0, cy, 0.0), 30.0, false, &mut out);
        }
        let deepest = out.iter().filter(|c| c.normal.y.abs() < 0.7).map(|c| c.depth).fold(0.0f32, f32::max);
        if deepest > 22.0 {
            bad.push((g.world.infos.get(id).unwrap().name.clone(), deepest));
        }
    }
    eprintln!("embedded bodies: {bad:?}");
    assert!(bad.len() * 10 <= g.world.bodies.len(), "too many bodies stuck in geometry: {bad:?}");
}


#[test]
fn procedural_animation_calibrates_on_real_skeletons() {
    let Some(vfs) = vfs() else { return };
    let scene = bm_models::Scene::load(&vfs, "Scenes/M01/M01_main").unwrap();
    let prov = ProceduralProvider::new();
    for prim in [21516u32, 3549, 3089, 4601, 4909] {
        let sk = scene.model(prim).and_then(|m| m.skeleton.clone()).expect("skeleton");
        let foot = sk.bone_index("LEFT_FOOT").unwrap();
        let head = sk.bone_index("HEAD").unwrap();
        let z = |p: &bm_models::Pose| sk.world_matrices(p)[foot].translation()[2];
        let bind = bm_models::Pose::bind(&sk);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for i in 0..16 {
            let p = prov.sample(&sk, &AnimRequest { state: AnimState::Walk, phase: i as f32 / 16.0, ..Default::default() });
            lo = lo.min(z(&p) - z(&bind));
            hi = hi.max(z(&p) - z(&bind));
            // all matrices finite
            assert!(sk.skinning_matrices(&p).iter().all(|m| m.to_cols_array().iter().all(|v| v.is_finite())));
        }
        assert!(hi > 8.0 && lo < -8.0, "prim {prim}: foot swing {lo}..{hi}");
        let dead = prov.sample(&sk, &AnimRequest { state: AnimState::Dead, dead_blend: 1.0, ..Default::default() });
        let h = sk.world_matrices(&dead)[head].translation();
        assert!(h[1] < 60.0, "prim {prim}: dead head height {}", h[1]);
        let crouch = prov.sample(&sk, &AnimRequest { state: AnimState::Crouch, ..Default::default() });
        let ch = sk.world_matrices(&crouch)[head].translation()[1];
        let bh = sk.world_matrices(&bind)[head].translation()[1];
        assert!(ch < bh - 25.0, "prim {prim}: crouch lowers the head ({bh} -> {ch})");
    }
}

#[test]
fn decoded_clips_drive_the_real_skeletons() {
    let Some(vfs) = vfs() else { return };
    let scene = Arc::new(bm_models::Scene::load(&vfs, "Scenes/M01/M01_main").unwrap());
    let prov = SceneClipProvider::new(scene.clone(), Box::new(ProceduralProvider::new()));
    // the player skeleton (Hero set) and NPC skeletons (Male_Reg set, the tourist woman uses the female clips)
    for (prim, hero) in [(21516u32, true), (3549, false), (4087, false), (4601, false), (4909, false)] {
        let sk = scene.model(prim).and_then(|m| m.skeleton.clone()).expect("skeleton");
        let foot = sk.bone_index("LEFT_FOOT").unwrap();
        let head = sk.bone_index("HEAD").unwrap();
        let w = |p: &bm_models::Pose, b: usize| sk.world_matrices(p)[b].translation();
        for state in [AnimState::Walk, AnimState::Run, AnimState::Idle, AnimState::Dead, AnimState::Crouch] {
            let clip = prov.clip_for(&sk, state, hero);
            if matches!(state, AnimState::Walk | AnimState::Run | AnimState::Idle | AnimState::Dead) {
                assert!(clip.is_some(), "prim {prim} hero {hero}: no clip for {state:?}");
            }
        }
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for i in 0..16 {
            let p = prov.sample(&sk, &AnimRequest { state: AnimState::Walk, phase: i as f32 / 16.0, hero, ..Default::default() });
            let z = w(&p, foot)[2];
            lo = lo.min(z);
            hi = hi.max(z);
            let h = w(&p, head)[1];
            assert!(h > 135.0 && h < 190.0, "prim {prim}: walking head height {h}");
            assert!(sk.skinning_matrices(&p).iter().all(|m| m.to_cols_array().iter().all(|v| v.is_finite())));
        }
        assert!(hi - lo > 30.0, "prim {prim}: walk foot swing {lo}..{hi}");
        // death: the head ends up near the floor
        let dead = prov.sample(&sk, &AnimRequest { state: AnimState::Dead, time: 5.0, hero, ..Default::default() });
        let h = w(&dead, head)[1];
        assert!(h < 70.0, "prim {prim}: dead head height {h}");
    }
}

// ------------------------------------------------------------------------------------------ decoded level data

/// Teleports the player, runs a few ticks (the room tracker runs at 10 Hz) and returns the reported room.
fn room_after_tp(g: &mut Game, p: Vec3) -> Option<String> {
    g.run_command(&format!("tp {} {} {}", p.x, p.y, p.z));
    for _ in 0..12 {
        g.tick(&PlayerInput::default());
    }
    g.world.player_room_name().map(str::to_string)
}

#[test]
fn m01_rooms_follow_the_player() {
    let Some(vfs) = vfs() else { return };
    let mut g = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
    assert!(g.stats.rooms >= 40, "{:?}", g.stats);
    assert!(g.world.rooms.matched >= g.world.rooms.records - 2, "almost every RMI box is owned by a room");
    // walk through the house: yard -> upstairs hallway -> drug lab staircase
    let stops = [
        (v3(-1108.0, -150.0, 2488.0), "M01_Outside_01b"),
        (v3(4900.0, 100.0, -2600.0), "M01_Hacienda_Room_102"),
        (v3(5400.0, -900.0, -1500.0), "M01_DrugLab_Staircase_B101"),
    ];
    for (p, want) in stops {
        let got = room_after_tp(&mut g, p);
        assert_eq!(got.as_deref(), Some(want), "room at {p:?}");
    }
    g.present(0.016);
    assert_eq!(g.ui_state().room.as_deref(), Some("M01_DrugLab_Staircase_B101"));
    assert!(g.ui_state().location.as_deref().unwrap_or("").contains("DrugLab"));
    // every owned box centre is inside some room
    let idx = g.world.rooms.clone();
    let map = idx.map().unwrap();
    for (i, r) in map.records.iter().enumerate() {
        if r.volume() > 0.0 && idx.rooms.iter().any(|room| room.volumes.contains(&i)) {
            assert!(idx.room_at(Vec3::from_array(r.center)).is_some(), "record {i} centre {:?} not in any room", r.center);
        }
    }
    // room changes were raised as script events
    let changes: Vec<_> = g.events().iter().filter(|e| e.event.name == "on_room_change").collect();
    assert!(changes.len() >= 2, "{} room change events", changes.len());
    assert!(changes.iter().any(|e| e.event.payload["to"] == "M01_DrugLab_Staircase_B101"));
    // security zones come from the ZoneControl lists
    assert!(!g.world.areas.zones.is_empty());
    assert!(g.world.areas.room_zone.iter().any(|z| z.is_some()));
}

#[test]
fn m06_and_hideout_rooms_are_reported() {
    let Some(vfs) = vfs() else { return };
    for base in ["Scenes/M06/M06_main", "Scenes/hideout/hideout_main"] {
        let mut g = Game::load(vfs.clone(), base, &LoadOptions::default()).unwrap();
        let idx = g.world.rooms.clone();
        assert!(!idx.is_empty(), "{base}: no rooms");
        let mut tested = 0;
        let mut seen = std::collections::BTreeSet::new();
        for room in idx.rooms.iter().take(40) {
            let Some(&vi) = room.volumes.first() else { continue };
            let r = &idx.map().unwrap().records[vi];
            if r.volume() <= 0.0 {
                continue;
            }
            let c = Vec3::from_array(r.center);
            // the box centre may be shared with a smaller box of another room: only the lookup is checked
            let want = idx.name_at(c).map(str::to_string);
            let got = room_after_tp(&mut g, c);
            assert_eq!(got, want, "{base}: room at {c:?}");
            if let Some(name) = got {
                seen.insert(name);
            }
            tested += 1;
        }
        eprintln!("{base}: {} rooms, {tested} probed, {} distinct names reported", idx.rooms.len(), seen.len());
        assert!(tested >= 3 && seen.len() >= 3, "{base}: only {} distinct rooms reported", seen.len());
    }
}

#[test]
fn m01_doors_use_the_decoded_controllers() {
    let Some(vfs) = vfs() else { return };
    let mut g = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
    // 26 single + 21 multi doors; double doors have two leaves, sliding elevator doors too
    assert_eq!(g.stats.doors, 47, "{:?}", g.stats);
    assert!(g.stats.door_leaves > 60);
    let w = &g.world;
    let locked = w.doors.iter().filter(|(_, d)| d.locked).count();
    assert!(locked >= 5, "{locked} locked doors");
    assert!(w.doors.iter().any(|(_, d)| d.locked && d.lock_difficulty == "EASY"));
    assert!(w.doors.iter().any(|(_, d)| matches!(d.motion, DoorMotion::Slide { .. })), "sliding doors present");
    assert!(w.doors.iter().any(|(_, d)| d.open), "some doors start open (IDS_OPEN)");
    // double doors open together
    let (id, _) = w.doors.iter().find(|(i, d)| w.infos.get(*i).unwrap().name == "HO_DoorDA_01/D1" && !d.locked).expect("double door");
    assert_eq!(w.door_group(id).len(), 2);
    // opening one leaf swings both away from the opener
    let center = g.world.door_center(id).unwrap();
    let from = center + v3(0.0, 0.0, 250.0);
    g.world.toggle_door(id, from);
    for _ in 0..90 {
        g.world.update_doors(1.0 / 60.0);
    }
    for leaf in g.world.door_group(id) {
        let d = g.world.doors.get(leaf).unwrap();
        assert!(d.open && d.angle.abs() > 1.2, "leaf swung open: {}", d.angle);
    }
}

#[test]
fn m01_npcs_open_doors_on_their_patrol() {
    let Some(vfs) = vfs() else { return };
    let mut g = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
    for _ in 0..(60 * 100) {
        g.tick(&PlayerInput::default());
    }
    let opened: Vec<&str> = g.world.doors.iter().filter(|(_, d)| d.open_count > 0).map(|(i, _)| g.world.infos.get(i).unwrap().name.as_str()).collect();
    assert!(opened.len() >= 4, "NPCs opened {} door leaves: {opened:?}", opened.len());
    // doors opened by NPCs close again once nobody is near
    let npc_open = g.world.doors.iter().filter(|(_, d)| d.open && d.opened_by_npc).count();
    let total_opened = g.world.doors.iter().filter(|(_, d)| d.open_count > 0).count();
    assert!(npc_open < total_opened, "auto close: {npc_open} still open of {total_opened}");
}

#[test]
fn m01_mission_comes_from_the_level_data() {
    use bm_game::data::ObjectiveKind;
    let Some(vfs) = vfs() else { return };
    let mut g = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
    {
        let m = &g.world.mission;
        let def = m.def.as_ref().unwrap();
        assert_eq!(def.title, "A VINTAGE YEAR");
        let texts: Vec<_> = m.objectives.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["Kill Don Fernando Delgado", "Kill Manuel Delgado", "Escape the vineyard"]);
        assert!(matches!(&def.objectives[0].kind, ObjectiveKind::KillExact { entity } if entity == "Characters!Special_characters!DonChilean"));
        assert!(matches!(&def.objectives[1].kind, ObjectiveKind::KillExact { entity } if entity == "Characters!Special_characters!DonChileanSon"));
        assert!(matches!(def.objectives[2].kind, ObjectiveKind::ReachExit { index: 0 }));
        assert_eq!(def.objectives[2].requires.len(), 2);
    }
    // play it: kill both targets, then stand in the exit area
    for name in ["Characters!Special_characters!DonChilean", "Characters!Special_characters!DonChileanSon"] {
        let id = g.world.find(name).unwrap();
        g.world.kill_now(id, g.world.player, Some("test"));
    }
    for _ in 0..3 {
        g.tick(&PlayerInput::default());
    }
    assert_eq!(g.world.mission.status, MissionStatus::Active, "the exit is still to be reached");
    let exit = g.world.areas.exits().next().unwrap().1.center();
    g.run_command(&format!("tp {} {} {}", exit.x, exit.y, exit.z));
    for _ in 0..10 {
        g.tick(&PlayerInput::default());
    }
    assert_eq!(g.world.mission.status, MissionStatus::Complete);
    assert!(g.events().iter().any(|e| e.event.name == "on_trigger" && e.event.payload["kind"] == "exit"));
}

#[test]
fn objectives_exist_for_every_level() {
    let Some(vfs) = vfs() else { return };
    for dir in ["M00", "M02", "M03", "M05", "M06", "M08", "M09", "M10", "M11", "M12", "M13"] {
        let base = format!("Scenes/{dir}/{dir}_main");
        let g = Game::load(vfs.clone(), &base, &LoadOptions::default()).unwrap_or_else(|e| panic!("{base}: {e}"));
        let m = &g.world.mission;
        assert_eq!(m.status, MissionStatus::Active, "{base}: no mission");
        let kills = m.def.as_ref().unwrap().objectives.iter().filter(|o| matches!(o.kind, bm_game::data::ObjectiveKind::KillExact { .. })).count();
        eprintln!("{base}: {} objectives ({} mapped kills, {} exits): {:?}", m.objectives.len(), kills, g.stats.exits, m.objectives.iter().map(|o| o.text.as_str()).collect::<Vec<_>>());
        // M13 only has an optional "no witnesses" objective
        assert!(!m.objectives.is_empty() && (kills >= 1 || dir == "M13"), "{base}: {:?}", m.objectives);
    }
}

#[test]
fn m01_room_drives_reverb_and_music_zone() {
    let Some(vfs) = vfs() else { return };
    let mut g = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
    let engine = bm_sound::MixerEngine::headless(48000);
    g.enable_audio(Box::new(engine.clone()), true).expect("audio");
    let idx = g.world.rooms.clone();
    let mut checked_zone = 0;
    let mut checked_reverb = 0;
    for room in idx.rooms.iter() {
        let zone = g.audio.as_ref().unwrap().zone_for_room(&room.name);
        let Some(&vi) = room.volumes.first() else { continue };
        let c = Vec3::from_array(idx.map().unwrap().records[vi].center);
        if idx.name_at(c) != Some(room.name.as_str()) {
            continue;
        }
        g.run_command(&format!("tp {} {} {}", c.x, c.y, c.z));
        for _ in 0..12 {
            g.frame(1.0 / 60.0, &PlayerInput::default());
        }
        let _ = engine.render(512);
        let a = g.audio.as_ref().unwrap();
        assert_eq!(a.room.as_deref(), Some(room.name.as_str()));
        if let Some(z) = zone {
            assert_eq!(a.music_zone, Some(z), "music zone of {}", room.name);
            checked_zone += 1;
        }
        if a.reverb_room.is_some() {
            checked_reverb += 1;
        }
    }
    assert!(checked_zone >= 4, "{checked_zone} rooms with a music zone were visited");
    assert!(checked_reverb >= 4, "{checked_reverb} rooms with a reverb preset were visited");
}

// ------------------------------------------------------------------------------------------------- saves

#[test]
fn m01_save_and_load_restores_state() {
    let Some(vfs) = vfs() else { return };
    let base = "Scenes/M01/M01_main";
    let mut g = Game::load(vfs.clone(), base, &LoadOptions { seed: 7, ..Default::default() }).unwrap();
    for i in 0..180 {
        g.tick(&script(i));
    }
    // change things that must survive: a dead target, an opened door, a taken pickup, the player's place
    let don = g.world.find("Characters!Special_characters!DonChilean").unwrap();
    g.world.kill_now(don, g.world.player, Some("test"));
    let (door, _) = g.world.doors.iter().find(|(i, d)| !d.locked && !d.open && g.world.infos.get(*i).unwrap().name == "HO_DoorDA_01/D1").unwrap();
    let from = g.world.door_center(door).unwrap() + v3(0.0, 0.0, 250.0);
    g.world.toggle_door(door, from);
    let (pick, _) = g.world.pickups.iter().find(|(_, p)| p.weapon == "hammer").unwrap();
    let pid = g.world.player.unwrap();
    g.world.requests.push(Request::Pickup { who: pid, item: pick });
    g.run_command("tp 4900 100 -2600");
    for _ in 0..120 {
        g.tick(&PlayerInput::default());
    }
    let items_before = g.world.players.get(pid).unwrap().items.len();
    let saved = g.save_state("test save");
    assert_eq!(saved.version, SAVE_VERSION);
    assert_eq!(saved.room.as_deref(), Some("M01_Hacienda_Room_102"));

    // through JSON and a file
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slot.json");
    saved.write(&path).unwrap();
    let loaded = SaveGame::read(&path).unwrap();
    assert_eq!(loaded.npcs.len(), saved.npcs.len());

    let mut g2 = Game::load(vfs.clone(), base, &LoadOptions { seed: 7, ..Default::default() }).unwrap();
    let don2 = g2.world.find("Characters!Special_characters!DonChilean").unwrap();
    assert!(g2.world.is_alive(don2), "a fresh level starts with the target alive");
    let rep = g2.load_state(&loaded).unwrap();
    assert!(rep.npcs >= 60 && rep.doors >= 60 && rep.pickups >= 30, "{rep:?}");
    assert_eq!(rep.npcs_missing, 0);
    let pid2 = g2.world.player.unwrap();
    assert!(g2.world.pos(pid2).unwrap().dist(g.world.pos(pid).unwrap()) < 0.5);
    assert!(!g2.world.is_alive(don2));
    assert_eq!(g2.world.players.get(pid2).unwrap().items.len(), items_before);
    assert_eq!(g2.world.time, g.world.time);
    assert_eq!(g2.world.rng, g.world.rng);
    let d2 = g2.world.doors.iter().find(|(i, _)| g2.world.infos.get(*i).unwrap().name == "HO_DoorDA_01/D1" && g2.world.infos.get(*i).unwrap().node == g.world.infos.get(door).unwrap().node).unwrap().1;
    assert!(d2.open && (d2.angle - g.world.doors.get(door).unwrap().angle).abs() < 1e-4);
    assert!(g2.world.pickups.iter().any(|(_, p)| p.taken && p.weapon == "hammer"));
    assert_eq!(g2.world.mission.objectives[0].done, g.world.mission.objectives[0].done);
    // the loaded game keeps running and re-announces the room
    for _ in 0..30 {
        g2.tick(&PlayerInput::default());
    }
    assert_eq!(g2.world.player_room_name(), Some("M01_Hacienda_Room_102"));
    assert!(g2.events().iter().any(|e| e.event.name == "on_game_loaded"));
    // the console commands go through the same code (slot files live in the user dir, not exercised here)
    assert!(g2.run_command("load nosuchslot_xyz").contains("no such slot"));

    // wrong level / newer version are refused
    let mut hide = Game::load(vfs, "Scenes/hideout/hideout_main", &LoadOptions::default()).unwrap();
    assert!(matches!(hide.load_state(&loaded), Err(SaveError::WrongLevel { .. })));
    let mut newer = loaded.clone();
    newer.version = SAVE_VERSION + 1;
    assert!(matches!(SaveGame::from_json(&newer.to_json().unwrap()), Err(SaveError::TooNew { .. })));
}

#[test]
fn saves_tolerate_missing_fields() {
    // an older / hand edited file with only a few fields still parses (serde defaults)
    let s = SaveGame::from_json(r#"{"version":1,"level":"Scenes/M01/M01_main","player":{"pos":[1,2,3]}}"#).unwrap();
    assert_eq!(s.player.pos, [1.0, 2.0, 3.0]);
    assert!(s.npcs.is_empty());
    assert!(SaveGame::from_json("not json").is_err());
}

#[test]
fn m01_room_portals_connect_rooms_and_sit_on_doors() {
    let Some(vfs) = vfs() else { return };
    let g = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default()).unwrap();
    let idx = g.world.rooms.clone();
    assert!(idx.portals.len() >= 60, "{} portals", idx.portals.len());
    // the volumes on either side of a portal belong to its two rooms (the boxes are loose, so only most do)
    let (mut ok, mut total) = (0, 0);
    for p in &idx.portals {
        total += 1;
        let before = idx.room_at(p.center + p.normal * 45.0);
        let after = idx.room_at(p.center - p.normal * 45.0);
        let pair = [before, after];
        if pair.contains(&Some(p.a)) || pair.contains(&Some(p.b)) {
            ok += 1;
        }
    }
    assert!(ok * 100 >= total * 70, "only {ok}/{total} portals touch one of their rooms");
    // the hacienda's front door joins the yard and a ground floor room; the whole house is connected to the outside
    let outside = idx.find("M01_Outside_01a").unwrap();
    assert!(idx.neighbors(outside).iter().any(|&n| idx.rooms[n].name.contains("Hacienda")), "front door");
    let kitchen = idx.find("M01_Hacienda_Kitchen_101").unwrap();
    let route = idx.route(kitchen, outside).expect("route kitchen -> outside");
    assert!(route.len() >= 2 && *route.last().unwrap() == outside);
    let upstairs = idx.find("M01_Hacienda_Room_201").unwrap();
    assert!(idx.route(upstairs, outside).is_some(), "upper floor reachable");
    // most doors stand in a portal
    let mut doors = 0;
    let mut on_portal = 0;
    for (id, d) in g.world.doors.iter() {
        if d.group != id.0 {
            continue; // one leaf per door
        }
        let c = g.world.door_center(id).unwrap();
        doors += 1;
        if idx.portals.iter().any(|p| p.center.dist(c) < 220.0) {
            on_portal += 1;
        }
    }
    assert!(doors >= 40 && on_portal * 100 >= doors * 50, "{on_portal}/{doors} doors have a portal within 2.2 m");
}
