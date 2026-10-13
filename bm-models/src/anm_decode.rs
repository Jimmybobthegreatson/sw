//! `.ANM` key data decoder (original implementation, see `docs/anim.md` for the reverse-engineering notes).
//!
//! A big `.ANM` (`M01_main.ANM`, 4 MB) is laid out as
//!
//! ```text
//! header + animation databases + (id, offset) tables        (not needed to play clips)
//! clip headers      N x 64 bytes, immediately followed by ...
//! key data pool     bit packed streams, up to the start of the bone name list
//! bone name list    "PELVIS", "SPINE", ... (ids used by the streams), then N clip names
//!                   ("anim:<Set>/<Name>#<Clip>" or "extra" for aliases), then the facial pose names
//! ```
//!
//! Every clip header holds the frame count / frame rate, the data size, and the offsets (relative to the
//! pool start) of up to four streams: **A** = the 74 "rig parameters" (10 quaternions + 64 scalars, solved to
//! bone transforms by [`crate::rig`]), **B** = extra bones (hand attachers, camera, weapon parts) as
//! quaternion + translation, **C** = the root (`GROUND`) motion, **D** = unused scalars.
//!
//! A stream stores frames in blocks of 8. Per channel (a "channel" is a quaternion, a scalar, or a quaternion
//! with three translation scalars) a static header gives the value range; the per-block data is delta coded
//! with an explicit end sample, and quaternions use the "smallest three" encoding.

/// Errors while decoding a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnmError {
    /// Unknown channel type nibble.
    BadType(u8),
    /// Stream offsets point outside the file.
    OutOfRange,
}

impl std::fmt::Display for AnmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnmError::BadType(t) => write!(f, "unknown ANM channel type {t}"),
            AnmError::OutOfRange => write!(f, "ANM stream out of range"),
        }
    }
}

impl std::error::Error for AnmError {}

/// Per channel type: `(offset, range, scalar count, has quaternion)`. Types 0..=8 exist in shipped data.
const TYPES: [(f32, f32, usize, bool); 9] = [
    (-std::f32::consts::PI, std::f32::consts::TAU, 1, false), // 0: angle
    (-1.0, 2.0, 1, false),                                    // 1: unit scalar
    (0.0, 0.0, 1, false),                                     // 2: scalar with explicit 16 bit range
    (0.0, 1.0, 0, true),                                      // 3: quaternion
    (-10.0, 20.0, 3, true),                                   // 4: quaternion + translation (+-10)
    (-100.0, 200.0, 3, true),                                 // 5: +-100
    (-1000.0, 2000.0, 3, true),                               // 6: +-1000
    (-10000.0, 20000.0, 3, true),                             // 7
    (-100000.0, 200000.0, 3, true),                           // 8
];

/// One decoded channel over all frames of a clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    /// Channel id (rig parameter index for stream A, bone-name index of the ANM bone list for B/C/D).
    pub id: u16,
    pub ty: u8,
    /// One unit quaternion `[x,y,z,w]` per frame (empty for scalar channels).
    pub quats: Vec<[f32; 4]>,
    /// Scalars (1 for types 0..2, 3 translation components for types 4..8), each with one value per frame.
    pub scalars: Vec<Vec<f32>>,
}

struct Bits<'a>(&'a [u8]);

impl Bits<'_> {
    /// Little endian bit reader (absolute bit position, up to 24 bits).
    #[inline]
    fn get(&self, pos: usize, n: u32) -> u32 {
        let b = pos >> 3;
        let mut v = 0u32;
        for k in 0..4 {
            v |= (*self.0.get(b + k).unwrap_or(&0) as u32) << (8 * k);
        }
        (v >> (pos & 7)) & ((1u32 << n) - 1)
    }
    #[inline]
    fn u16_at(&self, byte: usize) -> usize {
        *self.0.get(byte).unwrap_or(&0) as usize | (*self.0.get(byte + 1).unwrap_or(&0) as usize) << 8
    }
}

#[inline]
fn sx(v: u32, n: u32) -> i32 {
    if n == 0 {
        return 0;
    }
    let v = v as i32;
    if v & (1 << (n - 1)) != 0 { v - (1 << n) } else { v }
}

/// Decodes one stream (`data` starts at the first byte of the stream) into per frame channel values.
pub fn decode_stream(data: &[u8], nframes: usize) -> Result<Vec<Channel>, AnmError> {
    let bits = Bits(data);
    if data.is_empty() || nframes == 0 {
        return Ok(Vec::new());
    }
    let n = data[0] as usize;
    if data.len() < 1 + 3 * n {
        return Err(AnmError::OutOfRange);
    }
    let ids: Vec<u16> = (0..n).map(|k| bits.u16_at(1 + 2 * k) as u16).collect();
    let desc: Vec<u8> = (0..n).map(|k| data[1 + 2 * n + k]).collect();
    for &d in &desc {
        if (d & 15) as usize >= TYPES.len() {
            return Err(AnmError::BadType(d & 15));
        }
    }
    let mut bp = 8 * (3 * n + 1);
    if bp & 15 != 0 {
        bp += 16 - (bp & 15);
    }
    let p = bp / 8;
    let w2 = bits.u16_at(p + 2);
    let w4 = bits.u16_at(p + 4);
    let s1 = p * 8 + 0x30;
    let table = s1 + w2 * 16;
    let nblocks = nframes.div_ceil(8);

    let mut chans: Vec<Channel> = (0..n)
        .map(|k| {
            let (_, _, ns, _) = TYPES[(desc[k] & 15) as usize];
            Channel { id: ids[k], ty: desc[k] & 15, quats: Vec::with_capacity(nframes), scalars: vec![Vec::with_capacity(nframes); ns] }
        })
        .collect();

    for blk in 0..nblocks {
        let t0 = bits.u16_at(table / 8 + blk * 2);
        let t1 = bits.u16_at(table / 8 + blk * 2 + 2);
        let last = blk == nblocks - 1;
        let atab = table + nblocks * 16 + 0x10;
        let nf = if last { nframes - blk * 8 } else { 8 };
        let cnt = if nf == 8 { 9 } else { nf };
        let mut pa = atab + t0 * 16;
        let mut pb = atab + if last { t0 * 16 } else { t1 * 16 };
        let mut pc = atab + (t0 * 2 + w4) * 8;
        let mut esi = s1;

        for k in 0..n {
            let t = (desc[k] & 15) as usize;
            let nb = (desc[k] >> 4) as u32;
            let (mut off, mut rng, ns, hq) = TYPES[t];
            let mut qb = nb;
            if t == 2 {
                let a = sx(bits.get(esi, 16), 16);
                esi += 16;
                let b = sx(bits.get(esi, 16), 16);
                esi += 16;
                off = a as f32;
                rng = (b - a) as f32;
            } else if t >= 4 {
                qb = 12;
            }
            if hq {
                let v = bits.get(esi, 4);
                esi += 4;
                let flag = v & 1 != 0;
                let sel0 = (v >> 1) as u8;
                let mut sel = [0u8; 9];
                if flag {
                    sel = [sel0; 9];
                } else {
                    sel[0] = bits.get(pa, 3) as u8;
                    pa += 3;
                    sel[8] = bits.get(pb, 3) as u8;
                    pb += 3;
                    for j in 0..nf.saturating_sub(1) {
                        sel[1 + j] = bits.get(pc, 3) as u8;
                        pc += 3;
                    }
                }
                let mut comps = [[0i32; 3]; 9];
                for c in 0..3 {
                    let bb = qb + 1;
                    let v = bits.get(esi, bb);
                    esi += bb as usize;
                    let fl = v & 1 != 0;
                    let q0 = (v >> 1) as i32;
                    if fl {
                        for comp in comps.iter_mut() {
                            comp[c] = q0;
                        }
                    } else {
                        let bw = bits.get(esi, 4) + 1;
                        esi += 4;
                        let v1 = bits.get(pa, bw) as i32;
                        pa += bw as usize;
                        let v2 = bits.get(pb, bw) as i32;
                        pb += bw as usize;
                        comps[0][c] = v1 + q0;
                        comps[8][c] = v2 + q0;
                        let w = bits.get(pc, 4);
                        pc += 4;
                        for j in 1..nf {
                            let dd = if w > 0 { sx(bits.get(pc, w), w) } else { 0 };
                            pc += w as usize;
                            comps[j][c] = sx(((dd + comps[j - 1][c]) & 0xffff) as u32, 16);
                        }
                    }
                }
                let scale = 1.42 / (1u32 << qb) as f32;
                for j in 0..cnt {
                    let s = sel[j] as usize;
                    let kk = s >> 1;
                    let slots = [if kk == 0 { 1 } else { 0 }, if kk <= 1 { 2 } else { 1 }, if kk <= 2 { 3 } else { 2 }];
                    let mut q = [0f32; 4];
                    for c in 0..3 {
                        q[slots[c]] = comps[j][c] as f32 * scale - 0.71;
                    }
                    let ss = 1.0 - q.iter().map(|x| x * x).sum::<f32>();
                    if ss > 0.0 {
                        q[kk] = ss.sqrt() * if s & 1 != 0 { 1.0 } else { -1.0 };
                    } else {
                        q = [0.0, 0.0, 0.0, 1.0];
                    }
                    if j < nf {
                        chans[k].quats.push(q);
                    }
                }
            }
            if ns > 0 {
                let step = if nb >= 2 { 1.0 / ((1u32 << nb) - 1) as f32 } else { 1.0 } * rng;
                let bb = nb + 1;
                for sc in 0..ns {
                    let v = bits.get(esi, bb);
                    esi += bb as usize;
                    let fl = v & 1 != 0;
                    let q0 = (v >> 1) as i32;
                    if fl {
                        let val = q0 as f32 * step + off;
                        for _ in 0..nf {
                            chans[k].scalars[sc].push(val);
                        }
                    } else {
                        let bw = bits.get(esi, 4) + 1;
                        esi += 4;
                        let v1 = bits.get(pa, bw) as i32;
                        pa += bw as usize;
                        let v2 = bits.get(pb, bw) as i32;
                        pb += bw as usize;
                        let mut s = [0i32; 9];
                        s[0] = v1 + q0;
                        s[8] = v2 + q0;
                        let w = bits.get(pc, 4);
                        pc += 4;
                        for j in 1..nf {
                            let dd = if w > 0 { sx(bits.get(pc, w), w) } else { 0 };
                            pc += w as usize;
                            s[j] = dd + s[j - 1];
                        }
                        for j in 0..nf {
                            chans[k].scalars[sc].push(s[j] as f32 * step + off);
                        }
                    }
                }
            }
        }
    }
    Ok(chans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_extension() {
        assert_eq!(sx(0xffff, 16), -1);
        assert_eq!(sx(0x7fff, 16), 32767);
        assert_eq!(sx(0b100, 3), -4);
        assert_eq!(sx(5, 0), 0);
    }

    #[test]
    fn bit_reader_is_little_endian() {
        let b = Bits(&[0b1010_1010, 0b0000_1111, 0xff]);
        assert_eq!(b.get(0, 4), 0b1010);
        assert_eq!(b.get(4, 8), 0b1111_1010);
        assert_eq!(b.get(20, 8), 0x0f); // upper byte reads as zero past the end
    }

    #[test]
    fn empty_and_bad_streams() {
        assert!(decode_stream(&[], 5).unwrap().is_empty());
        // one channel of unknown type 15
        let d = [1u8, 0, 0, 0x0f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(decode_stream(&d, 3), Err(AnmError::BadType(15)));
    }
}
