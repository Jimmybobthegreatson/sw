//! `... --example actors -- scene`: ZHM3Actor nodes: group, props tail, controllers (names + short props)
use bm_formats::{loader::SceneFiles, prp::Value};
use bm_vfs::Vfs;
fn short(v: &Value) -> String {
    match v {
        Value::Raw(b) => format!("Raw[{}]", b.len()),
        Value::Container(c) => format!("C{}[{}]", c.len(), c.iter().map(short).collect::<Vec<_>>().join(",")),
        Value::Str(s) => format!("\"{}\"", s.rsplit(char::from(92u8)).next().unwrap_or("")),
        Value::Array(a) => format!("A[{}]", a.iter().map(short).collect::<Vec<_>>().join(",")),
        Value::F32(f) => format!("{f}"),
        Value::I32(f) => format!("{f}"),
        Value::Bool(b) => format!("{}", *b as u8),
        Value::Token(t) => t.clone(),
        o => format!("{o:?}"),
    }
}
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let limit: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(100);
    let g = sf.graph().unwrap();
    let mut n = 0;
    for node in g.nodes.iter().filter(|n| n.type_id == 0x2003e9) {
        n += 1; if n > limit { break; }
        let grp = node.parent.map(|p| g.nodes[p].name.clone()).unwrap_or_default();
        println!("[{}] {}", node.index, grp);
        println!("   props {}", node.props.iter().skip(3).map(short).collect::<Vec<_>>().join(" "));
        for c in &node.controllers { println!("   ctl {} {}", c.name, c.props.iter().map(short).collect::<Vec<_>>().join(" ")); }
    }
}
