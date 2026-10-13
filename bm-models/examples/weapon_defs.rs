//! `cargo run --release -p bm-models --example weapon_defs -- [scene]` - the item definition node of every weapon template
//! (`.../weapons.zip/Weapons/<class>/<name>`, types 0x100429 / 0x10042c): weapon type, hand, size, animation strings (hold /
//! reload / recoil clips), silenced token and the `Ground*` links, used to fill `bm-game/data/weapons.ron`.
use bm_formats::prp::Value;
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for n in &scene.nodes {
        let p = scene.path(n.index);
        let Some(rest) = p.split("weapons.zip/Weapons/").nth(1) else { continue };
        if rest.contains('/') && !rest.ends_with("_PICKUP") && rest.matches('/').count() != 1 {
            continue;
        }
        let props = &scene.graph.nodes[n.index].props;
        let toks: Vec<String> = props.iter().filter_map(|v| if let Value::Token(t) = v { Some(t.clone()) } else { None }).filter(|t| t != "BOUNDING_Static").collect();
        if !toks.iter().any(|t| t.starts_with("WT_")) {
            continue;
        }
        let strs: Vec<String> = props.iter().filter_map(|v| if let Value::Str(s) = v { Some(s.clone()) } else { None }).filter(|s| s.starts_with('/')).collect();
        let nums: Vec<String> = props.iter().filter_map(|v| if let Value::F32(f) = v { Some(format!("{f}")) } else { None }).collect();
        println!("{rest}  type {:#x}\n    tokens {toks:?}\n    clips {strs:?}\n    f32 {}", n.type_id, nums.join(" "));
    }
}
