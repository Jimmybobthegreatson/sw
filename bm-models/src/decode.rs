//! `.PRM` -> [`Model`] decoding (all vertex strides, skinned meshes, the 0x80/0xb0 byte object
//! variants).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bm_formats::prm::Prm;

use crate::math::Aabb;
use crate::mesh::{Mesh, Model, SubMesh, VertexData, VertexLayout};
use crate::skeleton::Skeleton;

#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    NotAModel(u32),
    NoObjects(u32),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotAModel(i) => write!(f, "PRM chunk {i} is not a model header"),
            DecodeError::NoObjects(i) => write!(f, "PRM model {i} has no decodable objects"),
        }
    }
}

impl std::error::Error for DecodeError {}

fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// `[B,G,R,A]` bytes (D3DCOLOR order) -> RGBA.
fn color_rgba(b: &[u8]) -> [u8; 4] {
    [b[2], b[1], b[0], b[3]]
}

/// Unsigned-byte packed direction: `[B,G,R,x]` with `v / 127.5 - 1` per component.
fn unpack_dir(b: &[u8]) -> [f32; 3] {
    let c = |v: u8| v as f32 / 127.5 - 1.0;
    let v = [c(b[2]), c(b[1]), c(b[0])];
    normalize3(v)
}

pub(crate) fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-8 { [0.0, 0.0, 1.0] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

pub(crate) fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub(crate) fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Decodes a raw vertex chunk. `stride` is the per-vertex byte size.
pub fn decode_vertices(bytes: &[u8], count: usize, stride: usize) -> Option<(VertexLayout, VertexData)> {
    let layout = VertexLayout::from_stride(stride)?;
    if bytes.len() < count * stride {
        return None;
    }
    let mut v = VertexData::default();
    v.positions.reserve(count);
    for i in 0..count {
        let b = &bytes[i * stride..(i + 1) * stride];
        v.positions.push([rd_f32(b, 0), rd_f32(b, 4), rd_f32(b, 8)]);
        match layout {
            VertexLayout::Standard28 => {
                v.normals.push(unpack_dir(&b[12..16]));
                v.colors.push(color_rgba(&b[16..20]));
                v.uvs.push([rd_f32(b, 20), rd_f32(b, 24)]);
                let n = *v.normals.last().unwrap();
                v.tangents.push(tangent_w(n, &b[28..32], &b[32..36]));
            }
            VertexLayout::Simple24 => {
                v.normals.push(normalize3([rd_f32(b, 12), rd_f32(b, 16), rd_f32(b, 20)]));
                v.colors.push(color_rgba(&b[24..28]));
                v.uvs.push([rd_f32(b, 28), rd_f32(b, 32)]);
            }
            VertexLayout::Skinned34 => {
                let w = [rd_f32(b, 12), rd_f32(b, 16), rd_f32(b, 20)];
                v.weights.push([w[0], w[1], w[2], (1.0 - w[0] - w[1] - w[2]).max(0.0)]);
                // bone indices are stored pre-multiplied by 3 (constant register offsets)
                v.joints.push([(b[24] / 3) as u16, (b[25] / 3) as u16, (b[26] / 3) as u16, (b[27] / 3) as u16]);
                v.normals.push(unpack_dir(&b[28..32]));
                v.colors.push(color_rgba(&b[32..36]));
                v.uvs.push([rd_f32(b, 36), rd_f32(b, 40)]);
                let n = *v.normals.last().unwrap();
                v.tangents.push(tangent_w(n, &b[44..48], &b[48..52]));
            }
            VertexLayout::PosColor10 => {
                v.colors.push(color_rgba(&b[12..16]));
            }
        }
    }
    Some((layout, v))
}

fn tangent_w(n: [f32; 3], t: &[u8], b: &[u8]) -> [f32; 4] {
    let t = unpack_dir(t);
    let bt = unpack_dir(b);
    let w = if dot3(cross3(n, t), bt) < 0.0 { -1.0 } else { 1.0 };
    [t[0], t[1], t[2], w]
}

/// Finds the vertex stride of a vertex chunk.
///
/// Normally `chunk_size == vertex_count * stride` (chunks are zero padded to 16 bytes, so the
/// remainder is `< 16`). A few primitives (sprite quads, shared pools) reference a chunk that holds
/// *more* vertices than `vertex_count`; there the stride is the one that divides the chunk size
/// evenly (preferring `0x28` when the 4 pad bytes at +36 are `0xcdcdcdcd`/0). Objects whose
/// `words[3] & 0xffff == 3` are skinned (`0x34`).
pub fn detect_stride(vbytes: &[u8], vcount: usize, skinned_hint: bool) -> Option<usize> {
    let order: [usize; 4] = if skinned_hint { [0x34, 0x28, 0x24, 0x10] } else { [0x28, 0x24, 0x10, 0x34] };
    if let Some(s) = order.into_iter().find(|s| s * vcount <= vbytes.len() && vbytes.len() - s * vcount < 16) {
        return Some(s);
    }
    let pad_ok = |s: usize| s != 0x28 || vbytes.len() < 40 || matches!(&vbytes[36..40], [0xcd, 0xcd, 0xcd, 0xcd] | [0, 0, 0, 0]);
    order.into_iter().find(|&s| vbytes.len() % s == 0 && vbytes.len() / s >= vcount && pad_ok(s))
}

/// Decodes the PRM model whose header sits in chunk `prim_id`.
pub fn decode_model(prm: &Prm, prim_id: u32) -> Result<Model, DecodeError> {
    let header = prm.model(prim_id as usize).ok_or(DecodeError::NotAModel(prim_id))?;
    let list = prm.chunk_words(header.objects_chunk as usize).unwrap_or_default();
    let mut meshes = Vec::new();
    for &oc in list.iter().take(header.object_count as usize) {
        if let Some(m) = decode_object(prm, oc) {
            meshes.push(m);
        }
    }
    if meshes.is_empty() && header.object_count > 0 {
        return Err(DecodeError::NoObjects(prim_id));
    }
    let skeleton = if meshes.iter().any(|m| m.is_skinned()) {
        Skeleton::from_prm(prm, header.parts_chunk).map(Arc::new)
    } else {
        None
    };
    Ok(Model {
        prim_id,
        bbox: Aabb { min: header.bbox_min, max: header.bbox_max },
        meshes,
        skeleton,
    })
}

/// First word of the 176 byte *water surface* objects (`ZSTDOBJ` named `WaterSurface*`).
/// `.MAT` instance id of the object whose words are `w`.
///
/// The material reference in object word 4 (`w4 >> 16`) is **instance id + 1** (0 = no material): heads reference
/// the hair material one slot early otherwise, `Glass_OFF` nodes would get the glass and vice versa, and the only
/// reference that can address the last instance of a `.MAT` (id 908 in M01) would be out of range. Verified by
/// keyword statistics over all static nodes of M01/M02/M06 (three times more node-name / material-name matches
/// with the bias than without it) and by the character head / hair / eyes geometry (`docs/models.md`).
pub fn material_of(w: &[u32]) -> u32 {
    (w[4] >> 16).saturating_sub(1)
}

pub const WATER_SURFACE_TAG: u32 = 0x000b_0101;

/// Water surface objects carry no vertex chain, only a grid description:
/// `w[10] x w[11]` cells over `w[12] x w[13]` cm starting at `(w[14], w[16])` in the `y = 0` plane
/// (the rest of the words are wave / color parameters, kept in `extra_words`).
fn decode_water_surface(object_chunk: u32, w: &[u32]) -> Mesh {
    let (nx, nz) = ((w[10].clamp(1, 256)) as usize, (w[11].clamp(1, 256)) as usize);
    let (sx, sz) = (f32::from_bits(w[12]), f32::from_bits(w[13]));
    let (x0, z0) = (f32::from_bits(w[14]), f32::from_bits(w[16]));
    let mut v = VertexData::default();
    for j in 0..=nz {
        for i in 0..=nx {
            v.positions.push([x0 + sx * i as f32 / nx as f32, 0.0, z0 + sz * j as f32 / nz as f32]);
            v.normals.push([0.0, 1.0, 0.0]);
            v.tangents.push([1.0, 0.0, 0.0, 1.0]);
            v.uvs.push([i as f32, j as f32]);
            v.colors.push([255; 4]);
        }
    }
    let mut indices = Vec::new();
    let row = nx + 1;
    for j in 0..nz {
        for i in 0..nx {
            let a = (j * row + i) as u32;
            let (b, c, d) = (a + 1, a + row as u32, a + row as u32 + 1);
            // clockwise when seen from +Y (stored winding)
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    Mesh {
        layout: VertexLayout::Standard28,
        vertices: v,
        submeshes: vec![SubMesh { first_index: 0, index_count: indices.len() as u32, material: material_of(&w) }],
        indices,
        source_chunk: object_chunk,
        variant: w[0],
        group: w[3],
        flags: w[7],
        extra_words: w[16..].to_vec(),
        part: 0,
        bone_palette: Vec::new(),
        palette_words: Vec::new(),
        name: "water_surface".into(),
    }
}

/// Decodes one PRM object chunk (`0x40`, `0x80` or `0xb0` bytes) into a one-submesh [`Mesh`].
pub fn decode_object(prm: &Prm, object_chunk: u32) -> Option<Mesh> {
    let w = prm.chunk_words(object_chunk as usize)?;
    if w.len() < 16 {
        return None;
    }
    if w[0] == WATER_SURFACE_TAG && w.len() >= 18 {
        return Some(decode_water_surface(object_chunk, &w));
    }
    // object -> model-ref chunk -> primitive descriptor chunk
    let model_ref = prm.chunk_words(w[10] as usize)?;
    let dchunk = *model_ref.first()? as usize;
    let d = prm.chunk_words(dchunk)?;
    if d.len() != 4 {
        return None;
    }
    let (vcount, vchunk, ichunk) = (d[0] as usize, d[1] as usize, d[3] as usize);
    if vcount == 0 {
        return None;
    }
    let vbytes = prm.chunk(vchunk)?;
    let stride = detect_stride(vbytes, vcount, (w[3] & 0xffff) == 3)?;
    let (layout, vertices) = decode_vertices(vbytes, vcount, stride)?;
    let ib = prm.chunk(ichunk)?;
    if ib.len() < 4 {
        return None;
    }
    let n = u16::from_le_bytes([ib[2], ib[3]]) as usize;
    let raw = ib.get(4..4 + 2 * n)?;
    let indices: Vec<u32> = raw.chunks_exact(2).map(|x| u16::from_le_bytes([x[0], x[1]]) as u32).collect();
    if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= vcount) {
        return None;
    }
    let palette_words = if layout == VertexLayout::Skinned34 && w[15] != 0 {
        prm.chunk_words(w[15] as usize).unwrap_or_default()
    } else {
        Vec::new()
    };
    let bone_palette = palette_from_words(&palette_words);
    let mut vertices = vertices;
    if layout == VertexLayout::Skinned34 && !bone_palette.is_empty() {
        for j in vertices.joints.iter_mut().flatten() {
            *j = bone_palette.get(*j as usize).copied().unwrap_or(0);
        }
    }
    let material = material_of(&w);
    Some(Mesh {
        layout,
        vertices,
        submeshes: vec![SubMesh { first_index: 0, index_count: indices.len() as u32, material }],
        indices,
        source_chunk: object_chunk,
        variant: w[0],
        group: w[3],
        flags: w[7],
        extra_words: w[16..].to_vec(),
        part: if layout == VertexLayout::Skinned34 || w.len() > 16 { 0 } else { w[15] },

        bone_palette,
        palette_words,
        name: String::new(),
    })
}

/// Expands the `(size_bytes, offset_bytes)` pairs of a skinned object's palette chunk into the
/// local -> skeleton bone index table. Each pair names the bone range
/// `offset/12 .. (offset+size)/12` (12 bytes = 3 float4 shader constants per bone).
pub fn palette_from_words(words: &[u32]) -> Vec<u16> {
    let mut out = Vec::new();
    for pair in words.chunks_exact(2) {
        let (size, offset) = (pair[0] as usize, pair[1] as usize);
        if size == 0 {
            break;
        }
        let first = offset / 12;
        out.extend((first..first + size / 12).map(|b| b as u16));
    }
    out
}

/// Lazily decodes and caches the models of one PRM (thread safe).
pub struct ModelCache {
    models: Mutex<HashMap<u32, Option<Arc<Model>>>>,
}

impl Default for ModelCache {
    fn default() -> Self {
        ModelCache { models: Mutex::new(HashMap::new()) }
    }
}

impl ModelCache {
    pub fn get(&self, prm: &Prm, prim_id: u32) -> Option<Arc<Model>> {
        let mut g = self.models.lock().unwrap();
        g.entry(prim_id).or_insert_with(|| decode_model(prm, prim_id).ok().map(Arc::new)).clone()
    }

    pub fn len(&self) -> usize {
        self.models.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
