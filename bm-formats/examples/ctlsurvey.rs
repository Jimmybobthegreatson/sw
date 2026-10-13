//! `... --example ctlsurvey -- scene`: histogram of controller names with a sample node and props.
use bm_formats::loader::SceneFiles;
use bm_vfs::Vfs;
use std::collections::BTreeMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let vfs = Vfs::from_env().unwrap();
    let sf = SceneFiles::load(&vfs, &a[0]).unwrap();
    let g = sf.graph().unwrap();
    let mut m: BTreeMap<String, (usize, usize, String)> = BTreeMap::new();
    for n in &g.nodes {
        for c in &n.controllers {
            let e = m.entry(c.name.clone()).or_insert((0, n.index, format!("{:?}", c.props)));
            e.0 += 1;
        }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by_key(|(_, e)| std::cmp::Reverse(e.0));
    for (name, (n, idx, props)) in v {
        let p: String = props.chars().take(200).collect();
        println!("{n:>5} {name:<34} e.g. [{idx}] {} | {p}", g.path(idx).rsplit('/').next().unwrap());
    }
}
