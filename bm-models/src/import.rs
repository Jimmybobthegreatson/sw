//! glTF 2.0 (`.glb`) import for mods: build a replacement [`Model`] from a Blender/DCC export.
//!
//! The reverse of [`gltf_export`](crate::gltf_export): nodes are flattened with their world
//! transforms baked into the vertices, the glTF coordinate system is converted back to Glacier
//! (Z mirror, metres -> centimetres), and every triangle primitive becomes a [`Mesh`] with one
//! submesh. Use [`prm_write::write_model`](crate::prm_write::write_model) to put the result into a
//! PRM and [`add_rgba_texture`] to add embedded textures to a scene `.TEX`.
//!
//! Material ids: a glTF material carrying `extras.bm_material` (what our exporter writes) keeps that
//! `.MAT` id, otherwise [`ImportOptions::material_ids`] (by material name) or
//! [`ImportOptions::default_material`] is used.

use std::collections::HashMap;
use std::sync::Arc;

use bm_formats::tex::{Tex, TexEntry, TexFormat, TexImage};

use crate::decode::{cross3, dot3, normalize3};
use crate::gltf_export::CoordSystem;
use crate::material::Image;
use crate::math::{Aabb, Mat4};
use crate::mesh::{Mesh, Model, SubMesh, VertexData, VertexLayout};
use crate::skeleton::Skeleton;

#[derive(Debug)]
pub enum ImportError {
    Gltf(String),
    /// A buffer is stored outside the `.glb` (external file / data URI), which is not supported.
    ExternalBuffer,
    NoGeometry,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Gltf(e) => write!(f, "glTF error: {e}"),
            ImportError::ExternalBuffer => write!(f, "external buffers are not supported (use .glb)"),
            ImportError::NoGeometry => write!(f, "the file contains no triangle geometry"),
        }
    }
}

impl std::error::Error for ImportError {}

#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub coords: CoordSystem,
    /// Multiplier applied to positions (metres -> centimetres = 100). Use 1.0 for native files.
    pub scale: f32,
    /// `.MAT` id for materials that name none.
    pub default_material: u32,
    /// glTF material name -> `.MAT` id overrides.
    pub material_ids: HashMap<String, u32>,
    /// Only import the node with this name and its descendants.
    pub node: Option<String>,
    /// Target skeleton: when given, primitives with JOINTS_0/WEIGHTS_0 become `Skinned34` meshes and
    /// joints are mapped to bones by node name (case-insensitive; unknown joints map to bone 0).
    pub skeleton: Option<Arc<Skeleton>>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            coords: CoordSystem::Gltf,
            scale: 100.0,
            default_material: 1,
            material_ids: HashMap::new(),
            node: None,
            skeleton: None,
        }
    }
}

/// Material information read from the glTF (for creating `.MAT` / `.TEX` entries).
#[derive(Clone, Debug)]
pub struct ImportedMaterial {
    pub gltf_index: usize,
    pub name: String,
    /// `.MAT` id assigned to the meshes using this material.
    pub bm_material: u32,
    pub base_color: [f32; 4],
    /// Decoded base color texture (PNG only).
    pub base_color_image: Option<Image>,
    pub double_sided: bool,
    pub alpha_blend: bool,
}

#[derive(Clone, Debug)]
pub struct ImportedModel {
    pub model: Model,
    pub materials: Vec<ImportedMaterial>,
    pub warnings: Vec<String>,
}

impl Model {
    /// Imports a `.glb` with default options (Gltf coordinates, metres -> cm, static meshes).
    pub fn from_glb(bytes: &[u8]) -> Result<Model, ImportError> {
        Ok(import_glb(bytes, &ImportOptions::default())?.model)
    }
}

fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::EXPAND);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

fn mat_from_gltf(m: [[f32; 4]; 4]) -> Mat4 {
    Mat4(m)
}

/// Imports a `.glb` (see module docs).
pub fn import_glb(bytes: &[u8], opts: &ImportOptions) -> Result<ImportedModel, ImportError> {
    let g = gltf::Gltf::from_slice(bytes).map_err(|e| ImportError::Gltf(e.to_string()))?;
    let blob = g.blob.as_deref();
    let mut warnings = Vec::new();
    let buffer = |b: gltf::Buffer| -> Option<&[u8]> {
        match b.source() {
            gltf::buffer::Source::Bin => blob,
            gltf::buffer::Source::Uri(_) => None,
        }
    };
    if g.buffers().any(|b| matches!(b.source(), gltf::buffer::Source::Uri(_))) {
        return Err(ImportError::ExternalBuffer);
    }

    // materials
    let mut materials: Vec<ImportedMaterial> = Vec::new();
    for m in g.materials() {
        let Some(gi) = m.index() else { continue };
        let name = m.name().unwrap_or("").to_string();
        let extras: Option<serde_json::Value> = m.extras().as_ref().and_then(|r| serde_json::from_str(r.get()).ok());
        let id = extras
            .as_ref()
            .and_then(|e| e.get("bm_material"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .or_else(|| opts.material_ids.get(&name).copied())
            .unwrap_or(opts.default_material);
        let pbr = m.pbr_metallic_roughness();
        let mut image = None;
        if let Some(info) = pbr.base_color_texture() {
            if let gltf::image::Source::View { view, .. } = info.texture().source().source() {
                if let Some(b) = blob {
                    let (o, l) = (view.offset(), view.length());
                    if let Some((w, h, rgba)) = b.get(o..o + l).and_then(decode_png) {
                        image = Some(Image {
                            id: 0,
                            name: info.texture().source().name().unwrap_or(&name).to_string(),
                            width: w,
                            height: h,
                            rgba,
                            source_format: "PNG".into(),
                            mip_count: 1,
                        });
                    } else {
                        warnings.push(format!("material {name}: base color texture is not a decodable PNG"));
                    }
                }
            }
        }
        materials.push(ImportedMaterial {
            gltf_index: gi,
            name,
            bm_material: id,
            base_color: pbr.base_color_factor(),
            base_color_image: image,
            double_sided: m.double_sided(),
            alpha_blend: m.alpha_mode() == gltf::material::AlphaMode::Blend,
        });
    }
    let mat_id = |idx: Option<usize>| -> u32 {
        idx.and_then(|i| materials.iter().find(|m| m.gltf_index == i)).map(|m| m.bm_material).unwrap_or(opts.default_material)
    };

    // walk nodes
    let scene = g.default_scene().or_else(|| g.scenes().next()).ok_or(ImportError::NoGeometry)?;
    let mut meshes: Vec<Mesh> = Vec::new();
    let mut stack: Vec<(gltf::Node, Mat4, bool)> =
        scene.nodes().map(|n| (n, Mat4::IDENTITY, opts.node.is_none())).collect();
    stack.reverse();
    while let Some((node, parent, active)) = stack.pop() {
        let world = parent.mul(&mat_from_gltf(node.transform().matrix()));
        let active = active || opts.node.as_deref().map(|n| node.name() == Some(n)).unwrap_or(false);
        for c in node.children() {
            stack.push((c, world, active));
        }
        if !active {
            continue;
        }
        let Some(mesh) = node.mesh() else { continue };
        // joint name -> bone mapping for skinned primitives
        let joint_map: Option<Vec<u16>> = match (&opts.skeleton, node.skin()) {
            (Some(sk), Some(skin)) => Some(
                skin.joints()
                    .map(|j| j.name().and_then(|n| sk.bone_index(n)).unwrap_or(0) as u16)
                    .collect(),
            ),
            _ => None,
        };
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                warnings.push(format!("mesh {:?}: non-triangle primitive skipped", mesh.name()));
                continue;
            }
            let reader = prim.reader(buffer);
            let Some(pos) = reader.read_positions() else { continue };
            let pos: Vec<[f32; 3]> = pos.collect();
            let n = pos.len();
            let mut indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..n as u32).collect(),
            };
            if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= n) {
                warnings.push(format!("mesh {:?}: invalid indices skipped", mesh.name()));
                continue;
            }
            // transform to Glacier space
            let conv = |p: [f32; 3]| -> [f32; 3] {
                if opts.coords == CoordSystem::Gltf { [p[0], p[1], -p[2]] } else { p }
            };
            let positions: Vec<[f32; 3]> = pos
                .iter()
                .map(|&p| {
                    let w = conv(world.transform_point(p));
                    [w[0] * opts.scale, w[1] * opts.scale, w[2] * opts.scale]
                })
                .collect();
            let mut normals: Vec<[f32; 3]> = match reader.read_normals() {
                Some(it) => it.map(|v| normalize3(conv(world.transform_vector(v)))).collect(),
                None => Vec::new(),
            };
            let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
                Some(t) => t.into_f32().collect(),
                None => vec![[0.0, 0.0]; n],
            };
            let colors: Vec<[u8; 4]> = match reader.read_colors(0) {
                Some(c) => c.into_rgba_u8().collect(),
                None => vec![[255, 255, 255, 255]; n],
            };
            let mut tangents: Vec<[f32; 4]> = match reader.read_tangents() {
                Some(it) => it
                    .map(|t| {
                        let d = normalize3(conv(world.transform_vector([t[0], t[1], t[2]])));
                        let w = if opts.coords == CoordSystem::Gltf { -t[3] } else { t[3] };
                        [d[0], d[1], d[2], if w < 0.0 { -1.0 } else { 1.0 }]
                    })
                    .collect(),
                None => Vec::new(),
            };
            // a mirrored node flips the winding
            if world.det3() < 0.0 {
                for t in indices.chunks_exact_mut(3) {
                    t.swap(1, 2);
                }
            }
            if normals.len() != n {
                normals = smooth_normals(&positions, &indices);
            }
            if tangents.len() != n {
                tangents = make_tangents(&positions, &normals, &uvs, &indices);
            }
            let mut vertices = VertexData { positions, normals, tangents, uvs, colors, ..Default::default() };
            let mut layout = VertexLayout::Standard28;
            if let (Some(map), Some(j), Some(w)) = (&joint_map, reader.read_joints(0), reader.read_weights(0)) {
                vertices.joints =
                    j.into_u16().map(|j| [0, 1, 2, 3].map(|k| map.get(j[k] as usize).copied().unwrap_or(0))).collect();
                vertices.weights = w
                    .into_f32()
                    .map(|w| {
                        let s = w[0] + w[1] + w[2] + w[3];
                        if s > 1e-6 { [w[0] / s, w[1] / s, w[2] / s, w[3] / s] } else { [1.0, 0.0, 0.0, 0.0] }
                    })
                    .collect();
                if vertices.joints.len() == n && vertices.weights.len() == n {
                    layout = VertexLayout::Skinned34;
                } else {
                    vertices.joints.clear();
                    vertices.weights.clear();
                }
            }
            let material = mat_id(prim.material().index());
            meshes.push(Mesh {
                layout,
                vertices,
                submeshes: vec![SubMesh { first_index: 0, index_count: indices.len() as u32, material }],
                indices,
                source_chunk: 0,
                variant: 0,
                group: 0,
                flags: 0,
                extra_words: Vec::new(),
                part: 0,
                bone_palette: Vec::new(),
                palette_words: Vec::new(),
                name: mesh.name().unwrap_or("").to_string(),
            });
        }
    }
    if meshes.is_empty() {
        return Err(ImportError::NoGeometry);
    }
    let bbox = meshes.iter().fold(Aabb::EMPTY, |b, m| b.union(&m.vertices.bbox()));
    Ok(ImportedModel {
        model: Model { prim_id: 0, bbox, meshes, skeleton: opts.skeleton.clone() },
        materials,
        warnings,
    })
}

/// Area weighted smooth vertex normals.
pub fn smooth_normals(pos: &[[f32; 3]], idx: &[u32]) -> Vec<[f32; 3]> {
    let mut n = vec![[0f32; 3]; pos.len()];
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        // stored (clockwise) winding: the visible normal is the negated right-handed cross product
        let f = cross3(e1, e2);
        for &i in t {
            for k in 0..3 {
                n[i as usize][k] -= f[k];
            }
        }
    }
    n.into_iter().map(normalize3).collect()
}

/// Per-vertex tangents (xyz + handedness) from UVs.
pub fn make_tangents(pos: &[[f32; 3]], nrm: &[[f32; 3]], uv: &[[f32; 2]], idx: &[u32]) -> Vec<[f32; 4]> {
    let mut tan = vec![[0f32; 3]; pos.len()];
    let mut bit = vec![[0f32; 3]; pos.len()];
    for t in idx.chunks_exact(3) {
        let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let e1 = [pos[i1][0] - pos[i0][0], pos[i1][1] - pos[i0][1], pos[i1][2] - pos[i0][2]];
        let e2 = [pos[i2][0] - pos[i0][0], pos[i2][1] - pos[i0][1], pos[i2][2] - pos[i0][2]];
        let (du1, dv1) = (uv[i1][0] - uv[i0][0], uv[i1][1] - uv[i0][1]);
        let (du2, dv2) = (uv[i2][0] - uv[i0][0], uv[i2][1] - uv[i0][1]);
        let det = du1 * dv2 - du2 * dv1;
        if det.abs() < 1e-12 {
            continue;
        }
        let r = 1.0 / det;
        for k in 0..3 {
            let tk = (e1[k] * dv2 - e2[k] * dv1) * r;
            let bk = (e2[k] * du1 - e1[k] * du2) * r;
            for &i in &[i0, i1, i2] {
                tan[i][k] += tk;
                bit[i][k] += bk;
            }
        }
    }
    (0..pos.len())
        .map(|i| {
            let n = nrm[i];
            let t = tan[i];
            let d = dot3(n, t);
            let o = [t[0] - n[0] * d, t[1] - n[1] * d, t[2] - n[2] * d];
            let l = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt();
            if l < 1e-8 {
                // any perpendicular
                let a = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
                let p = normalize3(cross3(n, a));
                return [p[0], p[1], p[2], 1.0];
            }
            let o = [o[0] / l, o[1] / l, o[2] / l];
            let w = if dot3(cross3(n, o), bit[i]) < 0.0 { -1.0 } else { 1.0 };
            [o[0], o[1], o[2], w]
        })
        .collect()
}

/// Adds an RGBA8 image to a scene `.TEX` (uncompressed `RGBA` format with a full mip chain) and
/// returns its new texture id (first free id >= 128 and < 2048). `None` if the table is full.
pub fn add_rgba_texture(tex: &mut Tex, name: &str, width: u32, height: u32, rgba: &[u8]) -> Option<u32> {
    if rgba.len() != (width * height * 4) as usize || width > 65535 || height > 65535 {
        return None;
    }
    let used: std::collections::HashSet<u32> = tex.slots.iter().map(|s| s.0).collect();
    let id = (128..2048u32).find(|i| !used.contains(i))?;
    let mut mips = vec![rgba.to_vec()];
    let (mut w, mut h) = (width as usize, height as usize);
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let prev = mips.last().unwrap();
        let mut out = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let s = |dx: usize, dy: usize| {
                        prev[((((y * 2 + dy).min(h - 1)) * w) + (x * 2 + dx).min(w - 1)) * 4 + c] as u32
                    };
                    out[(y * nw + x) * 4 + c] = ((s(0, 0) + s(1, 0) + s(0, 1) + s(1, 1) + 2) / 4) as u8;
                }
            }
        }
        mips.push(out);
        w = nw;
        h = nh;
    }
    let img = TexImage {
        id,
        format: TexFormat::Rgba,
        format2: TexFormat::Rgba.to_raw(),
        width: width as u16,
        height: height as u16,
        unk18: 0,
        unk1c: 1.0,
        unk20: 0,
        name: name.to_string(),
        mips,
        trailer: Vec::new(),
    };
    tex.entries.push(TexEntry::Image(img));
    tex.slots.push((id, tex.entries.len() - 1));
    tex.slots.sort();
    Some(id)
}
