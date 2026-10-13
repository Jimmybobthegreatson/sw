//! `cargo run -p bm-models --example scene_tree -- [scene] [depth]` - prints the scene hierarchy
//! (node index, type, child count, geometry triangle count of the subtree) down to `depth` (default 3).
use bm_models::scene::{NodeKind, Scene};
use bm_vfs::Vfs;

fn tris(scene: &Scene, n: usize) -> usize {
    scene
        .subtree(n)
        .into_iter()
        .filter(|&i| scene.nodes[i].kind.is_visible_geometry())
        .filter_map(|i| scene.node_model(i))
        .map(|m| m.triangle_count())
        .sum()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let depth: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    fn walk(scene: &Scene, i: usize, d: usize, max: usize) {
        let n = &scene.nodes[i];
        let w = n.world.translation();
        println!(
            "{:indent$}[{}] {} {:#x} {:?} children={} tris={} pos=({:.0},{:.0},{:.0}){}",
            "", i, n.name, n.type_id, n.kind, n.children.len(), tris(scene, i), w[0], w[1], w[2],
            if n.prim_id.is_some() { " (model)" } else { "" },
            indent = d * 2
        );
        if d < max {
            for &c in &n.children {
                if scene.nodes[c].kind != NodeKind::Other || !scene.nodes[c].children.is_empty() {
                    walk(scene, c, d + 1, max);
                }
            }
        }
    }
    walk(&scene, 0, 0, depth);
}
