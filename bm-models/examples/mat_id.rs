//! `cargo run -p bm-models --example mat_id -- <scene> <id> [out dir]` - full description of one `.MAT` instance (also unnamed ones).
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let id: u32 = a.get(1).and_then(|s| s.parse().ok()).expect("material id");
    let out = a.get(2).map(std::path::PathBuf::from);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    if let Some(i) = scene.materials.mat().instance(id) {
        println!("instance off {:#x} len {:#x} header {:x?} records {}", i.offset, i.len, i.header, i.records.len());
        let d = &scene.materials.mat().data;
        for k in (i.offset as usize - 0x20..i.offset as usize + 0x70).step_by(16) {
            println!("   {k:#07x}: {}", d[k..k + 16].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ") + "  " + &d[k..k + 16].iter().map(|&b| if b.is_ascii_graphic() { b as char } else { '.' }).collect::<String>());
        }
        let r7 = i.header[7] as usize;
        println!("   header[7] record at {r7:#x}: {:02x?}", &d[r7..r7 + 16]);
        for r in i.records.iter().take(24) { println!("   {} v={:#x} len={} kind={}", r.tag, r.value, r.len, r.kind); }
    }
    let d = scene.materials.desc(id).expect("material");
    println!("material {id} {:?}\n state {:?}\n colors {:?}\n floats {:?}\n flags {:?}", d.name, d.render_state, d.colors, d.floats, d.flags);
    for t in &d.textures {
        println!(" texture {:?}", t);
        if let (Some(o), Some(img)) = (&out, scene.materials.image(t.texture_id)) {
            let p = o.join(format!("id{}_{}_{}.png", id, t.slot, t.texture_id));
            let f = std::fs::File::create(&p).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.width, img.height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&img.rgba).unwrap();
            println!("   wrote {} ({}x{} {})", p.display(), img.width, img.height, img.source_format);
        }
    }
}
