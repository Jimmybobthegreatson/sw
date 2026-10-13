//! `cargo run -p bm-models --example lighting_dump -- <scene> [x y z]` - summary of the decoded lighting (fog, sun, lights, zones) and
//! the environments selected at a point.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let l = scene.lighting();
    println!("fog {:?}", l.fog);
    println!("sun sprite dir {:?}", l.sun_sprite_dir);
    println!("{} lights, {} environments, {} zones", l.lights.len(), l.environments.len(), l.zones.len());
    let suns: Vec<_> = l.environments.iter().filter(|e| e.kind == bm_models::lighting::EnvKind::Sun).collect();
    for e in &suns {
        let to_sun = [-e.direction[0], -e.direction[1], -e.direction[2]];
        let agree = l.sun_sprite_dir.map(|s| s[0] * to_sun[0] + s[1] * to_sun[1] + s[2] * to_sun[2]);
        println!("sun env '{}' character={} color {:.2?} x{} sky {:.2?} ambient {:.2?} dir-to-sun {:.3?} dot(sun sprite) {:?} shadow meshes {}", e.name, e.for_characters, e.color, e.intensity, e.sky, e.ambient, to_sun, agree, e.shadow_mesh_count);
    }
    let mut kinds = std::collections::BTreeMap::new();
    for x in &l.lights {
        *kinds.entry(format!("{:?}", x.shape)).or_insert(0) += 1;
    }
    println!("light shapes {kinds:?}");
    if std::env::var("BM_ZONES").is_ok() {
        for z in &l.zones {
            let n = &scene.nodes[z.node];
            let c = n.world.translation();
            println!("zone {} at ({:.0}, {:.0}, {:.0}) size {:.0?}", scene.path(z.node).rsplit('/').take(2).collect::<Vec<_>>().join(" <- "), c[0], c[1], c[2], [z.bounds_max[0] - z.bounds_min[0], z.bounds_max[1] - z.bounds_min[1], z.bounds_max[2] - z.bounds_min[2]]);
        }
    }
    if a.len() >= 4 {
        let p = [a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap()];
        let sel = scene.environments_at(p);
        println!("at {p:?}: zone {:?} level env {:?} character env {:?}", sel.zone.map(|z| scene.path(l.zones[z].node)), sel.level_env.map(|i| &l.environments[i]), sel.character_env.map(|i| &l.environments[i].name));
    }
}
