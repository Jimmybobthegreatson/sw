//! `cargo run -p bm-models --example outfit_survey -- [scene]` - for every `ZHM3Actor`: its outfit value, the resolved
//! outfit of its model, and the (LOD0) skinned materials that outfit draws (naming check for the outfit selection).
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for n in scene.nodes.iter().filter(|n| n.type_id == 0x2003e9) {
        let Some(m) = scene.node_model(n.index) else { continue };
        let want = scene.actor_outfit(n.index);
        let got = m.resolve_outfit(want.unwrap_or(1));
        let path = scene.path(n.index);
        let short: Vec<&str> = path.rsplit('/').take(2).collect();
        let mats: Vec<String> = m
            .meshes
            .iter()
            .filter(|x| x.is_lod0() && x.is_skinned() && x.in_outfit(got))
            .map(|x| scene.materials.desc(x.material()).map(|d| d.name.rsplit('/').next().unwrap_or("").to_string()).unwrap_or_default())
            .collect();
        println!("{:40} want {:?} got {:2} masks {:?} | {}", short.into_iter().rev().collect::<Vec<_>>().join("/"), want, got, m.outfit_masks(), mats.join(" "));
    }
}
