//! `cargo run -p bm-formats --example rooms -- Scenes/M13/M13_main`: lists ZROOM nodes with their room-specific props.
use bm_formats::{loader::SceneFiles, prp::Value};
use bm_vfs::Vfs;

fn short(v: &Value) -> String {
    match v {
        Value::Raw(b) => format!("Raw[{}]", b.len()),
        Value::Container(c) => format!("C{}[{}]", c.len(), c.iter().map(short).collect::<Vec<_>>().join(",")),
        Value::Str(s) => format!("\"{}\"", s.rsplit(char::from(92u8)).next().unwrap_or("")),
        Value::Array(a) => format!("A{}", a.len()),
        o => format!("{o:?}"),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let scene = a.first().cloned().unwrap_or("Scenes/M13/M13_main".into());
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &scene).unwrap();
    let g = sf.graph().unwrap();
    for node in g.nodes.iter().filter(|n| n.type_id == 0x100021) {
        let p = &node.props;
        let name = g.path(node.index);
        let name = name.rsplit('/').next().unwrap();
        let tail: Vec<String> = p.iter().skip(9).map(short).collect();
        let gi = node.index.saturating_sub(1);
        let h = sf.gms.geom(gi);
        let span = sf.gms.geom_span(gi).map(|s| s.len()).unwrap_or(0);
        println!("[{}] {} f0c={:#x} ctrl={:#x} seq={:#x} flags={:#x} span={} | {}", node.index, name, h.as_ref().map(|h| h.field_0c).unwrap_or(0), h.as_ref().map(|h| h.control).unwrap_or(0), h.as_ref().map(|h| h.seq).unwrap_or(0), h.as_ref().map(|h| h.flags).unwrap_or(0), span, tail.join(" "));
    }
}
