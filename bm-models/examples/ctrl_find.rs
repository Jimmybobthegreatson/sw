//! `ctrl_find <level> <controller name>` - prints nodes that carry a controller with that name and its properties.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    for n in &scene.graph.nodes {
        for c in &n.controllers {
            if c.name == a[2] {
                println!("{}  [{}]", scene.path(n.index), n.type_name().unwrap_or("?"));
                for (i, p) in c.props.iter().enumerate() {
                    println!("   [{i}] {p:?}");
                }
            }
        }
    }
}
