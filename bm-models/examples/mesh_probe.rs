//! `cargo run -p bm-models --example mesh_probe -- <scene> <prim id>` - lists every mesh of a model with its group word,
//! LOD / variant masks, triangle count and material name (outfit selection research).
use bm_models::scene::Scene;
use bm_vfs::Vfs;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let prim: u32 = a.get(1).and_then(|s| s.parse().ok()).expect("prim id");
    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let scene = Scene::load(&vfs, &name).expect("scene");
    let m = scene.model(prim).expect("model");
    for (i, mesh) in m.meshes.iter().enumerate() {
        let mat = scene.materials.desc(mesh.material()).map(|d| d.name.clone()).unwrap_or_default();
        let w = scene.prm.chunk_words(mesh.source_chunk as usize).unwrap_or_default();
        println!("      words {}", w.iter().take(16).map(|x| format!("{x:x}")).collect::<Vec<_>>().join(" "));
        let subs: Vec<String> = mesh.submeshes.iter().map(|s| format!("{}:{}t", scene.materials.desc(s.material).map(|d| d.name.rsplit('/').next().unwrap_or("").to_string()).unwrap_or_default(), s.index_count / 3)).collect();
        println!("      submeshes {}", subs.join(" "));
        println!(
            "{:3} grp={:#010x} lod={:#04x} var={:#04x} variant={:#x} flags={:#x} tris={:5} skinned={} y=[{:.0},{:.0}] mat#{} {}",
            i, mesh.group, mesh.lod_mask(), mesh.variant_mask(), mesh.variant, mesh.flags, mesh.triangle_count(), mesh.is_skinned(), mesh.bbox().min[1], mesh.bbox().max[1], mesh.material(), mat
        );
    }
}
