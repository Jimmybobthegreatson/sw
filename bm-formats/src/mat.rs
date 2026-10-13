//! `.MAT` - material database of a scene.
//!
//! Only the outer structure and the self-describing *property records* are decoded (confidence in
//! brackets); the class definition blocks are exposed raw.
//!
//! ```text
//! 0x00 u32 class_table_off         [high]  (= instance_table_off - 0x80)
//! 0x04 u32 instance_table_off      [high]
//! 0x08 u32 0
//! 0x0c u32 aux_table_off           [high]  (= instance_table_off + 0x2000, = file_len - 0x2000)
//! 0x10 ... "class" blocks: material class definitions. They consist of 16-byte string slots
//!          (e.g. "Sprites", "Standard", "Sprite.fx", "AmbientSprites") and 16-byte property
//!          descriptor records (see below). Class k starts at class_table[k] (k=0: 0x10). [medium]
//! ...      material instances (0x30 bytes up to several hundred bytes), referenced from the table
//! class_table_off:     u32[32]  offsets of the class blocks, 0 = unused
//! instance_table_off:  u32[2048] offset of the instance with that id (0 = none); ids start at 1
//! aux_table_off:       u32[2048] one small value per id (50, 1, 24, 47 ...), meaning unknown
//! ```
//!
//! ## Instance (medium)
//!
//! ```text
//! +0x00 u32 0x620 / 0x630 (size or version tag?)   +0x04 u32 class index (2, 6, 8 ...)
//! +0x08 u32 0                                       +0x0c u32 small id (0x34, 0x10, 0x11 ...)
//! +0x10 u32 8                                       +0x1c u32 self pointer (instance_off - 0x10)
//! +0x30 NUL-terminated material name, e.g. "Locations/M01/Landscape_M01_Jungle_01_Opac",
//!       "_Glacier/StaticShadows/Additive", "ScatterMaterial"
//! then 16-byte property records and the strings / values they point at
//! ```
//!
//! ## Property record (high for the layout, medium for the semantics)
//!
//! `[u8;4] tag (byte-reversed: stored "DIXT" means TXID)`, `u32 value`, `u32 len`, `u32 kind`
//!
//! | kind | meaning |
//! |---|---|
//! | 0 | `len == 1`: `value` is an `f32` (e.g. `OPAC`); `len > 1`: `value` is the file offset of `len` `f32` (e.g. `VALU` of `ScatterWidth`) |
//! | 1 | `value` is the **file offset** of a NUL-terminated string of `len` bytes (`NAME`, `BMOD` = blend mode "ADD", "Opaque" ...) |
//! | 2 | `value` is an integer (e.g. `BANE`/`ENAB` flags, `TXID` = **texture id**, `AREF` = alpha ref, `ATST`) |
//! | 3 | `value` is an offset into the class definition area, `len` an element count (`TEXT`, `COLO`, `BOOL`, `VTLF`, `RSTA`, `INST`, `BIND`...) |
//!
//! Known tags: `NAME`, `TYPE`, `PATH` (shader file e.g. `ScatterBillboard.fx`), `IDEN`, `VALI`,
//! `OTYP`, `STYP`, `LAYE`, `SUBC`, `CLAS`, `ENAB`, `TXID`, `BMOD`, `OPAC`, `ATST`, `AREF`, `AFEN`,
//! `CULL`, `ZBIA`, `ZFOF`, `RSTA`, `TEXT`, `COLO`, `BOOL`, `VTLF`, `SCRL`, `BIND`, `INST`, `VALU`, ...
//! (names are taken from the reversed tags; semantics are guesses).
//!
//! Kind-3 records are *structs*: `value` is the file offset of `len` consecutive records (which may
//! themselves be kind 3). Example (verified): `TEXT -> { NAME "mapDiffuse", ENAB 1, TXID 155,
//! TILU "TILED", TILV "TILED" }` - a texture slot. [`MatInstance::props`] expands them.
//!
//! `PRM` objects reference a material by `words[4] >> 16` (see `prm.rs`); texture ids from `TXID`
//! index the `.TEX` file.

use crate::io::{u32_at, FormatError, Result};

pub const CLASS_SLOTS: usize = 32;
pub const ID_SLOTS: usize = 2048;

#[derive(Clone, Debug, PartialEq)]
pub struct MatRecord {
    /// Absolute offset of the record.
    pub offset: u32,
    /// Tag in reading order ("TXID", "NAME" ...).
    pub tag: String,
    pub value: u32,
    pub len: u32,
    pub kind: u32,
}

#[derive(Clone, Debug)]
pub struct MatInstance {
    pub id: u32,
    pub offset: u32,
    /// Bytes until the next instance (or the class table).
    pub len: u32,
    pub header: [u32; 12],
    pub name: String,
    pub records: Vec<MatRecord>,
}

#[derive(Clone, Debug)]
pub struct Mat {
    pub data: Vec<u8>,
    pub class_table: [u32; CLASS_SLOTS],
    pub instances: Vec<MatInstance>,
    /// Raw aux table (2048 entries).
    pub aux: Vec<u32>,
}

impl Mat {
    pub fn parse(d: &[u8]) -> Result<Mat> {
        let h = |i: usize| u32_at(d, i * 4).ok_or(FormatError::Eof("MAT header"));
        let (ct, it, _z, at) = (h(0)? as usize, h(1)? as usize, h(2)?, h(3)? as usize);
        if it != ct + 4 * CLASS_SLOTS || at != it + 4 * ID_SLOTS || d.len() != at + 4 * ID_SLOTS {
            return Err(FormatError::Invalid("MAT table layout does not match".into()));
        }
        let mut class_table = [0u32; CLASS_SLOTS];
        for (i, c) in class_table.iter_mut().enumerate() {
            *c = u32_at(d, ct + i * 4).unwrap();
        }
        let aux: Vec<u32> = (0..ID_SLOTS).map(|i| u32_at(d, at + i * 4).unwrap()).collect();
        let mut offs: Vec<(u32, u32)> = Vec::new();
        for id in 0..ID_SLOTS {
            let o = u32_at(d, it + id * 4).unwrap();
            if o != 0 {
                offs.push((o, id as u32));
            }
        }
        offs.sort();
        let mut instances = Vec::with_capacity(offs.len());
        for (k, &(o, id)) in offs.iter().enumerate() {
            let end = offs.get(k + 1).map(|x| x.0 as usize).unwrap_or(ct);
            if o as usize + 0x30 > end || end > d.len() {
                return Err(FormatError::Invalid(format!("MAT instance {id} out of range")));
            }
            instances.push(parse_instance(d, id, o as usize, end));
        }
        instances.sort_by_key(|i| i.id);
        let mut mat = Mat { data: d.to_vec(), class_table, instances, aux };
        // names: instances without a name string at +0x30 (a record follows the header directly) take the root's NAME
        for i in 0..mat.instances.len() {
            let root_name = match mat.instances[i].root(&mat).and_then(|r| r.find("NAME").cloned()) {
                Some(MatValue::Str(s)) if !s.is_empty() => Some(s),
                _ => None,
            };
            if let Some(n) = root_name {
                mat.instances[i].name = n;
            }
        }
        Ok(mat)
    }

    pub fn instance(&self, id: u32) -> Option<&MatInstance> {
        self.instances.binary_search_by_key(&id, |i| i.id).ok().map(|i| &self.instances[i])
    }

    /// Best-effort class name: the string slot 0x20 bytes before the class block (class 0 = first
    /// slot at 0x10). Works for "Sprites", "StaticShadow", "Standard", "ScatterMaterial",
    /// "WaterSurface"; classes introduced by a `SUBC` list (e.g. class 6, the most used one in M01)
    /// have no plain name here and return `None`.
    pub fn class_name(&self, class: usize) -> Option<String> {
        let off = if class == 0 { 0x10 } else { (*self.class_table.get(class)? as usize).checked_sub(0x20)? };
        let t = self.data.get(off..off + 16)?;
        let n = t.iter().position(|&b| b == 0)?;
        let s = &t[..n];
        if s.len() >= 3 && s.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_') && !s.iter().all(|b| b.is_ascii_uppercase()) {
            Some(String::from_utf8_lossy(s).into_owned())
        } else {
            None
        }
    }

    /// NUL-terminated string at a file offset.
    pub fn string_at(&self, off: u32) -> Option<String> {
        let t = self.data.get(off as usize..)?;
        let n = t.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&t[..n]).into_owned())
    }
}

fn decode_record(mat: &Mat, r: &MatRecord, depth: usize) -> MatProp {
    let words = |n: u32| -> Option<Vec<u32>> {
        (0..n as usize).map(|i| u32_at(&mat.data, r.value as usize + i * 4)).collect()
    };
    let value = match r.kind {
        0 if r.len > 1 && r.len <= 64 => match words(r.len) {
            Some(w) => MatValue::Floats(w.into_iter().map(f32::from_bits).collect()),
            None => MatValue::Float(f32::from_bits(r.value)),
        },
        0 => MatValue::Float(f32::from_bits(r.value)),
        1 => MatValue::Str(mat.string_at(r.value).unwrap_or_default()),
        2 if r.len > 1 && r.len <= 64 => match words(r.len) {
            Some(w) => MatValue::Ints(w),
            None => MatValue::Int(r.value),
        },
        3 if depth < 6 && r.len <= 64 => {
            let mut kids = Vec::new();
            for i in 0..r.len as usize {
                let o = r.value as usize + i * 16;
                let Some(b) = mat.data.get(o..o + 16) else { break };
                let tag = &b[..4];
                let kind = u32::from_le_bytes(b[12..16].try_into().unwrap());
                if !tag.iter().all(|c| c.is_ascii_uppercase()) || kind > 3 {
                    break;
                }
                let mut t = [tag[0], tag[1], tag[2], tag[3]];
                t.reverse();
                let rec = MatRecord {
                    offset: o as u32,
                    tag: String::from_utf8_lossy(&t).into_owned(),
                    value: u32::from_le_bytes(b[4..8].try_into().unwrap()),
                    len: u32::from_le_bytes(b[8..12].try_into().unwrap()),
                    kind,
                };
                kids.push(decode_record(mat, &rec, depth + 1));
            }
            MatValue::Struct(kids)
        }
        _ => MatValue::Int(r.value),
    };
    MatProp { tag: r.tag.clone(), value }
}

fn parse_instance(d: &[u8], id: u32, off: usize, end: usize) -> MatInstance {
    let mut header = [0u32; 12];
    for (k, w) in header.iter_mut().enumerate() {
        *w = u32_at(d, off + k * 4).unwrap_or(0);
    }
    let name = {
        let t = &d[off + 0x30..end];
        let n = t.iter().position(|&b| b == 0).unwrap_or(t.len());
        String::from_utf8_lossy(&t[..n]).into_owned()
    };
    // Records: 16-byte aligned (relative to the instance) slots whose first four bytes are
    // uppercase letters and whose `kind` word is 0..=3.
    let mut records = Vec::new();
    let mut p = off + 0x30;
    while p + 16 <= end {
        let tag = &d[p..p + 4];
        if tag.iter().all(|b| b.is_ascii_uppercase()) {
            let kind = u32_at(d, p + 12).unwrap();
            if kind <= 3 {
                let mut t = [tag[0], tag[1], tag[2], tag[3]];
                t.reverse();
                records.push(MatRecord {
                    offset: p as u32,
                    tag: String::from_utf8_lossy(&t).into_owned(),
                    value: u32_at(d, p + 4).unwrap(),
                    len: u32_at(d, p + 8).unwrap(),
                    kind,
                });
            }
        }
        p += 16;
    }
    MatInstance { id, offset: off as u32, len: (end - off) as u32, header, name, records }
}

/// Decoded property value.
#[derive(Clone, Debug, PartialEq)]
pub enum MatValue {
    Float(f32),
    /// Kind 0 with `len > 1`: `len` floats stored at the record's offset (e.g. a color or `ScatterWidth`).
    Floats(Vec<f32>),
    Int(u32),
    /// Kind 2 with `len > 1`: `len` words stored at the record's offset.
    Ints(Vec<u32>),
    Str(String),
    /// Nested property list (kind 3).
    Struct(Vec<MatProp>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatProp {
    pub tag: String,
    pub value: MatValue,
}

/// One texture slot of a material (`TEXT` struct).
#[derive(Clone, Debug, PartialEq)]
pub struct TextureSlot {
    /// Slot name, e.g. `mapDiffuse`.
    pub slot: String,
    pub texture_id: u32,
    pub enabled: bool,
    /// `TILU` / `TILV` (e.g. "TILED").
    pub tile_u: String,
    pub tile_v: String,
}

impl MatProp {
    pub fn find(&self, tag: &str) -> Option<&MatValue> {
        if let MatValue::Struct(c) = &self.value {
            c.iter().find(|p| p.tag == tag).map(|p| &p.value)
        } else {
            None
        }
    }
}

impl MatInstance {
    /// Header-only instance (0x30 bytes: no name, no records, no root). The world never draws with them; the weapon models reference
    /// them for most of their parts (see `bm_models::material::stub_remap`).
    pub fn is_stub(&self) -> bool {
        self.len == 0x30 && self.records.is_empty()
    }

    /// The instance's root struct: the last `INST` record, `{ NAME "<material name>", BIND { ... } }`
    /// (verified on M01; every instance larger than the 0x30-byte stubs has one).
    pub fn root(&self, mat: &Mat) -> Option<MatProp> {
        let r = self.records.iter().rev().find(|r| r.tag == "INST" && r.kind == 3)?;
        Some(decode_record(mat, r, 0))
    }

    /// The shader/material properties: the children of the root's `BIND` struct (render state,
    /// texture slots, colors, flags ...). Falls back to all raw records when there is no `INST`.
    pub fn props(&self, mat: &Mat) -> Vec<MatProp> {
        if let Some(root) = self.root(mat) {
            if let MatValue::Struct(kids) = root.value {
                for k in kids {
                    if k.tag == "BIND" {
                        if let MatValue::Struct(c) = k.value {
                            return c;
                        }
                    }
                }
            }
        }
        self.records.iter().map(|r| decode_record(mat, r, 0)).collect()
    }

    /// The material's texture slots, in file order.
    pub fn textures(&self, mat: &Mat) -> Vec<TextureSlot> {
        let mut out = Vec::new();
        for p in self.props(mat) {
            if p.tag != "TEXT" {
                continue;
            }
            let get_s = |t: &str| match p.find(t) {
                Some(MatValue::Str(s)) => s.clone(),
                _ => String::new(),
            };
            let id = match p.find("TXID") {
                Some(MatValue::Int(v)) => *v,
                _ => continue,
            };
            out.push(TextureSlot {
                slot: get_s("NAME"),
                texture_id: id,
                enabled: matches!(p.find("ENAB"), Some(MatValue::Int(1))),
                tile_u: get_s("TILU"),
                tile_v: get_s("TILV"),
            });
        }
        out
    }

    /// All records with a tag.
    pub fn records_named<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a MatRecord> + 'a {
        self.records.iter().filter(move |r| r.tag == tag)
    }

    /// `header[1]`: *probably* an index into [`Mat::class_table`] (**unverified**: instance 287
    /// "ScatterMaterial" carries 6 while the class named ScatterMaterial is number 7).
    pub fn class_index(&self) -> usize {
        self.header[1] as usize
    }

    /// Shader file named by the instance's `PATH` record (e.g. `ScatterBillboard.fx`), if present.
    pub fn shader(&self, mat: &Mat) -> Option<String> {
        self.string_prop(mat, "PATH").filter(|s| !s.is_empty())
    }

    /// Texture ids referenced through `TXID` records, in file order.
    pub fn texture_ids(&self) -> Vec<u32> {
        self.records_named("TXID").map(|r| r.value).collect()
    }

    pub fn int_prop(&self, tag: &str) -> Option<u32> {
        self.records_named(tag).find(|r| r.kind == 2).map(|r| r.value)
    }

    pub fn float_prop(&self, tag: &str) -> Option<f32> {
        self.records_named(tag).find(|r| r.kind == 0).map(|r| f32::from_bits(r.value))
    }

    pub fn string_prop(&self, mat: &Mat, tag: &str) -> Option<String> {
        self.records_named(tag).find(|r| r.kind == 1).and_then(|r| mat.string_at(r.value))
    }
}
