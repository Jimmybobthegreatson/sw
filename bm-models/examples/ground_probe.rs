//! `cargo run -p bm-models --example ground_probe -- <scene> <x> <z> [ymin ymax]` - lists every triangle (of every node kind,
//! LOD and material, nothing filtered) crossed by the vertical line through (x, z): node, kind, mesh flags, material.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let x: f32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let z: f32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let ymin: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(-1e9);
    let ymax: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(1e9);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let mut hits = Vec::new();
    for n in &scene.nodes {
        let Some(model) = scene.node_model(n.index) else { continue };
        let bb = model.bbox.transformed(&n.world);
        if x < bb.min[0] || x > bb.max[0] || z < bb.min[2] || z > bb.max[2] {
            continue;
        }
        for (mi, mesh) in model.meshes.iter().enumerate() {
            for t in mesh.indices.chunks_exact(3) {
                let p: Vec<[f32; 3]> = t.iter().map(|&i| n.world.transform_point(mesh.vertices.positions[i as usize])).collect();
                // barycentric in XZ
                let (x0, z0) = (p[0][0], p[0][2]);
                let (x1, z1) = (p[1][0], p[1][2]);
                let (x2, z2) = (p[2][0], p[2][2]);
                let den = (z1 - z2) * (x0 - x2) + (x2 - x1) * (z0 - z2);
                if den.abs() < 1e-6 {
                    continue;
                }
                let l0 = ((z1 - z2) * (x - x2) + (x2 - x1) * (z - z2)) / den;
                let l1 = ((z2 - z0) * (x - x2) + (x0 - x2) * (z - z2)) / den;
                let l2 = 1.0 - l0 - l1;
                if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                    continue;
                }
                let y = l0 * p[0][1] + l1 * p[1][1] + l2 * p[2][1];
                if y < ymin || y > ymax {
                    continue;
                }
                let ny = (p[1][2] - p[0][2]) * (p[2][0] - p[0][0]) - (p[1][0] - p[0][0]) * (p[2][2] - p[0][2]);
                hits.push((y, n.index, mi, ny));
                break;
            }
        }
    }
    hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (y, ni, mi, _) in hits {
        let n = &scene.nodes[ni];
        let model = scene.node_model(ni).unwrap();
        let m = &model.meshes[mi];
        let d = scene.materials.desc(m.material());
        let (mn, bm, tex) = match &d {
            Some(d) => (d.name.clone(), format!("{:?} atst={} opac={:.2} cull={:?}", d.render_state.blend, d.render_state.alpha_test, d.render_state.opacity, d.render_state.cull), format!("{:?}", d.textures.iter().map(|t| (t.slot.as_str(), t.texture_id)).collect::<Vec<_>>())),
            None => ("<none>".into(), String::new(), String::new()),
        };
        println!(
            "y={:9.1} [{}] {} {:#x} {:?} mesh{} {:?} lodmask={:#x} var={:#x} flags={:#x} mat#{} '{}' {} {}",
            y, ni, scene.path(ni), n.type_id, n.kind, mi, m.layout, m.lod_mask(), m.variant, m.flags, m.material(), mn, bm, tex
        );
    }
}
