use bm_models::scene::{NodeKind, Scene};
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let min: f32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(15000.0);
    let vfs = Vfs::from_env().unwrap();
    let scene = Scene::load(&vfs, &name).unwrap();
    for d in scene.draw_list(&[NodeKind::Static, NodeKind::Scatter]) {
        let b = d.model.bbox.transformed(&d.world);
        let ext = (b.max[0] - b.min[0]).max(b.max[2] - b.min[2]);
        let far = [b.min[0].abs(), b.max[0].abs(), b.min[2].abs(), b.max[2].abs()].iter().cloned().fold(0.0f32, f32::max);
        if ext > min || far > min * 2.0 {
            let mats: Vec<String> = d.model.meshes.iter().filter_map(|m| scene.materials.desc(m.material()).map(|x| format!("{}:{:?}", x.name, x.render_state.blend))).collect();
            println!("{} ext {:.0} y {:.0}..{:.0} x {:.0}..{:.0} z {:.0}..{:.0} {:?}", scene.path(d.node).rsplit('/').next().unwrap(), ext, b.min[1], b.max[1], b.min[0], b.max[0], b.min[2], b.max[2], mats);
        }
    }
}
