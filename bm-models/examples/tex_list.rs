//! `cargo run -p bm-models --example tex_list -- <scene> [name substring] [out dir]` - lists the textures of a scene's `.TEX`
//! (id, slot, format, size, mips, name) and the id-list entries; with an out dir the matching images are written as PNG.
use bm_formats::tex::TexEntry;
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let pat = a.get(1).cloned().unwrap_or_default().to_lowercase();
    let out = a.get(2).map(std::path::PathBuf::from);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let tex = scene.materials.tex();
    for (slot, idx) in &tex.slots {
        match &tex.entries[*idx] {
            TexEntry::Image(i) => {
                if !i.name.to_lowercase().contains(&pat) {
                    continue;
                }
                println!("slot {slot:4} image id {:4} {} {}x{} mips {} '{}'", i.id, i.format.name(), i.width, i.height, i.mips.len(), i.name);
                if let (Some(o), Some(img)) = (&out, scene.materials.image(i.id)) {
                    let p = o.join(format!("tex{}_{}.png", i.id, i.name.replace(['/', '\\'], "_")));
                    let f = std::fs::File::create(&p).unwrap();
                    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.width, img.height);
                    enc.set_color(png::ColorType::Rgba);
                    enc.set_depth(png::BitDepth::Eight);
                    enc.write_header().unwrap().write_image_data(&img.rgba).unwrap();
                }
            }
            TexEntry::Cube { faces } => {
                if pat.is_empty() {
                    println!("slot {slot:4} id list {:?}", faces);
                }
            }
        }
    }
}
