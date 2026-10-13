//! `.TEX` - texture container of a scene.
//!
//! # File layout (confidence: high, round-trips byte-exactly on every shipped scene)
//!
//! ```text
//! 0x00 u32 table_off     = file_len - 0x4000
//! 0x04 u32 table_off+0x2000   (start of the upper half of the table)
//! 0x08 u32 0
//! 0x0c u32 4             (version?)
//! 0x10 entries ...       each starts on a 16 byte boundary (zero padded), in no particular id order;
//!                        besides the entries listed in the table there are *orphan* cube map
//!                        descriptors that sit right after an image entry and are not in the table
//! table_off: u32 offsets[4096]    indexed by texture id, 0 = unused
//! ```
//!
//! Texture ids start at 128 for ordinary images (materials reference them, see `.MAT` `TXID`
//! records). Ids 2048.. are (also) listed in the table's upper half; they point to *cube map*
//! descriptors.
//!
//! ## Image entry
//!
//! ```text
//! +0x00 u32 total_size           bytes of this entry including this field (before padding)
//! +0x04 [u8;4] format            FourCC stored *byte-reversed*: "1TXD" = DXT1, "3TXD" = DXT3,
//!                                "ABGR" = RGBA, "  8I" = I8, "8V8U" = U8V8, "NLAP" = PALN
//! +0x08 [u8;4] format again      (always identical to the first, purpose unknown)
//! +0x0c u32 id                   the texture id (= table index)
//! +0x10 u16 width, u16 height
//! +0x14 u32 mip_count            1..=10
//! +0x18 u32 unk18                0; 64 for U8V8 normal maps (unknown)
//! +0x1c f32 unk1c                1.0; 0.99215698 (= 253/255) for U8V8 (unknown, scale/bias?)
//! +0x20 u32 unk20                0
//! +0x24 NUL terminated name      e.g. "Locations/M01/WallStone_M01_02" (not unique!)
//! then mip_count x { u32 byte_len, data }   largest mip first
//! then (PALN only) trailer: u32 palette_count, palette_count x BGRA
//! ```
//!
//! Mip levels below 4x4 of a DXT texture still occupy one 4x4 block (8/16 bytes).
//!
//! ## Cube map descriptor
//!
//! `u32 n` (6, 4, 15 and 30 seen) followed by `n` `u32` texture ids; `4 + 4n` bytes. Told apart from
//! an image entry by the second dword: an image has a printable FourCC there, a list has an id.
//!
//! # Pixel formats
//!
//! * DXT1/DXT3/(DXT5 implemented, not seen): standard S3TC.
//! * RGBA: 4 bytes/pixel in the order R, G, B, A (verified by sampling wood/skin/gold textures).
//! * I8: 8-bit grayscale.
//! * U8V8: two bytes (u, v), *biased* unsigned normal-map style (a flat normal is `(127/128, 128)`, checked on the
//!   character normal maps); decoded as `(u, v, 255, 255)`.
//! * PALN: `w*h` palette indices; palette entries are 4 bytes, order **to be verified** (decoded as RGBA).

use crate::io::{invalid, u32_at, Cur, FormatError, PutLe, Result};

pub const TABLE_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexFormat {
    Dxt1,
    Dxt3,
    Dxt5,
    Rgba,
    I8,
    U8V8,
    Pal8,
    Other([u8; 4]),
}

impl TexFormat {
    /// `raw` is the 4 bytes as stored (byte-reversed FourCC).
    pub fn from_raw(raw: [u8; 4]) -> TexFormat {
        match &raw {
            b"1TXD" => TexFormat::Dxt1,
            b"3TXD" => TexFormat::Dxt3,
            b"5TXD" => TexFormat::Dxt5,
            b"ABGR" => TexFormat::Rgba,
            b"  8I" => TexFormat::I8,
            b"8V8U" => TexFormat::U8V8,
            b"NLAP" => TexFormat::Pal8,
            _ => TexFormat::Other(raw),
        }
    }

    pub fn to_raw(self) -> [u8; 4] {
        match self {
            TexFormat::Dxt1 => *b"1TXD",
            TexFormat::Dxt3 => *b"3TXD",
            TexFormat::Dxt5 => *b"5TXD",
            TexFormat::Rgba => *b"ABGR",
            TexFormat::I8 => *b"  8I",
            TexFormat::U8V8 => *b"8V8U",
            TexFormat::Pal8 => *b"NLAP",
            TexFormat::Other(r) => r,
        }
    }

    /// Human readable name ("DXT1", "RGBA" ...).
    pub fn name(self) -> String {
        let mut r = self.to_raw();
        r.reverse();
        String::from_utf8_lossy(&r).trim().to_string()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TexImage {
    pub id: u32,
    pub format: TexFormat,
    /// Second copy of the format code as stored (always equal to the first so far).
    pub format2: [u8; 4],
    pub width: u16,
    pub height: u16,
    pub unk18: u32,
    pub unk1c: f32,
    pub unk20: u32,
    pub name: String,
    /// Raw payload of every mip level, largest first.
    pub mips: Vec<Vec<u8>>,
    /// Bytes after the last mip (PALN: `u32 count` + BGRA palette), otherwise empty.
    pub trailer: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TexEntry {
    Image(TexImage),
    /// Texture id list: `u32 n` followed by `n` texture ids. n = 6 (cube map faces, probably),
    /// 4, 15 or 30 (frame lists?) seen - the purpose of the non-6 variants is unknown.
    Cube { faces: Vec<u32> },
}

#[derive(Clone, Debug)]
pub struct Tex {
    /// Header words 2 and 3 (0 and 4 in shipped files).
    pub header_words: [u32; 2],
    /// Entries in file order.
    pub entries: Vec<TexEntry>,
    /// Table slots: `(table index, index into entries)`, ascending by slot. Ordinary images sit at
    /// slot == their own id; some images and the listed cubes are additionally (or only) reachable
    /// through slots >= 2048 (the exact rule is **unknown**, e.g. image 133 at slot 2180).
    /// Cube descriptors that appear in no slot are "orphans".
    pub slots: Vec<(u32, usize)>,
}

impl Tex {
    pub fn parse(d: &[u8]) -> Result<Tex> {
        let t1 = u32_at(d, 0).ok_or(FormatError::Eof("TEX header"))? as usize;
        let t2 = u32_at(d, 4).ok_or(FormatError::Eof("TEX header"))? as usize;
        if t2 != t1 + 0x2000 || d.len() != t1 + 0x4000 {
            return invalid("TEX table position does not match the file size");
        }
        let header_words = [u32_at(d, 8).unwrap(), u32_at(d, 12).unwrap()];
        let mut table: Vec<(usize, u32)> = Vec::new();
        for i in 0..TABLE_SIZE {
            let off = u32_at(d, t1 + i * 4).unwrap() as usize;
            if off != 0 {
                table.push((off, i as u32));
            }
        }
        table.sort();
        let mut entries: Vec<TexEntry> = Vec::new();
        let mut slots: Vec<(u32, usize)> = Vec::new();
        let mut k = 0;
        while k < table.len() {
            let off = table[k].0;
            let e = parse_entry(d, off)?;
            let mut end = off + parse_len(d, off, &e)?;
            let idx = entries.len();
            entries.push(e);
            // every slot that points at this offset
            while k < table.len() && table[k].0 == off {
                slots.push((table[k].1, idx));
                k += 1;
            }
            // orphan cube descriptors between this entry and the next listed one
            let next = table.get(k).map(|t| t.0).unwrap_or(t1);
            loop {
                let aligned = end.div_ceil(16) * 16;
                if aligned >= next || !is_list_entry(d, aligned) {
                    break;
                }
                let e = parse_entry(d, aligned)?;
                end = aligned + parse_len(d, aligned, &e)?;
                entries.push(e);
            }
        }
        slots.sort();
        Ok(Tex { header_words, entries, slots })
    }

    /// Re-serialises: entries are laid out in `entries` order, 16-byte aligned, starting at 0x10.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut body: Vec<u8> = Vec::new();
        body.put_u32(0);
        body.put_u32(0);
        body.put_u32(self.header_words[0]);
        body.put_u32(self.header_words[1]);
        let mut offsets = Vec::with_capacity(self.entries.len());
        for (n, e) in self.entries.iter().enumerate() {
            // entries start on 16-byte boundaries; the first one directly follows the header
            if n > 0 {
                body.resize(body.len().div_ceil(16) * 16, 0);
            }
            offsets.push(body.len() as u32);
            write_entry(&mut body, e);
        }
        body.resize(body.len().div_ceil(16) * 16, 0);
        let t1 = body.len() as u32;
        body[0..4].copy_from_slice(&t1.to_le_bytes());
        body[4..8].copy_from_slice(&(t1 + 0x2000).to_le_bytes());
        let mut table = vec![0u32; TABLE_SIZE];
        for &(slot, idx) in &self.slots {
            if (slot as usize) < TABLE_SIZE {
                table[slot as usize] = offsets[idx];
            }
        }
        for v in table {
            body.put_u32(v);
        }
        body
    }

    /// Image with the given texture id (the id stored in the entry).
    pub fn image(&self, id: u32) -> Option<&TexImage> {
        self.images().find(|i| i.id == id)
    }

    pub fn images(&self) -> impl Iterator<Item = &TexImage> {
        self.entries.iter().filter_map(|e| if let TexEntry::Image(i) = e { Some(i) } else { None })
    }

    /// Entry reachable through a table slot.
    pub fn entry_at_slot(&self, slot: u32) -> Option<&TexEntry> {
        let i = self.slots.binary_search_by_key(&slot, |s| s.0).ok()?;
        self.entries.get(self.slots[i].1)
    }

    /// All cube descriptors (listed or orphan).
    pub fn cubes(&self) -> impl Iterator<Item = &Vec<u32>> {
        self.entries.iter().filter_map(|e| if let TexEntry::Cube { faces } = e { Some(faces) } else { None })
    }
}

/// True for an id-list entry: small non-zero count and a non-FourCC second dword.
fn is_list_entry(d: &[u8], off: usize) -> bool {
    let (Some(n), Some(second)) = (u32_at(d, off), d.get(off + 4..off + 8)) else { return false };
    n >= 1 && n <= 64 && !second.iter().all(|&b| (0x20..0x7f).contains(&b))
}

fn parse_len(d: &[u8], off: usize, e: &TexEntry) -> Result<usize> {
    Ok(match e {
        TexEntry::Cube { faces } => 4 + 4 * faces.len(),
        TexEntry::Image(_) => u32_at(d, off).ok_or(FormatError::Eof("TEX entry size"))? as usize,
    })
}

fn parse_entry(d: &[u8], off: usize) -> Result<TexEntry> {
    let mut c = Cur::at(d, off);
    let size = c.u32("TEX entry size")?;
    if is_list_entry(d, off) {
        let mut faces = Vec::new();
        for _ in 0..size {
            faces.push(c.u32("cube face")?);
        }
        return Ok(TexEntry::Cube { faces });
    }
    let end = off + size as usize;
    if end > d.len() {
        return invalid(format!("TEX entry at {off:#x} exceeds the file"));
    }
    let fa: [u8; 4] = c.bytes(4, "format")?.try_into().unwrap();
    let fb: [u8; 4] = c.bytes(4, "format2")?.try_into().unwrap();
    let id = c.u32("id")?;
    let height = c.u16("height")?;
    let width = c.u16("width")?;
    let mip_count = c.u32("mip count")? as usize;
    if mip_count == 0 || mip_count > 16 {
        return invalid(format!("TEX entry {id}: {mip_count} mips"));
    }
    let unk18 = c.u32("unk18")?;
    let unk1c = c.f32("unk1c")?;
    let unk20 = c.u32("unk20")?;
    let name = c.cstr("name")?;
    let mut mips = Vec::with_capacity(mip_count);
    for _ in 0..mip_count {
        let n = c.u32("mip size")? as usize;
        mips.push(c.bytes(n, "mip data")?.to_vec());
    }
    if c.p > end {
        return invalid(format!("TEX entry {id} overruns its size"));
    }
    let trailer = d[c.p..end].to_vec();
    Ok(TexEntry::Image(TexImage {
        id,
        format: TexFormat::from_raw(fa),
        format2: fb,
        width,
        height,
        unk18,
        unk1c,
        unk20,
        name,
        mips,
        trailer,
    }))
}

fn write_entry(out: &mut Vec<u8>, e: &TexEntry) {
    match e {
        TexEntry::Cube { faces } => {
            out.put_u32(faces.len() as u32);
            for f in faces {
                out.put_u32(*f);
            }
        }
        TexEntry::Image(i) => {
            let start = out.len();
            out.put_u32(0); // patched below
            out.extend_from_slice(&i.format.to_raw());
            out.extend_from_slice(&i.format2);
            out.put_u32(i.id);
            out.put_u16(i.height);
            out.put_u16(i.width);
            out.put_u32(i.mips.len() as u32);
            out.put_u32(i.unk18);
            out.put_f32(i.unk1c);
            out.put_u32(i.unk20);
            out.put_cstr_latin1(&i.name);
            for m in &i.mips {
                out.put_u32(m.len() as u32);
                out.extend_from_slice(m);
            }
            out.extend_from_slice(&i.trailer);
            let n = (out.len() - start) as u32;
            out[start..start + 4].copy_from_slice(&n.to_le_bytes());
        }
    }
}

impl TexImage {
    pub fn mip_size(&self, level: usize) -> (u32, u32) {
        ((self.width as u32 >> level).max(1), (self.height as u32 >> level).max(1))
    }

    /// Decodes mip `level` to tightly packed RGBA8 (R, G, B, A byte order).
    pub fn decode_rgba(&self, level: usize) -> Result<Vec<u8>> {
        let data = self.mips.get(level).ok_or_else(|| FormatError::Invalid("mip level out of range".into()))?;
        let (w, h) = self.mip_size(level);
        let (w, h) = (w as usize, h as usize);
        match self.format {
            TexFormat::Dxt1 | TexFormat::Dxt3 | TexFormat::Dxt5 => decode_dxt(self.format, data, w, h),
            TexFormat::Rgba => {
                need(data, w * h * 4)?;
                let mut out = Vec::with_capacity(w * h * 4);
                out.extend_from_slice(&data[..w * h * 4]); // already R, G, B, A
                Ok(out)
            }
            TexFormat::I8 => {
                need(data, w * h)?;
                Ok(data[..w * h].iter().flat_map(|&g| [g, g, g, 255]).collect())
            }
            TexFormat::U8V8 => {
                need(data, w * h * 2)?;
                Ok(data[..w * h * 2]
                    .chunks_exact(2)
                    .flat_map(|p| [p[0], p[1], 255, 255])
                    .collect())
            }
            TexFormat::Pal8 => {
                // trailer: u32 count + BGRA palette
                need(data, w * h)?;
                let count = u32_at(&self.trailer, 0).unwrap_or(0) as usize;
                let pal = self.trailer.get(4..4 + count * 4).ok_or(FormatError::Eof("palette"))?;
                let mut out = Vec::with_capacity(w * h * 4);
                for &i in &data[..w * h] {
                    let k = (i as usize).min(count.saturating_sub(1)) * 4;
                    out.extend_from_slice(&pal[k..k + 4]);
                }
                Ok(out)
            }
            TexFormat::Other(r) => Err(FormatError::Unsupported(format!("texture format {r:?}"))),
        }
    }
}

fn need(d: &[u8], n: usize) -> Result<()> {
    if d.len() < n { Err(FormatError::Eof("pixel data")) } else { Ok(()) }
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

fn decode_dxt(fmt: TexFormat, data: &[u8], w: usize, h: usize) -> Result<Vec<u8>> {
    let bw = w.div_ceil(4).max(1);
    let bh = h.div_ceil(4).max(1);
    let bsize = if fmt == TexFormat::Dxt1 { 8 } else { 16 };
    need(data, bw * bh * bsize)?;
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let blk = &data[(by * bw + bx) * bsize..][..bsize];
            let (ablk, cblk) = if fmt == TexFormat::Dxt1 { (&blk[..0], blk) } else { (&blk[..8], &blk[8..]) };
            let c0 = u16::from_le_bytes([cblk[0], cblk[1]]);
            let c1 = u16::from_le_bytes([cblk[2], cblk[3]]);
            let p0 = rgb565(c0);
            let p1 = rgb565(c1);
            let mut pal = [[0u8; 4]; 4];
            pal[0] = [p0[0], p0[1], p0[2], 255];
            pal[1] = [p1[0], p1[1], p1[2], 255];
            if c0 > c1 || fmt != TexFormat::Dxt1 {
                for k in 0..3 {
                    pal[2][k] = ((2 * p0[k] as u32 + p1[k] as u32) / 3) as u8;
                    pal[3][k] = ((p0[k] as u32 + 2 * p1[k] as u32) / 3) as u8;
                }
                pal[2][3] = 255;
                pal[3][3] = 255;
            } else {
                for k in 0..3 {
                    pal[2][k] = ((p0[k] as u32 + p1[k] as u32) / 2) as u8;
                }
                pal[2][3] = 255;
                pal[3] = [0, 0, 0, 0];
            }
            let idx = u32::from_le_bytes([cblk[4], cblk[5], cblk[6], cblk[7]]);
            let alpha = block_alpha(fmt, ablk);
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx * 4 + px, by * 4 + py);
                    if x >= w || y >= h {
                        continue;
                    }
                    let sel = (idx >> (2 * (py * 4 + px))) & 3;
                    let mut p = pal[sel as usize];
                    if fmt != TexFormat::Dxt1 {
                        p[3] = alpha[py * 4 + px];
                    }
                    out[(y * w + x) * 4..][..4].copy_from_slice(&p);
                }
            }
        }
    }
    Ok(out)
}

fn block_alpha(fmt: TexFormat, a: &[u8]) -> [u8; 16] {
    let mut out = [255u8; 16];
    match fmt {
        TexFormat::Dxt3 => {
            for i in 0..16 {
                let nib = (a[i / 2] >> (4 * (i % 2))) & 0xf;
                out[i] = nib * 17;
            }
        }
        TexFormat::Dxt5 => {
            let (a0, a1) = (a[0] as u32, a[1] as u32);
            let mut tab = [0u8; 8];
            tab[0] = a0 as u8;
            tab[1] = a1 as u8;
            if a0 > a1 {
                for k in 1..7u32 {
                    tab[(k + 1) as usize] = (((7 - k) * a0 + k * a1) / 7) as u8;
                }
            } else {
                for k in 1..5u32 {
                    tab[(k + 1) as usize] = (((5 - k) * a0 + k * a1) / 5) as u8;
                }
                tab[6] = 0;
                tab[7] = 255;
            }
            let mut bits = 0u64;
            for (k, &b) in a[2..8].iter().enumerate() {
                bits |= (b as u64) << (8 * k);
            }
            for i in 0..16 {
                out[i] = tab[((bits >> (3 * i)) & 7) as usize];
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxt1_solid_block() {
        // color0 = white (0xffff), color1 = 0, indices all 0
        let data = [0xff, 0xff, 0, 0, 0, 0, 0, 0];
        let out = decode_dxt(TexFormat::Dxt1, &data, 4, 4).unwrap();
        assert!(out.chunks(4).all(|p| p == [255, 255, 255, 255]));
    }

    #[test]
    fn write_parse_roundtrip_synthetic() {
        let img = TexImage {
            id: 128,
            format: TexFormat::Rgba,
            format2: *b"ABGR",
            width: 2,
            height: 1,
            unk18: 0,
            unk1c: 1.0,
            unk20: 0,
            name: "test/tex".into(),
            mips: vec![vec![1, 2, 3, 4, 5, 6, 7, 8]],
            trailer: vec![],
        };
        let t = Tex {
            header_words: [0, 4],
            entries: vec![TexEntry::Image(img), TexEntry::Cube { faces: vec![1, 2, 3, 4, 5, 6] }],
            slots: vec![(128, 0), (2100, 1)],
        };
        let bytes = t.to_bytes();
        let back = Tex::parse(&bytes).unwrap();
        assert_eq!(back.entries, t.entries);
        assert_eq!(back.slots, t.slots);
        assert_eq!(back.to_bytes(), bytes);
        assert_eq!(back.image(128).unwrap().decode_rgba(0).unwrap(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
