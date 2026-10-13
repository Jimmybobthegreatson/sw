//! `cargo run -p bm-formats --example dump -- [scene] [texture-id|name-substring] [out.png]`
//!
//! Prints the file listing of a scene ZIP (default `Scenes/M01/M01_main.ZIP`), a summary of the
//! scene graph / textures / localisation inside it and decodes one texture to a PNG
//! (default: the first >= 256x256 DXT1 image, written to the current directory).
//! Needs `BM_GAME_DIR` (or the default install location).

use std::fs::File;
use std::io::BufWriter;

use bm_formats::{buf::Buf, gms::Gms, prp::Prp, scene::SceneGraph, tex::Tex};
use bm_vfs::Vfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scene = args.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".to_string());
    let scene = scene.trim_end_matches(".ZIP").trim_end_matches(".zip").to_string();
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");

    println!("== {scene}.ZIP ==");
    let mut entries = vfs.list_archive(&format!("{scene}.ZIP")).expect("scene zip");
    entries.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    for e in &entries {
        println!("{:>12}  {}", e.size, e.path);
    }

    // scene graph
    let gms = Gms::parse(&vfs.open(&format!("{scene}.GMS")).unwrap()).unwrap();
    let buf = Buf::parse(&vfs.open(&format!("{scene}.BUF")).unwrap()).unwrap();
    let prp = Prp::parse(&vfs.open(&format!("{scene}.PRP")).unwrap()).unwrap();
    let graph = SceneGraph::build(&gms, &buf, &prp).unwrap();
    println!("\n== scene graph: {} nodes, {} definitions ==", graph.nodes.len(), graph.definitions.len());
    for n in graph.nodes.iter().take(12) {
        println!("  [{:>5}] {:#08x} {:<40} children={} pos={:?}", n.index, n.type_id, n.name, n.children.len(), n.position);
    }

    // textures
    let tex = Tex::parse(&vfs.open(&format!("{scene}.TEX")).unwrap()).unwrap();
    let n_img = tex.images().count();
    println!("\n== textures: {n_img} images, {} entries in file ==", tex.entries.len());
    let pick = match args.get(1) {
        Some(a) => match a.parse::<u32>() {
            Ok(id) => tex.image(id),
            Err(_) => tex.images().find(|i| i.name.to_lowercase().contains(&a.to_lowercase())),
        },
        None => tex.images().find(|i| i.width >= 256 && i.height >= 256 && i.format == bm_formats::tex::TexFormat::Dxt1),
    }
    .expect("no matching texture");
    let rgba = pick.decode_rgba(0).expect("decode");
    let out = args.get(2).cloned().unwrap_or_else(|| format!("tex_{}.png", pick.id));
    let mut enc = png::Encoder::new(BufWriter::new(File::create(&out).unwrap()), pick.width as u32, pick.height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    println!(
        "wrote {out}: id {} \"{}\" {} {}x{} ({} mips)",
        pick.id,
        pick.name,
        pick.format.name(),
        pick.width,
        pick.height,
        pick.mips.len()
    );
}
