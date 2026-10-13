//! `cargo run -p bm-models --example facing_probe -- [scene]` - which way characters face: for every actor skeleton prints
//! the bind pose direction of heel -> toe, the root motion of the walk clip and the swing of the feet along Z in the clip.
use bm_models::scene::{NodeKind, Scene};
use bm_vfs::Vfs;
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let bank = scene.anim_bank().expect("bank");
    let mut seen: BTreeMap<u32, String> = BTreeMap::new();
    for n in scene.nodes.iter().filter(|n| n.kind == NodeKind::Actor || n.type_id == 0x2003e9 || n.type_id == 0x2003e8) {
        if let Some(p) = n.prim_id {
            seen.entry(p).or_insert(n.name.clone());
        }
    }
    for (p, nm) in seen {
        let Some(m) = scene.model(p) else { continue };
        let Some(sk) = &m.skeleton else { continue };
        let Some(solver) = bm_models::rig::RigSolver::new(sk) else { continue };
        let bind = sk.world_matrices(&bm_models::Pose::bind(sk));
        let find = |n: &[&str]| n.iter().find_map(|n| sk.bone_index(n));
        let (foot, toe) = (find(&["L_FOOT", "LEFT_FOOT", "L_ANKLE"]), find(&["L_TOE", "LEFT_TOE", "L_TOES", "L_TOE_0"]));
        let mut s = format!("prim {p:5} {nm:26} bones {:3}", sk.bones.len());
        if let (Some(f), Some(t)) = (foot, toe) {
            let (fp, tp) = (bind[f].translation(), bind[t].translation());
            s += &format!(" | bind heel->toe dz {:+.1}", tp[2] - fp[2]);
        } else {
            let names: Vec<&str> = sk.bones.iter().filter(|b| b.name.to_ascii_uppercase().contains("FOOT") || b.name.to_ascii_uppercase().contains("TOE")).map(|b| b.name.as_str()).collect();
            s += &format!(" | foot/toe bones: {names:?}");
        }
        for set in ["Hero/Hero_Walk#Walk_Forward", "Male_Reg/Male_Reg_Walk#Walk"] {
            let Some(i) = bank.find(set) else { continue };
            let Some(an) = bank.animation_with(i, sk, Some(&solver)) else { continue };
            let rm = an.root_motion.first().zip(an.root_motion.last()).map(|(a, b)| [b.1[0] - a.1[0], b.1[2] - a.1[2]]);
            let hips = sk.bone_index("PELVIS").or_else(|| sk.bone_index("HIPS"));
            let mut zs = Vec::new();
            if let (Some(f), Some(h)) = (foot, hips) {
                for k in 0..8 {
                    let t = an.duration * k as f32 / 8.0;
                    let w = sk.world_matrices(&an.sample(sk, t));
                    zs.push(w[f].translation()[2] - w[h].translation()[2]);
                }
            }
            s += &format!("\n    {set}: root dx,dz {rm:?} dur {:.2}s leftfoot z-vs-pelvis over cycle {:?}", an.duration, zs.iter().map(|z| z.round()).collect::<Vec<_>>());
        }
        println!("{s}");
    }
}
