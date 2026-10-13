//! Prints the material of every mesh of a node path suffix: `modelmats M01 equip_binoculars_01/ground01`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let scene = bm_models::Scene::load(&vfs, &format!("Scenes/{0}/{0}_main", a[1])).expect("scene");
    let suf = a[2].to_ascii_lowercase();
    for i in 0..scene.nodes.len() {
        if !scene.path(i).to_ascii_lowercase().ends_with(&suf) {
            continue;
        }
        let Some(m) = scene.node_model(i) else { continue };
        let remap = scene.materials.stub_remap("Equip_Binoculars_01", m.meshes.iter().map(|x| x.material()));
        for me in &m.meshes {
            let id = me.material();
            let d = scene.materials.desc(id);
            let r = remap.get(&id).copied();
            let rd = r.and_then(|r| scene.materials.desc(r));
            println!("mesh mat {id} {:?} lod0 {} tris {} -> remap {:?} {:?}", d.map(|x| x.name.clone()), me.is_lod0(), me.triangle_count(), r, rd.map(|x| x.name.clone()));
        }
    }
}
