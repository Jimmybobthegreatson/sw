//! `controller_survey <level>` - counts the controller names of every scene node and prints the nodes that have controllers
//! mentioning clothes, zones or allowed flags.
use std::collections::BTreeMap;
fn main() {
    let level = std::env::args().nth(1).unwrap_or_else(|| "M01".into());
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", level)).expect("scene");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for n in &scene.graph.nodes {
        for c in &n.controllers {
            *counts.entry(c.name.clone()).or_default() += 1;
            let lower = format!("{:?}", c.props).to_ascii_lowercase();
            if c.name.to_ascii_lowercase().contains("cloth") || lower.contains("cloth") || lower.contains("zone") || lower.contains("allowed") {
                println!("{}\t{}\t{:?}", scene.path(n.index), c.name, c.props);
            }
        }
    }
    for (k, v) in counts {
        println!("CTRL {v}\t{k}");
    }
}
