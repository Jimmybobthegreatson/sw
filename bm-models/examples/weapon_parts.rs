//! `cargo run --release -p bm-models --example weapon_parts -- [scene] [weapon substring]` - research probe for the modular
//! weapon models: per weapon (every `Ground*` link below `weapons.zip/Weapons/*`) the meshes grouped by *part id*
//! (`Mesh::part`, object word 15) with material names, LOD masks, triangle counts and bounds, plus the
//! `WeaponUpgradeControl` nodes (`AllLevels/equipment.zip/AllLevels/weaponupgrades.zip`): their `_Bones/W_Bone_*` nodes, the stock
//! bone list of the control node and the upgrade nodes (requires / excludes / adds / removes) with their properties.
use bm_formats::prp::Value;
use bm_models::scene::Scene;
use bm_vfs::Vfs;
use std::collections::BTreeMap;

fn strs(v: &Value) -> Vec<String> {
    match v {
        Value::Container(c) | Value::Array(c) => c
            .iter()
            .filter_map(|x| if let Value::Str(s) = x { Some(s.rsplit('\\').next().unwrap_or(s).to_string()) } else { None })
            .collect(),
        _ => Vec::new(),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let filter = a.get(1).map(|s| s.to_ascii_lowercase());
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    for n in &scene.nodes {
        let p = scene.path(n.index);
        let Some(rest) = p.split("weapons.zip/Weapons/").nth(1) else { continue };
        if !n.name.starts_with("Ground") || n.prim_id.is_none() || rest.contains("_PICKUP") {
            continue;
        }
        if filter.as_ref().is_some_and(|f| !rest.to_ascii_lowercase().contains(f)) {
            continue;
        }
        let Some(m) = scene.node_model(n.index) else { continue };
        let mut parts: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (i, mesh) in m.meshes.iter().enumerate() {
            parts.entry(mesh.part).or_default().push(i);
        }
        println!("== {rest}  prim {}  meshes {}  parts {}", n.prim_id.unwrap(), m.meshes.len(), parts.len());
        for (part, ms) in &parts {
            println!("  part {part:#x}");
            for &i in ms {
                let mesh = &m.meshes[i];
                let b = mesh.bbox();
                let mat = scene.materials.desc(mesh.material()).map(|d| d.name.clone()).unwrap_or_default();
                let mat: String = mat.chars().map(|c| if c.is_ascii_graphic() { c } else { '?' }).collect();
                println!(
                    "    #{i:<3} lod={:#04x} tris={:4} x[{:.0},{:.0}] y[{:.0},{:.0}] z[{:.0},{:.0}] flags={:#x} {}",
                    mesh.lod_mask(), mesh.triangle_count(), b.min[0], b.max[0], b.min[1], b.max[1], b.min[2], b.max[2], mesh.flags, mat
                );
            }
        }
    }
    println!("\n== WeaponUpgradeControl");
    for c in &scene.nodes {
        let p = scene.path(c.index);
        if !p.contains("weaponupgrades.zip/WeaponUpgradeControl/") || c.type_id != 0x100440 {
            continue;
        }
        if filter.as_ref().is_some_and(|f| !c.name.to_ascii_lowercase().contains(f)) {
            continue;
        }
        let g = &scene.graph.nodes[c.index];
        let stock: Vec<String> = g.props.iter().flat_map(strs).collect();
        println!("-- {}  stock bones {:?}", c.name, stock);
        for ch in scene.nodes.iter().filter(|x| x.parent == Some(c.index)) {
            if ch.name == "_Bones" {
                let bones: Vec<&str> = scene.nodes.iter().filter(|x| x.parent == Some(ch.index)).map(|x| x.name.as_str()).collect();
                println!("   bones (node order) {bones:?}");
            } else if ch.type_id == 0x200436 {
                let gp = &scene.graph.nodes[ch.index].props;
                let tok = gp.iter().find_map(|v| if let Value::Token(t) = v { if t.starts_with("UT_") { Some(t.clone()) } else { None } } else { None }).unwrap_or_default();
                let tier = gp.iter().find_map(|v| if let Value::Token(t) = v { if t.starts_with("UPGRADE_TIER") { Some(t.clone()) } else { None } } else { None }).unwrap_or_default();
                let lists: Vec<Vec<String>> = gp.iter().filter(|v| matches!(v, Value::Container(_))).map(strs).collect();
                let props: Vec<String> = scene.graph.nodes[ch.index].controllers.iter().map(|k| format!("{:?}", &k.props[0..k.props.len().min(3)])).collect();
                println!("   upgrade {} {tok} {tier} requires/excl/add/remove {lists:?} props {props:?}", ch.name);
            }
        }
    }
}
