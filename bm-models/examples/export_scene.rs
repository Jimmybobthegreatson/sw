//! `cargo run --release -p bm-models --example export_scene -- [scene] [out.glb] [options]`
//!
//! Exports a scene (default `Scenes/M01/M01_main`) or a subtree to a glTF binary.
//!
//! Options: `--node <name-or-index>` export only that subtree, `--max-nodes <n>`, `--tex <px>` texture
//! size cap (default 512, 0 = full size), `--no-textures`, `--native` (keep Glacier coordinates),
//! `--cm` (keep centimetres), `--colors` (write vertex colors), `--all` (also bounds/shadow meshes
//! and shadow materials), `--obj` (write Wavefront OBJ instead; requires --node or --max-nodes).
//! Needs `BM_GAME_DIR` (or the default install location).

use bm_models::gltf_export::{export_scene_glb, CoordSystem, ExportOptions};
use bm_models::obj::export_scene_obj;
use bm_models::scene::{NodeKind, Scene};
use bm_vfs::Vfs;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |args: &mut Vec<String>, f: &str| -> bool {
        if let Some(p) = args.iter().position(|a| a == f) {
            args.remove(p);
            true
        } else {
            false
        }
    };
    let opt = |args: &mut Vec<String>, f: &str| -> Option<String> {
        let p = args.iter().position(|a| a == f)?;
        args.remove(p);
        if p < args.len() { Some(args.remove(p)) } else { None }
    };
    let node = opt(&mut args, "--node");
    let max_nodes = opt(&mut args, "--max-nodes").and_then(|s| s.parse::<usize>().ok());
    let tex = opt(&mut args, "--tex").and_then(|s| s.parse::<u32>().ok()).unwrap_or(512);
    let no_tex = flag(&mut args, "--no-textures");
    let native = flag(&mut args, "--native");
    let cm = flag(&mut args, "--cm");
    let colors = flag(&mut args, "--colors");
    let all = flag(&mut args, "--all");
    let obj = flag(&mut args, "--obj");
    let scene_name = args.first().cloned().unwrap_or_else(|| "Scenes/M01/M01_main".into());
    let scene_name = scene_name.trim_end_matches(".ZIP").trim_end_matches(".zip").to_string();
    let default_out = format!("{}.{}", scene_name.rsplit('/').next().unwrap(), if obj { "obj" } else { "glb" });
    let out = args.get(1).cloned().unwrap_or(default_out);

    let vfs = Vfs::from_env().expect("game dir (set BM_GAME_DIR)");
    let t0 = std::time::Instant::now();
    let scene = Scene::load(&vfs, &scene_name).expect("scene");
    eprintln!("loaded {} nodes in {:.1?}", scene.nodes.len(), t0.elapsed());

    let mut opts = ExportOptions::default();
    opts.max_geometry_nodes = max_nodes;
    opts.max_texture_dim = if tex == 0 { None } else { Some(tex) };
    opts.textures = !no_tex;
    opts.vertex_colors = colors;
    if native {
        opts.coords = CoordSystem::Native;
    }
    if cm {
        opts.scale = 1.0;
    }
    if all {
        opts.kinds.extend([NodeKind::Bound, NodeKind::ShadowMesh]);
        opts.include_pos_only = true;
        opts.include_shadow_materials = true;
    }
    if let Some(n) = node {
        opts.root = Some(match n.parse::<usize>() {
            Ok(i) => i,
            Err(_) => *scene.graph.find_by_name(&n).first().expect("no node with that name"),
        });
    }
    if obj {
        let o = export_scene_obj(&scene, &opts);
        std::fs::write(&out, o.obj).unwrap();
        std::fs::write(out.replace(".obj", ".mtl"), o.mtl).unwrap();
        for (name, bytes) in o.textures {
            std::fs::write(std::path::Path::new(&out).with_file_name(name), bytes).unwrap();
        }
        eprintln!("wrote {out}");
        return;
    }
    let r = export_scene_glb(&scene, &opts);
    std::fs::write(&out, &r.glb).unwrap();
    let s = &r.stats;
    eprintln!(
        "wrote {out}: {:.1} MB, {} nodes ({} geometry), {} meshes / {} primitives, {} tris stored / {} instanced, {} materials, {} textures, {} skins",
        s.bytes as f64 / 1e6, s.nodes, s.geometry_nodes, s.meshes, s.primitives, s.unique_triangles, s.instanced_triangles, s.materials, s.textures, s.skins
    );
    if let Some(b) = s.bbox {
        eprintln!("world bbox (cm): {:.0?} .. {:.0?}", b.min, b.max);
    }
    eprintln!("total {:.1?}", t0.elapsed());
}
