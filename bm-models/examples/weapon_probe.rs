//! `cargo run -p bm-models --example weapon_probe -- [scene]` - hand / attacher bones of the player skeleton and, per weapon
//! template (`.../Weapons/<class>/<name>` with a `Ground01` model link): model bounds and the muzzle marker position.
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    // hero skeleton
    if let Some(h) = scene.nodes.iter().find(|n| n.type_id == 0x2003e8 || n.type_id == 0x2003e9).and_then(|n| n.prim_id) {
        if let Some(sk) = scene.model(h).and_then(|m| m.skeleton.clone()) {
            let w = sk.world_matrices(&bm_models::Pose::bind(&sk));
            for (i, b) in sk.bones.iter().enumerate() {
                let u = b.name.to_ascii_uppercase();
                if u.contains("ATTACH") || u.contains("HAND") && !u.contains("FINGER") && !u.contains("_F") || u.contains("FREE_BONE") {
                    let t = w[i].translation();
                    println!("bone {i:3} {:28} parent {:?} bind pos ({:.1},{:.1},{:.1})", b.name, b.parent, t[0], t[1], t[2]);
                }
            }
        }
    }
    for n in &scene.nodes {
        let p = scene.path(n.index);
        if !p.contains("weapons.zip/Weapons/") || !p.ends_with("/Ground01") || p.contains("PICKUP") {
            continue;
        }
        let Some(prim) = n.prim_id else { continue };
        let Some(m) = scene.node_model(n.index) else { continue };
        let parent = n.parent.map(|q| &scene.nodes[q]).unwrap();
        let muzzle = scene.nodes.iter().find(|c| c.parent == Some(parent.index) && c.name == "PosBox_MuzzleExit");
        let mp = muzzle.map(|c| c.world.translation());
        let pw = parent.world.inverse_affine();
        let ml = muzzle.map(|c| pw.transform_point(c.world.translation()));
        let bb = m.computed_bbox(); let (lo, hi) = (bb.min, bb.max);
        println!("{:60} prim {prim:5} bounds {:?}..{:?} muzzle(local to weapon) {:?} world {:?}", p.rsplit("weapons.zip/Weapons/").next().unwrap_or(&p), lo.map(|v| v.round()), hi.map(|v| v.round()), ml.map(|v| v.map(|x| x.round())), mp.map(|v| v.map(|x| x.round())));
    }
}
