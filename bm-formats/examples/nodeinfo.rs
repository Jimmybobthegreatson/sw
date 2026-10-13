//! `... --example nodeinfo -- scene <type-hex|ctl:Name|name:substr> [limit]`: compact node dump (props short form).
use bm_formats::{loader::SceneFiles, prp::Value};
use bm_vfs::Vfs;
fn short(v: &Value) -> String {
    match v {
        Value::Raw(b) => format!("Raw[{}]", b.len()),
        Value::Container(c) => format!("C{}[{}]", c.len(), c.iter().map(short).collect::<Vec<_>>().join(",")),
        Value::Str(s) => format!("\"{}\"", s.replace(char::from(92u8), "/")),
        Value::Array(a) => format!("A[{}]", a.iter().map(short).collect::<Vec<_>>().join(",")),
        Value::F32(f) => format!("{f}"),
        Value::I32(f) => format!("{f}"),
        Value::Bool(b) => format!("{}", *b as u8),
        Value::Token(t) => t.clone(),
        Value::Object(o) => format!("O[{}]", o.iter().map(short).collect::<Vec<_>>().join(",")),
        o => format!("{o:?}"),
    }
}
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let q = a.get(1).cloned().unwrap_or_default();
    let limit: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let g = sf.graph().unwrap();
    let mut n = 0;
    for node in &g.nodes {
        let ok = if let Some(c) = q.strip_prefix("ctl:") { node.controllers.iter().any(|x| x.name == c) }
            else if let Some(s) = q.strip_prefix("name:") { g.path(node.index).to_lowercase().contains(&s.to_lowercase()) }
            else { u32::from_str_radix(q.trim_start_matches("0x"), 16).map(|t| node.type_id == t).unwrap_or(false) };
        if !ok { continue; }
        n += 1; if n > limit { break; }
        println!("[{}] {:#x} {} pos={:?}", node.index, node.type_id, g.path(node.index).rsplit("M01_CHARACTERS.zip/").next().unwrap(), node.position);
        println!("   props {}", node.props.iter().skip(3).map(short).collect::<Vec<_>>().join(" "));
        for c in &node.controllers { println!("   ctl {} {}", c.name, c.props.iter().map(short).collect::<Vec<_>>().join(" ")); }
    }
}
