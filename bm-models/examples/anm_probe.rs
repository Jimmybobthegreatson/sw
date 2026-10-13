//! `cargo run -p bm-models --example anm_probe -- [scene]` - per actor skeleton: bone count, rig solver availability, leg
//! length and the head height reached by the `Male_Reg` / `Female_Reg` / `Hero` walk clips (animation retarget sanity).
use bm_models::scene::{NodeKind, Scene};
use bm_vfs::Vfs;
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let bank = scene.anim_bank().expect("bank");
    let mut seen: BTreeMap<u32, (String, String)> = BTreeMap::new();
    for n in scene.nodes.iter().filter(|n| n.kind == NodeKind::Actor || n.type_id == 0x2003e9 || n.type_id == 0x2003e8) {
        if let Some(p) = n.prim_id {
            let db = scene.actor_database(n.index).unwrap_or_default();
            seen.entry(p).or_insert((db, n.name.clone()));
        }
    }
    for (p, (db, nm)) in seen {
        let Some(m) = scene.model(p) else { continue };
        let Some(sk) = &m.skeleton else { continue };
        let solver = bm_models::rig::RigSolver::new(sk);
        let mut s = format!("prim {p:5} {db:28} {nm:28} bones {:3} solver {}", sk.bones.len(), solver.is_some());
        if let Some(sv) = &solver {
            s += &format!(" leg {:.1}", sv.leg_length());
            for set in ["Male_Reg/Male_Reg_Walk#Walk", "Female_Reg/Female_Reg_Walk#Walk", "Hero/Hero_Walk#Walk"] {
                if let Some(i) = bank.find(set).or_else(|| bank.find_suffix(&set[set.find('/').unwrap() + 1..])) {
                    if let Some(an) = bank.animation_with(i, sk, Some(sv)) {
                        let pose = an.sample(sk, 0.3);
                        let w = sk.world_matrices(&pose);
                        let h = sk.bone_index("HEAD").map(|h| w[h].translation()[1]).unwrap_or(0.0);
                        s += &format!(" | {} head {:.0}", set.split('/').next().unwrap(), h);
                    }
                }
            }
        }
        println!("{s}");
    }
}
