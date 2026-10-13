//! Headless gameplay tests on a synthetic level (no game files needed).

use bm_game::collision::{Surface, Tri};
use bm_game::world::{EntityId, Kind, Request, Transform};
use bm_game::*;
use bm_script::{ScriptHost, ScriptRuntime};
use serde_json::json;
use std::fs;
use std::path::Path;

fn quad(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> Vec<Tri> {
    vec![Tri::new(a, b, c, Surface::Stone), Tri::new(a, c, d, Surface::Stone)]
}

fn floor(half: f32) -> Vec<Tri> {
    quad(v3(-half, 0.0, -half), v3(half, 0.0, -half), v3(half, 0.0, half), v3(-half, 0.0, half))
}

/// Vertical wall in the plane `z`, x in `[x0, x1]`, 300 cm high.
fn wall_z(z: f32, x0: f32, x1: f32) -> Vec<Tri> {
    quad(v3(x0, 0.0, z), v3(x1, 0.0, z), v3(x1, 300.0, z), v3(x0, 300.0, z))
}

fn world(tris: Vec<Tri>, seed: u64) -> World {
    World::new(CollisionWorld::new(tris), GameData::embedded(), seed)
}

fn forward_input() -> PlayerInput {
    PlayerInput { move_y: 1.0, ..Default::default() }
}

fn player_y(g: &Game) -> f32 {
    g.world.pos(g.world.player.unwrap()).unwrap().y
}

#[test]
fn player_stays_on_the_floor() {
    let mut w = world(floor(3000.0), 1);
    w.spawn_player(v3(0.0, 80.0, 0.0), 0.0, None, None);
    let mut g = Game::from_world(w);
    let mut inputs = vec![forward_input(), PlayerInput { move_x: 1.0, run: true, ..Default::default() }, PlayerInput { move_y: -1.0, ..Default::default() }];
    inputs.push(PlayerInput { crouch: true, move_y: 1.0, ..Default::default() });
    for _ in 0..60 {
        g.tick(&PlayerInput::default()); // fall onto the floor
    }
    for chunk in 0..8 {
        let inp = inputs[chunk % inputs.len()].clone();
        for _ in 0..120 {
            g.tick(&inp);
            assert!(player_y(&g).abs() < 0.6, "y = {}", player_y(&g));
        }
    }
    let p = g.world.player_pos().unwrap();
    assert!(p.x.abs() > 1.0 || p.z.abs() > 1.0, "the player moved: {p:?}");
}

#[test]
fn camera_follows_behind_the_player_and_stays_out_of_walls() {
    let mut tris = floor(3000.0);
    tris.extend(wall_z(-120.0, -1000.0, 1000.0));
    let mut w = world(tris, 1);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    let mut g = Game::from_world(w);
    for _ in 0..60 {
        g.tick(&PlayerInput::default());
    }
    let cam = g.world.camera().unwrap();
    // wanted 260 cm behind (z = -260) but the wall at z = -120 is in the way
    assert!(cam.pos.z > -120.0, "camera clipped through the wall: {:?}", cam.pos);
    assert!(cam.forward.z > 0.9);
}

fn guard_world(seed: u64) -> Game {
    let mut w = world(floor(4000.0), seed);
    w.spawn_player(v3(0.0, 0.0, -600.0), 0.0, None, None);
    let route = vec![v3(1200.0, 0.0, 400.0), v3(1200.0, 0.0, 1200.0), v3(300.0, 0.0, 1200.0)];
    w.spawn_npc("Guard_A", Role::Guard, v3(300.0, 0.0, 400.0), 0.0, route, None, None);
    w.spawn_npc("Civilian_A", Role::Civilian, v3(-800.0, 0.0, 800.0), 1.0, vec![], None, None);
    Game::from_world(w)
}

#[test]
fn npcs_spawn_patrol_and_run_deterministically() {
    let run = |seed: u64| {
        let mut g = guard_world(seed);
        assert_eq!(g.world.count_kind(Kind::Npc), 2);
        let start = g.world.pos(g.world.find("Guard_A").unwrap()).unwrap();
        for _ in 0..(60 * 25) {
            g.tick(&PlayerInput::default());
        }
        let guard = g.world.find("Guard_A").unwrap();
        let p = g.world.pos(guard).unwrap();
        assert!(p.dist_xz(start) > 100.0, "guard patrolled: {start:?} -> {p:?}");
        assert!(p.y.abs() < 0.6);
        let snapshot: Vec<(u64, [f32; 3])> = g.world.transforms.iter().map(|(i, t)| (i.0, t.pos.to_array())).collect();
        (snapshot, g.world.rng.clone(), g.take_events().len())
    };
    let a = run(7);
    let b = run(7);
    assert_eq!(a, b, "same seed must give identical state");
    let c = run(8);
    assert_eq!(a.0.len(), c.0.len());
}

fn aim_level_input() -> PlayerInput {
    // pitch up from the default (-0.15) to look level, draw the weapon
    PlayerInput { look_dy: 0.15, draw: true, ..Default::default() }
}

/// Player at the origin facing +Z with a guard 800 cm ahead. Returns (game, guard id).
fn duel() -> (Game, EntityId) {
    let mut w = world(floor(4000.0), 3);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    let g = w.spawn_npc("Guard_Target", Role::Guard, v3(0.0, 0.0, 800.0), std::f32::consts::PI, vec![], None, None);
    let mut game = Game::from_world(w);
    game.tick(&aim_level_input());
    game.tick(&PlayerInput::default());
    (game, g)
}

#[test]
fn hitscan_kill_fires_script_events_and_completes_the_objective() {
    let (mut g, guard) = duel();
    g.world.start_mission(
        MissionDef::parse(
            r#"(id: "T", title: "t", level: "", objectives: [
                (id: "k", text: "kill", kind: Kill(entity: "Guard_Target")),
                (id: "x", text: "exit", kind: ReachPoint(pos: (0.0, 0.0, 0.0), radius: 200.0), requires: ["k"]),
            ], fail: [PlayerDead])"#,
        )
        .unwrap(),
    );
    // fire: rising edge each time
    let mut fired = 0;
    for i in 0..400 {
        let fire = i % 20 == 0 && g.world.is_alive(guard);
        if fire {
            fired += 1;
        }
        g.tick(&PlayerInput { fire, ..Default::default() });
        if !g.world.is_alive(guard) {
            break;
        }
    }
    assert!(!g.world.is_alive(guard), "guard should be dead after {fired} shots");
    // let the mission logic run
    for _ in 0..30 {
        g.tick(&PlayerInput::default());
    }
    let names: Vec<String> = g.events().iter().map(|e| e.event.name.clone()).collect();
    for want in ["on_level_load", "on_entity_spawn", "on_damage", "on_kill", "on_mission_event"] {
        assert!(names.iter().any(|n| n == want), "missing {want} in {names:?}");
    }
    let dmg = g.events().iter().position(|e| e.event.name == "on_damage").unwrap();
    let kill = g.events().iter().position(|e| e.event.name == "on_kill").unwrap();
    assert!(dmg < kill, "damage precedes the kill");
    let kill_ev = &g.events()[kill].event;
    assert_eq!(kill_ev.payload["victim"], json!(guard.0));
    assert_eq!(kill_ev.payload["killer"], json!(g.world.player.unwrap().0));
    assert_eq!(g.world.mission.status, MissionStatus::Complete, "kill + standing on the exit point completes the mission");
    // the UI reflects it
    g.present(0.016);
    assert_eq!(g.ui.mission_status, MissionStatus::Complete);
    assert!(g.ui.vars.contains_key("health") && g.ui.vars.contains_key("ammo_clip"));
}

#[test]
fn guard_notices_the_armed_player_and_shoots_back() {
    let mut w = world(floor(4000.0), 11);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    // guard faces the player from 900 cm away
    w.spawn_npc("Guard_B", Role::Guard, v3(0.0, 0.0, 900.0), std::f32::consts::PI, vec![], None, None);
    let mut g = Game::from_world(w);
    g.tick(&PlayerInput { draw: true, ..Default::default() }); // weapon out
    let mut saw_alert = false;
    let mut hurt = false;
    for _ in 0..(60 * 20) {
        g.tick(&PlayerInput::default());
        if g.max_npc_alert() == AlertState::Alerted {
            saw_alert = true;
        }
        let hp = g.world.healths.get(g.world.player.unwrap()).unwrap().hp;
        if hp < 100.0 {
            hurt = true;
            break;
        }
    }
    assert!(saw_alert, "guard must become alerted when it sees a drawn weapon");
    assert!(hurt, "alerted guard shoots the player");
}

#[test]
fn walls_block_line_of_sight_for_perception() {
    let mut tris = floor(4000.0);
    tris.extend(wall_z(450.0, -2000.0, 2000.0));
    let mut w = world(tris, 5);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    w.spawn_npc("Guard_C", Role::Guard, v3(0.0, 0.0, 900.0), std::f32::consts::PI, vec![], None, None);
    let mut g = Game::from_world(w);
    g.tick(&PlayerInput { draw: true, ..Default::default() });
    for _ in 0..(60 * 10) {
        g.tick(&PlayerInput::default());
    }
    assert_eq!(g.max_npc_alert(), AlertState::Idle, "no line of sight, no alert");
}

fn door_game() -> (Game, EntityId) {
    let mut tris = floor(3000.0);
    tris.extend(wall_z(300.0, -1500.0, -60.0));
    tris.extend(wall_z(300.0, 60.0, 1500.0));
    let mut w = world(tris, 2);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    // door leaf: hinge at x = -60, extends to x = +60 when closed
    let closed = Transform::from_pose(v3(-60.0, 0.0, 300.0), 0.0).world;
    let leaf = quad(v3(0.0, 0.0, 0.0), v3(120.0, 0.0, 0.0), v3(120.0, 250.0, 0.0), v3(0.0, 250.0, 0.0));
    let h = w.collision.add_dynamic(leaf, &closed);
    let door = w.spawn_door("HO_DoorSA_01", closed, Some(h), None, None);
    (Game::from_world(w), door)
}

#[test]
fn closed_door_blocks_then_opens_on_interact() {
    let (mut g, door) = door_game();
    for _ in 0..240 {
        g.tick(&forward_input());
    }
    let z = g.world.player_pos().unwrap().z;
    assert!(z < 300.0 - 25.0, "blocked by the closed door, z = {z}");
    // the prompt is offered
    assert_eq!(g.world.players.get(g.world.player.unwrap()).unwrap().prompt.as_ref().map(|p| p.action.as_str()), Some("open_door"));
    g.tick(&PlayerInput { interact: true, ..Default::default() });
    for _ in 0..90 {
        g.tick(&PlayerInput::default());
    }
    assert!(g.world.doors.get(door).unwrap().open);
    for _ in 0..240 {
        g.tick(&forward_input());
    }
    let z = g.world.player_pos().unwrap().z;
    assert!(z > 330.0, "walked through the open door, z = {z}");
    assert!(g.events().iter().any(|e| e.event.name == "on_action" && e.event.payload["action"] == "open_door"));
}

#[test]
fn pickups_are_added_to_the_inventory() {
    let mut w = world(floor(2000.0), 4);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    let item = w.spawn_pickup("Items!WeaponsLyingAround!Hammer_Test", "hammer", Transform::from_pose(v3(0.0, 0.0, 90.0), 0.0).world, None);
    let mut g = Game::from_world(w);
    for _ in 0..5 {
        g.tick(&PlayerInput::default());
    }
    let pid = g.world.player.unwrap();
    assert_eq!(g.world.players.get(pid).unwrap().prompt.as_ref().map(|p| p.action.clone()), Some("pick_up".to_string()));
    let before = g.world.players.get(pid).unwrap().items.len();
    g.tick(&PlayerInput { interact: true, ..Default::default() });
    g.tick(&PlayerInput::default());
    assert_eq!(g.world.players.get(pid).unwrap().items.len(), before + 1);
    assert!(g.world.pickups.get(item).unwrap().taken);
    assert!(g.events().iter().any(|e| e.event.name == "on_pickup" && e.event.payload["item"] == "hammer"));
}

// ------------------------------------------------------------------------------------------- scripts

fn mk_mod(root: &Path, id: &str, lua: &str) {
    let dir = root.join(id);
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::write(
        dir.join("mod.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\npermissions = [\"world\", \"entities.read\", \"entities.write\", \"ui\", \"audio\"]\n[scripts]\nentry = [\"scripts/main.lua\"]\n"
        ),
    )
    .unwrap();
    fs::write(dir.join("scripts/main.lua"), lua).unwrap();
}

fn attach(g: &mut Game, tmp: &Path, lua: &str) {
    let mods = tmp.join("Mods");
    mk_mod(&mods, "testmod", lua);
    let dirs = bm_script::ModDirs::with_state_dir(vec![mods], &tmp.join("state"));
    let mgr = bm_script::ModManager::new(dirs).unwrap();
    let set = mgr.resolve();
    let mut rt = ScriptRuntime::new(g.host(), bm_script::RuntimeConfig::default());
    let (_, reports) = rt.load_mods(&set);
    assert!(reports.iter().all(|r| r.ok), "{reports:?}");
    g.attach_scripts(rt);
}

#[test]
fn scripts_can_veto_and_scale_damage_and_react_to_kills() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut g, guard) = duel();
    attach(
        &mut g,
        tmp.path(),
        r#"
        local vetoed = 0
        events.on("on_damage", function(ev)
          if vetoed < 1 then vetoed = vetoed + 1; ev.cancelled = true; return end
          ev.amount = ev.amount * 0.1
        end)
        events.on("on_kill", function(ev)
          entity.spawn("npc_civilian", { name = "ScriptSpawned", pos = { x = 500, y = 0, z = -500 } })
          ui.message("boom", 3)
        end)
        timer.after(0.5, function() ui.message("timer fired", 2) end)
        "#,
    );
    // first shot is vetoed
    g.tick(&PlayerInput { fire: true, ..Default::default() });
    g.tick(&PlayerInput::default());
    assert_eq!(g.world.healths.get(guard).unwrap().hp, 60.0, "vetoed hit does not hurt");
    assert!(g.events().iter().any(|e| e.cancelled && e.event.name == "on_damage"));
    // later shots are scaled to 10 %: head shot = 34 * 3 * 0.1
    for _ in 0..30 {
        g.tick(&PlayerInput::default());
    }
    g.tick(&PlayerInput { fire: true, ..Default::default() });
    g.tick(&PlayerInput::default());
    let hp = g.world.healths.get(guard).unwrap().hp;
    let last = g.events().iter().rev().find(|e| e.event.name == "on_damage" && !e.cancelled).unwrap();
    let amount = last.event.payload["amount"].as_f64().unwrap() as f32;
    let zone = last.event.payload["zone"].as_str().unwrap_or("").to_string();
    let base = 34.0 * if zone == "head" { 3.0 } else if zone == "legs" { 0.6 } else { 1.0 };
    assert!((amount - base * 0.1).abs() < 0.01, "the script scaled the amount to 10 %: {amount} for a {zone} hit");
    assert!((hp - (60.0 - amount)).abs() < 0.01, "scaled damage applied, hp = {hp}");
    // keep shooting until it dies -> on_kill -> script spawns a civilian
    for i in 0..4000 {
        if !g.world.is_alive(guard) {
            break;
        }
        g.tick(&PlayerInput { fire: i % 16 == 0, ..Default::default() });
    }
    assert!(!g.world.is_alive(guard));
    g.tick(&PlayerInput::default());
    g.tick(&PlayerInput::default());
    let spawned = g.world.find("ScriptSpawned").expect("script spawned an entity");
    assert_eq!(g.world.infos.get(spawned).unwrap().kind, Kind::Npc);
    // timers and ui messages
    for _ in 0..60 {
        g.tick(&PlayerInput::default());
    }
    g.present(0.016);
    let msgs: Vec<_> = g.ui.messages.iter().map(|m| m.text.clone()).collect();
    assert!(msgs.iter().any(|m| m == "boom") || msgs.iter().any(|m| m == "timer fired"), "{msgs:?}");
}

#[test]
fn script_host_reads_the_world() {
    let (mut g, guard) = duel();
    g.tick(&PlayerInput::default());
    let host = g.host();
    host.refresh(&g.world);
    assert_eq!(host.player(), g.world.player);
    assert!(host.game_time() > 0.0);
    let info = host.entity_info(guard).unwrap();
    assert_eq!(info.name, "Guard_Target");
    assert_eq!(info.kind, "npc");
    assert!((info.position.z - 800.0).abs() < 1.0);
    assert_eq!(host.find_entity("guard_target"), Some(guard));
    let q = host.query_entities(&bm_script::EntityQuery { kind: Some("npc".into()), alive_only: true, ..Default::default() });
    assert_eq!(q.len(), 1);
    let hit = host.raycast(bm_script::Vec3::new(0.0, 100.0, 60.0), bm_script::Vec3::new(0.0, 100.0, 2000.0)).unwrap().unwrap();
    assert_eq!(hit.entity, Some(guard));
    assert!(hit.distance > 700.0 && hit.distance < 800.0);
    let down = host.raycast(bm_script::Vec3::new(100.0, 100.0, 100.0), bm_script::Vec3::new(100.0, -100.0, 100.0)).unwrap().unwrap();
    assert!(down.entity.is_none() && (down.position.y).abs() < 0.1);
    assert_eq!(host.get_component(guard, "health").unwrap().unwrap()["hp"], json!(60.0));
    // writes are queued, then applied by the game
    host.damage_entity(guard, 10.0, g.world.player).unwrap();
    let id = host.spawn_entity(&bm_script::SpawnRequest { template: "npc_guard".into(), name: Some("Hostile".into()), ..Default::default() }).unwrap();
    assert!(host.entity_info(id).is_some(), "spawned entities are visible immediately");
    assert!(host.spawn_entity(&bm_script::SpawnRequest { template: "bogus".into(), ..Default::default() }).is_err());
    g.apply_host_commands();
    g.world.resolve_requests(&mut |_, _| true);
    assert_eq!(g.world.healths.get(guard).unwrap().hp, 50.0);
    assert_eq!(g.world.find("Hostile"), Some(id));
}

#[test]
fn requests_can_be_vetoed_directly() {
    let (mut g, guard) = duel();
    g.world.requests.push(Request::Damage { victim: guard, attacker: None, amount: 99.0, weapon: None, zone: None, point: Vec3::ZERO, dir: Vec3::UP });
    g.world.resolve_requests(&mut |_, _| false);
    assert!(g.world.is_alive(guard));
    g.world.requests.push(Request::Damage { victim: guard, attacker: None, amount: 99.0, weapon: None, zone: None, point: Vec3::ZERO, dir: Vec3::UP });
    g.world.resolve_requests(&mut |_, _| true);
    assert!(!g.world.is_alive(guard));
}

#[test]
fn render_and_ui_state_are_populated() {
    let (mut g, _guard) = duel();
    let rs = g.render_state();
    assert!(rs.camera.is_some());
    assert!(rs.entities.iter().any(|e| e.kind == Kind::Player));
    assert!(rs.entities.iter().any(|e| e.kind == Kind::Npc));
    for e in &rs.entities {
        assert!(e.world.translation().iter().all(|v| v.is_finite()));
    }
    g.frame(0.05, &PlayerInput::default());
    assert!(g.ui_state().vars.contains_key("health"));
    // a fixed step accumulator: 0.05 s -> 3 ticks
    let t0 = g.world.tick;
    g.frame(0.05, &PlayerInput::default());
    assert_eq!(g.world.tick - t0, 3);
}

#[test]
fn console_commands() {
    let (mut g, _) = duel();
    assert!(g.run_command("tp 100 0 100").starts_with("ok"));
    g.tick(&PlayerInput::default());
    let p = g.world.player_pos().unwrap();
    assert!((p.x - 100.0).abs() < 1.0 && (p.z - 100.0).abs() < 1.0);
    assert!(g.run_command("give smg").starts_with("ok"));
    assert_eq!(g.run_command("kill guard_target"), "killed 1");
}

// ---------------------------------------------------------------------- doors, areas, saves (synthetic)

/// A wall with a doorway at x in [-60, 60] closed by a double door (two leaves hinged at the outer edges, `+X` extending
/// leaves) at z = 300; returns the game and the two leaf entities.
fn double_door_game(npc: bool) -> (Game, EntityId, EntityId) {
    let mut tris = floor(3000.0);
    tris.extend(wall_z(300.0, -1500.0, -60.0));
    tris.extend(wall_z(300.0, 60.0, 1500.0));
    let mut w = world(tris, 3);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    if npc {
        // the guard patrols between two points on either side of the door
        let route = vec![v3(0.0, 0.0, 800.0), v3(0.0, 0.0, 0.0)];
        w.spawn_npc("Guard_Door", Role::Guard, v3(0.0, 0.0, 700.0), std::f32::consts::PI, route, None, None);
    }
    // left leaf: hinge x = -60, extends +x to 0; right leaf: hinge x = +60, extends -x (its frame is turned 180 degrees)
    let left_closed = Transform::from_pose(v3(-60.0, 0.0, 300.0), 0.0).world;
    let right_closed = Transform::from_pose(v3(60.0, 0.0, 300.0), std::f32::consts::PI).world;
    let leaf = quad(v3(0.0, 0.0, 0.0), v3(60.0, 0.0, 0.0), v3(60.0, 250.0, 0.0), v3(0.0, 250.0, 0.0));
    let hl = w.collision.add_dynamic(leaf.clone(), &left_closed);
    let hr = w.collision.add_dynamic(leaf, &right_closed);
    let l = w.spawn_door("HO_DoorDA_01/D1", left_closed, Some(hl), None, None);
    let r = w.spawn_door("HO_DoorDA_01/D2", right_closed, Some(hr), None, None);
    for id in [l, r] {
        let d = w.doors.get_mut(id).unwrap();
        d.group = l.0;
        d.motion = DoorMotion::Swing { ext: 1.0, max: std::f32::consts::FRAC_PI_2 * 0.95 };
    }
    (Game::from_world(w), l, r)
}

#[test]
fn double_doors_open_together_away_from_the_opener() {
    let (mut g, l, r) = double_door_game(false);
    assert_eq!(g.world.door_group(l).len(), 2);
    // opener at z = 0 (before the door): both leaves swing towards +z
    g.world.toggle_door(r, v3(0.0, 0.0, 0.0));
    for _ in 0..90 {
        g.world.update_doors(1.0 / 60.0);
    }
    for id in [l, r] {
        let d = g.world.doors.get(id).unwrap();
        assert!(d.open && d.angle.abs() > 1.3, "leaf open: {}", d.angle);
        // the free end of the leaf moved to the far side (+z) of the wall line
        let (lo, hi) = g.world.collision.dynamics[d.collider.unwrap() as usize].aabb();
        assert!(hi.z > 340.0 && lo.z > 290.0, "leaf swung away from the opener: {lo:?} {hi:?}");
    }
    // walking straight through the opening works now
    for _ in 0..360 {
        g.tick(&forward_input());
    }
    assert!(g.world.player_pos().unwrap().z > 340.0, "{:?}", g.world.player_pos());
    // closing it again puts the colliders back
    g.world.toggle_door(l, v3(0.0, 0.0, 600.0));
    for _ in 0..90 {
        g.world.update_doors(1.0 / 60.0);
    }
    assert!(g.world.doors.iter().all(|(_, d)| !d.open && d.angle.abs() < 0.01));
}

#[test]
fn locked_doors_stay_shut_for_everybody() {
    let (mut g, l, r) = double_door_game(false);
    g.world.doors.get_mut(r).unwrap().locked = true;
    g.world.toggle_door(l, v3(0.0, 0.0, 0.0));
    for _ in 0..60 {
        g.world.update_doors(1.0 / 60.0);
    }
    assert!(g.world.doors.iter().all(|(_, d)| !d.open && d.angle == 0.0), "a locked leaf locks the whole door");
    assert!(!g.world.npc_open_door_ahead(v3(0.0, 0.0, 250.0), v3(0.0, 0.0, 1.0)));
}

#[test]
fn guards_open_doors_on_their_route_and_the_doors_close_again() {
    let (mut g, l, _r) = double_door_game(true);
    let guard = g.world.find("Guard_Door").unwrap();
    // settle, then let the guard patrol for a while
    let mut max_z: f32 = 0.0;
    let mut min_z: f32 = 1e9;
    for _ in 0..(60 * 40) {
        g.tick(&PlayerInput::default());
        let z = g.world.pos(guard).unwrap().z;
        max_z = max_z.max(z);
        min_z = min_z.min(z);
    }
    assert!(g.world.doors.get(l).unwrap().open_count >= 1, "the guard opened the door");
    assert!(min_z < 150.0 && max_z > 600.0, "the guard walked through the doorway in both directions (z {min_z}..{max_z})");
    // nobody is near any more: the door closes by itself
    g.world.set_pose(guard, v3(0.0, 0.0, 2000.0), 0.0);
    if let Some(b) = g.world.bodies.get_mut(guard) {
        b.pos = v3(0.0, 0.0, 2000.0);
    }
    g.world.set_pose(g.world.player.unwrap(), v3(0.0, 0.0, -1500.0), 0.0);
    if let Some(b) = g.world.bodies.get_mut(g.world.player.unwrap()) {
        b.pos = v3(0.0, 0.0, -1500.0);
    }
    g.world.set_door(l, true, v3(0.0, 0.0, 100.0), true);
    for _ in 0..(60 * 12) {
        g.world.update_doors(1.0 / 60.0);
    }
    assert!(!g.world.doors.get(l).unwrap().open, "NPC-opened doors close after a while");
}

#[test]
fn trigger_volumes_raise_events_and_complete_exit_objectives() {
    use bm_game::areas::{TriggerKind, TriggerVolume};
    use bm_game::data::{FailCondition, ObjectiveDef, ObjectiveKind};
    let mut w = world(floor(4000.0), 5);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    let exit = Transform::from_pose(v3(0.0, 0.0, 1000.0), 0.0).world;
    w.areas.triggers.push(TriggerVolume::new(1, "Exit", TriggerKind::Exit, [200.0, 400.0, 200.0], exit));
    w.areas.inside = vec![false];
    w.start_mission(MissionDef {
        id: "T".into(),
        title: "Test".into(),
        level: String::new(),
        player_start: None,
        objectives: vec![ObjectiveDef { id: "exit".into(), text: "Leave".into(), kind: ObjectiveKind::ReachExit { index: 0 }, optional: false, requires: vec![] }],
        fail: vec![FailCondition::PlayerDead],
    });
    let mut g = Game::from_world(w);
    for _ in 0..30 {
        g.tick(&PlayerInput::default());
    }
    assert_eq!(g.world.mission.status, MissionStatus::Active);
    for _ in 0..(60 * 6) {
        g.tick(&forward_input());
        if g.world.mission.status == MissionStatus::Complete {
            break;
        }
    }
    assert_eq!(g.world.mission.status, MissionStatus::Complete);
    let trig: Vec<_> = g.events().iter().filter(|e| e.event.name == "on_trigger").collect();
    assert!(!trig.is_empty() && trig[0].event.payload["entered"] == true && trig[0].event.payload["kind"] == "exit");
}

#[test]
fn save_state_round_trips_the_player_and_the_mission() {
    let mut w = world(floor(3000.0), 9);
    w.spawn_player(v3(10.0, 0.0, 20.0), 0.5, None, None);
    let mut g = Game::from_world(w);
    for _ in 0..30 {
        g.tick(&forward_input());
    }
    let pid = g.world.player.unwrap();
    g.world.healths.get_mut(pid).unwrap().hp = 42.0;
    g.world.mission.picked.push("hammer".into());
    let saved = g.save_state("synthetic");
    let json = saved.to_json().unwrap();
    let back = SaveGame::from_json(&json).unwrap();
    assert_eq!(back.label, "synthetic");
    assert_eq!(back.version, SAVE_VERSION);

    let mut w2 = world(floor(3000.0), 9);
    w2.spawn_player(v3(-500.0, 0.0, -500.0), 0.0, None, None);
    let mut g2 = Game::from_world(w2);
    // the synthetic world has no scene name: saves refuse a mismatching level but accept an equal (empty) one
    let rep = g2.load_state(&back).unwrap();
    assert_eq!(rep.npcs, 0);
    let pid2 = g2.world.player.unwrap();
    assert!(g2.world.pos(pid2).unwrap().dist(g.world.pos(pid).unwrap()) < 0.01);
    assert_eq!(g2.world.healths.get(pid2).unwrap().hp, 42.0);
    assert_eq!(g2.world.mission.picked, vec!["hammer".to_string()]);
    assert_eq!(g2.world.time, g.world.time);
    let mut other = back.clone();
    other.level = "Scenes/M99/M99_main".into();
    assert!(matches!(g2.load_state(&other), Err(SaveError::WrongLevel { .. })));
}

#[test]
fn weapon_command_gives_a_gun_and_puts_it_in_hand() {
    let mut w = world(floor(2000.0), 9);
    w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
    let mut g = Game::from_world(w);
    let list = g.run_command("weapon list");
    assert!(list.contains("remington") && list.contains("snubnosed"), "{list}");
    let out = g.run_command("weapon remington");
    assert!(out.starts_with("gave remington"), "{out}");
    let pid = g.world.player.unwrap();
    let p = g.world.players.get(pid).unwrap();
    assert_eq!(p.items[p.current].id, "remington");
    assert!(g.run_command("weapon nope").starts_with("unknown weapon"));
}
