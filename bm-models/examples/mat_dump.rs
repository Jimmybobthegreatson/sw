//! `cargo run -p bm-models --example mat_dump -- <scene> <material name substring> [out dir]` - prints the material
//! description (blend state, textures, colors, flags) and writes its textures as PNG.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let pat = a.get(1).cloned().unwrap_or_default().to_lowercase();
    let out = std::path::PathBuf::from(a.get(2).cloned().unwrap_or_else(|| ".".into()));
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for id in 1..4096u32 {
        let Some(d) = scene.materials.desc(id) else { continue };
        if !d.name.to_lowercase().contains(&pat) {
            continue;
        }
        println!("material {id} '{}' shader={:?}\n  state {:?}\n  colors {:?}\n  floats {:?}\n  flags {:?}", d.name, d.shader, d.render_state, d.colors, d.floats, d.flags);
        for t in &d.textures {
            println!("  texture {:?}", t);
            if let Some(img) = scene.materials.image(t.texture_id) {
                let p = out.join(format!("mat{}_{}_{}.png", id, t.slot, t.texture_id));
                let f = std::fs::File::create(&p).unwrap();
                let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.width, img.height);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                enc.write_header().unwrap().write_image_data(&img.rgba).unwrap();
                println!("    wrote {} ({}x{}, {})", p.display(), img.width, img.height, img.source_format);
            }
        }
    }
}
