//! `cargo run -p bm-models --example actor_probe -- [scene] [name filter]` - dumps every `ZHM3Actor`: raw property
//! values, children (clothing bundles, links) and which outfit bits / LOD0 meshes (with material names) its model has.
use bm_models::scene::{NodeKind, Scene};
use bm_vfs::Vfs;
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let filter = a.get(1).cloned().unwrap_or_default();
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for n in scene.nodes.iter().filter(|n| n.kind == NodeKind::Actor || n.type_id == 0x2003e9) {
        let path = scene.path(n.index);
        if !path.contains(&filter) {
            continue;
        }
        println!("[{}] {} type={:#x} prim={:?}", n.index, path, n.type_id, n.prim_id);
        let g = &scene.graph.nodes[n.index];
        println!("   props ({}): {:?}", g.props.len(), &g.props[3.min(g.props.len())..]);
        for c in &g.controllers {
            println!("   controller {} {:?}", c.name, c.props);
        }
        for &ch in &n.children {
            let c = &scene.nodes[ch];
            println!("   child [{}] {} {:#x} {:?} prim={:?} props={:?}", ch, c.name, c.type_id, c.kind, c.prim_id, &scene.graph.nodes[ch].props[3.min(scene.graph.nodes[ch].props.len())..]);
        }
        if let Some(m) = scene.node_model(n.index) {
            let mut by_variant: BTreeMap<u8, Vec<String>> = BTreeMap::new();
            for mesh in m.meshes.iter().filter(|x| x.is_lod0() && x.is_skinned()) {
                let mat = scene.materials.desc(mesh.material()).map(|d| d.name.clone()).unwrap_or_default();
                by_variant.entry(mesh.variant_mask()).or_default().push(mat);
            }
            println!("   model: {} meshes, {} bones", m.meshes.len(), m.skeleton.as_ref().map(|s| s.bones.len()).unwrap_or(0));
            for (v, mats) in by_variant {
                println!("   variant mask {:#04x}: {}", v, mats.join(" | "));
            }
        }
    }
}
