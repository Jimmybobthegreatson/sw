//! `cargo run -p bm-formats --example nodes -- Scenes/M13/M13_main <type-hex|name-substr> [limit]`
//! Prints scene nodes of a type (or whose path contains the text) with positions, props and controllers.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let scene = a.first().cloned().unwrap_or("Scenes/M13/M13_main".into());
    let filt = a.get(1).cloned().unwrap_or_default();
    let limit: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(40);
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &scene).unwrap();
    let g = sf.graph().unwrap();
    let tid = u32::from_str_radix(filt.trim_start_matches("0x"), 16).ok().filter(|_| filt.starts_with("0x"));
    let mut n = 0;
    for node in &g.nodes {
        let ok = match tid {
            Some(t) => node.type_id == t,
            None => g.path(node.index).to_lowercase().contains(&filt.to_lowercase()),
        };
        if !ok { continue; }
        n += 1;
        if n > limit { break; }
        println!("[{}] {:#x} {} parent={:?} pos={:?}", node.index, node.type_id, g.path(node.index), node.parent, node.position);
        println!("    props={:?}", node.props);
        for c in &node.controllers { println!("    ctl {} {:?}", c.name, c.props); }
    }
}
