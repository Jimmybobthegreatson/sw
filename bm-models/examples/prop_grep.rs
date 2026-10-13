//! `prop_grep <level> <needle>...` - scene nodes whose properties (or name) contain a needle (case-insensitive).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    let needles: Vec<String> = a[2..].iter().map(|s| s.to_ascii_lowercase()).collect();
    for n in &scene.graph.nodes {
        let mut hay = format!("{} {:?}", n.name, n.props).to_ascii_lowercase();
        for c in &n.controllers {
            hay.push_str(&format!(" {} {:?}", c.name, c.props));
        }
        for nd in &needles {
            if hay.contains(nd.as_str()) {
                println!("{}\t[{}]\tmatch {}", scene.path(n.index), n.type_name().unwrap_or("?"), nd);
                break;
            }
        }
    }
}
