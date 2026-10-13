//! `.OCT` - the "physics octree": a list of small packed bounding-volume trees over the scene's static objects.
//!
//! Layout (verified on all 68 scenes, see `docs/formats.md`):
//!
//! ```text
//! u32 1, 1, 1, 1                                   file header (16 bytes)
//! blocks, each starting on a 16 byte boundary (zero fill between blocks):
//!   kind A (world / room trees, scale ~0.05 .. 32):
//!     u32 size, f32 scale, f32 origin[3], u32 0, 0, 0          outer header (0x20 bytes)
//!   kind B (per object trees in metres, scale 3200 and similar):
//!     u32 size, u32 0x00ff4f1c, f32 origin[3], f32 scale, u32 runtime pointer (stale), u32 0
//!   inner header (shared with .RMC/.RMI):  u32 leaf_off, f32 origin[3], f32 scale        (0x14 bytes)
//!   [u16; 3] tree nodes, 0xCD fill, then leaves of 16 bytes: u32 id, u16 min[3], u16 max[3]
//! tail: u32 0xffffffff, u32 0, then (u32 a, u32 b) pairs
//! ```
//!
//! The quantised leaf boxes are decoded ([`crate::roommap::Grid`]); the meaning of the leaf ids and of the tail pairs is
//! unknown (they look like object / geometry indices; the tail pairs map ids to 32-byte steps). The tree nodes are exposed
//! as raw `[u16; 3]` triples.
//!
//! The empty placeholder of tiny scenes is 100 bytes (`1,1,1,1`, zeros, `0xffffffff`); two-leaf files of 180 bytes exist.

use crate::io::{u32_at, FormatError, Result};
use crate::roommap::{read_tree, Grid, Leaf};

#[derive(Clone, Debug, PartialEq)]
pub struct OctHeader {
    pub version_words: [u32; 4],
    /// First word of the first block (size of that block) or 0 for placeholders.
    pub word4: u32,
    /// Grid scale of the first block (0.08 in M01).
    pub scale: f32,
    /// Grid origin of the first block.
    pub origin: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// Outer header `size, scale, origin, 0, 0, 0`.
    A,
    /// Outer header `size, 0x00ff4f1c, origin, scale, pointer, 0` (`size` counts the outer header).
    B,
}

#[derive(Clone, Debug)]
pub struct OctBlock {
    pub kind: BlockKind,
    /// Byte offset of the block in the file.
    pub offset: usize,
    pub grid: Grid,
    pub tree: Vec<[u16; 3]>,
    pub leaves: Vec<Leaf>,
}

#[derive(Clone, Debug)]
pub struct Oct {
    pub header: OctHeader,
    pub blocks: Vec<OctBlock>,
    /// `(a, b)` word pairs after the `0xffffffff, 0` marker (unknown meaning).
    pub tail: Vec<(u32, u32)>,
    /// Every byte of the file.
    pub raw: Vec<u8>,
}

const B_FLAGS: u32 = 0x00ff_4f1c;

fn f(d: &[u8], o: usize) -> Option<f32> {
    u32_at(d, o).map(f32::from_bits)
}

fn parse_block(d: &[u8], p: usize) -> Option<(OctBlock, usize)> {
    let size = u32_at(d, p)? as usize;
    let w1 = u32_at(d, p + 4)?;
    let w1f = f32::from_bits(w1);
    let (kind, grid, end) = if w1 == B_FLAGS {
        let origin = [f(d, p + 8)?, f(d, p + 12)?, f(d, p + 16)?];
        let scale = f(d, p + 20)?;
        (BlockKind::B, Grid { origin, scale }, p + size)
    } else if (1.0e-3..1.0e4).contains(&w1f) {
        let origin = [f(d, p + 8)?, f(d, p + 12)?, f(d, p + 16)?];
        (BlockKind::A, Grid { origin, scale: w1f }, p + size + 0x20)
    } else {
        return None;
    };
    let inner = p + 0x20;
    let leaf_off = inner + u32_at(d, inner)? as usize;
    if end > d.len() || leaf_off > end || !(1.0e-3..1.0e6).contains(&grid.scale) {
        return None;
    }
    let mut tree_end = leaf_off;
    while tree_end > inner + 0x14 && d[tree_end - 1] == 0xcd {
        tree_end -= 1;
    }
    let tree_end = inner + 0x14 + (tree_end - inner - 0x14) / 6 * 6;
    let tree = read_tree(d, inner + 0x14, tree_end);
    let mut leaves = Vec::new();
    let mut q = leaf_off;
    while q + 16 <= end {
        leaves.push(Leaf::parse(d, q)?);
        q += 16;
    }
    Some((OctBlock { kind, offset: p, grid, tree, leaves }, end))
}

impl Oct {
    pub fn parse(d: &[u8]) -> Result<Oct> {
        let w = |i: usize| u32_at(d, i * 4).ok_or(FormatError::Eof("OCT header"));
        let version_words = [w(0)?, w(1)?, w(2)?, w(3)?];
        let mut header = OctHeader { version_words, word4: 0, scale: 0.0, origin: [0.0; 3] };
        let mut blocks: Vec<OctBlock> = Vec::new();
        let mut p = 0x10;
        let mut tail = Vec::new();
        while p + 8 <= d.len() {
            // zero fill
            while p + 16 <= d.len() && d[p..p + 16].iter().all(|&b| b == 0) {
                p += 16;
            }
            if p + 8 > d.len() {
                break;
            }
            if u32_at(d, p) == Some(0xffff_ffff) {
                // tail marker (+ pairs)
                let mut q = p + 8;
                while q + 8 <= d.len() {
                    tail.push((u32_at(d, q).unwrap(), u32_at(d, q + 4).unwrap()));
                    q += 8;
                }
                p = d.len();
                break;
            }
            match parse_block(d, p) {
                Some((b, end)) => {
                    blocks.push(b);
                    p = end;
                }
                None => {
                    return Err(FormatError::Invalid(format!("OCT: cannot parse block at {p:#x}")));
                }
            }
        }
        let _ = p;
        if let Some(b) = blocks.first() {
            header.word4 = u32_at(d, b.offset).unwrap_or(0);
            header.scale = b.grid.scale;
            header.origin = b.grid.origin;
        }
        Ok(Oct { header, blocks, tail, raw: d.to_vec() })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.raw.clone()
    }

    /// True for the 100-byte placeholder used by scenes without static geometry.
    pub fn is_empty_placeholder(&self) -> bool {
        self.raw.len() == 100
    }

    /// Total number of leaves over all blocks.
    pub fn leaf_count(&self) -> usize {
        self.blocks.iter().map(|b| b.leaves.len()).sum()
    }

    /// World boxes of every leaf of the blocks of kind `kind`.
    pub fn leaf_boxes(&self, kind: BlockKind) -> impl Iterator<Item = (u32, [f32; 3], [f32; 3])> + '_ {
        self.blocks.iter().filter(move |b| b.kind == kind).flat_map(|b| b.leaves.iter().map(move |l| {
            let (lo, hi) = b.grid.world_box(&l.qbox);
            (l.target, lo, hi)
        }))
    }
}
