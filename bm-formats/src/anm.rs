//! `.ANM` - animation data (skeleton names, animation databases, key data).
//!
//! The animation payload is **not decoded** (that is the job of `bm-models`); this module exposes
//! what could be verified:
//!
//! ```text
//! 0x00 "MNA\0"                   magic ("ANM" reversed)
//! 0x04 u32 size | 0x80000000     file size with bit 31 set
//! 0x08 u32 size                  file size
//! 0x0c ...                       section table / counts, layout differs between small and big files
//! ```
//!
//! ## Small "skeleton" ANM (e.g. `Loader_Sequence`, 1472 bytes) - medium confidence
//!
//! ```text
//! 0x48 u32 bone_count (60)        0x50 u32 pose_count (55)
//! 0x68 bone_count bone names then pose_count pose names, NUL separated ("PELVIS", "SPINE", ...,
//!      "LEFT_HAND_ATTACHER", "RIGHT_HAND_ATTACHER", then facial poses "Neutral", "A_Mouth_Widen" ...)
//! other words (0x594, 0x33c, 0x21c at +0x30, +0x3c, +0x44) point into / near the name pool or
//! are unknown
//! ```
//!
//! ## Big ANM (e.g. M01_main, 4 MB) - low confidence
//!
//! A sequence of per-character *animation database* blocks, each carrying the string
//! `anmcol:animationdatabase#<Name>` (e.g. `M01_Don`, `M01_MovieStarAgent`, `SniperRifle_SakoTRG_01`)
//! twice, preceded by an index list `0, 1, 2, ...` and followed by key data. [`Anm::databases`]
//! lists these markers.

use crate::io::{invalid, u32_at, FormatError, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct AnmDatabase {
    /// Offset of the `anmcol:animationdatabase#...` string.
    pub offset: u32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    pub bones: Vec<String>,
    pub poses: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Anm {
    pub size: u32,
    /// First 26 header words (0x68 bytes).
    pub header_words: Vec<u32>,
    pub skeleton: Option<Skeleton>,
    pub databases: Vec<AnmDatabase>,
    pub raw: Vec<u8>,
}

const DB_MARK: &[u8] = b"anmcol:animationdatabase#";

impl Anm {
    pub fn parse(d: &[u8]) -> Result<Anm> {
        if d.len() < 0x68 || &d[..4] != b"MNA\0" {
            return invalid("not an ANM file");
        }
        let size = u32_at(d, 8).unwrap();
        if u32_at(d, 4) != Some(size | 0x8000_0000) || size as usize != d.len() {
            return invalid("ANM size fields do not match the file length");
        }
        let header_words: Vec<u32> = (0..26).map(|i| u32_at(d, i * 4).unwrap()).collect();
        let skeleton = parse_skeleton(d);
        let mut databases = Vec::new();
        let mut i = 0;
        while let Some(pos) = find(d, DB_MARK, i) {
            let s = pos + DB_MARK.len();
            let e = d[s..].iter().position(|&b| b == 0).map(|n| s + n).ok_or(FormatError::Eof("ANM db name"))?;
            databases.push(AnmDatabase { offset: pos as u32, name: String::from_utf8_lossy(&d[s..e]).into_owned() });
            i = e;
        }
        Ok(Anm { size, header_words, skeleton, databases, raw: d.to_vec() })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.raw.clone()
    }
}

fn find(d: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    d.get(from..)?.windows(pat.len()).position(|w| w == pat).map(|p| p + from)
}

fn parse_skeleton(d: &[u8]) -> Option<Skeleton> {
    let bones = u32_at(d, 0x48)? as usize;
    let poses = u32_at(d, 0x50)? as usize;
    if bones == 0 || bones > 512 || poses > 512 {
        return None;
    }
    let mut p = 0x68;
    let mut read = |n: usize| -> Option<Vec<String>> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let e = d.get(p..)?.iter().position(|&b| b == 0)? + p;
            let s = &d[p..e];
            if s.is_empty() || !s.iter().all(|b| (0x20..0x7f).contains(b)) {
                return None;
            }
            v.push(String::from_utf8_lossy(s).into_owned());
            p = e + 1;
        }
        Some(v)
    };
    let b = read(bones)?;
    let po = read(poses)?;
    Some(Skeleton { bones: b, poses: po })
}
