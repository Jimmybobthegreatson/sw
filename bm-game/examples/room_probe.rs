//! `cargo run --release -p bm-game --example rooms -- Scenes/M01/M01_main [x y z]`: room map coverage and point lookup.
use bm_game::rooms::RoomIndex;
use bm_models::Scene;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let base = a.first().cloned().unwrap_or("Scenes/M01/M01_main".into());
    let vfs = bm_vfs::Vfs::from_env().unwrap();
    let scene = Scene::load(&vfs, &base).unwrap();
    let rmi = bm_formats::roommap::RoomMap::parse(&vfs.open(&format!("{base}.RMI")).unwrap()).ok();
    let idx = RoomIndex::build(&scene, rmi);
    println!("{base}: {} rooms, {}/{} RMI boxes owned; ZROOMs {} ids {} by_order {}", idx.rooms.len(), idx.matched, idx.records, idx.room_nodes, idx.distinct_ids, idx.by_order);
    for r in idx.rooms.iter().take(200) {
        println!("  {:<44} {:<36} inside={} volumes={}", r.name, r.location, r.inside, r.volumes.len());
    }
    if a.len() >= 4 {
        let p = bm_game::v3(a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap());
        println!("room at {:?}: {:?}", p, idx.name_at(p));
    }
}
