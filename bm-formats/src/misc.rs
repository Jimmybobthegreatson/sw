//! Tiny / opaque companion files of a scene: `.ZGF`, `.SUP`, `.SGD` (fixed stubs) and the packed
//! spatial files `.RMC` / `.RMI`.
//!
//! All of them are kept as raw bytes (byte-exact round trip) plus a typed view of what is known.
//!
//! # `.ZGF` (17 bytes in every scene, confidence: structure high, meaning unknown)
//! `u32 8, u32 17 (file size), u8 1, "TFGZ" (= "ZGFT" reversed), u32 8`.
//!
//! # `.SUP` (24 bytes in every scene)
//! `u32 0, u32 0x80000018 (size | 0x80000000), u32 24 (size), u32 1, "DLCF" (= "FCLD" reversed), u32 8`.
//!
//! # `.SGD` (8 bytes in every scene)
//! `u32 0xffffffff, u32 0xcdcdcdcd` - an *empty* marker (0xcdcdcdcd = uninitialised MSVC debug fill).
//!
//! # `.RMC` / `.RMI`
//! Moved to [`crate::roommap`] (decoded).

/// Expected constant contents of the three stub files.
pub const ZGF_BYTES: [u8; 17] = [8, 0, 0, 0, 17, 0, 0, 0, 1, b'T', b'F', b'G', b'Z', 8, 0, 0, 0];
pub const SUP_BYTES: [u8; 24] = [0, 0, 0, 0, 0x18, 0, 0, 0x80, 0x18, 0, 0, 0, 1, 0, 0, 0, b'D', b'L', b'C', b'F', 8, 0, 0, 0];
pub const SGD_EMPTY: [u8; 8] = [0xff, 0xff, 0xff, 0xff, 0xcd, 0xcd, 0xcd, 0xcd];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StubFile {
    pub data: Vec<u8>,
}

impl StubFile {
    pub fn is_zgf(&self) -> bool {
        self.data == ZGF_BYTES
    }
    pub fn is_sup(&self) -> bool {
        self.data == SUP_BYTES
    }
    pub fn is_sgd_empty(&self) -> bool {
        self.data == SGD_EMPTY
    }
}

pub use crate::roommap::{RoomMap, RoomMapHeader};
