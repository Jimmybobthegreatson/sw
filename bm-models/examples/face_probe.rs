//! `cargo run -p bm-models --example face_probe -- <scene> [model chunk]` - dumps the facial pose tables of a skinned model's skeleton block.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn hex(prm: &bm_formats::prm::Prm, c: usize, max: usize) {
    let Some(b) = prm.chunk(c) else { println!("  chunk {c}: none"); return };
    println!("  chunk {c}: {} bytes (unk1 {} unk2 {})", b.len(), prm.chunks[c].unk1, prm.chunks[c].unk2);
    for (k, w) in b.chunks(4).enumerate().take(max) {
        if w.len() < 4 { break; }
        let u = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        let f = f32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        let fs = if f.is_finite() && f.abs() > 1e-6 && f.abs() < 1e9 { format!("{f}") } else { String::new() };
        println!("    +{:04x} {:08x} {:>11} {}", k * 4, u, u as i32, fs);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let prm = &scene.prm;
    let mut seen = std::collections::HashSet::new();
    for mc in prm.model_chunks() {
        let Some(m) = prm.model(mc) else { continue };
        if m.parts_chunk == 0 { continue; }
        let Some(parts) = prm.chunk_words(m.parts_chunk as usize) else { continue };
        let list = parts.get(1).copied().unwrap_or(0) as usize;
        if list == 0 || !seen.insert(list) { continue; }
        let Some(lw) = prm.chunk_words(list) else { continue };
        let bones = lw[0];
        println!("model chunk {mc} parts {} list {list} bones {bones} list words: {:?}", m.parts_chunk, lw);
        if let Some(sel) = a.get(1).and_then(|s| s.parse::<usize>().ok()) {
            if sel != mc { continue; }
            for (i, w) in lw.iter().enumerate().skip(4) {
                println!(" list[{i}] -> chunk {w}");
                if *w > 0 { hex(prm, *w as usize, 24); }
            }
        } else if let Some(&p) = lw.get(11) {
            // summarise pose bank
            if let Some(w) = prm.chunk_words(p as usize) { println!("  list[11] chunk {p}: {:?}", &w[..w.len().min(12)]); }
        }
    }
}
