//! `.GMS` - "Glacier Mission/Scene" binary: the flat geometry (entity) table of a scene.
//!
//! # Container (confidence: high, verified on every scene shipped)
//!
//! ```text
//! u32 uncompressed_len
//! u32 stored_file_len      (= whole file length including this 9 byte header)
//! u8  flag                 0 = payload is raw DEFLATE, otherwise stored (flag != 0 never seen)
//! ...payload
//! ```
//!
//! # Decompressed body (high unless noted)
//!
//! ```text
//! 0x00 u32 table_off        offset of the geometry table
//! 0x04 u32 0, 0x08 u32 0    unknown (always 0)
//! 0x0c u32 4                unknown
//! 0x10 u32 stats_off        offset of the type statistics table
//! 0x14.. u32 x 3            unknown offsets/sizes (low confidence: ends of regions?)
//! 0x20..0x60                unknown header words
//! 0x60..                    objects (the ROOT object lives at 0x60), each geom header is 0x40
//!                           bytes, followed by 0x10 (position-like) and 0x30 (matrix-like) extra
//!                           bytes for some types (object size varies 0x40/0x50/0x80, medium)
//! table_off: u32 count, then `count` x { u32 offset_word, u32 tag }
//! stats_off: u32 count, then `count` x { u32 type_id, u32 count, i32 unk }
//! ```
//!
//! Table entry `offset_word`: low 24 bits are the object address divided by 4
//! (`addr = (word & 0xffffff) * 4`); the top byte is some small value (0..11, meaning unknown,
//! it does **not** correlate with tree depth). `tag`: 0 for ~half of the entries, otherwise
//! increasing even numbers (meaning unknown, probably a mesh/primitive slot counter).
//!
//! The table lists the geometries of the scene in **depth-first order** of the scene tree
//! (verified: the `.PRP` object tree has exactly `count + 1` nodes and the i-th table entry belongs
//! to PRP node `i + 1`, node 0 being the implicit ROOT). The hierarchy itself therefore comes from
//! the PRP (`crate::prp::PrpScene`), *not* from the GMS.
//!
//! # Geom header (offsets relative to the object address, high confidence unless noted)
//!
//! | off | meaning |
//! |---|---|
//! | 0x00 | offset of the NUL-terminated name inside the `.BUF` |
//! | 0x04 | self pointer = `addr - 0x30` (every entry; **not** the parent) |
//! | 0x08 | pointer to the object's **position block** (`3 x f32 position + 4 bytes pad`), stored *before* the header (usually `addr - 0x10`, sometimes `addr - 0x40` when a larger block precedes); high - matches the PRP position for 8787 of 8792 M01 objects |
//! | 0x0c | unknown, grows slowly (0x79e, 0x7a4 ...) |
//! | 0x14 | type id (`0x100001` ZGROUP, `0x200002` ZSTDOBJ ... see docs/formats.md) |
//! | 0x1c | control/flag bits (unknown) |
//! | 0x30 | sequence number (unknown) |
//! | 0x34 | flags (0, 0x100, 0x200 seen) |
//! | 0x40.. | per-type extension. In the 0x40-byte pre-blocks the 0x30 bytes after the position are mostly zero; the rotation matrix is **not** reliably stored here - the PRP is authoritative for rotation and position, see `scene.rs` |

use std::io::{Read, Write};

use crate::io::{invalid, u32_at, FormatError, PutLe, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GmsEntry {
    pub index: usize,
    /// Raw first word of the table entry.
    pub word0: u32,
    /// Raw second word of the table entry (the "tag").
    pub tag: u32,
    /// Object address inside the body.
    pub addr: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeStat {
    pub type_id: u32,
    pub count: u32,
    pub unk: i32,
}

/// Decoded view of the first 0x40 bytes of a geometry object.
#[derive(Clone, Debug, PartialEq)]
pub struct GeomHeader {
    pub addr: u32,
    pub name_offset: u32,
    pub self_ptr: u32,
    pub transform_ptr: u32,
    pub field_0c: u32,
    pub type_id: u32,
    pub control: u32,
    pub seq: u32,
    pub flags: u32,
    /// All 16 words, for fields not named above.
    pub words: [u32; 16],
}

#[derive(Clone, Debug)]
pub struct Gms {
    /// Decompressed body.
    pub body: Vec<u8>,
    /// True if the source file used DEFLATE (flag byte 0).
    pub was_compressed: bool,
    pub entries: Vec<GmsEntry>,
    pub stats: Vec<TypeStat>,
}

impl Gms {
    pub fn parse(file: &[u8]) -> Result<Gms> {
        if file.len() < 9 {
            return Err(FormatError::Eof("GMS header"));
        }
        let usize_ = u32_at(file, 0).unwrap() as usize;
        let flag = file[8];
        let (body, was_compressed) = if flag == 0 {
            let mut out = Vec::with_capacity(usize_);
            flate2::read::DeflateDecoder::new(&file[9..])
                .read_to_end(&mut out)
                .map_err(|e| FormatError::Invalid(format!("GMS deflate: {e}")))?;
            (out, true)
        } else {
            (file[9..].to_vec(), false)
        };
        if body.len() != usize_ {
            return invalid(format!("GMS body is {} bytes, header says {usize_}", body.len()));
        }
        Gms::from_body(body, was_compressed)
    }

    pub fn from_body(body: Vec<u8>, was_compressed: bool) -> Result<Gms> {
        let table = u32_at(&body, 0).ok_or(FormatError::Eof("GMS table offset"))? as usize;
        let count = u32_at(&body, table).ok_or(FormatError::Eof("GMS table count"))? as usize;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let o = table + 4 + i * 8;
            let w0 = u32_at(&body, o).ok_or(FormatError::Eof("GMS table entry"))?;
            let tag = u32_at(&body, o + 4).ok_or(FormatError::Eof("GMS table entry"))?;
            let addr = (w0 & 0x00ff_ffff) * 4;
            if addr as usize + 0x40 > body.len() {
                return invalid(format!("GMS entry {i} points outside the body"));
            }
            entries.push(GmsEntry { index: i, word0: w0, tag, addr });
        }
        let stats_off = u32_at(&body, 0x10).ok_or(FormatError::Eof("GMS stats offset"))? as usize;
        let n = u32_at(&body, stats_off).ok_or(FormatError::Eof("GMS stats count"))? as usize;
        let mut stats = Vec::with_capacity(n);
        for i in 0..n {
            let o = stats_off + 4 + i * 12;
            stats.push(TypeStat {
                type_id: u32_at(&body, o).ok_or(FormatError::Eof("GMS stat"))?,
                count: u32_at(&body, o + 4).ok_or(FormatError::Eof("GMS stat"))?,
                unk: u32_at(&body, o + 8).ok_or(FormatError::Eof("GMS stat"))? as i32,
            });
        }
        Ok(Gms { body, was_compressed, entries, stats })
    }

    /// Header words (first 0x40 bytes) of entry `i`.
    pub fn geom(&self, i: usize) -> Option<GeomHeader> {
        let e = self.entries.get(i)?;
        let a = e.addr as usize;
        let mut w = [0u32; 16];
        for (k, v) in w.iter_mut().enumerate() {
            *v = u32_at(&self.body, a + k * 4)?;
        }
        Some(GeomHeader {
            addr: e.addr,
            name_offset: w[0],
            self_ptr: w[1],
            transform_ptr: w[2],
            field_0c: w[3],
            type_id: w[5],
            control: w[7],
            seq: w[12],
            flags: w[13],
            words: w,
        })
    }

    /// Raw bytes from this object's address up to the next object address (includes the
    /// per-type extension and, probably, the next object's transform pre-block).
    pub fn geom_span(&self, i: usize) -> Option<&[u8]> {
        let a = self.entries.get(i)?.addr;
        let next = self.entries.iter().map(|e| e.addr).filter(|&x| x > a).min().unwrap_or(self.table_offset());
        self.body.get(a as usize..next as usize)
    }

    /// Position stored in the GMS pre-block of entry `i` (cross-checked against the PRP).
    pub fn geom_position(&self, i: usize) -> Option<[f32; 3]> {
        let g = self.geom(i)?;
        let o = g.transform_ptr as usize;
        let f = |k: usize| u32_at(&self.body, o + k * 4).map(f32::from_bits);
        Some([f(0)?, f(1)?, f(2)?])
    }

    pub fn table_offset(&self) -> u32 {
        u32_at(&self.body, 0).unwrap_or(0)
    }

    /// Serialises back to the file container. The body is stored unchanged; with `compress` it is
    /// DEFLATE-compressed (different bytes than the original encoder, but decodes identically).
    pub fn to_bytes(&self, compress: bool) -> Result<Vec<u8>> {
        let payload = if compress {
            let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
            enc.write_all(&self.body).map_err(|e| FormatError::Invalid(e.to_string()))?;
            enc.finish().map_err(|e| FormatError::Invalid(e.to_string()))?
        } else {
            self.body.clone()
        };
        let mut out = Vec::with_capacity(9 + payload.len());
        out.put_u32(self.body.len() as u32);
        out.put_u32((9 + payload.len()) as u32);
        out.put_u8(if compress { 0 } else { 1 });
        out.extend_from_slice(&payload);
        Ok(out)
    }
}
