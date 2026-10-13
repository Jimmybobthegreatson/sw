//! Research: contact sheet of the sprite textures used by a scene's particle templates (alpha over grey).
use bm_models::Scene;
use bm_formats::prp::Value;
use bm_vfs::Vfs;
fn main() {
    let vfs = Vfs::from_env().unwrap();
    let base = std::env::args().nth(1).unwrap_or("Scenes/M01/M01_main".into());
    let out = std::env::args().nth(2).unwrap_or("sheet.png".into());
    let scene = Scene::load(&vfs, &base).unwrap();
    let mut ids = std::collections::BTreeSet::new();
    for n in &scene.graph.nodes { if n.type_id == 0x2000e5 { if let Value::I32(p) = n.props[4] { if let Some(w) = scene.prm.chunk_words(p as usize) { ids.insert(w[1]); } } } }
    let cell = 128usize; let cols = 8usize; let rows = (ids.len() + cols - 1) / cols;
    let mut img = vec![60u8; cell * cols * cell * rows * 4];
    for px in img.chunks_exact_mut(4) { px[3] = 255; }
    for (k, id) in ids.iter().enumerate() {
        println!("{k}: tex {id}");
        let Some(im) = scene.materials.image(*id) else { continue };
        let (cx, cy) = ((k % cols) * cell, (k / cols) * cell);
        for y in 0..cell { for x in 0..cell {
            let sx = x * im.width as usize / cell; let sy = y * im.height as usize / cell;
            let s = &im.rgba[(sy * im.width as usize + sx) * 4..][..4];
            let d = &mut img[((cy + y) * cell * cols + cx + x) * 4..][..4];
            let a = if im.has_alpha() { s[3] as f32 / 255.0 } else { 1.0 };
            for c in 0..3 { d[c] = (s[c] as f32 * a + d[c] as f32 * (1.0 - a)) as u8; }
        } }
        println!("   {} {}x{} {}", im.name, im.width, im.height, im.source_format);
    }
    let f = std::fs::File::create(&out).unwrap();
    let mut e = png::Encoder::new(f, (cell * cols) as u32, (cell * rows) as u32);
    e.set_color(png::ColorType::Rgba); e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&img).unwrap();
}
