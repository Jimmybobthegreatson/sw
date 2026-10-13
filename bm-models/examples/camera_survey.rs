//! `camera_survey` - security camera nodes of every level with their properties.
fn main() {
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    for l in ["M00","M01","M02","M03","M04","M05","M06","M08","M09","M10","M11","M12","M13","hideout"] {
        let Ok(scene) = bm_models::Scene::load(&vfs, &format!("Scenes/{l}/{l}_main")) else { continue };
        for n in &scene.graph.nodes {
            let tn = n.type_name().unwrap_or("?");
            let cam_ctrl = n.controllers.iter().any(|c| c.name.contains("Camera") || c.name == "SecurityZone");
            if tn.contains("Camera") || cam_ctrl {
                println!("{l}\t{}\t[{}]\t{:?}", scene.path(n.index), tn, n.controllers.iter().filter(|c| c.name.contains("Camera")).map(|c| format!("{}{:?}", c.name, c.props)).collect::<Vec<_>>());
            }
        }
    }
}
