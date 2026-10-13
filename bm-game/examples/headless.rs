//! Headless simulation report: `cargo run --release -p bm-game --example headless -- [Scenes/M01/M01_main] [seconds]`
use bm_game::*;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let base = args.get(1).cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let secs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let vfs = Arc::new(bm_vfs::Vfs::from_env().expect("game dir (BM_GAME_DIR)"));
    let t0 = std::time::Instant::now();
    let mut g = Game::load(vfs, &base, &LoadOptions::default()).expect("load");
    println!("loaded {base} in {:.2}s: {:?}", t0.elapsed().as_secs_f32(), g.stats);
    let start: Vec<(EntityId, Vec3)> = g.world.ais.iter().map(|(id, _)| (id, g.world.pos(id).unwrap())).collect();
    let t1 = std::time::Instant::now();
    for _ in 0..secs * 60 {
        g.tick(&PlayerInput::default());
    }
    println!("{secs}s simulated in {:.2}s (nav cells cached: {}, expansions: {})", t1.elapsed().as_secs_f32(), g.world.nav.cached_nodes(), g.world.nav.expansions);
    let mut moved = 0;
    let mut other_floor = 0;
    let mut grounded = 0;
    for (id, p0) in &start {
        let p = g.world.pos(*id).unwrap();
        if p.dist_xz(*p0) > 100.0 {
            moved += 1;
        }
        if (p.y - p0.y).abs() > 150.0 {
            other_floor += 1;
            println!("  {} changed height {:.0} -> {:.0}", g.world.infos.get(*id).unwrap().name, p0.y, p.y);
        }
        if g.world.bodies.get(*id).map_or(false, |b| b.grounded) {
            grounded += 1;
        }
    }
    println!("NPCs: {} total, {moved} moved >1 m, {grounded} grounded, {other_floor} changed floor by >1.5 m", start.len());
    let opened: Vec<_> = g.world.doors.iter().filter(|(_, d)| d.open_count > 0).map(|(id, d)| (g.world.infos.get(id).unwrap().name.clone(), d.open_count)).collect();
    println!("doors: {} leaves, {} opened by NPCs during the run: {:?}", g.world.doors.len(), opened.len(), opened);
    println!("player room: {:?} zone {:?}", g.world.player_room_name(), g.world.player_zone_name());
    let rs = g.render_state();
    println!("render state: {} entities, {} lights, camera {:?}", rs.entities.len(), rs.lights.len(), rs.camera.map(|c| c.pos));
    println!("events logged: {}", g.events().len());
}
