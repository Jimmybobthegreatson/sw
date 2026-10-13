//! `hero_outfits [scene]` - outfit values of the player model and the materials each one draws.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let hero = scene.nodes.iter().find(|n| n.type_id == 0x2003e8).expect("hero");
    let m = scene.node_model(hero.index).expect("model");
    println!("masks {:?}", m.outfit_masks());
    for mask in m.outfit_masks() {
        let mats: Vec<String> = m.meshes.iter().filter(|x| x.is_lod0() && x.is_skinned() && x.in_outfit(mask)).map(|x| scene.materials.desc(x.material()).map(|d| d.name.rsplit('/').next().unwrap_or("").to_string()).unwrap_or_default()).collect();
        println!("{mask:3}: {}", mats.join(" "));
    }
}
