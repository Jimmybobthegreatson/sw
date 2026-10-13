//! Lists model nodes near a point and tells whether the renderer's node-kind filter keeps them: `ring M01 x z radius_cm`.
use bm_models::scene::NodeKind;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    let (cx, cz, r): (f32, f32, f32) = (a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
    let kept = [NodeKind::Static, NodeKind::Scatter, NodeKind::Actor, NodeKind::ClothBundle];
    for (i, n) in scene.nodes.iter().enumerate() {
        let Some(m) = scene.node_model(i) else { continue };
        let bb = m.computed_bbox().transformed(&n.world);
        let (mx, mz) = ((bb.min[0] + bb.max[0]) * 0.5, (bb.min[2] + bb.max[2]) * 0.5);
        let dist = ((mx - cx).powi(2) + (mz - cz).powi(2)).sqrt();
        let ext = ((bb.max[0] - bb.min[0]).max(bb.max[2] - bb.min[2])) * 0.5;
        if dist - ext > r {
            continue;
        }
        let h = bb.max[1] - bb.min[1];
        let mats: Vec<String> = m.meshes.iter().filter_map(|me| scene.materials.desc(me.material()).map(|d| d.name.clone())).take(2).collect();
        println!(
            "{}\t{:?}\tkept={}\tdist {:.0}\theight {:.0}\ty {:.0}..{:.0}\ttris {}\t{}\t{:?}",
            i, n.kind, kept.contains(&n.kind), dist, h, bb.min[1], bb.max[1], m.triangle_count(), scene.path(i).rsplit('/').next().unwrap_or(""), mats
        );
    }
}
