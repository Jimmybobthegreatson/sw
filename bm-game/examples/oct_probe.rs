//! `... --example oct_probe -- Scenes/M01/M01_main`: does the OCT leaf id refer to a scene node whose bounds are the leaf box?
use bm_formats::oct::{BlockKind, Oct};
use bm_models::Scene;

fn main() {
    let base = std::env::args().nth(1).unwrap_or("Scenes/M01/M01_main".into());
    let vfs = bm_vfs::Vfs::from_env().unwrap();
    let scene = Scene::load(&vfs, &base).unwrap();
    let oct = Oct::parse(&vfs.open(&format!("{base}.OCT")).unwrap()).unwrap();
    for (name, shift) in [("id = node index", 0i64), ("id = node index + 1", 1), ("id = node index - 1", -1)] {
        let (mut tot, mut inside, mut with_model) = (0, 0, 0);
        for b in oct.blocks.iter().filter(|b| b.kind == BlockKind::A) {
            for l in &b.leaves {
                let (lo, hi) = b.grid.world_box(&l.qbox);
                let ni = l.target as i64 + shift;
                if ni < 0 || ni as usize >= scene.nodes.len() {
                    continue;
                }
                let n = &scene.nodes[ni as usize];
                tot += 1;
                if n.prim_id.is_some() {
                    with_model += 1;
                }
                let tol = 2.0 / b.grid.scale + 1.0;
                if let Some(bb) = scene.world_bbox(ni as usize) {
                    // the leaf box encloses the node's world bounds (conservative quantisation)
                    let enc = (0..3).all(|k| bb.min[k] >= lo[k] - tol && bb.max[k] <= hi[k] + tol);
                    if enc {
                        inside += 1;
                    }
                }
            }
        }
        println!("{name}: {tot} leaves, node origin inside the leaf box {inside}, node has a model {with_model}");
    }
}
