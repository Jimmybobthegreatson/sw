//! `hmas_survey [scene...]` - every ZHM3HmAs (disguise definition): quality, allowed zones, cloth name.
use bm_formats::prp::Value;
fn main() {
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    let mut names: Vec<String> = std::env::args().skip(1).collect();
    if names.is_empty() {
        names = ["M00", "M01", "M02", "M03", "M04", "M05", "M06", "M08", "M09", "M10", "M11", "M12", "M13", "hideout"].iter().map(|s| s.to_string()).collect();
    }
    for l in names {
        let scene = match bm_models::Scene::load(&vfs, &format!("Scenes/{l}/{l}_main")) { Ok(s) => s, Err(e) => { eprintln!("{l}: {e}"); continue } };
        for (i, n) in scene.graph.nodes.iter().enumerate() {
            if n.type_id != 0x100446 { continue; }
            let q = n.props.get(11).and_then(|v| if let Value::Token(t) = v { Some(t.clone()) } else { None }).unwrap_or_default();
            let z = match n.props.get(12) { Some(Value::StrArray(a)) => a.join(","), _ => "-".into() };
            let cn = match n.props.get(16) { Some(Value::Str(s)) => s.clone(), _ => String::new() };
            let name = scene.path(i).rsplit('/').next().unwrap_or("").to_string();
            println!("{l}\t{q}\t[{z}]\t{cn}\t{name}");
        }
    }
}
