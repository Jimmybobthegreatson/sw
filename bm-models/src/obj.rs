//! Wavefront OBJ/MTL export (verification / quick look). Scene exports bake the world transforms
//! into the vertices.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use crate::gltf_export::{select_geometry_nodes, CoordSystem, ExportOptions};
use crate::material::MaterialLibrary;
use crate::math::Mat4;
use crate::mesh::{Mesh, Model};
use crate::scene::Scene;

/// Result of an OBJ export: the `.obj` text, the `.mtl` text and `(file name, PNG bytes)` textures.
pub struct ObjExport {
    pub obj: String,
    pub mtl: String,
    pub textures: Vec<(String, Vec<u8>)>,
    pub triangles: usize,
}

struct W<'a> {
    opts: &'a ExportOptions,
    mats: &'a MaterialLibrary,
    obj: String,
    mtl: String,
    textures: BTreeMap<u32, Vec<u8>>,
    mtl_done: HashMap<u32, String>,
    v_base: usize,
    triangles: usize,
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

impl<'a> W<'a> {
    fn new(opts: &'a ExportOptions, mats: &'a MaterialLibrary) -> W<'a> {
        W {
            opts,
            mats,
            obj: String::from("# exported by bm-models\nmtllib model.mtl\n"),
            mtl: String::new(),
            textures: BTreeMap::new(),
            mtl_done: HashMap::new(),
            v_base: 1,
            triangles: 0,
        }
    }

    fn material(&mut self, id: u32) -> String {
        if let Some(n) = self.mtl_done.get(&id) {
            return n.clone();
        }
        let desc = self.mats.desc(id);
        let name = format!("m{}_{}", id, sanitize(desc.as_ref().map(|d| d.name.as_str()).unwrap_or("none")));
        let _ = writeln!(self.mtl, "newmtl {name}");
        if let Some(d) = &desc {
            let c = d.diffuse_color();
            let _ = writeln!(self.mtl, "Kd {:.4} {:.4} {:.4}", c[0], c[1], c[2]);
            if d.render_state.blend.is_alpha_blended() {
                let _ = writeln!(self.mtl, "d {:.4}", (c[3] * d.render_state.opacity).clamp(0.0, 1.0));
            }
            if self.opts.textures {
                if let Some(t) = d.diffuse_texture() {
                    if let Some(img) = self.mats.image(t) {
                        let png = png_bytes(&img.rgba, img.width, img.height);
                        self.textures.entry(t).or_insert(png);
                        let _ = writeln!(self.mtl, "map_Kd tex{t}.png");
                    }
                }
            }
        }
        let _ = writeln!(self.mtl);
        self.mtl_done.insert(id, name.clone());
        name
    }

    fn mesh(&mut self, mesh: &Mesh, m: &Mat4) {
        if !self.opts.wants_mesh(mesh) {
            return;
        }
        let c = self.opts.coords;
        let sc = self.opts.scale;
        let fix = |p: [f32; 3]| if c == CoordSystem::Gltf { [p[0], p[1], -p[2]] } else { p };
        let v = &mesh.vertices;
        for p in &v.positions {
            let w = fix(m.transform_point(*p));
            let _ = writeln!(self.obj, "v {:.5} {:.5} {:.5}", w[0] * sc, w[1] * sc, w[2] * sc);
        }
        for uv in &v.uvs {
            let _ = writeln!(self.obj, "vt {:.5} {:.5}", uv[0], 1.0 - uv[1]);
        }
        for n in &v.normals {
            let w = fix(m.transform_vector(*n));
            let _ = writeln!(self.obj, "vn {:.5} {:.5} {:.5}", w[0], w[1], w[2]);
        }
        let (has_uv, has_n) = (!v.uvs.is_empty(), !v.normals.is_empty());
        // a mirrored node matrix flips the winding
        let flip = m.det3() < 0.0;
        for sm in &mesh.submeshes {
            if sm.index_count == 0 {
                continue;
            }
            let mat = self.material(sm.material);
            let _ = writeln!(self.obj, "usemtl {mat}");
            for t in mesh.submesh_indices(sm).chunks_exact(3) {
                let mut idx = [t[0] as usize + self.v_base, t[1] as usize + self.v_base, t[2] as usize + self.v_base];
                if flip {
                    idx.swap(1, 2);
                }
                let f = |i: usize| match (has_uv, has_n) {
                    (true, true) => format!("{i}/{i}/{i}"),
                    (true, false) => format!("{i}/{i}"),
                    (false, true) => format!("{i}//{i}"),
                    _ => format!("{i}"),
                };
                let _ = writeln!(self.obj, "f {} {} {}", f(idx[0]), f(idx[1]), f(idx[2]));
                self.triangles += 1;
            }
        }
        self.v_base += v.len();
    }

    fn finish(self) -> ObjExport {
        ObjExport {
            obj: self.obj,
            mtl: self.mtl,
            textures: self.textures.into_iter().map(|(id, b)| (format!("tex{id}.png"), b)).collect(),
            triangles: self.triangles,
        }
    }
}

fn png_bytes(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(rgba).expect("png data");
    }
    out
}

/// OBJ of one model in model space (scaled by `opts.scale`).
pub fn export_model_obj(model: &Model, materials: &MaterialLibrary, opts: &ExportOptions) -> ObjExport {
    let mut w = W::new(opts, materials);
    let _ = writeln!(w.obj, "o model_{}", model.prim_id);
    for m in &model.meshes {
        w.mesh(m, &Mat4::IDENTITY);
    }
    w.finish()
}

/// OBJ of a scene with baked world transforms (use `opts.root` / `opts.max_geometry_nodes` for large scenes).
pub fn export_scene_obj(scene: &Scene, opts: &ExportOptions) -> ObjExport {
    let mut w = W::new(opts, &scene.materials);
    for g in select_geometry_nodes(scene, opts) {
        let Some(model) = scene.node_model(g) else { continue };
        let _ = writeln!(w.obj, "o {}", sanitize(&scene.nodes[g].name));
        let world = scene.nodes[g].world;
        for m in &model.meshes {
            w.mesh(m, &world);
        }
    }
    w.finish()
}
