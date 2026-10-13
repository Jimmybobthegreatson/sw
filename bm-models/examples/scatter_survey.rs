//! `cargo run -p bm-models --example scatter_survey -- <scene> [limit]` - raw description of the `ZScatterObj` (0x90109) objects:
//! material, bounding box, table pointers, header floats and the pattern records (research tool, see docs/models.md).
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let limit: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(12);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let mut shown = 0;
    for n in &scene.nodes {
        if n.type_id != 0x2000ce {
            continue;
        }
        let Some(p) = n.prim_id else { continue };
        let Some(h) = scene.prm.model(p as usize) else { continue };
        let Some(objs) = scene.prm.chunk_words(h.objects_chunk as usize) else { continue };
        for oc in objs {
            let Some(w) = scene.prm.chunk_words(oc as usize) else { continue };
            if w.first() != Some(&0x90109) {
                continue;
            }
            shown += 1;
            if shown > limit {
                return;
            }
            let mat = scene.materials.desc((w[4] >> 16).saturating_sub(1));
            let f = |i: usize| f32::from_bits(w[i]);
            let cells = scene.prm.chunk_words(w[11] as usize).unwrap_or_default();
            let recs: Vec<String> = cells.chunks(4).take(8).map(|c| format!("({},{},{},{})", c[0], c[1], c[2], c[3])).collect();
            let tuples: Vec<String> = cells.iter().skip(32).take(24).map(|x| format!("{:.3}", f32::from_bits(*x))).collect();
            println!("    first instance floats (table +0x80): {}", tuples.join(" "));
            let types = scene.prm.chunk(w[13] as usize).unwrap_or_default();
            let mut hist = [0usize; 8];
            for &t in types.iter().take(w[14] as usize) {
                hist[(t as usize).min(7)] += 1;
            }
            let (mn, mx) = (h.bbox_min, h.bbox_max);
            println!(
                "[{}] {} pos {:.0?} mat '{}' {:?}\n    bbox {:.0?}..{:.0?} size {:.0?} n={} w15={} w16..21={:.1?} {:.1?} w22={:#x} tint={:?}/{:?} flags w6={:#x} w7={:#x} w10={:#x} w29={}\n    patterns {}\n    type histogram {:?}",
                n.index,
                scene.path(n.index).rsplit('/').next().unwrap_or(""),
                n.world.translation(),
                mat.as_ref().map(|d| d.name.as_str()).unwrap_or("?"),
                mat.as_ref().map(|d| d.floats.iter().filter(|(k, _)| k.starts_with("Scatter")).map(|(k, v)| format!("{k}={}", v[0])).collect::<Vec<_>>()),
                mn,
                mx,
                [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]],
                w[14],
                f(15),
                [f(16), f(17), f(18)],
                [f(19), f(20), f(21)],
                w[22],
                [f(23), f(24), f(25)],
                [f(26), f(27), f(28)],
                w[6],
                w[7],
                w[10],
                w[29],
                recs.join(" "),
                hist
            );
        }
    }
}
