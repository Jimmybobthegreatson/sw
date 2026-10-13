//! `cargo run -p bm-models --example mat_list -- <scene> [from] [to]` - lists material ids with name, blend mode and diffuse texture.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let from: u32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    let to: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(4096);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for id in from..=to {
        if let Some(d) = scene.materials.desc(id) {
            println!("{id:4} {:50} {:?} diffuse={:?} normal={:?}", d.name, d.render_state.blend, d.texture("mapDiffuse"), d.texture("mapNormal"));
        }
    }
}
