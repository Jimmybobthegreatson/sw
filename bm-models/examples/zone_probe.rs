//! `cargo run -p bm-models --example zone_probe -- <scene> <x> <y> <z> [radius]` - environment nodes near a point and the bound
//! volumes (any node whose model has the `_Glacier/InsideBound` material) that contain it (zone research).
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let p: Vec<f32> = (1..4).map(|i| a.get(i).and_then(|s| s.parse().ok()).unwrap_or(0.0)).collect();
    let radius: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(4000.0);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let pt = [p[0], p[1], p[2]];
    println!("point {pt:?}");
    println!("-- environments within {radius}");
    let mut envs: Vec<(f32, usize)> = scene
        .nodes
        .iter()
        .filter(|n| n.type_id == 0x80000d)
        .map(|n| {
            let t = n.world.translation();
            (((t[0] - pt[0]).powi(2) + (t[1] - pt[1]).powi(2) + (t[2] - pt[2]).powi(2)).sqrt(), n.index)
        })
        .filter(|(d, _)| *d < radius)
        .collect();
    envs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    for (d, i) in envs.iter().take(30) {
        let n = &scene.nodes[*i];
        let par: Vec<String> = {
            let mut v = vec![];
            let mut c = n.parent;
            while let Some(x) = c {
                v.push(scene.nodes[x].name.clone());
                if v.len() >= 3 {
                    break;
                }
                c = scene.nodes[x].parent;
            }
            v
        };
        println!("  d {:7.0} [{}] {} (parents {:?}) pos {:.0?}", d, i, n.name, par, n.world.translation());
    }
    println!("-- bound volumes containing the point (model bbox in node space)");
    for n in &scene.nodes {
        let Some(m) = scene.node_model(n.index) else { continue };
        let is_inside = m.meshes.iter().any(|me| scene.materials.desc(me.material()).is_some_and(|d| d.name.contains("InsideBound")));
        if !is_inside {
            continue;
        }
        let inv = n.world.inverse_affine();
        let l = inv.transform_point(pt);
        let b = m.bbox;
        if (0..3).all(|k| l[k] >= b.min[k] && l[k] <= b.max[k]) {
            let vol = (b.max[0] - b.min[0]) * (b.max[1] - b.min[1]) * (b.max[2] - b.min[2]);
            println!("  [{}] {}  vol {:.3e} bbox {:.0?}..{:.0?}", n.index, scene.path(n.index), vol, b.min, b.max);
        }
    }
}
