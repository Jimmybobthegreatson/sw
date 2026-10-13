//! `... --example levelctl -- scene`: the level-control script parameters (target actors, exit list) and LOC objectives.
use bm_formats::{gameplay::*, loader::SceneFiles};
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    let idx = PathIndex::new(&g);
    for s in scripts(&g).iter().filter(|s| s.module.to_lowercase().contains("level")) {
        println!("script {} on [{}] {}", s.module, s.node, g.nodes[s.node].name);
        for t in s.strings() {
            let r = idx.resolve(&t);
            println!("    {} -> {}", t.rsplit(char::from(92u8)).next().unwrap_or(""), r.map(|i| format!("[{}] {:#x} {}", i, g.nodes[i].type_id, g.nodes[i].name)).unwrap_or("-".into()));
        }
    }
    let level = a[0].rsplit('/').next().unwrap().trim_end_matches("_main").to_string();
    for o in mission_objectives(sf.loc.as_ref().unwrap(), &level) { println!("  obj {} {:?} {} / {}", o.key, o.kind, o.text, o.who); }
}
