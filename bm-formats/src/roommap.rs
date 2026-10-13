//! `.RMC` / `.RMI` - room volume maps ("Room Collision" / "Room Inside" trees), plus the packed-tree pieces
//! shared with [`crate::oct`].
//!
//! Layout (all little endian, verified on every shipped scene; see `docs/formats.md`):
//!
//! ```text
//! u32  leaf_off          byte offset of the leaf list (the tree ends where the 0xCD padding starts)
//! f32  origin[3]         centre of the quantisation grid (world units)
//! f32  scale             quantisation steps per world unit (world = origin + (q - 32768) / scale)
//! [u16; 3] x N          tree nodes (6 bytes each; bit 15 of word 0 = leaf flag, see `docs/formats.md`)
//! 0xCD padding up to leaf_off
//! leaf list, 16 bytes each:  u32 record_offset, u16 min[3], u16 max[3]   (quantised axis aligned box)
//! records, 84 bytes each (file order differs from leaf order):
//!     u32 axis_aligned (1 = `half` is given along the world axes)   f32 rotation[9] (stored in reversed axis order)
//!     f32 centre[3]  (world centre of the box)       f32 pivot[3] (node origin of the `InsideBound` zone, 0 otherwise)
//!     f32 half_extent[3] (along the local axes)      u32 id (same for every box of one room)    u32 link
//! ```
//!
//! `.RMI` ("inside") boxes are exactly the `InsideBound*` `ZBOUND` volumes of the scene graph (position + PRM bounding
//! box, one per room volume) plus a few extra helper boxes; `.RMC` ("collision") has a slightly larger set of boxes
//! (mostly the same volumes grown to include floors / ceilings).

use crate::io::{invalid, u32_at, FormatError, PutLe, Result};

pub const RECORD_SIZE: usize = 84;
pub const LEAF_SIZE: usize = 16;
const HEADER_SIZE: usize = 20;

/// Quantised axis aligned box (`u16` grid coordinates, see [`RoomMapHeader::dequantize`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QBox {
    pub min: [u16; 3],
    pub max: [u16; 3],
}

/// Leaf of a packed tree: a payload word (record byte offset in RMC/RMI, object id in `.OCT`) and its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub target: u32,
    pub qbox: QBox,
}

impl Leaf {
    pub fn parse(d: &[u8], p: usize) -> Option<Leaf> {
        let target = u32_at(d, p)?;
        let mut v = [0u16; 6];
        for (i, x) in v.iter_mut().enumerate() {
            *x = u16::from_le_bytes(d.get(p + 4 + i * 2..p + 6 + i * 2)?.try_into().ok()?);
        }
        Some(Leaf { target, qbox: QBox { min: [v[0], v[1], v[2]], max: [v[3], v[4], v[5]] } })
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.put_u32(self.target);
        for v in self.qbox.min.iter().chain(self.qbox.max.iter()) {
            out.put_u16(*v);
        }
    }
}

/// Origin/scale of a packed tree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub origin: [f32; 3],
    pub scale: f32,
}

impl Grid {
    /// World coordinate of a quantised value on `axis`.
    pub fn dequantize(&self, axis: usize, q: u16) -> f32 {
        self.origin[axis] + (q as f32 - 32768.0) / self.scale
    }

    pub fn world_box(&self, b: &QBox) -> ([f32; 3], [f32; 3]) {
        let mut lo = [0.0; 3];
        let mut hi = [0.0; 3];
        for a in 0..3 {
            lo[a] = self.dequantize(a, b.min[a]);
            hi[a] = self.dequantize(a, b.max[a]);
        }
        (lo, hi)
    }

    /// Resolution of one grid step in world units.
    pub fn step(&self) -> f32 {
        1.0 / self.scale
    }
}

/// Reads the 6-byte tree nodes in `d[from..to]`.
pub fn read_tree(d: &[u8], from: usize, to: usize) -> Vec<[u16; 3]> {
    let mut v = Vec::new();
    let mut p = from;
    while p + 6 <= to {
        let w = |o: usize| u16::from_le_bytes([d[p + o], d[p + o + 1]]);
        v.push([w(0), w(2), w(4)]);
        p += 6;
    }
    v
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoomMapHeader {
    /// Byte offset of the leaf list.
    pub leaf_off: u32,
    pub origin: [f32; 3],
    pub scale: f32,
}

impl RoomMapHeader {
    pub fn grid(&self) -> Grid {
        Grid { origin: self.origin, scale: self.scale }
    }
}

/// One 84-byte box record.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomRecord {
    /// 1 when the rotation is the identity (axis aligned box), else 0.
    pub axis_aligned: u32,
    /// Stored rotation (reversed axis order: row 0 = image of Z, row 1 = Y, row 2 = X; see `docs/models.md`).
    pub rotation: [f32; 9],
    /// World centre of the box.
    pub center: [f32; 3],
    /// Node origin of the `InsideBound` zone the box was made from (zero for helper boxes).
    pub pivot: [f32; 3],
    /// Half extents along the local X, Y, Z axes.
    pub half: [f32; 3],
    /// Identifier shared by all boxes of one room (not a node index; see `docs/formats.md`).
    pub id: u32,
    pub link: u32,
}

impl RoomRecord {
    pub fn parse(d: &[u8], p: usize) -> Option<RoomRecord> {
        let w = |i: usize| u32_at(d, p + i * 4);
        let f = |i: usize| w(i).map(f32::from_bits);
        let mut rotation = [0.0; 9];
        for (i, r) in rotation.iter_mut().enumerate() {
            *r = f(1 + i)?;
        }
        Some(RoomRecord {
            axis_aligned: w(0)?,
            rotation,
            center: [f(10)?, f(11)?, f(12)?],
            pivot: [f(13)?, f(14)?, f(15)?],
            half: [f(16)?, f(17)?, f(18)?],
            id: w(19)?,
            link: w(20)?,
        })
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.put_u32(self.axis_aligned);
        for v in self.rotation.iter().chain(&self.center).chain(&self.pivot).chain(&self.half) {
            out.put_f32(*v);
        }
        out.put_u32(self.id);
        out.put_u32(self.link);
    }

    /// World direction of local axis `j` (0 = X, 1 = Y, 2 = Z): stored row `2 - j`.
    pub fn axis(&self, j: usize) -> [f32; 3] {
        let r = (2 - j) * 3;
        [self.rotation[r], self.rotation[r + 1], self.rotation[r + 2]]
    }

    /// True when `half` is already expressed along the world axes (quarter turns of the source `InsideBound` are
    /// pre-applied by the exporter and `rotation` is informational); false for arbitrary angles (oriented box).
    pub fn is_world_aligned(&self) -> bool {
        self.axis_aligned == 1
    }

    /// Coordinates of world point `p` in the box frame (world axes for axis aligned records).
    pub fn to_local(&self, p: [f32; 3]) -> [f32; 3] {
        let d = [p[0] - self.center[0], p[1] - self.center[1], p[2] - self.center[2]];
        if self.is_world_aligned() {
            return d;
        }
        let mut l = [0.0; 3];
        for (j, out) in l.iter_mut().enumerate() {
            let a = self.axis(j);
            *out = a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
        }
        l
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        let l = self.to_local(p);
        (0..3).all(|k| l[k].abs() <= self.half[k])
    }

    pub fn volume(&self) -> f32 {
        8.0 * self.half[0] * self.half[1] * self.half[2]
    }

    /// World axis aligned bounds of the oriented box.
    pub fn world_aabb(&self) -> ([f32; 3], [f32; 3]) {
        let mut ext = [0.0f32; 3];
        if self.is_world_aligned() {
            ext = self.half;
        } else {
            for j in 0..3 {
                let a = self.axis(j);
                for k in 0..3 {
                    ext[k] += a[k].abs() * self.half[j];
                }
            }
        }
        ([self.center[0] - ext[0], self.center[1] - ext[1], self.center[2] - ext[2]], [self.center[0] + ext[0], self.center[1] + ext[1], self.center[2] + ext[2]])
    }
}

/// A parsed `.RMC` / `.RMI` file.
#[derive(Clone, Debug)]
pub struct RoomMap {
    pub header: RoomMapHeader,
    /// Tree nodes (`[u16; 3]`), bit 15 of word 0 marks leaf style nodes; semantics partly unknown.
    pub tree: Vec<[u16; 3]>,
    /// Bytes between the last tree node and the leaf list (0xCD fill).
    pub padding: Vec<u8>,
    pub leaves: Vec<Leaf>,
    /// Records in file order; `leaves[i].target` is the byte offset of its record.
    pub records: Vec<RoomRecord>,
    /// Byte offset of the first record.
    pub records_off: u32,
    raw: Vec<u8>,
}

impl RoomMap {
    pub fn parse(d: &[u8]) -> Result<RoomMap> {
        let w = |i: usize| u32_at(d, i * 4).ok_or(FormatError::Eof("RMC/RMI header"));
        let header = RoomMapHeader {
            leaf_off: w(0)?,
            origin: [f32::from_bits(w(1)?), f32::from_bits(w(2)?), f32::from_bits(w(3)?)],
            scale: f32::from_bits(w(4)?),
        };
        let leaf_off = header.leaf_off as usize;
        if leaf_off < HEADER_SIZE || leaf_off > d.len() || !(1.0e-4..1.0e5).contains(&header.scale) {
            return invalid(format!("RMC/RMI: bad header (leaf offset {leaf_off:#x}, scale {})", header.scale));
        }
        // leaves until the first record
        let mut leaves = Vec::new();
        let mut first_rec = d.len();
        let mut p = leaf_off;
        while p + LEAF_SIZE <= d.len() && p < first_rec {
            let l = Leaf::parse(d, p).ok_or(FormatError::Eof("RMC/RMI leaf"))?;
            first_rec = first_rec.min(l.target as usize);
            leaves.push(l);
            p += LEAF_SIZE;
        }
        if p != first_rec && !leaves.is_empty() {
            return invalid(format!("RMC/RMI: leaf list ends at {p:#x} but first record is at {first_rec:#x}"));
        }
        let n_rec = if leaves.is_empty() { 0 } else { (d.len() - first_rec) / RECORD_SIZE };
        if n_rec != leaves.len() || (d.len() - first_rec) % RECORD_SIZE != 0 {
            return invalid(format!("RMC/RMI: {} leaves but {n_rec} records", leaves.len()));
        }
        let mut records = Vec::with_capacity(n_rec);
        for i in 0..n_rec {
            records.push(RoomRecord::parse(d, first_rec + i * RECORD_SIZE).ok_or(FormatError::Eof("RMC/RMI record"))?);
        }
        for l in &leaves {
            let o = l.target as usize;
            if o < first_rec || (o - first_rec) % RECORD_SIZE != 0 || (o - first_rec) / RECORD_SIZE >= n_rec {
                return invalid(format!("RMC/RMI: leaf points to {o:#x}, not a record"));
            }
        }
        // tree: strip the 0xCD fill
        let mut tree_end = leaf_off;
        while tree_end > HEADER_SIZE && d[tree_end - 1] == 0xcd {
            tree_end -= 1;
        }
        let tree_end = HEADER_SIZE + (tree_end - HEADER_SIZE) / 6 * 6;
        let tree = read_tree(d, HEADER_SIZE, tree_end);
        let padding = d[tree_end..leaf_off].to_vec();
        Ok(RoomMap { header, tree, padding, leaves, records, records_off: first_rec as u32, raw: d.to_vec() })
    }

    /// Re-serialises the parsed structure (byte identical to the input for every shipped file).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.raw.len());
        o.put_u32(self.header.leaf_off);
        for v in self.header.origin {
            o.put_f32(v);
        }
        o.put_f32(self.header.scale);
        for n in &self.tree {
            for w in n {
                o.put_u16(*w);
            }
        }
        o.extend_from_slice(&self.padding);
        for l in &self.leaves {
            l.write(&mut o);
        }
        for r in &self.records {
            r.write(&mut o);
        }
        o
    }

    pub fn grid(&self) -> Grid {
        self.header.grid()
    }

    /// Record of leaf `i` (leaf order = order of the quantised box list).
    pub fn leaf_record(&self, i: usize) -> Option<&RoomRecord> {
        let l = self.leaves.get(i)?;
        self.records.get((l.target.checked_sub(self.records_off)? as usize) / RECORD_SIZE)
    }

    /// Dequantised world box of leaf `i`.
    pub fn leaf_world_box(&self, i: usize) -> Option<([f32; 3], [f32; 3])> {
        self.leaves.get(i).map(|l| self.grid().world_box(&l.qbox))
    }

    /// Indices (in file order of `records`) of every box containing `p`.
    pub fn records_at(&self, p: [f32; 3]) -> Vec<usize> {
        let g = self.grid();
        let mut out = Vec::new();
        for l in &self.leaves {
            let (lo, hi) = g.world_box(&l.qbox);
            if (0..3).any(|k| p[k] < lo[k] || p[k] > hi[k]) {
                continue;
            }
            let ri = (l.target - self.records_off) as usize / RECORD_SIZE;
            if self.records[ri].contains(p) {
                out.push(ri);
            }
        }
        out
    }

    /// The smallest box containing `p` (rooms nest: a small room box lies inside the big outdoor box).
    pub fn smallest_at(&self, p: [f32; 3]) -> Option<usize> {
        self.records_at(p).into_iter().min_by(|&a, &b| self.records[a].volume().total_cmp(&self.records[b].volume()))
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Vec<u8> {
        // one axis aligned box centred at (100, 0, 0), half extents (10, 20, 30)
        let mut d = Vec::new();
        d.put_u32(0x20); // leaf offset
        for v in [0.0f32, 0.0, 0.0] {
            d.put_f32(v);
        }
        d.put_f32(1.0); // scale
        d.extend_from_slice(&[0, 0, 1, 0, 0, 0]); // one tree node
        d.extend_from_slice(&[0xcd; 6]);
        assert_eq!(d.len(), 0x20);
        let rec_off = 0x30u32;
        Leaf { target: rec_off, qbox: QBox { min: [32768 + 90, 32768 - 20, 32768 - 30], max: [32768 + 110, 32768 + 20, 32768 + 30] } }.write(&mut d);
        RoomRecord {
            axis_aligned: 1,
            rotation: [0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0],
            center: [100.0, 0.0, 0.0],
            pivot: [0.0; 3],
            half: [10.0, 20.0, 30.0],
            id: 7,
            link: 0,
        }
        .write(&mut d);
        d
    }

    #[test]
    fn synthetic_roundtrip_and_query() {
        let d = tiny();
        let m = RoomMap::parse(&d).unwrap();
        assert_eq!(m.to_bytes(), d);
        assert_eq!(m.len(), 1);
        assert_eq!(m.tree, vec![[0, 1, 0]]);
        assert!(m.smallest_at([105.0, 5.0, -25.0]).is_some());
        assert!(m.smallest_at([121.0, 0.0, 0.0]).is_none());
        let (lo, hi) = m.leaf_world_box(0).unwrap();
        assert_eq!((lo[0], hi[2]), (90.0, 30.0));
    }

    #[test]
    fn rotated_box() {
        // 90 degrees about Y: local X axis (stored row 2) points along world +Z
        let r = RoomRecord { axis_aligned: 0, rotation: [-1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], center: [0.0; 3], pivot: [0.0; 3], half: [100.0, 10.0, 10.0], id: 0, link: 0 };
        assert_eq!(r.axis(0), [0.0, 0.0, 1.0]);
        assert!(r.contains([5.0, 0.0, 90.0]));
        assert!(!r.contains([90.0, 0.0, 5.0]));
        let (lo, hi) = r.world_aabb();
        assert_eq!((hi[2], hi[0]), (100.0, 10.0));
        assert_eq!(lo[2], -100.0);
    }

    #[test]
    fn world_aligned_records_ignore_rotation() {
        // quarter-turned zone: extents already in world axes
        let r = RoomRecord { axis_aligned: 1, rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0], center: [0.0; 3], pivot: [0.0; 3], half: [400.0, 190.0, 200.0], id: 0, link: 0 };
        assert!(r.contains([390.0, 0.0, 190.0]));
        assert!(!r.contains([0.0, 0.0, 210.0]));
        assert_eq!(r.world_aabb().1, [400.0, 190.0, 200.0]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(RoomMap::parse(&[0u8; 8]).is_err());
        let mut d = tiny();
        d.truncate(d.len() - 4);
        assert!(RoomMap::parse(&d).is_err());
    }
}
