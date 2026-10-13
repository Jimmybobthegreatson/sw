use bm_models::scene::Scene;
use bm_vfs::Vfs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let prm = &scene.prm;
    if let Some(f) = a.get(1) {
        for n in &scene.nodes { if n.name.contains(f.as_str()) { println!("node {} prim {:?} kind {:?} children {:?}", n.name, n.prim_id, n.kind, n.children.iter().map(|&c| (scene.nodes[c].name.clone(), scene.nodes[c].prim_id)).collect::<Vec<_>>()); } }
    }
    let mut seen = std::collections::HashSet::new();
    for mc in prm.model_chunks() {
        let Some(m) = scene.model(mc as u32) else { continue };
        let Some(sk) = &m.skeleton else { continue };
        if sk.face.is_empty() { continue; }
        if !seen.insert((sk.bones.len(), sk.face.iter().map(|f| f.deltas.len()).sum::<usize>())) { continue; }
        println!("model {mc}: bones {} poses {} deltas {}", sk.bones.len(), sk.face.len(), sk.face.iter().map(|f| f.deltas.len()).sum::<usize>());
        for n in ["FA_JAW", "FA_LOWER_LIP", "FA_UPPER_LIP", "HEAD", "FA_RIGHT_MOUTH_EDGE"] {
            if let Some(i) = sk.bone_index(n) {
                let b = &sk.bones[i];
                let w = &b.bind_world;
                println!("  {n} parent {:?} wt {:?} x{:?} y{:?} z{:?}", b.parent, w.translation(), w.transform_vector([1.,0.,0.]), w.transform_vector([0.,1.,0.]), w.transform_vector([0.,0.,1.]));
            }
        }
        // D_Closed is pose index 23
        for d in &sk.face[23].deltas { println!("  D_Closed bone {} {} t {:?} ang {} axis {:?}", d.bone, sk.bones[d.bone].name, d.translation, d.angle, d.axis); }
    }
}
