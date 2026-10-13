//! `cargo run -p bm-models --example light_chunks -- <scene> [type hex] [limit]` - raw PRM light/environment records of every
//! light node (research tool).
use bm_formats::prp::Value;
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let ty = a.get(1).and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    let limit: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(40);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let mut n_shown = 0;
    for n in &scene.graph.nodes {
        if !matches!(n.type_id, 0x800020 | 0x800023 | 0x800024 | 0x80000d | 0x8000f5) {
            continue;
        }
        if ty.is_some_and(|t| t != n.type_id) {
            continue;
        }
        let Some(Value::I32(c)) = n.props.get(4) else { continue };
        let Some(w) = scene.prm.chunk_words(*c as usize) else {
            println!("[{}] {} chunk {} missing", n.index, n.name, c);
            continue;
        };
        n_shown += 1;
        if n_shown > limit {
            break;
        }
        let f = |x: u32| {
            let v = f32::from_bits(x);
            if v.is_finite() && v.abs() > 1e-6 && v.abs() < 1e7 { format!("{v:.4}") } else { format!("{x:x}") }
        };
        let words: Vec<String> = w.iter().enumerate().map(|(i, x)| if i >= 5 { f(*x) } else { format!("{x:x}") }).collect();
        println!("{:#x} {:<40} chunk {:5} len {:3}: {}", n.type_id, n.name.chars().take(40).collect::<String>(), c, w.len() * 4, words.join(" "));
    }
}
