//! Lists LOC leaves of a scene whose path or text contains the filter: `locfind M01 fiber`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    let f = a[2].to_ascii_lowercase();
    let loc = scene.loc.as_ref().expect("scene has LOC");
    for (path, leaf) in loc.leaves() {
        let text = leaf.text();
        if path.to_ascii_lowercase().contains(&f) || text.to_ascii_lowercase().contains(&f) {
            println!("{path} = {}", text.replace('\n', "\n"));
        }
    }
}
