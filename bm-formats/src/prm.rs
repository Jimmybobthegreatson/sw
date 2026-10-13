//! `.PRM` - "primitive" file: a table of chunks holding models, parts, vertex and index buffers.
//!
//! Full mesh decoding is left to `bm-models`; this module provides the verified container, the
//! typed headers and raw vertex/index access.
//!
//! # Container (confidence: high, round-trips byte-exactly)
//!
//! ```text
//! chunk 0 (offset 0, 16 bytes) is the header itself:
//!   u32 table_off     offset of the chunk table (= end of the last chunk)
//!   u32 chunk_count
//!   u32 table_off     (again)
//!   u32 0
//! chunks are stored back to back; every chunk size and offset is a multiple of 16
//! table_off: chunk_count x { u32 offset, u32 size, u32 unk1, u32 unk2 }
//! ```
//!
//! `unk1` is small (1 for ~90 % of the chunks, up to a few thousand for the shared "parts" chunk),
//! `unk2` is always 0. Meaning unknown (reference counts?).
//!
//! # Chunk graph (high; the pointers are *chunk indices*, not offsets)
//!
//! Scene objects of type `ZSTDOBJ` store a *prim id* in their PRP properties: that is the index of
//! a **model chunk**:
//!
//! ```text
//! model chunk (0x40 bytes, first word 0x00070100):
//!   +0x00 u32 0x00070100 (type/version tag)       +0x04, +0x08 u32 0
//!   +0x0c u32 2 (unknown)
//!   +0x10 u32 parts_chunk      shared 16-byte chunk (unknown content, unk1 = number of users)
//!   +0x14 u32 object_count
//!   +0x18 u32 objects_chunk    chunk holding `object_count` u32 chunk indices (padded)
//!   +0x1c u32 0
//!   +0x20 f32 bbox_min[3]      +0x2c f32 bbox_max[3]
//!   +0x38 u32 x 2 (zero)
//! object chunk (0x40 bytes, first word 0x00080489 in all cases seen):
//!   +0x0c u32 0x00ff0000 (flags?)        +0x10 u32 (material_id << 16) | 0x11e... (medium: the high
//!                                         half equals the MAT instance id, e.g. 0x11e = 286)
//!   +0x14 u32 2   +0x18 u32 0xff455337 (color/tint?)   +0x1c u32 flags (0x80, 0x400080 ...)
//!   +0x28 u32 model_ref_chunk  -> 16-byte chunk whose first word is the primitive-descriptor chunk
//!   +0x2c u32 1 ...
//! primitive descriptor (16 bytes): u32 vertex_count, u32 vertex_chunk, u32 unk (varies: 18, 4845 ...),
//!                                  u32 index_chunk
//! index chunk:  u16 1 (kind?), u16 index_count, index_count x u16 (triangle LIST; count % 3 == 0 and
//!               every index < vertex_count in all 4461 primitives of M01 that follow this layout)
//! vertex chunk: vertex_count x stride bytes, stride = chunk_size / vertex_count (rounded down to a
//!               multiple of 4). Observed strides: 0x28 (40, by far the most common), 0x34 (52),
//!               0x10 (16), 0x24 (36).
//! ```
//!
//! ## Vertex layouts (medium confidence, from sampling; stride 0x28 verified on thousands of meshes)
//!
//! ```text
//! stride 0x28: f32 pos[3] | i8[4] normal? (e.g. 7f 7f ff 00) | u8[4] color RGBA (e.g. 63 6e 75 ff)
//!              | f32 uv[2] | i8[4] tangent? | i8[4] binormal? | u32 0xcdcdcdcd (padding)
//! stride 0x10: f32 pos[3] | u8[4] (color / flags)
//! stride 0x24 / 0x34: skinned or multi-UV variants - layout not decoded
//! ```
//!
//! About 4 % of the objects in M01 do not follow the chain above (skinned/special meshes); the
//! model agent has to look at them individually.

use crate::io::{invalid, u32_at, FormatError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub offset: u32,
    pub size: u32,
    pub unk1: u32,
    pub unk2: u32,
}

#[derive(Clone, Debug)]
pub struct Prm {
    pub data: Vec<u8>,
    pub chunks: Vec<Chunk>,
}

/// 0x40-byte model header (see module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct ModelHeader {
    pub chunk: usize,
    pub tag: u32,
    pub parts_chunk: u32,
    pub object_count: u32,
    pub objects_chunk: u32,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
}

/// One drawable object of a model.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelObject {
    pub chunk: usize,
    /// All 16 words of the object chunk.
    pub words: [u32; 16],
    /// `words[4] >> 16`: raw material reference = `.MAT` instance id + 1 (0 = none), see [`ModelObject::material_instance`].
    pub material_id: u32,
    pub prim: Option<PrimDescriptor>,
}

impl ModelObject {
    /// The `.MAT` instance id this object uses (`material_id - 1`; 0 = no material).
    pub fn material_instance(&self) -> u32 {
        self.material_id.saturating_sub(1)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrimDescriptor {
    pub chunk: usize,
    pub vertex_count: u32,
    pub vertex_chunk: u32,
    pub unk: u32,
    pub index_chunk: u32,
}

pub const MODEL_TAG: u32 = 0x0007_0100;

impl Prm {
    pub fn parse(d: &[u8]) -> Result<Prm> {
        if d.len() < 16 {
            return Err(FormatError::Eof("PRM header"));
        }
        let table = u32_at(d, 0).unwrap() as usize;
        let count = u32_at(d, 4).unwrap() as usize;
        if u32_at(d, 8).unwrap() as usize != table || u32_at(d, 12) != Some(0) {
            return invalid("PRM header words do not match");
        }
        if table + count * 16 != d.len() {
            return invalid("PRM table does not end at end of file");
        }
        let mut chunks = Vec::with_capacity(count);
        for i in 0..count {
            let o = table + i * 16;
            let c = Chunk {
                offset: u32_at(d, o).unwrap(),
                size: u32_at(d, o + 4).unwrap(),
                unk1: u32_at(d, o + 8).unwrap(),
                unk2: u32_at(d, o + 12).unwrap(),
            };
            if (c.offset as usize + c.size as usize) > table {
                return invalid(format!("PRM chunk {i} outside the data area"));
            }
            chunks.push(c);
        }
        Ok(Prm { data: d[..table].to_vec(), chunks })
    }

    /// Re-serialises the data area and rebuilds the header and table.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.data.clone();
        let table = out.len() as u32;
        out[0..4].copy_from_slice(&table.to_le_bytes());
        out[4..8].copy_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        out[8..12].copy_from_slice(&table.to_le_bytes());
        for c in &self.chunks {
            out.extend_from_slice(&c.offset.to_le_bytes());
            out.extend_from_slice(&c.size.to_le_bytes());
            out.extend_from_slice(&c.unk1.to_le_bytes());
            out.extend_from_slice(&c.unk2.to_le_bytes());
        }
        out
    }

    /// Builds a PRM from raw chunk payloads `(unk1, data)`. Chunk 0 (the 16-byte header) is
    /// generated; the given chunks become chunks 1.. . Payloads are zero padded to 16 bytes.
    pub fn build(chunks: &[(u32, Vec<u8>)]) -> Prm {
        let mut data = vec![0u8; 16];
        let mut table = vec![Chunk { offset: 0, size: 16, unk1: 1, unk2: 0 }];
        for (unk1, payload) in chunks {
            let off = data.len() as u32;
            data.extend_from_slice(payload);
            while data.len() % 16 != 0 {
                data.push(0);
            }
            table.push(Chunk { offset: off, size: (data.len() as u32) - off, unk1: *unk1, unk2: 0 });
        }
        let mut p = Prm { data, chunks: table };
        let t = p.data.len() as u32;
        p.data[0..4].copy_from_slice(&t.to_le_bytes());
        p.data[4..8].copy_from_slice(&(p.chunks.len() as u32).to_le_bytes());
        p.data[8..12].copy_from_slice(&t.to_le_bytes());
        p
    }

    pub fn chunk(&self, i: usize) -> Option<&[u8]> {
        let c = self.chunks.get(i)?;
        self.data.get(c.offset as usize..(c.offset + c.size) as usize)
    }

    pub fn chunk_words(&self, i: usize) -> Option<Vec<u32>> {
        let b = self.chunk(i)?;
        Some(b.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect())
    }

    /// Interprets chunk `i` as a model header, if it looks like one.
    pub fn model(&self, i: usize) -> Option<ModelHeader> {
        let w = self.chunk_words(i)?;
        if w.len() != 16 || w[0] != MODEL_TAG {
            return None;
        }
        let f = |k: usize| f32::from_bits(w[k]);
        Some(ModelHeader {
            chunk: i,
            tag: w[0],
            parts_chunk: w[4],
            object_count: w[5],
            objects_chunk: w[6],
            bbox_min: [f(8), f(9), f(10)],
            bbox_max: [f(11), f(12), f(13)],
        })
    }

    /// Indices of all chunks that look like model headers.
    pub fn model_chunks(&self) -> Vec<usize> {
        (0..self.chunks.len()).filter(|&i| self.chunks[i].size == 0x40 && self.model(i).is_some()).collect()
    }

    /// Objects of a model (objects whose pointer chain is broken are skipped).
    pub fn model_objects(&self, m: &ModelHeader) -> Vec<ModelObject> {
        let mut out = Vec::new();
        let Some(list) = self.chunk_words(m.objects_chunk as usize) else { return out };
        for &oc in list.iter().take(m.object_count as usize) {
            let Some(w) = self.chunk_words(oc as usize) else { continue };
            if w.len() != 16 {
                continue;
            }
            let mut words = [0u32; 16];
            words.copy_from_slice(&w);
            out.push(ModelObject { chunk: oc as usize, words, material_id: words[4] >> 16, prim: self.object_prim(&words) });
        }
        out
    }

    fn object_prim(&self, ow: &[u32; 16]) -> Option<PrimDescriptor> {
        let model_ref = self.chunk_words(ow[10] as usize)?;
        let dchunk = *model_ref.first()? as usize;
        let d = self.chunk_words(dchunk)?;
        if d.len() != 4 {
            return None;
        }
        Some(PrimDescriptor { chunk: dchunk, vertex_count: d[0], vertex_chunk: d[1], unk: d[2], index_chunk: d[3] })
    }

    /// Raw vertex bytes and stride of a primitive.
    pub fn prim_vertices(&self, p: &PrimDescriptor) -> Option<(&[u8], usize)> {
        let b = self.chunk(p.vertex_chunk as usize)?;
        if p.vertex_count == 0 {
            return None;
        }
        let mut stride = b.len() / p.vertex_count as usize;
        stride -= stride % 4;
        // chunk padding can make the stride look like a multiple of 0x28
        if stride != 0x28 && stride % 0x28 == 0 {
            stride = 0x28;
        }
        if stride == 0 {
            return None;
        }
        Some((b, stride))
    }

    /// Triangle-list indices of a primitive (`None` if the index chunk does not follow the layout).
    pub fn prim_indices(&self, p: &PrimDescriptor) -> Option<Vec<u16>> {
        let b = self.chunk(p.index_chunk as usize)?;
        if b.len() < 4 {
            return None;
        }
        let n = u16::from_le_bytes([b[2], b[3]]) as usize;
        let raw = b.get(4..4 + 2 * n)?;
        Some(raw.chunks_exact(2).map(|x| u16::from_le_bytes([x[0], x[1]])).collect())
    }
}
