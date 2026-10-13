//! `... --example raw -- scene node_index`: dumps Raw payloads inside a node's props.
use bm_formats::{loader::SceneFiles, prp::Value};
use bm_vfs::Vfs;
fn walk(v: &Value, out: &mut Vec<Vec<u8>>) {
    match v {
        Value::Raw(b) => out.push(b.clone()),
        Value::Container(c) | Value::Array(c) | Value::Object(c) => c.iter().for_each(|x| walk(x, out)),
        _ => {}
    }
}
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    let idx: usize = a[1].parse().unwrap();
    let mut raws = vec![];
    for p in &g.nodes[idx].props { walk(p, &mut raws); }
    for c in &g.nodes[idx].controllers { for p in &c.props { walk(p, &mut raws); } }
    for r in raws {
        println!("raw {} bytes", r.len());
        for (i, ch) in r.chunks(16).enumerate() {
            println!("{:04x}: {}", i * 16, ch.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
        }
    }
}
