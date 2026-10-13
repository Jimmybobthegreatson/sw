//! `node_dump <level> <path substring>` - full dump (type, props, controllers) of nodes whose path contains the substring.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    for n in &scene.graph.nodes {
        if !scene.path(n.index).to_ascii_lowercase().contains(&a[2].to_ascii_lowercase()) { continue; }
        println!("{} [{}] props={:?}", scene.path(n.index), n.type_name().unwrap_or("?"), n.props);
        for c in &n.controllers {
            println!("   ctrl {} {:?}", c.name, c.props);
        }
    }
}
