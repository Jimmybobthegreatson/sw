//! `cargo run -p bm-models --example node_find -- <scene> <substring or 0xtype> [props]` - lists nodes by name/type.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let pat = a.get(1).cloned().unwrap_or_default();
    let show = a.get(2).is_some();
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let ty = pat.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok());
    for n in &scene.nodes {
        let hit = match ty {
            Some(t) => n.type_id == t,
            None => scene.path(n.index).to_lowercase().contains(&pat.to_lowercase()),
        };
        if hit {
            println!("[{}] {} {:#x} {:?} prim={:?} parent={:?}", n.index, scene.path(n.index), n.type_id, n.kind, n.prim_id, n.parent);
            if show {
                let g = &scene.graph.nodes[n.index];
                println!("     props {:?}", g.props);
                for c in &g.controllers {
                    println!("     ctrl {} {:?}", c.name, c.props);
                }
            }
        }
    }
}
