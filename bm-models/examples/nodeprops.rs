//! `nodeprops <scene> <index>...` - prints the decoded properties of scene nodes.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    for s in &a[2..] {
        let i: usize = s.parse().unwrap();
        let n = &scene.graph.nodes[i];
        println!("{i} {:#x} {:?} {}", n.type_id, n.type_name(), scene.path(i));
        for (k, v) in n.props.iter().enumerate() {
            println!("   [{k}] {v:?}");
        }
    }
}
