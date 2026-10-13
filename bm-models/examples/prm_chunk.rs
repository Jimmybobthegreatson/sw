//! `cargo run -p bm-models --example prm_chunk -- <scene> <chunk> [more chunks...]` - hex + float/int dump of raw PRM chunks.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for c in a.iter().skip(1) {
        let Ok(i) = c.parse::<usize>() else { continue };
        let Some(b) = scene.prm.chunk(i) else {
            println!("chunk {i}: none");
            continue;
        };
        println!("chunk {i}: {} bytes", b.len());
        for (k, w) in b.chunks(4).enumerate() {
            if w.len() < 4 {
                break;
            }
            let u = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            let f = f32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            let fs = if f.is_finite() && f.abs() > 1e-6 && f.abs() < 1e9 { format!("{f}") } else { String::new() };
            println!("  +{:03x} {:08x} {:>11} {}", k * 4, u, u as i32, fs);
            if k > 80 {
                println!("  ...");
                break;
            }
        }
    }
}
