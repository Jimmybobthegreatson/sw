//! `type_find <type-id hex>` - scene nodes of that type id in every level (path, props count).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let want = u32::from_str_radix(a[1].trim_start_matches("0x"), 16).unwrap();
    let vfs = bm_vfs::Vfs::from_env().expect("game dir");
    for l in ["M00","M01","M02","M03","M04","M05","M06","M08","M09","M10","M11","M12","M13","hideout"] {
        let Ok(scene) = bm_models::Scene::load(&vfs, &format!("Scenes/{l}/{l}_main")) else { continue };
        for n in &scene.graph.nodes {
            if n.type_id == want {
                println!("{l}\t{}\t{} props", scene.path(n.index), n.props.len());
            }
        }
    }
}
