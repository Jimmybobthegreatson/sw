//! Research: dump every ZParticleTemplate of a scene with its sprite model/material.
use bm_formats::prp::Value;
use bm_models::Scene;
use bm_vfs::Vfs;
fn f(v: &Value) -> String {
    match v {
        Value::F32(x) => format!("{x}"),
        Value::I32(x) => format!("i{x}"),
        Value::Bool(b) => (if *b { "T" } else { "F" }).into(),
        Value::Array(a) => format!("[{}]", a.iter().map(f).collect::<Vec<_>>().join(",")),
        o => format!("{o:?}"),
    }
}
fn main() {
    let vfs = Vfs::from_env().unwrap();
    let base = std::env::args().nth(1).unwrap_or("Scenes/M01/M01_main".into());
    let scene = Scene::load(&vfs, &base).unwrap();
    for n in &scene.nodes {
        if n.type_id != 0x2000e5 { continue; }
        let g = &scene.graph.nodes[n.index];
        let mut line = format!("{:28}", g.name);
        for i in 6..=29 { if i == 6 || i == 22 { line += &format!(" {}", f(&g.props[i])); } else { line += &format!(" {}", f(&g.props[i])); } }
        println!("{line}");
        let pid = match &g.props[4] { Value::I32(x) => *x as usize, _ => 0 };
        match scene.prm.chunk_words(pid) {
            None => println!("      sprite chunk none"),
            Some(w) => {
                let im = scene.materials.image(w[1]);
                println!("      chunk {:x?} tex {:?}", &w[..5], im.map(|im| format!("{}x{} {} {} mips{} alpha{}", im.width, im.height, im.source_format, im.name, im.mip_count, im.has_alpha())));
            }
        }
    }
}
