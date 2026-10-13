//! `cargo run -p bm-models --example light_axes -- <scene>` - for every spot-like light: which local axis points down? (cone axis research)
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let mut sums = [[0.0f32; 6]; 3];
    let mut counts = [0usize; 3];
    let mut down = [[0usize; 6]; 3];
    for n in &scene.nodes {
        let k = match n.type_id {
            0x800023 => 0,
            0x800020 => 1,
            0x80000d => 2,
            _ => continue,
        };
        counts[k] += 1;
        let axes = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
        for (i, ax) in axes.iter().enumerate() {
            let d = n.world.transform_vector(*ax);
            sums[k][i] += d[1];
            if d[1] < -0.5 {
                down[k][i] += 1;
            }
        }
    }
    let names = ["+X", "-X", "+Y", "-Y", "+Z", "-Z"];
    for (k, t) in ["spot", "spot-square", "env"].iter().enumerate() {
        println!("{t}: {} nodes", counts[k]);
        for i in 0..6 {
            println!("   local {} -> mean world y {:+.2}, pointing down (<-0.5): {}", names[i], sums[k][i] / counts[k].max(1) as f32, down[k][i]);
        }
    }
}
