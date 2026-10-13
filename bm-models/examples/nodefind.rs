//! Lists scene nodes whose name contains the filter: `nodefind M01 health`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    let f = a[2].to_ascii_lowercase();
    for (i, n) in scene.nodes.iter().enumerate() {
        if n.name.to_ascii_lowercase().contains(&f) || scene.path(i).to_ascii_lowercase().contains(&f) {
            println!("{i}\t{:#x}\t{:?}\t{}", n.type_id, n.type_name, scene.path(i));
        }
    }
}
