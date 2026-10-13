//! Tests against the user's own game files (skipped when `BM_GAME_DIR` / the default install is absent).

use std::sync::OnceLock;

use bm_models::gltf_export::{export_model_glb, export_scene_glb, CoordSystem, ExportOptions};
use bm_models::import::{import_glb, ImportOptions};
use bm_models::mesh::{Model, VertexLayout};
use bm_models::obj::export_scene_obj;
use bm_models::prm_write::{build_prm, write_model};
use bm_models::scene::{NodeKind, Scene};
use bm_vfs::{game_dir_if_present, Vfs};

fn vfs() -> Option<&'static Vfs> {
    static V: OnceLock<Option<Vfs>> = OnceLock::new();
    V.get_or_init(|| {
        let Some(dir) = game_dir_if_present() else {
            eprintln!("BM_GAME_DIR not available, skipping");
            return None;
        };
        Some(Vfs::new(dir).unwrap())
    })
    .as_ref()
}

fn m01() -> Option<&'static Scene> {
    static S: OnceLock<Option<Scene>> = OnceLock::new();
    S.get_or_init(|| vfs().map(|v| Scene::load(v, "Scenes/M01/M01_main").expect("load M01"))).as_ref()
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unique_prims(scene: &Scene) -> Vec<u32> {
    let mut v: Vec<u32> = scene.nodes.iter().filter_map(|n| n.prim_id).collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[test]
fn models_decode_for_all_non_scatter_nodes() {
    let Some(sc) = m01() else { return };
    let (mut total, mut ok) = (0, 0);
    let mut scatter_fail = 0;
    for n in &sc.nodes {
        let Some(p) = n.prim_id else { continue };
        let decoded = sc.model(p).is_some();
        if n.kind == NodeKind::Scatter {
            // ZScatterObj objects (words[0] == 0x90109) are procedural scatter descriptions, not meshes
            scatter_fail += (!decoded) as usize;
            continue;
        }
        total += 1;
        ok += decoded as usize;
    }
    eprintln!("M01: {ok}/{total} non-scatter nodes decode; {scatter_fail} scatter nodes are procedural");
    assert_eq!(ok, total, "every non-scatter model must decode");
    assert!(total > 5000);
    // water surfaces become grid meshes
    let water = sc
        .nodes
        .iter()
        .filter(|n| n.name.starts_with("WaterSurface"))
        .filter_map(|n| sc.node_model(n.index))
        .find(|m| m.meshes.iter().any(|x| x.name == "water_surface"))
        .expect("a water surface grid model");
    assert!(water.triangle_count() >= 8);
}

#[test]
fn all_strides_and_layouts_are_present() {
    let Some(sc) = m01() else { return };
    let mut seen = std::collections::HashSet::new();
    for p in unique_prims(sc) {
        if let Some(m) = sc.model(p) {
            for me in &m.meshes {
                seen.insert(me.layout);
                assert_eq!(me.vertices.positions.len(), me.vertices.colors.len());
            }
        }
    }
    for l in [VertexLayout::Standard28, VertexLayout::Simple24, VertexLayout::Skinned34, VertexLayout::PosColor10] {
        assert!(seen.contains(&l), "layout {l:?} not found");
    }
}

#[test]
fn normals_tangents_and_winding_are_consistent() {
    let Some(sc) = m01() else { return };
    let (mut tri_neg, mut tri_pos) = (0usize, 0usize);
    let (mut tan_dot, mut tan_n) = (0f64, 0usize);
    for p in unique_prims(sc) {
        let Some(m) = sc.model(p) else { continue };
        for me in &m.meshes {
            let v = &me.vertices;
            for (i, n) in v.normals.iter().enumerate() {
                let l = dot(*n, *n);
                assert!((l - 1.0).abs() < 1e-3, "normal not unit: {n:?}");
                if let Some(t) = v.tangents.get(i) {
                    tan_dot += dot(*n, [t[0], t[1], t[2]]).abs() as f64;
                    tan_n += 1;
                }
            }
            if v.normals.is_empty() {
                continue;
            }
            for t in me.indices.chunks_exact(3) {
                let p0 = v.positions[t[0] as usize];
                let fnrm = cross(sub(v.positions[t[1] as usize], p0), sub(v.positions[t[2] as usize], p0));
                let d = dot(fnrm, v.normals[t[0] as usize]);
                if d < 0.0 {
                    tri_neg += 1;
                } else if d > 0.0 {
                    tri_pos += 1;
                }
            }
        }
    }
    // stored winding is clockwise (D3D, left-handed): right-handed cross product opposes the normal
    let frac = tri_neg as f64 / (tri_neg + tri_pos) as f64;
    eprintln!("winding: {:.2}% of triangles clockwise; mean |n.t| = {:.4}", frac * 100.0, tan_dot / tan_n as f64);
    assert!(frac > 0.98, "winding fraction {frac}");
    assert!(tan_dot / (tan_n as f64) < 0.05, "tangents are not perpendicular to normals");
}

#[test]
fn skinned_meshes_bind_to_the_right_bones() {
    let Some(sc) = m01() else { return };
    let (mut near, mut total, mut shuffled) = (0usize, 0usize, 0usize);
    let mut skinned_models = 0;
    for p in unique_prims(sc) {
        let Some(m) = sc.model(p) else { continue };
        let Some(sk) = m.skeleton.as_ref() else { continue };
        skinned_models += 1;
        for me in m.meshes.iter().filter(|m| m.is_skinned()) {
            assert!(me.bone_palette.len() <= 26);
            for i in 0..me.vertices.len() {
                let w = me.vertices.weights[i];
                let sum = w[0] + w[1] + w[2] + w[3];
                assert!((sum - 1.0).abs() < 1e-3 && w.iter().all(|x| *x >= -1e-6), "bad weights {w:?}");
                let j = me.vertices.joints[i][0] as usize;
                assert!(j < sk.bones.len(), "joint {j} out of range ({} bones)", sk.bones.len());
                if w[0] < 0.99 {
                    continue;
                }
                let d = |b: usize| {
                    let t = sk.bones[b].bind_world.translation();
                    let q = sub(t, me.vertices.positions[i]);
                    dot(q, q).sqrt()
                };
                total += 1;
                near += (d(j) < 40.0) as usize;
                shuffled += (d((j + 37) % sk.bones.len()) < 40.0) as usize;
            }
        }
    }
    assert!(skinned_models > 10);
    let (f, c) = (near as f64 / total as f64, shuffled as f64 / total as f64);
    eprintln!("{skinned_models} skinned models: {:.1}% of weight-1 vertices within 40 cm of their bone (control {:.1}%)", f * 100.0, c * 100.0);
    assert!(f > 0.9 && f > c * 1.8);
}

#[test]
fn skeleton_hierarchy_is_self_consistent() {
    let Some(sc) = m01() else { return };
    let mut checked = 0;
    for p in unique_prims(sc) {
        let Some(m) = sc.model(p) else { continue };
        let Some(sk) = m.skeleton.as_ref() else { continue };
        for (i, b) in sk.bones.iter().enumerate() {
            if let Some(par) = b.parent {
                assert!(par < i, "parents precede children");
                let w = sk.bones[par].bind_world.mul(&b.bind_local);
                for c in 0..4 {
                    for r in 0..4 {
                        assert!((w.0[c][r] - b.bind_world.0[c][r]).abs() < 0.05, "bone {} {c}{r}: {} vs {}", b.name, w.0[c][r], b.bind_world.0[c][r]);
                    }
                }
            }
            assert!((b.bind_world.det3() - 1.0).abs() < 1e-2, "bone {} is not a proper rotation", b.name);
        }
        checked += 1;
        if checked > 30 {
            break;
        }
    }
    assert!(checked > 0);
    // the player model has the well known 100-bone skeleton starting with GROUND/PELVIS
    let hit = unique_prims(sc).into_iter().filter_map(|p| sc.model(p)).find_map(|m| m.skeleton.clone().filter(|s| s.bones.len() == 100));
    let sk = hit.expect("a 100 bone skeleton");
    assert_eq!(sk.bones[0].name, "GROUND");
    assert_eq!(sk.bones[1].name, "PELVIS");
    assert!(sk.bone_index("HEAD").is_some());
    // pose API: bind pose skinning matrices are identity
    let pose = bm_models::skeleton::Pose::bind(&sk);
    for m in sk.skinning_matrices(&pose) {
        for c in 0..4 {
            for r in 0..4 {
                let e = if c == r { 1.0 } else { 0.0 };
                assert!((m.0[c][r] - e).abs() < 0.01, "skinning matrix not identity at bind pose");
            }
        }
    }
}

#[test]
fn node_transforms_are_proper_rotations() {
    let Some(sc) = m01() else { return };
    let mut mirrored = 0;
    for n in &sc.nodes {
        if n.world.det3() < 0.0 {
            mirrored += 1;
        }
    }
    // the stored matrices have determinant -1 (reversed axes); after conversion none may be mirrored
    assert_eq!(mirrored, 0);
}

#[test]
fn materials_and_textures_resolve() {
    let Some(sc) = m01() else { return };
    let mut with_tex = 0;
    let mut decoded = 0;
    let mut total = 0;
    for p in unique_prims(sc).into_iter().take(800) {
        let Some(m) = sc.model(p) else { continue };
        for id in m.materials() {
            total += 1;
            let d = sc.materials.desc(id);
            assert!(d.is_some(), "material {id} missing");
            if let Some(t) = d.unwrap().diffuse_texture() {
                with_tex += 1;
                if let Some(img) = sc.materials.image(t) {
                    decoded += 1;
                    assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
                }
            }
        }
    }
    eprintln!("{total} material uses, {with_tex} with diffuse texture, {decoded} decoded");
    assert!(with_tex as f64 > total as f64 * 0.5);
    assert!(decoded as f64 > with_tex as f64 * 0.8);
    // a known material
    let d = sc.materials.desc(286).unwrap();
    assert_eq!(d.name, "Locations/M01/Landscape_M01_Jungle_01_Opac");
    assert_eq!(d.diffuse_texture(), Some(155));
}

fn gltf_stats(glb: &[u8]) -> (usize, usize, [f32; 3], [f32; 3]) {
    let g = gltf::Gltf::from_slice(glb).expect("exported glb parses");
    let blob = g.blob.as_deref().unwrap();
    let mut prims = 0;
    let mut tris = 0;
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    // world bbox over all mesh nodes
    fn walk(node: gltf::Node, parent: [[f32; 4]; 4], blob: &[u8], prims: &mut usize, tris: &mut usize, mn: &mut [f32; 3], mx: &mut [f32; 3]) {
        let m = bm_models::math::Mat4(parent).mul(&bm_models::math::Mat4(node.transform().matrix()));
        if let Some(mesh) = node.mesh() {
            for p in mesh.primitives() {
                *prims += 1;
                let r = p.reader(|_| Some(blob));
                *tris += r.read_indices().unwrap().into_u32().count() / 3;
                for v in r.read_positions().unwrap() {
                    let w = m.transform_point(v);
                    for k in 0..3 {
                        mn[k] = mn[k].min(w[k]);
                        mx[k] = mx[k].max(w[k]);
                    }
                }
            }
        }
        for c in node.children() {
            walk(c, m.0, blob, prims, tris, mn, mx);
        }
    }
    for s in g.scenes() {
        for n in s.nodes() {
            walk(n, bm_models::math::Mat4::IDENTITY.0, blob, &mut prims, &mut tris, &mut mn, &mut mx);
        }
    }
    (prims, tris, mn, mx)
}

#[test]
fn glb_export_of_a_building_parses_back_with_matching_counts_and_bbox() {
    let Some(sc) = m01() else { return };
    let hac = *sc.graph.find_by_name("M01/M01_LOC_Ins_Hacienda.zip").first().expect("hacienda group");
    let mut opts = ExportOptions::default();
    opts.root = Some(hac);
    opts.max_texture_dim = Some(64);
    let r = export_scene_glb(sc, &opts);
    assert!(r.stats.geometry_nodes > 500 && r.stats.instanced_triangles > 50_000);
    assert!(r.stats.textures > 20 && r.stats.materials > 20);
    let (prims, _tris, mn, mx) = gltf_stats(&r.glb);
    assert_eq!(prims, {
        // every instance of a mesh shares its primitives; the parsed count of *instances* is larger
        // than the stored primitives, so recompute the expected value from the node list
        let g = gltf::Gltf::from_slice(&r.glb).unwrap();
        g.nodes().filter_map(|n| n.mesh()).map(|m| m.primitives().count()).sum::<usize>()
    });
    // instanced triangle count and stored triangle count agree with the file
    let g = gltf::Gltf::from_slice(&r.glb).unwrap();
    let blob = g.blob.as_deref().unwrap();
    let mut inst_tris = 0usize;
    for n in g.nodes() {
        if let Some(m) = n.mesh() {
            for p in m.primitives() {
                inst_tris += p.reader(|_| Some(blob)).read_indices().unwrap().into_u32().count() / 3;
            }
        }
    }
    assert_eq!(inst_tris, r.stats.instanced_triangles);
    let mut stored = 0usize;
    for m in g.meshes() {
        for p in m.primitives() {
            stored += p.reader(|_| Some(blob)).read_indices().unwrap().into_u32().count() / 3;
        }
    }
    assert_eq!(stored, r.stats.unique_triangles);
    // bbox: glTF is mirrored in Z and scaled by 0.01
    let b = r.stats.bbox.unwrap();
    let want_min = [b.min[0] * 0.01, b.min[1] * 0.01, -b.max[2] * 0.01];
    let want_max = [b.max[0] * 0.01, b.max[1] * 0.01, -b.min[2] * 0.01];
    for k in 0..3 {
        // model bboxes come from the PRM header, the glb from the vertices: allow 5% + 5 cm slack
        let tol = 0.05 + 0.05 * (want_max[k] - want_min[k]).abs();
        assert!((mn[k] - want_min[k]).abs() < tol, "min[{k}] {} vs {}", mn[k], want_min[k]);
        assert!((mx[k] - want_max[k]).abs() < tol, "max[{k}] {} vs {}", mx[k], want_max[k]);
    }
    // glb is consistent: the right-handed CCW winding agrees with the normals
    let (mut ok, mut bad) = (0usize, 0usize);
    'outer: for n in g.nodes().filter(|n| n.mesh().is_some()).take(200) {
        for p in n.mesh().unwrap().primitives() {
            let r = p.reader(|_| Some(blob));
            let pos: Vec<[f32; 3]> = r.read_positions().unwrap().collect();
            let nrm: Vec<[f32; 3]> = r.read_normals().unwrap().collect();
            let idx: Vec<u32> = r.read_indices().unwrap().into_u32().collect();
            for t in idx.chunks_exact(3) {
                let p0 = pos[t[0] as usize];
                let d = dot(cross(sub(pos[t[1] as usize], p0), sub(pos[t[2] as usize], p0)), nrm[t[0] as usize]);
                if d > 0.0 { ok += 1 } else if d < 0.0 { bad += 1 }
            }
            if ok + bad > 200_000 {
                break 'outer;
            }
        }
    }
    assert!(ok as f64 / (ok + bad) as f64 > 0.98, "glTF winding {ok}/{bad}");
}

#[test]
fn skinned_actor_exports_with_skin() {
    let Some(sc) = m01() else { return };
    let actor = sc.nodes.iter().find(|n| n.kind == NodeKind::Actor && n.prim_id.is_some() && sc.node_model(n.index).map(|m| m.skeleton.is_some()).unwrap_or(false)).expect("actor");
    let mut opts = ExportOptions::default();
    opts.root = Some(actor.index);
    let r = export_scene_glb(sc, &opts);
    assert_eq!(r.stats.skins, 1);
    let g = gltf::Gltf::from_slice(&r.glb).unwrap();
    assert_eq!(g.skins().count(), 1);
    let skin = g.skins().next().unwrap();
    assert!(skin.joints().count() >= 90);
    let blob = g.blob.as_deref().unwrap();
    let rd = skin.reader(|_| Some(blob));
    assert_eq!(rd.read_inverse_bind_matrices().unwrap().count(), skin.joints().count());
    // meshes carry joints + weights
    let p = g.meshes().next().unwrap().primitives().next().unwrap();
    assert!(p.get(&gltf::Semantic::Joints(0)).is_some() && p.get(&gltf::Semantic::Weights(0)).is_some());
}

#[test]
fn obj_export_matches_triangle_counts() {
    let Some(sc) = m01() else { return };
    let mut opts = ExportOptions::default();
    opts.max_geometry_nodes = Some(60);
    opts.textures = false;
    let o = export_scene_obj(sc, &opts);
    let faces = o.obj.lines().filter(|l| l.starts_with("f ")).count();
    assert_eq!(faces, o.triangles);
    assert!(faces > 100);
    let g = export_scene_glb(sc, &opts);
    assert_eq!(faces, g.stats.instanced_triangles);
    assert!(o.mtl.contains("newmtl"));
}

#[test]
fn glb_roundtrip_import_and_prm_write() {
    let Some(sc) = m01() else { return };
    // a decent static model with several materials
    let (pid, model) = unique_prims(sc)
        .into_iter()
        .filter_map(|p| sc.model(p).map(|m| (p, m)))
        .filter(|(_, m)| m.skeleton.is_none() && m.meshes.len() >= 2 && m.triangle_count() > 400 && m.meshes.iter().all(|x| x.layout == VertexLayout::Standard28))
        .find(|(_, m)| m.materials().len() >= 2)
        .expect("multi material model");
    let opts = ExportOptions { scale: 0.01, lod0_only: false, ..Default::default() };
    let glb = export_model_glb(&model, &sc.materials, &opts);
    let imp = import_glb(&glb.glb, &ImportOptions::default()).expect("import");
    let m2 = &imp.model;
    assert_eq!(m2.triangle_count(), model.triangle_count(), "triangle count survives glb round trip");
    assert_eq!(m2.vertex_count(), model.vertex_count());
    let (a, b) = (model.computed_bbox(), m2.computed_bbox());
    for k in 0..3 {
        assert!((a.min[k] - b.min[k]).abs() < 0.01 && (a.max[k] - b.max[k]).abs() < 0.01, "bbox {a:?} vs {b:?}");
    }
    // materials keep their MAT ids through extras.bm_material
    assert_eq!(m2.materials(), model.materials());
    // vertex data survives within quantisation
    let (v1, v2) = (&model.meshes[0].vertices, &m2.meshes[0].vertices);
    for i in (0..v1.len()).step_by(7) {
        for k in 0..3 {
            assert!((v1.positions[i][k] - v2.positions[i][k]).abs() < 0.01);
        }
        assert!(dot(v1.normals[i], v2.normals[i]) > 0.999);
        assert_eq!(v1.uvs[i], v2.uvs[i]);
    }

    // write into a copy of the real PRM, in place
    let mut prm = sc.prm.clone();
    let before_chunks = prm.chunks.len();
    let rep = write_model(&mut prm, pid, m2).expect("write");
    assert_eq!(rep.triangles, m2.triangle_count());
    assert!(prm.chunks.len() > before_chunks);
    let bytes = prm.to_bytes();
    let prm2 = bm_formats::prm::Prm::parse(&bytes).expect("rewritten PRM parses");
    let back = bm_models::decode::decode_model(&prm2, pid).expect("decode written model");
    assert_eq!(back.triangle_count(), m2.triangle_count());
    assert_eq!(back.vertex_count(), m2.vertex_count());
    assert_eq!(back.materials(), m2.materials());
    for k in 0..3 {
        assert!((back.bbox.min[k] - b.min[k]).abs() < 0.01 && (back.bbox.max[k] - b.max[k]).abs() < 0.01);
    }
    let (v3, v2) = (&back.meshes[0].vertices, &m2.meshes[0].vertices);
    for i in 0..v3.len() {
        assert_eq!(v3.positions[i], v2.positions[i]);
        assert_eq!(v3.uvs[i], v2.uvs[i]);
        assert!(dot(v3.normals[i], v2.normals[i]) > 0.99);
    }
    // all other models are untouched
    for other in unique_prims(sc).into_iter().filter(|&p| p != pid).take(50) {
        let (x, y) = (bm_models::decode::decode_model(&sc.prm, other), bm_models::decode::decode_model(&prm2, other));
        assert_eq!(x.is_ok(), y.is_ok());
        if let (Ok(x), Ok(y)) = (x, y) {
            assert_eq!(x, y);
        }
    }

    // standalone PRM
    let (standalone, id) = build_prm(m2).expect("build");
    let sp = bm_formats::prm::Prm::parse(&standalone.to_bytes()).unwrap();
    let sb = bm_models::decode::decode_model(&sp, id).unwrap();
    assert_eq!(sb.triangle_count(), m2.triangle_count());
}

#[test]
fn skinned_models_survive_a_prm_rewrite() {
    let Some(sc) = m01() else { return };
    let (pid, model) = unique_prims(sc)
        .into_iter()
        .filter_map(|p| sc.model(p).map(|m| (p, m)))
        .find(|(_, m)| m.skeleton.is_some())
        .expect("skinned model");
    let mut prm = sc.prm.clone();
    write_model(&mut prm, pid, &model).expect("write skinned");
    let prm2 = bm_formats::prm::Prm::parse(&prm.to_bytes()).unwrap();
    let back = bm_models::decode::decode_model(&prm2, pid).unwrap();
    assert_eq!(back.triangle_count(), model.triangle_count());
    assert!(back.skeleton.is_some());
    // joints and weights survive (palettes are rebuilt, but resolve to the same bones)
    let (a, b) = (&model.meshes[0].vertices, &back.meshes[0].vertices);
    for i in 0..a.len() {
        assert_eq!(a.joints[i][0], b.joints[i][0]);
        assert!((a.weights[i][0] - b.weights[i][0]).abs() < 1e-6);
    }
}

#[test]
fn every_scene_decodes_without_panics() {
    let Some(vfs) = vfs() else { return };
    let mut bases: Vec<String> = vfs.list("Scenes/**/*.prp").into_iter().map(|e| e.path[..e.path.len() - 4].to_string()).collect();
    bases.sort();
    let (mut ok, mut total) = (0usize, 0usize);
    let mut tris = 0usize;
    for b in bases.iter().take(12) {
        let sc = Scene::load(vfs, b).unwrap_or_else(|e| panic!("{b}: {e}"));
        for p in unique_prims(&sc) {
            total += 1;
            if let Some(m) = sc.model(p) {
                ok += 1;
                tris += m.triangle_count();
            }
        }
    }
    eprintln!("{ok}/{total} models decoded over {} scenes, {tris} triangles", bases.len().min(12));
    assert!(ok as f64 / total as f64 > 0.9);
}

#[test]
fn coordinate_options_do_not_change_triangle_counts() {
    let Some(sc) = m01() else { return };
    let p = unique_prims(sc).into_iter().find_map(|p| sc.model(p).filter(|m| m.triangle_count() > 100 && m.skeleton.is_none())).unwrap();
    let a = export_model_glb(&p, &sc.materials, &ExportOptions::default());
    let mut o = ExportOptions::default();
    o.coords = CoordSystem::Native;
    o.scale = 1.0;
    let b = export_model_glb(&p, &sc.materials, &o);
    assert_eq!(a.stats.unique_triangles, b.stats.unique_triangles);
    let _: &Model = &p;
}

#[test]
fn anm_index_lists_databases_and_clips() {
    let Some(sc) = m01() else { return };
    let idx = bm_models::anim::anm_index(&sc.anm);
    eprintln!("ANM: {} databases, {} clip paths, {} anim objects", idx.databases.len(), idx.clip_paths.len(), idx.anim_objects.len());
    assert!(idx.databases.iter().any(|d| d == "M01_Don"));
    assert!(idx.databases.iter().any(|d| d == "Hitman"));
    assert!(idx.clip_paths.iter().any(|p| p == "/Movement/Run/Forward"));
    assert!(idx.anim_objects.len() > 500);
    assert!(idx.anim_objects.iter().any(|a| a.contains("Base_Pose")));
}

#[test]
fn replacement_model_works_as_loose_file_mod() {
    let Some(sc) = m01() else { return };
    let Some(dir) = game_dir_if_present() else { return };
    // pick a ZSTDOBJ model and replace it with a simple box built from scratch (via glb round trip)
    let node = sc.nodes.iter().find(|n| n.type_id == 0x200002 && n.name.starts_with("Landscape_Rocks")).expect("rock node");
    let pid = node.prim_id.unwrap();
    let orig = sc.model(pid).unwrap();
    // simple replacement: the original geometry but only its first mesh (triangle count must drop)
    let mut repl = (*orig).clone();
    repl.meshes.truncate(1);
    repl.meshes[0].submeshes[0].material = repl.meshes[0].submeshes[0].material.max(1);
    let mut prm = sc.prm.clone();
    write_model(&mut prm, pid, &repl).unwrap();

    let mod_dir = std::env::temp_dir().join(format!("bm_models_test_mod_{}", std::process::id()));
    let scenes = mod_dir.join("Scenes").join("M01");
    std::fs::create_dir_all(&scenes).unwrap();
    std::fs::write(scenes.join("M01_main.PRM"), prm.to_bytes()).unwrap();

    let mut v = Vfs::new(dir).unwrap();
    v.add_mod_dir(&mod_dir).unwrap();
    let modded = Scene::load(&v, "Scenes/M01/M01_main").unwrap();
    let m2 = modded.model(pid).unwrap();
    assert_eq!(m2.meshes.len(), 1);
    assert_eq!(m2.triangle_count(), repl.triangle_count());
    // untouched models are identical
    let other = sc.nodes.iter().filter_map(|n| n.prim_id).find(|&p| p != pid).unwrap();
    assert_eq!(*modded.model(other).unwrap(), *sc.model(other).unwrap());
    let _ = std::fs::remove_dir_all(&mod_dir);
}

// ---------------------------------------------------------------------------------------------------------
// .ANM key data
// ---------------------------------------------------------------------------------------------------------

fn bank() -> Option<&'static bm_models::AnmBank> {
    static B: OnceLock<Option<bm_models::AnmBank>> = OnceLock::new();
    B.get_or_init(|| m01().and_then(|sc| bm_models::AnmBank::parse(&sc.anm.raw))).as_ref()
}

fn player_skeleton(sc: &Scene) -> Option<std::sync::Arc<bm_models::Skeleton>> {
    unique_prims(sc).into_iter().filter_map(|p| sc.model(p)).find_map(|m| m.skeleton.clone().filter(|s| s.bones.len() == 100))
}

fn world_positions(sk: &bm_models::Skeleton, pose: &bm_models::Pose) -> Vec<[f32; 3]> {
    sk.world_matrices(pose).iter().map(|m| m.translation()).collect()
}

#[test]
fn anm_bank_finds_every_clip_and_decodes_it() {
    let Some(b) = bank() else { return };
    assert!(b.clip_count() > 800, "{}", b.clip_count());
    assert!(b.bone_names.iter().any(|n| n == "GROUND") && b.bone_names.len() > 150);
    let mut decoded = 0;
    for i in 0..b.clip_count() {
        let c = b.raw_clip(i).unwrap_or_else(|e| panic!("clip {} ({}): {e}", i, b.clips[i].name));
        assert_eq!(c.frames, b.clips[i].frames as usize);
        for p in &c.params {
            assert!(p.iter().all(|v| v.is_finite()), "non finite parameter in {}", b.clips[i].name);
        }
        for q in c.params.iter().flat_map(|p| (0..10).filter(|k| c.mask[4 * k]).map(move |k| [p[4 * k], p[4 * k + 1], p[4 * k + 2], p[4 * k + 3]])) {
            let l = q.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((l - 1.0).abs() < 0.01, "quaternion length {l} in {}", b.clips[i].name);
        }
        decoded += 1;
    }
    eprintln!("decoded {decoded} clips");
    let walk = b.find("Male_Reg/Male_Reg_Walk#Walk").expect("walk clip");
    assert_eq!((b.clips[walk].frames, b.clips[walk].fps), (29, 25));
    assert!(b.find_suffix("#Base_Pose").is_some());
}

#[test]
fn walk_clip_is_an_anatomically_sane_loop() {
    let (Some(sc), Some(b)) = (m01(), bank()) else { return };
    let sk = player_skeleton(sc).expect("player skeleton");
    let solver = bm_models::rig::RigSolver::new(&sk).expect("humanoid");
    let walk = b.find("Male_Reg/Male_Reg_Walk#Walk").unwrap();
    let anim = b.animation_with(walk, &sk, Some(&solver)).expect("walk animation");
    assert!(anim.looping, "walk must loop");
    assert!((anim.duration - 28.0 / 25.0).abs() < 1e-4);
    let bind = bm_models::Pose::bind(&sk);
    let ix = |n: &str| sk.bone_index(n).unwrap();
    let (lf, rf, head, pel) = (ix("LEFT_FOOT"), ix("RIGHT_FOOT"), ix("HEAD"), ix("PELVIS"));
    let bind_len: Vec<f32> = (0..sk.bones.len())
        .map(|i| match sk.bones[i].parent {
            Some(p) => {
                let a = world_positions(&sk, &bind);
                let d = [a[i][0] - a[p][0], a[i][1] - a[p][1], a[i][2] - a[p][2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            }
            None => 0.0,
        })
        .collect();
    let driven = solver.driven_bones();
    let (mut fz_min, mut fz_max) = (f32::MAX, f32::MIN);
    for k in 0..=28 {
        let t = k as f32 / 25.0;
        let pose = anim.sample(&sk, t);
        let w = world_positions(&sk, &pose);
        // upright figure of human height with feet near the floor
        assert!(w[head][1] > 150.0 && w[head][1] < 190.0, "head height {} at frame {k}", w[head][1]);
        assert!(w[lf][1] > -2.0 && w[rf][1] > -2.0 && w[lf][1] < 40.0, "feet {} {} at frame {k}", w[lf][1], w[rf][1]);
        assert!(w[pel][1] > 80.0 && w[pel][1] < 110.0);
        fz_min = fz_min.min(w[lf][2]);
        fz_max = fz_max.max(w[lf][2]);
        // limb bones keep their length (rotations only; the spine is re-spaced within 2 %)
        for &i in &driven {
            let Some(p) = sk.bones[i].parent else { continue };
            if matches!(sk.bones[i].name.as_str(), "PELVIS" | "SPINE" | "SPINE_1" | "SPINE_2") {
                continue;
            }
            let d = [w[i][0] - w[p][0], w[i][1] - w[p][1], w[i][2] - w[p][2]];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            assert!((l - bind_len[i]).abs() < 0.02 * bind_len[i].max(1.0) + 0.05, "{} length {l} vs {} at frame {k}", sk.bones[i].name, bind_len[i]);
        }
    }
    // the IK reaches its targets: the left ankle sits at the (x, z, y) of parameters 16..18 of every frame
    let raw = b.raw_clip(walk).unwrap();
    for (f, p) in raw.params.iter().enumerate() {
        let w = world_positions(&sk, &anim.sample(&sk, f as f32 / 25.0));
        let want = [p[46], p[48], p[47]];
        for k in 0..3 {
            assert!((w[lf][k] - want[k]).abs() < 0.6, "frame {f} ankle {:?} vs target {want:?}", w[lf]);
        }
    }
    // the feet swing about +-35 cm front to back
    assert!(fz_max - fz_min > 50.0, "stride {}", fz_max - fz_min);
    // continuity between consecutive frames and a closed loop
    let a = world_positions(&sk, &anim.sample(&sk, 0.0));
    let z = world_positions(&sk, &anim.sample(&sk, anim.duration));
    for i in 0..a.len() {
        let d = ((a[i][0] - z[i][0]).powi(2) + (a[i][1] - z[i][1]).powi(2) + (a[i][2] - z[i][2]).powi(2)).sqrt();
        assert!(d < 3.0, "{} jumps {d} at the loop point", sk.bones[i].name);
    }
    let mut prev = world_positions(&sk, &anim.sample(&sk, 0.0));
    for k in 1..=28 {
        let cur = world_positions(&sk, &anim.sample(&sk, k as f32 / 25.0));
        for i in 0..cur.len() {
            let d = ((cur[i][0] - prev[i][0]).powi(2) + (cur[i][1] - prev[i][1]).powi(2) + (cur[i][2] - prev[i][2]).powi(2)).sqrt();
            assert!(d < 25.0, "{} moves {d} cm in one frame ({k})", sk.bones[i].name);
        }
        prev = cur;
    }
    // walking forward: the root travels about 1.3 m/s along -Z
    let speed = anim.root_speed();
    assert!(speed > 100.0 && speed < 200.0, "root speed {speed}");
}

#[test]
fn base_pose_is_close_to_the_bind_pose() {
    let (Some(sc), Some(b)) = (m01(), bank()) else { return };
    let sk = player_skeleton(sc).unwrap();
    let i = b.find("Male_Reg/Male_Reg#Base_Pose").unwrap();
    let anim = b.animation(i, &sk).unwrap();
    let pose = anim.sample(&sk, 0.0);
    let bind = bm_models::Pose::bind(&sk);
    let ix = |n: &str| sk.bone_index(n).unwrap();
    for n in ["PELVIS", "SPINE", "NECK", "HEAD", "LEFT_HAND", "RIGHT_HAND", "LEFT_FINGER_1", "LEFT_FOOT"] {
        let (a, bq) = (pose.bones[ix(n)].rotation, bind.bones[ix(n)].rotation);
        let d: f32 = (0..4).map(|k| (a[k] - bq[k]).abs()).sum::<f32>().min((0..4).map(|k| (a[k] + bq[k]).abs()).sum());
        assert!(d < 0.35, "{n}: {a:?} vs bind {bq:?}");
    }
    // pelvis height of the pose matches the model's height (97.5 cm)
    let w = world_positions(&sk, &pose);
    assert!((w[ix("PELVIS")][1] - 97.5).abs() < 6.0, "{}", w[ix("PELVIS")][1]);
}

#[test]
fn every_main_clip_builds_an_animation_for_the_player_skeleton() {
    let (Some(sc), Some(b)) = (m01(), bank()) else { return };
    let sk = player_skeleton(sc).unwrap();
    let solver = bm_models::rig::RigSolver::new(&sk).unwrap();
    let mut built = 0;
    for i in 0..b.clip_count() {
        if let Some(a) = b.animation_with(i, &sk, Some(&solver)) {
            let p = a.sample(&sk, a.duration * 0.5);
            let w = world_positions(&sk, &p);
            assert!(w.iter().all(|v| v.iter().all(|x| x.is_finite())), "{}", b.clips[i].name);
            built += 1;
        }
    }
    eprintln!("{built}/{} clips build", b.clip_count());
    assert!(built > 800);
}

// ---------------------------------------------------------------------------------------------------------
// material references and per-actor outfits
// ---------------------------------------------------------------------------------------------------------

#[test]
fn material_references_are_instance_id_plus_one() {
    // ZSHADOWMESHOBJ meshes must resolve to shadow / helper materials. Without the +1 bias (the raw `w4 >> 16`) none of the
    // shadow meshes of M03 do.
    let Some(dir) = game_dir_if_present() else { return };
    let vfs = Vfs::new(dir).unwrap();
    for s in ["M01", "M03"] {
        let sc = Scene::load(&vfs, &format!("Scenes/{s}/{s}_main")).unwrap();
        let (mut total, mut ok) = (0, 0);
        for n in sc.nodes.iter().filter(|n| n.type_id == 0x200110) {
            let Some(m) = sc.node_model(n.index) else { continue };
            for me in &m.meshes {
                total += 1;
                ok += sc.materials.desc(me.material()).is_some_and(|d| d.is_shadow_only() || d.name.starts_with("_Glacier")) as usize;
            }
        }
        assert!(total > 20 && ok == total, "{s}: {ok}/{total} shadow meshes use shadow materials");
    }
}

fn outfit_materials(sc: &Scene, node: usize) -> Vec<String> {
    let m = sc.node_model(node).expect("model");
    let o = m.resolve_outfit(sc.actor_outfit(node).unwrap_or(1));
    m.meshes
        .iter()
        .filter(|x| x.is_lod0() && x.is_skinned() && x.in_outfit(o))
        .map(|x| sc.materials.desc(x.material()).map(|d| d.name.clone()).unwrap_or_default())
        .collect()
}

#[test]
fn every_actor_wears_one_complete_outfit_with_its_own_head() {
    let Some(sc) = m01() else { return };
    let (mut actors, mut with_head, mut with_body) = (0, 0, 0);
    for n in sc.nodes.iter().filter(|n| n.type_id == 0x2003e9) {
        let Some(m) = sc.node_model(n.index) else { continue };
        let o = m.resolve_outfit(sc.actor_outfit(n.index).unwrap_or(1));
        let mats = outfit_materials(sc, n.index);
        actors += 1;
        // the face is a `*_HEAD` material (the women use the `*_BODY` skin atlas together with a hair material)
        with_head += mats.iter().any(|x| x.to_lowercase().contains("head") || x.contains("Female_01_BODY")) as usize;
        // the drawn meshes cover the figure from the feet to the head
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for me in m.meshes.iter().filter(|x| x.is_lod0() && x.is_skinned() && x.in_outfit(o)) {
            let b = me.bbox();
            lo = lo.min(b.min[1]);
            hi = hi.max(b.max[1]);
        }
        with_body += (lo < 15.0 && hi > 160.0) as usize;
        // exactly one head material per outfit
        let heads = mats.iter().filter(|x| x.to_lowercase().ends_with("_head")).collect::<std::collections::BTreeSet<_>>();
        assert!(heads.len() <= 1, "{}: several heads {heads:?}", sc.path(n.index));
    }
    assert!(actors > 60);
    assert!(with_head * 100 >= actors * 90, "{with_head}/{actors} actors have a head material");
    assert!(with_body * 100 >= actors * 90, "{with_body}/{actors} actors cover the body");
    // the Don and the film star share one model but wear their own clothes and faces
    let find = |s: &str| sc.nodes.iter().find(|n| n.type_id == 0x2003e9 && sc.path(n.index).contains(s)).unwrap().index;
    let don = outfit_materials(sc, find("DonChilean/Ground"));
    let star = outfit_materials(sc, find("MovieStar/Ground"));
    assert!(don.iter().any(|m| m.ends_with("DonChilean_01_HEAD")) && !don.iter().any(|m| m.contains("MovieStar_01")), "{don:?}");
    assert!(star.iter().any(|m| m.contains("MovieStar")) && !star.iter().any(|m| m.contains("DonChilean_01_HEAD")), "{star:?}");
}

#[test]
fn anm_banks_of_every_level_decode_and_animate_their_actors() {
    let Some(vfs) = vfs() else { return };
    let mut bases: Vec<String> = vfs.list("Scenes/**/*.prp").into_iter().map(|e| e.path[..e.path.len() - 4].to_string()).collect();
    bases.sort();
    let (mut banks, mut clips, mut actors, mut animated) = (0, 0, 0, 0);
    for b in &bases {
        let sc = Scene::load(vfs, b).unwrap_or_else(|e| panic!("{b}: {e}"));
        let Some(bank) = sc.anim_bank() else { continue };
        banks += 1;
        for i in 0..bank.clip_count() {
            let c = bank.raw_clip(i).unwrap_or_else(|e| panic!("{b}: clip {}: {e}", bank.clips[i].name));
            assert!(c.params.iter().all(|p| p.iter().all(|v| v.is_finite())), "{b}: {}", bank.clips[i].name);
            clips += 1;
        }
        // every actor skeleton can play the walk and idle clips of its set
        let mut seen = std::collections::BTreeSet::new();
        for n in sc.nodes.iter().filter(|n| n.type_id == 0x2003e9) {
            let Some(p) = n.prim_id else { continue };
            if !seen.insert(p) {
                continue;
            }
            let Some(sk) = sc.model(p).and_then(|m| m.skeleton.clone()) else { continue };
            actors += 1;
            let solver = bm_models::rig::RigSolver::new(&sk);
            let walk = bank.find("Male_Reg/Male_Reg_Walk#Walk").and_then(|i| bank.animation_with(i, &sk, solver.as_ref()));
            if let Some(a) = walk {
                let pose = a.sample(&sk, 0.4);
                let w = sk.world_matrices(&pose);
                let head = sk.bone_index("HEAD").map(|h| w[h].translation()[1]).unwrap_or(0.0);
                assert!(head > 120.0 && head < 200.0, "{b}: prim {p}: head height {head}");
                animated += 1;
            }
        }
    }
    eprintln!("{banks} banks, {clips} clips decoded, {animated}/{actors} actor skeletons animated");
    assert!(banks >= 12 && clips > 8000);
    assert!(animated * 10 >= actors * 9, "{animated}/{actors}");
}

// ---- lighting (docs/formats.md, "Lights and environments") --------------------------------------------------------------

fn scene_of(name: &str) -> Option<Scene> {
    vfs().and_then(|v| Scene::load(v, name).ok())
}

#[test]
fn light_records_are_self_consistent_in_every_level() {
    let Some(v) = vfs() else { return };
    let mut checked = 0usize;
    for name in ["M00/M00_main", "M01/M01_main", "M02/M02_main", "M03/M03_main", "M05/M05_main", "M06/M06_main", "M11/M11_main", "M13/M13_main", "hideout/hideout_main"] {
        let Ok(sc) = Scene::load(v, &format!("Scenes/{name}")) else { continue };
        let l = sc.lighting();
        assert!(!l.lights.is_empty(), "{name}: no lights decoded");
        for x in &l.lights {
            assert!(x.range_end > 0.0 && x.range_start >= 0.0 && x.range_end >= x.range_start, "{name} {}: range {}..{}", x.name, x.range_start, x.range_end);
            assert!(x.intensity > -5.0 && x.intensity < 10.0, "{name} {}: intensity {}", x.name, x.intensity);
            let n = (x.direction[0].powi(2) + x.direction[1].powi(2) + x.direction[2].powi(2)).sqrt();
            assert!((n - 1.0).abs() < 1e-3);
            if let Some(c) = x.cone {
                // the stored cosines and the inverse cosine range must agree with the stored angles
                assert!((c.cos_outer - c.outer.cos()).abs() < 1e-3 && (c.cos_inner - c.inner.cos()).abs() < 1e-3, "{name} {}: cone {c:?}", x.name);
                assert!(c.inner <= c.outer + 1e-4);
                assert!((c.inv_cos_range * (c.cos_inner - c.cos_outer) - 1.0).abs() < 0.02, "{name} {}: {c:?}", x.name);
            }
            checked += 1;
        }
        for e in &l.environments {
            assert!(e.intensity >= 0.0 && e.intensity < 10.0, "{name} {}: {}", e.name, e.intensity);
        }
    }
    assert!(checked > 500, "only {checked} lights checked");
}

#[test]
fn sun_environment_agrees_with_the_backdrop_sun_sprite() {
    for (name, min_dot) in [("Scenes/M01/M01_main", 0.99), ("Scenes/M13/M13_main", 0.999), ("Scenes/M05/M05_main", 0.99)] {
        let Some(sc) = scene_of(name) else { return };
        let l = sc.lighting();
        let sprite = l.sun_sprite_dir.expect("backdrop sun sprite");
        let sun = l.environments.iter().filter(|e| e.kind == bm_models::lighting::EnvKind::Sun && e.shadow_mesh_count > 0).next().expect("sun environment with static shadow meshes");
        let to_sun = [-sun.direction[0], -sun.direction[1], -sun.direction[2]];
        let d = dot(sprite, to_sun);
        assert!(d > min_dot, "{name}: sun direction {to_sun:?} vs sprite {sprite:?} (dot {d})");
        assert!(to_sun[1] > 0.3, "{name}: the sun is above the horizon");
    }
}

#[test]
fn zones_select_the_right_environment_and_fog_is_scaled_by_far() {
    let Some(m01) = m01() else { return };
    let fog = &m01.lighting().fog;
    assert_eq!(fog.camera_far, 50_000.0);
    assert!((fog.start - 10_000.0).abs() < 1.0 && (fog.full - 150_000.0).abs() < 1.0, "{fog:?}");
    // the M01 start position is outdoors: sun environment + its character twin
    let sel = m01.environments_at([-1108.0, -100.0, 2488.0]);
    let l = m01.lighting();
    let env = &l.environments[sel.level_env.expect("level env")];
    assert_eq!(env.name, "Ambient_Sun_01");
    assert!(env.directional().iter().any(|&c| c > 0.9));
    assert_eq!(l.environments[sel.character_env.expect("character env")].name, "Ambient_env_character_01");

    let Some(m06) = scene_of("Scenes/M06/M06_main") else { return };
    // inside the hotel lobby: an ambient-only environment, no sun
    let sel = m06.environments_at([-1784.0, 100.0, -9821.0]);
    let l = m06.lighting();
    assert!(sel.zone.is_some());
    let env = &l.environments[sel.level_env.expect("lobby env")];
    assert_eq!(env.kind, bm_models::lighting::EnvKind::Ambient, "{env:?}");
    assert_eq!(env.directional(), [0.0; 3]);
}

/// Facial pose banks: `Neutral` is empty, `A_Mouth_Open` moves the jaw, `D_Closed` only lip bones, `B_Blink` the lids.
#[test]
fn facial_pose_banks_match_the_pose_names() {
    let Some(scene) = m01() else { return };
    let names = &scene.anim_bank().expect("anm bank").facial_poses;
    let idx = |n: &str| names.iter().position(|x| x == n).unwrap();
    let mut checked = 0;
    for prim in unique_prims(scene) {
        let Some(m) = scene.model(prim) else { continue };
        let Some(sk) = &m.skeleton else { continue };
        if sk.face.is_empty() {
            continue;
        }
        let bone = |p: &str| sk.face[idx(p)].deltas.iter().map(|d| sk.bones[d.bone].name.clone()).collect::<Vec<_>>();
        assert!(sk.face[idx("Neutral")].deltas.is_empty());
        assert!(bone("A_Mouth_Open").iter().any(|b| b == "FA_JAW"));
        assert!(bone("B_Blink").iter().any(|b| b.contains("EYELID")));
        assert!(bone("D_Closed").iter().all(|b| b.contains("LIP") || b.contains("JAW") || b.contains("MOUTH")));
        checked += 1;
    }
    assert!(checked >= 3, "only {checked} skeletons with a face bank");
}
