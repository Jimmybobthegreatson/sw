//! glTF 2.0 (`.glb`) export of models and whole scenes (hierarchy, materials, textures, skins).
//!
//! Glacier space is left-handed Y-up in centimetres with clockwise front faces; glTF is
//! right-handed Y-up in metres with counter-clockwise front faces. [`CoordSystem::Gltf`] converts
//! by mirroring Z (`z' = -z`; positions, normals, tangents (`w` flipped) and every node matrix
//! `C * M * C`). A Z mirror turns the stored clockwise winding into counter-clockwise, so
//! **indices are written unchanged**. A uniform scale node (`scale`, default `0.01`) sits at the
//! root of the exported scene.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Value};

use crate::material::{BlendMode, CullMode, Image, MaterialLibrary, TileMode};
use crate::math::{Aabb, Mat4};
use crate::mesh::{Mesh, Model, VertexLayout};
use crate::scene::{NodeKind, Scene};
use crate::skeleton::Skeleton;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordSystem {
    /// Convert to glTF conventions (mirror Z, keep Y up).
    Gltf,
    /// Keep native Glacier coordinates (viewers will show the mesh mirrored / inside-out).
    Native,
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// Node kinds to include when exporting a scene (default: `Static`, `Scatter`, `Actor`).
    pub kinds: Vec<NodeKind>,
    pub coords: CoordSystem,
    /// Scale applied by a root node (cm -> m = 0.01). Use 1.0 to keep centimetres.
    pub scale: f32,
    /// Embed (decoded, PNG encoded) diffuse textures.
    pub textures: bool,
    /// Downscale textures (box filter) so that neither side exceeds this many pixels.
    pub max_texture_dim: Option<u32>,
    /// Write COLOR_0 (baked vertex colors; usually darkens the diffuse color a lot).
    pub vertex_colors: bool,
    /// Include `PosColor10` meshes (shadow/collision style, untextured).
    pub include_pos_only: bool,
    /// Include shadow-only materials (`SHADOW` / `STATICSHADOW` blend modes).
    pub include_shadow_materials: bool,
    /// Only export meshes of the most detailed LOD ([`Mesh::is_lod0`]).
    pub lod0_only: bool,
    /// For skinned meshes: only the outfit with this value ([`Mesh::in_outfit`]; actors use 1, 5, 9, 13); `None` = all.
    pub actor_variant: Option<u8>,
    /// Write skins for skinned models (bind pose).
    pub skins: bool,
    /// Only the subtree below this scene node (plus its ancestors' transforms).
    pub root: Option<usize>,
    /// Optional cap on exported geometry nodes (testing / quick previews).
    pub max_geometry_nodes: Option<usize>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            kinds: vec![NodeKind::Static, NodeKind::Scatter, NodeKind::Actor],
            lod0_only: true,
            actor_variant: Some(1),
            coords: CoordSystem::Gltf,
            scale: 0.01,
            textures: true,
            max_texture_dim: None,
            vertex_colors: false,
            include_pos_only: false,
            include_shadow_materials: false,
            skins: true,
            root: None,
            max_geometry_nodes: None,
        }
    }
}

impl ExportOptions {
    /// Whether a mesh passes the LOD / variant / layout filters of these options.
    pub fn wants_mesh(&self, m: &Mesh) -> bool {
        if m.triangle_count() == 0 {
            return false;
        }
        if self.lod0_only && !m.is_lod0() {
            return false;
        }
        if let Some(v) = self.actor_variant {
            if m.is_skinned() && !m.in_outfit(v) {
                return false;
            }
        }
        if m.layout == VertexLayout::PosColor10 && !self.include_pos_only {
            return false;
        }
        true
    }
}

/// What an export produced (for tests / logging).
#[derive(Clone, Debug, Default)]
pub struct ExportStats {
    pub nodes: usize,
    pub geometry_nodes: usize,
    pub meshes: usize,
    pub primitives: usize,
    /// Triangles drawn when every instance is counted.
    pub instanced_triangles: usize,
    /// Triangles stored in the file (each distinct model once).
    pub unique_triangles: usize,
    pub materials: usize,
    pub textures: usize,
    pub skins: usize,
    pub bytes: usize,
    /// World space bounding box (model space for model exports) of the exported geometry, in
    /// native Glacier units before the coordinate conversion.
    pub bbox: Option<Aabb>,
}

/// An exported `.glb` plus its statistics.
pub struct GlbExport {
    pub glb: Vec<u8>,
    pub stats: ExportStats,
}

const SGN: [f32; 4] = [1.0, 1.0, -1.0, 1.0];

fn conv_mat(m: &Mat4, c: CoordSystem) -> Mat4 {
    if c == CoordSystem::Native {
        return *m;
    }
    let mut r = m.0;
    for (col, rc) in r.iter_mut().enumerate() {
        for (row, v) in rc.iter_mut().enumerate() {
            *v *= SGN[col] * SGN[row];
        }
    }
    Mat4(r)
}

fn conv_pos(p: [f32; 3], c: CoordSystem) -> [f32; 3] {
    if c == CoordSystem::Gltf { [p[0], p[1], -p[2]] } else { p }
}

struct Builder<'a> {
    opts: &'a ExportOptions,
    materials: &'a MaterialLibrary,
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    meshes: Vec<Value>,
    nodes: Vec<Value>,
    skins: Vec<Value>,
    gl_materials: Vec<Value>,
    textures: Vec<Value>,
    images: Vec<Value>,
    samplers: Vec<Value>,
    mat_map: HashMap<u32, usize>,
    tex_map: HashMap<(u32, u32, u32), usize>,
    img_map: HashMap<u32, usize>,
    sampler_map: HashMap<(u32, u32), usize>,
    model_mesh: HashMap<u32, Option<usize>>,
    stats: ExportStats,
}

impl<'a> Builder<'a> {
    fn new(opts: &'a ExportOptions, materials: &'a MaterialLibrary) -> Builder<'a> {
        Builder {
            opts,
            materials,
            bin: Vec::new(),
            views: vec![],
            accessors: vec![],
            meshes: vec![],
            nodes: vec![],
            skins: vec![],
            gl_materials: vec![],
            textures: vec![],
            images: vec![],
            samplers: vec![],
            mat_map: HashMap::new(),
            tex_map: HashMap::new(),
            img_map: HashMap::new(),
            sampler_map: HashMap::new(),
            model_mesh: HashMap::new(),
            stats: ExportStats::default(),
        }
    }

    fn add_view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let off = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let mut v = json!({"buffer": 0, "byteOffset": off, "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.views.push(v);
        self.views.len() - 1
    }

    #[allow(clippy::too_many_arguments)]
    fn add_accessor(
        &mut self,
        bytes: &[u8],
        target: Option<u32>,
        comp: u32,
        count: usize,
        ty: &str,
        normalized: bool,
        minmax: Option<(Vec<f32>, Vec<f32>)>,
    ) -> usize {
        let view = self.add_view(bytes, target);
        let mut a = json!({"bufferView": view, "componentType": comp, "count": count, "type": ty});
        if normalized {
            a["normalized"] = json!(true);
        }
        if let Some((mn, mx)) = minmax {
            a["min"] = json!(mn);
            a["max"] = json!(mx);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn sampler(&mut self, u: TileMode, v: TileMode) -> usize {
        let code = |t: TileMode| match t {
            TileMode::Tiled => 10497u32,
            TileMode::Mirrored => 33648,
            TileMode::Clamp => 33071,
        };
        let key = (code(u), code(v));
        if let Some(&i) = self.sampler_map.get(&key) {
            return i;
        }
        self.samplers.push(json!({"magFilter": 9729, "minFilter": 9987, "wrapS": key.0, "wrapT": key.1}));
        self.sampler_map.insert(key, self.samplers.len() - 1);
        self.samplers.len() - 1
    }

    fn image(&mut self, tex_id: u32) -> Option<usize> {
        if let Some(&i) = self.img_map.get(&tex_id) {
            return Some(i);
        }
        let img = self.materials.image(tex_id)?;
        let png = encode_png(&img, self.opts.max_texture_dim);
        let view = self.add_view(&png, None);
        self.images.push(json!({"bufferView": view, "mimeType": "image/png", "name": format!("tex{}_{}", tex_id, img.name)}));
        self.img_map.insert(tex_id, self.images.len() - 1);
        self.stats.textures += 1;
        Some(self.images.len() - 1)
    }

    fn texture(&mut self, tex_id: u32, u: TileMode, v: TileMode) -> Option<usize> {
        let key = (tex_id, if u == TileMode::Tiled { 0 } else { 1 + u as u32 }, if v == TileMode::Tiled { 0 } else { 1 + v as u32 });
        if let Some(&i) = self.tex_map.get(&key) {
            return Some(i);
        }
        let img = self.image(tex_id)?;
        let sampler = self.sampler(u, v);
        self.textures.push(json!({"source": img, "sampler": sampler}));
        self.tex_map.insert(key, self.textures.len() - 1);
        Some(self.textures.len() - 1)
    }

    /// glTF material index for a MAT id (`None` -> default material).
    fn material(&mut self, id: u32) -> Option<usize> {
        if let Some(&i) = self.mat_map.get(&id) {
            return Some(i);
        }
        let desc = self.materials.desc(id)?;
        let rs = &desc.render_state;
        let mut base = desc.diffuse_color();
        base[3] *= rs.opacity;
        let mut pbr = json!({"metallicFactor": 0.0, "roughnessFactor": 0.9});
        let mut alpha_mode = "OPAQUE";
        if self.opts.textures {
            if let Some(b) = desc.textures.iter().find(|t| t.enabled && t.slot == "mapDiffuse") {
                if let Some(t) = self.texture(b.texture_id, b.tile_u, b.tile_v) {
                    pbr["baseColorTexture"] = json!({"index": t});
                    let has_alpha = self.materials.image(b.texture_id).map(|i| i.has_alpha()).unwrap_or(false);
                    if rs.alpha_test {
                        alpha_mode = "MASK";
                    } else if has_alpha && rs.blend.is_alpha_blended() {
                        alpha_mode = "BLEND";
                    }
                }
            }
        }
        if rs.blend.is_alpha_blended() && alpha_mode == "OPAQUE" && base[3] < 0.999 {
            alpha_mode = "BLEND";
        }
        pbr["baseColorFactor"] = json!(base);
        let mut m = json!({
            "name": if desc.name.is_empty() { format!("mat{id}") } else { desc.name.clone() },
            "pbrMetallicRoughness": pbr,
            "alphaMode": alpha_mode,
            "doubleSided": rs.cull == CullMode::TwoSided || alpha_mode != "OPAQUE",
            "extras": {
                "bm_material": id,
                "bm_blend": format!("{:?}", rs.blend),
                "bm_shader": desc.shader.clone().unwrap_or_default(),
                "bm_alpha_ref": rs.alpha_ref,
            },
        });
        if alpha_mode == "MASK" {
            m["alphaCutoff"] = json!((rs.alpha_ref as f32 / 255.0).clamp(0.01, 0.99));
        }
        if let Some(t) = desc.illumination_texture() {
            let _ = t; // self-illumination maps are intentionally not exported (needs an emissive map)
        }
        self.gl_materials.push(m);
        self.mat_map.insert(id, self.gl_materials.len() - 1);
        Some(self.gl_materials.len() - 1)
    }

    fn mesh_wanted(&self, m: &Mesh) -> bool {
        self.opts.wants_mesh(m)
    }

    fn submesh_wanted(&self, mat: u32) -> bool {
        if self.opts.include_shadow_materials {
            return true;
        }
        match self.materials.desc(mat) {
            Some(d) => !matches!(d.render_state.blend, BlendMode::Shadow | BlendMode::StaticShadow),
            None => true,
        }
    }

    /// glTF mesh index for a model (deduplicated), `None` when nothing is drawable.
    fn model_mesh(&mut self, model: &Model, with_skin: bool) -> Option<usize> {
        if let Some(&r) = self.model_mesh.get(&model.prim_id) {
            return r;
        }
        let c = self.opts.coords;
        let mut prims = Vec::new();
        for (mi, mesh) in model.meshes.iter().enumerate() {
            if !self.mesh_wanted(mesh) {
                continue;
            }
            let v = &mesh.vertices;
            let n = v.len();
            let pos: Vec<f32> = v.positions.iter().flat_map(|&p| conv_pos(p, c)).collect();
            let bb = v.bbox();
            let (mn, mx) = {
                let a = conv_pos(bb.min, c);
                let b = conv_pos(bb.max, c);
                (vec![a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])], vec![a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])])
            };
            let mut attrs = serde_json::Map::new();
            let a = self.add_accessor(&f32_bytes(&pos), Some(34962), 5126, n, "VEC3", false, Some((mn, mx)));
            attrs.insert("POSITION".into(), json!(a));
            if !v.normals.is_empty() {
                let d: Vec<f32> = v.normals.iter().flat_map(|&p| conv_pos(p, c)).collect();
                let a = self.add_accessor(&f32_bytes(&d), Some(34962), 5126, n, "VEC3", false, None);
                attrs.insert("NORMAL".into(), json!(a));
            }
            if !v.tangents.is_empty() {
                let d: Vec<f32> = v
                    .tangents
                    .iter()
                    .flat_map(|t| {
                        let p = conv_pos([t[0], t[1], t[2]], c);
                        [p[0], p[1], p[2], if c == CoordSystem::Gltf { -t[3] } else { t[3] }]
                    })
                    .collect();
                let a = self.add_accessor(&f32_bytes(&d), Some(34962), 5126, n, "VEC4", false, None);
                attrs.insert("TANGENT".into(), json!(a));
            }
            if !v.uvs.is_empty() {
                let d: Vec<f32> = v.uvs.iter().flatten().copied().collect();
                let a = self.add_accessor(&f32_bytes(&d), Some(34962), 5126, n, "VEC2", false, None);
                attrs.insert("TEXCOORD_0".into(), json!(a));
            }
            if self.opts.vertex_colors && !v.colors.is_empty() {
                let d: Vec<u8> = v.colors.iter().flatten().copied().collect();
                let a = self.add_accessor(&d, Some(34962), 5121, n, "VEC4", true, None);
                attrs.insert("COLOR_0".into(), json!(a));
            }
            if with_skin && v.is_skinned() {
                let j: Vec<u16> = v.joints.iter().flatten().copied().collect();
                let a = self.add_accessor(&u16_bytes(&j), Some(34962), 5123, n, "VEC4", false, None);
                attrs.insert("JOINTS_0".into(), json!(a));
                let w: Vec<f32> = v.weights.iter().flatten().copied().collect();
                let a = self.add_accessor(&f32_bytes(&w), Some(34962), 5126, n, "VEC4", false, None);
                attrs.insert("WEIGHTS_0".into(), json!(a));
            }
            for sm in &mesh.submeshes {
                if sm.index_count == 0 || !self.submesh_wanted(sm.material) {
                    continue;
                }
                let idx = mesh.submesh_indices(sm);
                let ia = if n <= 65535 {
                    let d: Vec<u16> = idx.iter().map(|&i| i as u16).collect();
                    self.add_accessor(&u16_bytes(&d), Some(34963), 5123, d.len(), "SCALAR", false, None)
                } else {
                    self.add_accessor(&u32_bytes(idx), Some(34963), 5125, idx.len(), "SCALAR", false, None)
                };
                let mut prim = json!({"attributes": Value::Object(attrs.clone()), "indices": ia, "mode": 4});
                if let Some(m) = self.material(sm.material) {
                    prim["material"] = json!(m);
                }
                prim["extras"] = json!({"bm_object_chunk": mesh.source_chunk, "bm_variant": mesh.variant, "bm_flags": mesh.flags, "bm_mesh": mi});
                prims.push(prim);
                self.stats.primitives += 1;
                self.stats.unique_triangles += idx.len() / 3;
            }
        }
        let r = if prims.is_empty() {
            None
        } else {
            self.meshes.push(json!({"name": format!("model_{}", model.prim_id), "primitives": prims}));
            self.stats.meshes += 1;
            Some(self.meshes.len() - 1)
        };
        self.model_mesh.insert(model.prim_id, r);
        r
    }

    fn model_triangles(&self, model: &Model) -> usize {
        model
            .meshes
            .iter()
            .filter(|m| self.mesh_wanted(m))
            .map(|m| m.submeshes.iter().filter(|s| self.submesh_wanted(s.material)).map(|s| s.index_count as usize / 3).sum::<usize>())
            .sum()
    }

    fn push_node(&mut self, n: Value) -> usize {
        self.nodes.push(n);
        self.stats.nodes += 1;
        self.nodes.len() - 1
    }

    /// Creates the joint nodes + skin of a skeleton; returns `(armature node, skin index)`.
    fn add_skin(&mut self, sk: &Skeleton, name: &str) -> (usize, usize) {
        let c = self.opts.coords;
        let base = self.nodes.len();
        let mut joint_ids = Vec::new();
        let mut roots = Vec::new();
        for (i, b) in sk.bones.iter().enumerate() {
            let local = conv_mat(&b.bind_local, c);
            let (t, q, s) = local.decompose();
            let mut node = json!({"name": b.name, "translation": t, "rotation": q});
            if s.iter().any(|x| (x - 1.0).abs() > 1e-4) {
                node["scale"] = json!(s);
            }
            self.nodes.push(node);
            self.stats.nodes += 1;
            joint_ids.push(base + i);
            if b.parent.is_none() {
                roots.push(base + i);
            }
        }
        for (i, b) in sk.bones.iter().enumerate() {
            if let Some(p) = b.parent {
                let id = joint_ids[i];
                let parent = &mut self.nodes[joint_ids[p]];
                match parent.get_mut("children") {
                    Some(Value::Array(a)) => a.push(json!(id)),
                    _ => parent["children"] = json!([id]),
                }
            }
        }
        let ibm: Vec<f32> = sk.bones.iter().flat_map(|b| conv_mat(&b.bind_world.inverse_affine(), c).to_cols_array()).collect();
        let acc = self.add_accessor(&f32_bytes(&ibm), None, 5126, sk.bones.len(), "MAT4", false, None);
        self.skins.push(json!({"name": format!("{name}_skin"), "joints": joint_ids, "inverseBindMatrices": acc, "skeleton": roots.first().copied().unwrap_or(base)}));
        self.stats.skins += 1;
        let arm = self.push_node(json!({"name": format!("{name}_armature"), "children": roots}));
        (arm, self.skins.len() - 1)
    }

    fn finish(mut self, scene_roots: Vec<usize>) -> GlbExport {
        self.stats.materials = self.gl_materials.len();
        let mut root = json!({
            "asset": {"version": "2.0", "generator": "bm-models (Hitman: Blood Money tools)"},
            "scene": 0,
            "scenes": [{"name": "scene", "nodes": scene_roots}],
            "nodes": self.nodes,
            "buffers": [{"byteLength": self.bin.len()}],
            "bufferViews": self.views,
            "accessors": self.accessors,
        });
        if !self.meshes.is_empty() {
            root["meshes"] = json!(self.meshes);
        }
        if !self.gl_materials.is_empty() {
            root["materials"] = json!(self.gl_materials);
        }
        if !self.textures.is_empty() {
            root["textures"] = json!(self.textures);
            root["images"] = json!(self.images);
            root["samplers"] = json!(self.samplers);
        }
        if !self.skins.is_empty() {
            root["skins"] = json!(self.skins);
        }
        let mut stats = self.stats;
        let glb = write_glb(&root, &self.bin);
        stats.bytes = glb.len();
        GlbExport { glb, stats }
    }
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn u16_bytes(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn u32_bytes(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn encode_png(img: &Image, max_dim: Option<u32>) -> Vec<u8> {
    let (mut w, mut h) = (img.width as usize, img.height as usize);
    let mut data = img.rgba.clone();
    if let Some(m) = max_dim {
        while w.max(h) > m as usize && w > 1 && h > 1 {
            let (nw, nh) = (w / 2, h / 2);
            let mut out = vec![0u8; nw * nh * 4];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let s = |dx: usize, dy: usize| data[((y * 2 + dy) * w + x * 2 + dx) * 4 + c] as u32;
                        out[(y * nw + x) * 4 + c] = ((s(0, 0) + s(1, 0) + s(0, 1) + s(1, 1) + 2) / 4) as u8;
                    }
                }
            }
            data = out;
            w = nw;
            h = nh;
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(&data).expect("png data");
    }
    out
}

fn write_glb(json: &Value, bin: &[u8]) -> Vec<u8> {
    let mut j = serde_json::to_vec(json).expect("json");
    while j.len() % 4 != 0 {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while b.len() % 4 != 0 {
        b.push(0);
    }
    let total = 12 + 8 + j.len() + if b.is_empty() { 0 } else { 8 + b.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&j);
    if !b.is_empty() {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&b);
    }
    out
}

/// Exports one model (model space, no scene transform).
pub fn export_model_glb(model: &Model, materials: &MaterialLibrary, opts: &ExportOptions) -> GlbExport {
    let mut b = Builder::new(opts, materials);
    let skel = if opts.skins { model.skeleton.clone() } else { None };
    let mesh = b.model_mesh(model, skel.is_some());
    let mut roots = Vec::new();
    if let Some(mi) = mesh {
        let mut node = json!({"name": format!("model_{}", model.prim_id), "mesh": mi});
        if let Some(sk) = &skel {
            let (arm, skin) = b.add_skin(sk, &format!("model_{}", model.prim_id));
            node["skin"] = json!(skin);
            roots.push(arm);
        }
        roots.push(b.push_node(node));
        b.stats.geometry_nodes = 1;
        b.stats.instanced_triangles = b.model_triangles(model);
    }
    let bbox = model.computed_bbox();
    b.stats.bbox = if bbox.is_empty() { None } else { Some(bbox) };
    if opts.scale != 1.0 {
        let child_roots = std::mem::take(&mut roots);
        let r = b.push_node(json!({"name": "scale", "scale": [opts.scale, opts.scale, opts.scale], "children": child_roots}));
        roots.push(r);
    }
    b.finish(roots)
}

/// Scene nodes an export with `opts` would include (kinds filter, subtree root, node cap).
pub fn select_geometry_nodes(scene: &Scene, opts: &ExportOptions) -> Vec<usize> {
    let top = opts.root.unwrap_or(0);
    let mut geo = Vec::new();
    for i in scene.subtree(top) {
        let n = &scene.nodes[i];
        if n.prim_id.is_some() && opts.kinds.contains(&n.kind) {
            if let Some(max) = opts.max_geometry_nodes {
                if geo.len() >= max {
                    break;
                }
            }
            geo.push(i);
        }
    }
    geo
}

/// Exports a scene (or the subtree `opts.root`) with its node hierarchy.
pub fn export_scene_glb(scene: &Scene, opts: &ExportOptions) -> GlbExport {
    let mut b = Builder::new(opts, &scene.materials);
    let c = opts.coords;
    let top = opts.root.unwrap_or(0);

    // geometry nodes to export
    let mut keep: HashSet<usize> = HashSet::new();
    let geo = select_geometry_nodes(scene, opts);
    for &g in &geo {
        let mut cur = Some(g);
        while let Some(i) = cur {
            if !keep.insert(i) {
                break;
            }
            cur = scene.nodes[i].parent;
        }
    }
    // gltf node index per scene node, created depth-first from the top
    let mut gl_index: HashMap<usize, usize> = HashMap::new();
    let mut bbox = Aabb::EMPTY;
    // iterate in index order: parents precede children
    let mut order: Vec<usize> = keep.iter().copied().collect();
    order.sort_unstable();
    // allocate nodes first (children are patched afterwards)
    for &i in &order {
        let n = &scene.nodes[i];
        let mut node = json!({"name": n.name});
        let local = if i == top && top != 0 { scene.nodes[i].world } else { n.local };
        let local = conv_mat(&local, c);
        if local != Mat4::IDENTITY {
            let (t, q, s) = local.decompose();
            node["translation"] = json!(t);
            node["rotation"] = json!(q);
            if s.iter().any(|x| (x - 1.0).abs() > 1e-5) {
                node["scale"] = json!(s);
            }
        }
        node["extras"] = json!({"bm_type": n.type_id, "bm_type_name": n.type_name.unwrap_or(""), "bm_index": n.index});
        gl_index.insert(i, b.push_node(node));
    }
    // geometry attachments
    for &g in &geo {
        let n = &scene.nodes[g];
        let Some(model) = scene.node_model(g) else { continue };
        let skel = if opts.skins { model.skeleton.clone() } else { None };
        let Some(mi) = b.model_mesh(&model, skel.is_some()) else { continue };
        b.stats.geometry_nodes += 1;
        b.stats.instanced_triangles += b.model_triangles(&model);
        bbox = bbox.union(&model.bbox.transformed(&n.world));
        let gi = gl_index[&g];
        if let Some(sk) = &skel {
            // skinned instance: armature + a child mesh node that references the skin
            let (arm, skin) = b.add_skin(sk, &n.name);
            let mesh_node = b.push_node(json!({"name": format!("{}_mesh", n.name), "mesh": mi, "skin": skin}));
            push_child(&mut b.nodes[gi], arm);
            push_child(&mut b.nodes[gi], mesh_node);
        } else {
            b.nodes[gi]["mesh"] = json!(mi);
        }
    }
    // hierarchy
    let mut roots = Vec::new();
    for &i in &order {
        let n = &scene.nodes[i];
        let gi = gl_index[&i];
        match n.parent.filter(|p| keep.contains(p) && i != top) {
            Some(p) => {
                let pi = gl_index[&p];
                push_child(&mut b.nodes[pi], gi);
            }
            None => roots.push(gi),
        }
    }
    b.stats.bbox = if bbox.is_empty() { None } else { Some(bbox) };
    if opts.scale != 1.0 {
        let r = b.push_node(json!({"name": "scale", "scale": [opts.scale, opts.scale, opts.scale], "children": roots}));
        roots = vec![r];
    }
    b.finish(roots)
}

fn push_child(node: &mut Value, child: usize) {
    match node.get_mut("children") {
        Some(Value::Array(a)) => a.push(json!(child)),
        _ => node["children"] = json!([child]),
    }
}

/// Convenience used by tests / tools: per-material triangle counts of a model.
pub fn triangles_per_material(model: &Model) -> BTreeMap<u32, usize> {
    let mut m = BTreeMap::new();
    for me in &model.meshes {
        for s in &me.submeshes {
            *m.entry(s.material).or_insert(0) += s.index_count as usize / 3;
        }
    }
    m
}
