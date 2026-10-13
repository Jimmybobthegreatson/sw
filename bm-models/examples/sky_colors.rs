//! `cargo run -p bm-models --example sky_colors -- <scene>` - vertex colours of the sky sphere by height (to compare with the fog colour definition).
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for n in &scene.nodes {
        if !n.name.to_lowercase().starts_with("skysphere_01") || n.name.to_lowercase().contains("bloom") {
            continue;
        }
        let Some(m) = scene.node_model(n.index) else { continue };
        for mesh in &m.meshes {
            let mut rows: Vec<(f32, [u8; 4])> = mesh.vertices.positions.iter().zip(&mesh.vertices.colors).map(|(p, c)| (p[1], *c)).collect();
            rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            println!("{} verts {} material {:?}", n.name, rows.len(), scene.materials.desc(mesh.material()).map(|d| d.name.clone()));
            let step = (rows.len() / 24).max(1);
            for (y, c) in rows.iter().step_by(step) {
                println!("  y {:9.0} rgba {:?}", y, c);
            }
        }
    }
    for d in &scene.graph.definitions {
        if d.name == "CameraBackgroundColor" {
            println!("CameraBackgroundColor {:?}", d.value);
        }
    }
}
