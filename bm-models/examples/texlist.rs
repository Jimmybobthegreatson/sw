//! Lists the textures of a scene whose name contains the filter: `texlist M01 OSD`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    let filter = a.get(2).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let tex = scene.materials.tex();
    for id in 0..4096u32 {
        if let Some(t) = tex.image(id) {
            if t.name.to_ascii_lowercase().contains(&filter) {
                println!("{id}\t{}\t{}x{}\t{:?}\tmips {}", t.name, t.width, t.height, t.format, t.mips.len());
            }
        }
    }
}
