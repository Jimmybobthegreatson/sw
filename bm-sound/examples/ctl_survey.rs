//! Research tool: prints the properties of audio controllers. Usage: ctl_survey <scene> <controller> [n]
use bm_formats::{buf::Buf, gms::Gms, prp::{Prp, Value}, scene::SceneGraph};
use bm_sound::snd_db::SndDb;
use bm_vfs::Vfs;
fn short(v: &Value, db: &SndDb) -> String {
    match v {
        Value::Str(s) => { let t: String = s.chars().rev().take(50).collect::<Vec<_>>().into_iter().rev().collect(); format!("S\"{t}\"") }
        Value::I32(i) => match db.object_at(*i as u32) { Some(o) => format!("I{i}=>{:?}:{}", o.kind, o.path), None => format!("I{i}") },
        Value::Container(c) | Value::Array(c) => format!("[{}]", c.iter().take(6).map(|x| short(x, db)).collect::<Vec<_>>().join(", ")) + &if c.len() > 6 { format!("..{}", c.len()) } else { String::new() },
        o => format!("{o:?}"),
    }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let scene = a.get(1).cloned().unwrap_or("Scenes/M01/M01_main".into());
    let ctl = a.get(2).cloned().unwrap_or("Audio_PlaySound".into());
    let n: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(6);
    let vfs = Vfs::from_env().unwrap();
    let db = SndDb::parse(&vfs.open(&format!("{scene}.SND")).unwrap());
    let g = Gms::parse(&vfs.open(&format!("{scene}.GMS")).unwrap()).unwrap();
    let b = Buf::parse(&vfs.open(&format!("{scene}.BUF")).unwrap()).unwrap();
    let p = Prp::parse(&vfs.open(&format!("{scene}.PRP")).unwrap()).unwrap();
    let gr = SceneGraph::build(&g, &b, &p).unwrap();
    let mut k = 0;
    for nd in &gr.nodes {
        for c in nd.controllers.iter().filter(|c| c.name == ctl) {
            if k >= n { return; }
            k += 1;
            println!("== {:#x} {} @ {:?}", nd.type_id, nd.name, nd.position);
            for (i, v) in c.props.iter().enumerate() { println!("   [{i}] {}", short(v, &db)); }
            if ctl == "ZSNDOBJ" { }
        }
        if ctl == "node" && nd.type_id == 0x200012 && k < n { k += 1; println!("== {} p6={}", nd.name, short(&nd.props[6], &db)); }
    }
}
