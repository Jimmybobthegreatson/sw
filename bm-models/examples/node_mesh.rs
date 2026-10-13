//! `cargo run -p bm-models --example node_mesh -- <scene> <name substring> [max nodes]` - per-mesh statistics (triangles, UV range,
//! world bbox, material) of the nodes whose path contains the text.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let pat = a.get(1).cloned().unwrap_or_default().to_lowercase();
    let max: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let mut shown = 0;
    for n in &scene.nodes {
        if !scene.path(n.index).to_lowercase().contains(&pat) {
            continue;
        }
        let Some(model) = scene.node_model(n.index) else { continue };
        shown += 1;
        if shown > max {
            break;
        }
        println!("[{}] {} {:#x} {:?} pos {:?}", n.index, scene.path(n.index), n.type_id, n.kind, n.world.translation());
        println!("   world det {:.3} cols {:?}", n.world.det3(), n.world.0);
        println!("   header bbox {:.0?}..{:.0?} computed {:.0?}..{:.0?}", model.bbox.min, model.bbox.max, model.computed_bbox().min, model.computed_bbox().max);
        for (i, m) in model.meshes.iter().enumerate() {
            let v = &m.vertices;
            let (mut umin, mut umax, mut vmin, mut vmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for uv in &v.uvs {
                umin = umin.min(uv[0]);
                umax = umax.max(uv[0]);
                vmin = vmin.min(uv[1]);
                vmax = vmax.max(uv[1]);
            }
            let mut bb = m.bbox().transformed(&n.world);
            if m.vertices.is_empty() {
                bb = bm_models::math::Aabb::EMPTY;
            }
            let (mut agree, mut total, mut ny_sum) = (0usize, 0usize, 0.0f32);
            for t in m.indices.chunks_exact(3) {
                let p: Vec<[f32; 3]> = t.iter().map(|&i| m.vertices.positions[i as usize]).collect();
                let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                if let Some(nrm) = m.vertices.normals.get(t[0] as usize) {
                    let dot = c[0] * nrm[0] + c[1] * nrm[1] + c[2] * nrm[2];
                    total += 1;
                    if dot < 0.0 {
                        agree += 1;
                    }
                    ny_sum += nrm[1];
                }
            }
            println!("   winding agrees with normals (cw): {}/{}  mean normal.y {:.2}", agree, total, ny_sum / total.max(1) as f32);
            if !v.colors.is_empty() {
                let n = v.colors.len() as f32;
                let mean: Vec<f32> = (0..4).map(|c| v.colors.iter().map(|x| x[c] as f32).sum::<f32>() / n).collect();
                let (mn, mx) = (0..3).fold((255u8, 0u8), |(a, b), c| (a.min(v.colors.iter().map(|x| x[c]).min().unwrap()), b.max(v.colors.iter().map(|x| x[c]).max().unwrap())));
                println!("   vertex colour mean rgba {:.0?} min {} max {}", mean, mn, mx);
            }
            let d = scene.materials.desc(m.material());
            let mname = d.as_ref().map(|d| d.name.clone()).unwrap_or_default();
            let bl = d.as_ref().map(|d| format!("{:?}", d.render_state.blend)).unwrap_or_default();
            println!(
                "   mesh{} {:?} tris {} verts {} uv u[{:.2},{:.2}] v[{:.2},{:.2}] bbox {:.0?}..{:.0?} lod={:#x} mat#{} '{}' {}",
                i, m.layout, m.triangle_count(), v.len(), umin, umax, vmin, vmax, bb.min, bb.max, m.lod_mask(), m.material(), mname, bl
            );
        }
    }
}
