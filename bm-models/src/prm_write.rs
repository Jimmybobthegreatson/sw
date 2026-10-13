//! Writing [`Model`]s back into `.PRM` files (replacement models for mods).
//!
//! [`write_model`] replaces the geometry of an existing model chunk **in place**: new vertex /
//! index / descriptor / object chunks are appended to the PRM and the model header is re-pointed,
//! so every scene node (`prim id`) and every other model keeps working untouched. The resulting
//! `Prm::to_bytes()` can be dropped into a mod directory as a loose
//! `Scenes/<level>/<scene>.PRM` (the VFS gives it priority over the archive copy).
//!
//! Limits (see `docs/models.md`):
//! * 16-bit indices: at most 65 535 vertices and 65 535 indices per object (larger meshes are split
//!   per submesh automatically, but a single submesh cannot exceed that).
//! * Skinned objects: at most [`MAX_BONES_PER_OBJECT`] distinct bones per object (the engine skins
//!   in vertex shader constants), and the model needs an existing skeleton block in its `parts`
//!   chunk (reuse the original model's chunk with [`write_model`]).
//! * Vertex normals / tangents are quantised to 8 bits like the originals.
//! * Unknown header fields are copied from an existing object of the model when available.

use std::collections::BTreeMap;

use bm_formats::prm::{Chunk, Prm};

use crate::decode::{cross3, normalize3};
use crate::mesh::{Mesh, Model, SubMesh, VertexData, VertexLayout};

/// Observed maximum palette size of shipped skinned objects (26 bones).
pub const MAX_BONES_PER_OBJECT: usize = 26;

#[derive(Debug, Clone, PartialEq)]
pub enum WriteError {
    /// The target chunk is not a model header.
    NotAModel(u32),
    /// A mesh/submesh has more vertices or indices than 16-bit indexing allows.
    TooManyVertices { vertices: usize },
    TooManyBones { bones: usize },
    /// A skinned mesh was written into a model without a skeleton block.
    NoSkeleton,
    Empty,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::NotAModel(i) => write!(f, "PRM chunk {i} is not a model header"),
            WriteError::TooManyVertices { vertices } => write!(f, "{vertices} vertices exceed the 16-bit index limit"),
            WriteError::TooManyBones { bones } => write!(f, "{bones} bones in one object (limit {MAX_BONES_PER_OBJECT})"),
            WriteError::NoSkeleton => write!(f, "skinned mesh but the target model has no skeleton block"),
            WriteError::Empty => write!(f, "nothing to write"),
        }
    }
}

impl std::error::Error for WriteError {}

fn pack_dir(d: [f32; 3]) -> [u8; 4] {
    let q = |x: f32| (((x.clamp(-1.0, 1.0)) + 1.0) * 127.5).round().clamp(0.0, 255.0) as u8;
    [q(d[2]), q(d[1]), q(d[0]), 0]
}

fn put_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

/// Encodes a vertex buffer for `layout`. Missing attributes get neutral defaults. For skinned
/// layouts `palette` maps skeleton bone index -> local palette slot.
pub fn encode_vertices(
    layout: VertexLayout,
    v: &VertexData,
    palette: Option<&BTreeMap<u16, u8>>,
) -> Vec<u8> {
    let n = v.len();
    let mut out = Vec::with_capacity(n * layout.stride());
    for i in 0..n {
        let p = v.positions[i];
        put_f32s(&mut out, &p);
        let normal = v.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
        let color = v.colors.get(i).copied().unwrap_or([255, 255, 255, 255]);
        let uv = v.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
        let tangent = v.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]);
        let t3 = normalize3([tangent[0], tangent[1], tangent[2]]);
        let bin = {
            let b = cross3(normalize3(normal), t3);
            [b[0] * tangent[3], b[1] * tangent[3], b[2] * tangent[3]]
        };
        let col = [color[2], color[1], color[0], color[3]];
        match layout {
            VertexLayout::Standard28 => {
                out.extend_from_slice(&pack_dir(normalize3(normal)));
                out.extend_from_slice(&col);
                put_f32s(&mut out, &uv);
                out.extend_from_slice(&pack_dir(t3));
                out.extend_from_slice(&pack_dir(normalize3(bin)));
                out.extend_from_slice(&0xcdcd_cdcdu32.to_le_bytes());
            }
            VertexLayout::Simple24 => {
                put_f32s(&mut out, &normalize3(normal));
                out.extend_from_slice(&col);
                put_f32s(&mut out, &uv);
            }
            VertexLayout::Skinned34 => {
                let w = v.weights.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]);
                put_f32s(&mut out, &[w[0], w[1], w[2]]);
                let j = v.joints.get(i).copied().unwrap_or([0; 4]);
                for k in 0..4 {
                    let local = palette.and_then(|pm| pm.get(&j[k]).copied()).unwrap_or(0);
                    out.push(local * 3);
                }
                out.extend_from_slice(&pack_dir(normalize3(normal)));
                out.extend_from_slice(&col);
                put_f32s(&mut out, &uv);
                out.extend_from_slice(&pack_dir(t3));
                out.extend_from_slice(&pack_dir(normalize3(bin)));
            }
            VertexLayout::PosColor10 => {
                out.extend_from_slice(&col);
            }
        }
    }
    out
}

/// Builds the palette chunk words and the bone -> local slot map for the bones used by a mesh.
fn build_palette(v: &VertexData) -> Result<(Vec<u32>, BTreeMap<u16, u8>, usize), WriteError> {
    let mut used: Vec<u16> = Vec::new();
    for (j, w) in v.joints.iter().zip(&v.weights) {
        for k in 0..4 {
            if w[k] > 0.0 || k == 0 {
                used.push(j[k]);
            }
        }
    }
    used.sort_unstable();
    used.dedup();
    if used.len() > MAX_BONES_PER_OBJECT {
        return Err(WriteError::TooManyBones { bones: used.len() });
    }
    let map: BTreeMap<u16, u8> = used.iter().enumerate().map(|(i, &b)| (b, i as u8)).collect();
    let mut words = Vec::new();
    let mut ranges = 0;
    let mut i = 0;
    while i < used.len() {
        let start = used[i];
        let mut len = 1;
        while i + len < used.len() && used[i + len] == start + len as u16 {
            len += 1;
        }
        words.push(len as u32 * 12);
        words.push(start as u32 * 12);
        ranges += 1;
        i += len;
    }
    while words.len() % 4 != 0 {
        words.push(0);
    }
    Ok((words, map, ranges))
}

fn push_chunk(prm: &mut Prm, bytes: &[u8], unk1: u32) -> u32 {
    let off = prm.data.len() as u32;
    prm.data.extend_from_slice(bytes);
    while prm.data.len() % 16 != 0 {
        prm.data.push(0);
    }
    prm.chunks.push(Chunk { offset: off, size: prm.data.len() as u32 - off, unk1, unk2: 0 });
    (prm.chunks.len() - 1) as u32
}

fn words_bytes(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Splits a mesh into one single-submesh mesh per submesh with compacted vertex buffers.
fn split_submeshes(m: &Mesh) -> Vec<Mesh> {
    let mut out = Vec::new();
    for sm in &m.submeshes {
        let idx = m.submesh_indices(sm);
        if idx.is_empty() {
            continue;
        }
        let mut remap: BTreeMap<u32, u32> = BTreeMap::new();
        let mut order: Vec<u32> = Vec::new();
        for &i in idx {
            remap.entry(i).or_insert_with(|| {
                order.push(i);
                (order.len() - 1) as u32
            });
        }
        let v = &m.vertices;
        let nv = VertexData {
            positions: pick_vec(&v.positions, &order),
            normals: pick_vec(&v.normals, &order),
            tangents: pick_vec(&v.tangents, &order),
            uvs: pick_vec(&v.uvs, &order),
            colors: pick_vec(&v.colors, &order),
            joints: pick_vec(&v.joints, &order),
            weights: pick_vec(&v.weights, &order),
        };
        let indices: Vec<u32> = idx.iter().map(|i| remap[i]).collect();
        let mut mm = m.clone();
        mm.vertices = nv;
        mm.submeshes = vec![SubMesh { first_index: 0, index_count: indices.len() as u32, material: sm.material }];
        mm.indices = indices;
        out.push(mm);
    }
    out
}

fn pick_vec<T: Clone>(src: &[T], order: &[u32]) -> Vec<T> {
    if src.is_empty() { Vec::new() } else { order.iter().map(|&i| src[i as usize].clone()).collect() }
}

/// Default object words for meshes that did not come from a PRM.
fn object_words(m: &Mesh, sm: &SubMesh, modelref: u32, palette_chunk: u32, ranges: usize) -> Vec<u32> {
    let skinned = m.layout == VertexLayout::Skinned34;
    let mut w = vec![0u32; 16];
    w[0] = if m.variant != 0 && m.variant != crate::decode::WATER_SURFACE_TAG { m.variant } else if skinned { 0x80089 } else { 0x80189 };
    w[3] = if m.group != 0 { m.group } else if skinned { 0x0101_0003 } else { 0x00ff_0000 };
    w[4] = (sm.material + 1) << 16; // references are instance id + 1, see `decode::material_of`
    w[5] = 2;
    w[6] = 0xff80_8080;
    w[7] = if m.flags != 0 { m.flags } else { 0x80 };
    w[10] = modelref;
    w[11] = if skinned { 0 } else { 1 };
    if skinned {
        w[14] = ranges as u32;
        w[15] = palette_chunk;
    } else if m.part != 0 {
        w[14] = 1;
        w[15] = m.part;
    }
    w.extend_from_slice(&m.extra_words);
    w
}

/// Outcome of a model write.
#[derive(Clone, Debug, PartialEq)]
pub struct WriteReport {
    pub model_chunk: u32,
    pub objects: Vec<u32>,
    pub vertices: usize,
    pub triangles: usize,
}

/// Replaces the objects of the PRM model in chunk `prim_id` with the meshes of `model`.
pub fn write_model(prm: &mut Prm, prim_id: u32, model: &Model) -> Result<WriteReport, WriteError> {
    let header = prm.model(prim_id as usize).ok_or(WriteError::NotAModel(prim_id))?;
    let mut hw = prm.chunk_words(prim_id as usize).ok_or(WriteError::NotAModel(prim_id))?;
    let has_skeleton = prm
        .chunk_words(header.parts_chunk as usize)
        .map(|p| p.get(1).copied().unwrap_or(0) != 0)
        .unwrap_or(false);

    let meshes: Vec<Mesh> = model.meshes.iter().flat_map(split_submeshes).collect();
    if meshes.is_empty() {
        return Err(WriteError::Empty);
    }
    let mut object_chunks = Vec::new();
    let (mut nverts, mut ntris) = (0, 0);
    let mut bbox = crate::math::Aabb::EMPTY;
    for m in &meshes {
        if m.vertices.len() > 65535 || m.indices.len() > 65535 {
            return Err(WriteError::TooManyVertices { vertices: m.vertices.len() });
        }
        let sm = &m.submeshes[0];
        let skinned = m.layout == VertexLayout::Skinned34;
        if skinned && !has_skeleton {
            return Err(WriteError::NoSkeleton);
        }
        let (pal_words, pal_map, ranges) = if skinned { build_palette(&m.vertices)? } else { (vec![], BTreeMap::new(), 0) };
        let vbytes = encode_vertices(m.layout, &m.vertices, if skinned { Some(&pal_map) } else { None });
        let vchunk = push_chunk(prm, &vbytes, 1);
        let mut ib = Vec::with_capacity(4 + 2 * m.indices.len());
        ib.extend_from_slice(&1u16.to_le_bytes());
        ib.extend_from_slice(&(m.indices.len() as u16).to_le_bytes());
        for &i in &m.indices {
            ib.extend_from_slice(&(i as u16).to_le_bytes());
        }
        let ichunk = push_chunk(prm, &ib, 1);
        let tris = m.indices.len() / 3;
        let desc = [m.vertices.len() as u32, vchunk, 5 * (tris.max(1) as u32 - 1), ichunk];
        let dchunk = push_chunk(prm, &words_bytes(&desc), 1);
        let mref = push_chunk(prm, &words_bytes(&[dchunk, 0, 0, 0]), 1);
        let pchunk = if skinned { push_chunk(prm, &words_bytes(&pal_words), 1) } else { 0 };
        let ow = object_words(m, sm, mref, pchunk, ranges);
        let ochunk = push_chunk(prm, &words_bytes(&ow), 1);
        object_chunks.push(ochunk);
        nverts += m.vertices.len();
        ntris += tris;
        bbox = bbox.union(&m.vertices.bbox());
    }
    let list = push_chunk(prm, &words_bytes(&object_chunks), 1);
    hw[5] = object_chunks.len() as u32;
    hw[6] = list;
    for k in 0..3 {
        hw[8 + k] = bbox.min[k].to_bits();
        hw[11 + k] = bbox.max[k].to_bits();
    }
    let c = prm.chunks[prim_id as usize].clone();
    let bytes = words_bytes(&hw);
    prm.data[c.offset as usize..c.offset as usize + bytes.len()].copy_from_slice(&bytes);
    Ok(WriteReport { model_chunk: prim_id, objects: object_chunks, vertices: nverts, triangles: ntris })
}

/// Builds a new standalone PRM containing just `model` (static meshes only; skinned meshes need
/// the skeleton block of an existing model, use [`write_model`] for those). Returns the PRM and
/// the prim id of the model chunk.
pub fn build_prm(model: &Model) -> Result<(Prm, u32), WriteError> {
    let mut prm = Prm::build(&[
        (1, vec![0u8; 16]),                                        // chunk 1: parts (empty)
        (1, words_bytes(&[0, 0, 0, 0])),                           // chunk 2: placeholder objects list
        (
            1,
            words_bytes(&[0x0007_0100, 0, 0, 2, 1, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]), // chunk 3: model header
        ),
    ]);
    write_model(&mut prm, 3, model)?;
    Ok((prm, 3))
}
