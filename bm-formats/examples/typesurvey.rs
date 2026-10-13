//! Prints, for every geometry type id of a scene, the number of nodes and a few sample names.
use bm_formats::{buf::Buf, gms::Gms, prp::Prp, scene::SceneGraph};
use bm_vfs::Vfs;
use std::collections::BTreeMap;

fn main() {
    let scene = std::env::args().nth(1).unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().unwrap();
    let gms = Gms::parse(&vfs.open(&format!("{scene}.GMS")).unwrap()).unwrap();
    let buf = Buf::parse(&vfs.open(&format!("{scene}.BUF")).unwrap()).unwrap();
    let prp = Prp::parse(&vfs.open(&format!("{scene}.PRP")).unwrap()).unwrap();
    let g = SceneGraph::build(&gms, &buf, &prp).unwrap();
    let mut by: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    for n in &g.nodes {
        by.entry(n.type_id).or_default().push(&n.name);
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));
    for (t, names) in v.iter().take(40) {
        println!("{t:#08x} {:>5}  {}", names.len(), names.iter().take(4).copied().collect::<Vec<_>>().join(", "));
    }
}
