//! `cargo run -p bm-models --example light_survey -- <scene> [max per type]` - dumps the scene definitions and the raw property values /
//! controllers of every light / environment / backdrop-like node type (research tool for `docs/formats.md`, lighting section).
use bm_models::scene::Scene;
use bm_vfs::Vfs;
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let max: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(4);
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    println!("== definitions ==");
    for d in &scene.graph.definitions {
        println!("  {} kind {} {:?}", d.name, d.kind, d.value);
    }
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for n in &scene.graph.nodes {
        *counts.entry(n.type_id).or_default() += 1;
    }
    println!("== node types ==");
    for (t, c) in &counts {
        println!("  {:#08x} {:<24} x{}", t, bm_formats::scene::type_name(*t).unwrap_or("?"), c);
    }
    println!("== light / environment nodes ==");
    let mut shown: BTreeMap<u32, usize> = BTreeMap::new();
    for n in &scene.graph.nodes {
        let interesting = matches!(n.type_id >> 20, 0x8) || n.name.to_lowercase().contains("light") || n.name.to_lowercase().contains("sun") || n.name.to_lowercase().contains("env") || n.name.to_lowercase().contains("fog");
        if !interesting {
            continue;
        }
        let c = shown.entry(n.type_id).or_default();
        *c += 1;
        if *c > max {
            continue;
        }
        println!("[{}] {} {:#x} {} parent {:?}", n.index, scene.path(n.index), n.type_id, bm_formats::scene::type_name(n.type_id).unwrap_or("?"), n.parent);
        println!("     props {:?}", n.props);
        for ctl in &n.controllers {
            println!("     ctrl {} {:?}", ctl.name, ctl.props);
        }
    }
}
