//! Shows how the stub materials of the Landscape_Rocks nodes resolve: `stubs M01 Landscape_Rocks_11`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    let m = scene.materials.mat();
    for (i, n) in scene.nodes.iter().enumerate() {
        if !n.name.eq_ignore_ascii_case(&a[2]) {
            continue;
        }
        let Some(model) = scene.node_model(i) else { continue };
        for me in &model.meshes {
            let id = me.material();
            let inst = m.instance(id);
            let r = scene.materials.stub_remap("", [id]);
            println!("node {i} mesh mat id {id} name {:?} stub {} -> {:?} ({:?})", inst.map(|x| x.name.clone()), inst.is_some_and(|x| x.is_stub()), r.get(&id), r.get(&id).and_then(|t| m.instance(*t)).map(|x| x.name.clone()));
        }
        break;
    }
}
