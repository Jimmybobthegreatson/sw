//! Survey of particle-related nodes in all scenes (research tool).
//! usage: survey [scene-substring] [max-samples-per-type] [typeid-hex,...]
use bm_formats::{loader::SceneFiles, prp::Value};
use bm_vfs::Vfs;
fn show(v: &Value) -> String {
    match v {
        Value::Array(a) => {
            let s: Vec<String> = a.iter().map(show).collect();
            format!("[{}]", s.join(","))
        }
        Value::F32(f) => format!("{f}"),
        Value::I32(f) => format!("i{f}"),
        Value::I16(f) => format!("s{f}"),
        Value::Bool(b) => format!("{}", *b as u8).replace('1', "T").replace('0', "F"),
        Value::Str(s) => format!("{s:?}"),
        Value::Token(s) => format!("<{s}>"),
        o => format!("{o:?}"),
    }
}
fn main() {
    let vfs = Vfs::from_env().unwrap();
    let only = std::env::args().nth(1).filter(|s| s != "-");
    let max: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    let types: Vec<u32> = std::env::args().nth(3).map(|s| s.split(',').map(|x| u32::from_str_radix(x, 16).unwrap()).collect()).unwrap_or(vec![0x2000ff, 0x2000e5, 0x2000e4, 0x10044e]);
    let mut scenes = vfs.list("Scenes/**/*.zip");
    scenes.sort_by(|a, b| a.path.cmp(&b.path));
    let mut shown: std::collections::HashMap<u32, usize> = Default::default();
    let mut total: std::collections::BTreeMap<u32, usize> = Default::default();
    for s in scenes {
        let path = s.path.clone();
        let base = path.trim_end_matches(".ZIP").trim_end_matches(".zip").to_string();
        if let Some(o) = &only { if !base.contains(o.as_str()) { continue; } }
        let Ok(sf) = SceneFiles::load(&vfs, &base) else { continue };
        let Ok(g) = sf.graph() else { continue };
        let mut counts: std::collections::BTreeMap<u32, usize> = Default::default();
        for n in &g.nodes {
            if !types.contains(&n.type_id) { continue; }
            *counts.entry(n.type_id).or_default() += 1;
            *total.entry(n.type_id).or_default() += 1;
            let c = shown.entry(n.type_id).or_default();
            if *c < max {
                *c += 1;
                println!("== {base} #{} {:#x} '{}' parent={:?}", n.index, n.type_id, n.name, n.parent.map(|p| g.nodes[p].name.clone()));
                for (i, p) in n.props.iter().enumerate().skip(3) { println!("   p[{i}] {}", show(p)); }
                for c in &n.controllers { println!("   ctrl {} {}", c.name, c.props.iter().map(show).collect::<Vec<_>>().join(" ")); }
            }
        }
        println!("-- {base}: {counts:x?}");
    }
    println!("TOTAL {total:x?}");
}
