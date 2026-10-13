//! `.SGP` - the **room portal graph** ("scene graph path"): door openings between rooms and the walking costs
//! between the openings of a room.
//!
//! * `planes` are *portals*: world space quads (centre, normal, four corners) in the openings between two rooms
//!   (M01: `(3900, 305, -3300)` is the front door of the hacienda).
//! * `paths` is a table of node names; the rooms (`ZROOM`, e.g. `...\M01_Hacienda_Room_101`) are entries of it, the
//!   rest are map / overlay groups. Plane word 3 / word 4 = path indices of the two rooms the portal connects, the
//!   lower index first, each connection stored once (confidence: high; verified by door positions and by the
//!   rooms of the `.RMI` boxes on either side, see [`Sgp::portals`]).
//! * `edges` connect the portals that touch a common room pairwise with their euclidean distance as weight (an
//!   intra-room routing graph for room-level path planning, 95 %+ of the weights are exactly the centre distances).
//! * `packed` / `records` / `indices` hold precomputed routing data (bit packed, undecoded).
//!
//! # Layout (confidence: sizes/ordering high - re-serialises byte-exactly; semantics medium/low)
//!
//! ```text
//! u32 n_planes, u32 n_edges, u32 n_paths, u32 n_packed, u32 n_indices        (header, 20 bytes)
//! Plane  planes[n_planes]        92 bytes each
//! Edge   edges[n_edges]          12 bytes each: u32 a, u32 b, f32 weight    (graph over plane indices)
//! u32    packed[n_packed]        bit packed (two u16 per word, top bit often set); meaning unknown
//! u32    records[n_paths][3]     packed words like the previous array; meaning unknown
//! u32    indices[n_indices]      plane indices (all < n_planes), 0,1,2... in the first entries
//! Path   paths[n_paths]          NUL-terminated string + u32 running index (0, 1, 2 ...)
//! ```
//!
//! Plane (23 words): `f32 1.0, u32 0, u32 0, u32 0x14, u32 id` (the last two vary: 0x39, 0x2c ...),
//! then `f32 center[3]`, `f32 normal[3]` (0,0,-1), `f32 corners[4][3]`.
//! Empty files are 20 zero bytes (all counts 0).

use crate::io::{invalid, Cur, FormatError, PutLe, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    /// The five leading words (first is `f32` 1.0). Meaning of words 1..4 unknown.
    pub head: [u32; 5],
    pub center: [f32; 3],
    pub normal: [f32; 3],
    pub corners: [[f32; 3]; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sgp {
    pub planes: Vec<Plane>,
    pub edges: Vec<Edge>,
    pub packed: Vec<u32>,
    pub records: Vec<[u32; 3]>,
    pub indices: Vec<u32>,
    pub paths: Vec<(String, u32)>,
}

/// A door opening between two rooms (a plane of the `.SGP`).
#[derive(Clone, Debug, PartialEq)]
pub struct Portal {
    /// Index into [`Sgp::planes`].
    pub plane: usize,
    /// Path index (into [`Sgp::paths`]) of the first room (the lower index).
    pub from: usize,
    /// Path index of the room on the other side.
    pub to: usize,
    pub center: [f32; 3],
    pub normal: [f32; 3],
    pub corners: [[f32; 3]; 4],
}

impl Sgp {
    /// Portals with their two rooms (planes whose room indices do not point into the path table are skipped).
    pub fn portals(&self) -> Vec<Portal> {
        let n = self.paths.len() as u32;
        self.planes
            .iter()
            .enumerate()
            .filter(|(_, p)| p.head[3] < n && p.head[4] < n)
            .map(|(i, p)| Portal { plane: i, from: p.head[3] as usize, to: p.head[4] as usize, center: p.center, normal: p.normal, corners: p.corners })
            .collect()
    }

    pub fn parse(d: &[u8]) -> Result<Sgp> {
        let mut c = Cur::new(d);
        let n_planes = c.u32("n_planes")? as usize;
        let n_edges = c.u32("n_edges")? as usize;
        let n_paths = c.u32("n_paths")? as usize;
        let n_packed = c.u32("n_packed")? as usize;
        let n_idx = c.u32("n_indices")? as usize;
        let need = n_planes * 92 + n_edges * 12 + n_packed * 4 + n_paths * 12 + n_idx * 4;
        if need > c.remaining() {
            return invalid("SGP counts exceed file size");
        }
        let v3 = |c: &mut Cur| -> Result<[f32; 3]> { Ok([c.f32("f")?, c.f32("f")?, c.f32("f")?]) };
        let mut planes = Vec::with_capacity(n_planes);
        for _ in 0..n_planes {
            let mut head = [0u32; 5];
            for h in head.iter_mut() {
                *h = c.u32("plane head")?;
            }
            let center = v3(&mut c)?;
            let normal = v3(&mut c)?;
            let corners = [v3(&mut c)?, v3(&mut c)?, v3(&mut c)?, v3(&mut c)?];
            planes.push(Plane { head, center, normal, corners });
        }
        let mut edges = Vec::with_capacity(n_edges);
        for _ in 0..n_edges {
            edges.push(Edge { a: c.u32("edge")?, b: c.u32("edge")?, weight: c.f32("edge")? });
        }
        let mut packed = Vec::with_capacity(n_packed);
        for _ in 0..n_packed {
            packed.push(c.u32("packed")?);
        }
        let mut records = Vec::with_capacity(n_paths);
        for _ in 0..n_paths {
            records.push([c.u32("record")?, c.u32("record")?, c.u32("record")?]);
        }
        let mut indices = Vec::with_capacity(n_idx);
        for _ in 0..n_idx {
            indices.push(c.u32("index")?);
        }
        let mut paths = Vec::with_capacity(n_paths);
        for _ in 0..n_paths {
            let s = c.cstr("path")?;
            paths.push((s, c.u32("path index")?));
        }
        if !c.eof() {
            return Err(FormatError::Invalid(format!("{} trailing bytes in SGP", c.remaining())));
        }
        Ok(Sgp { planes, edges, packed, records, indices, paths })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.put_u32(self.planes.len() as u32);
        o.put_u32(self.edges.len() as u32);
        o.put_u32(self.paths.len() as u32);
        o.put_u32(self.packed.len() as u32);
        o.put_u32(self.indices.len() as u32);
        for p in &self.planes {
            p.head.iter().for_each(|&h| o.put_u32(h));
            p.center.iter().for_each(|&f| o.put_f32(f));
            p.normal.iter().for_each(|&f| o.put_f32(f));
            p.corners.iter().flatten().for_each(|&f| o.put_f32(f));
        }
        for e in &self.edges {
            o.put_u32(e.a);
            o.put_u32(e.b);
            o.put_f32(e.weight);
        }
        self.packed.iter().for_each(|&v| o.put_u32(v));
        for r in &self.records {
            r.iter().for_each(|&v| o.put_u32(v));
        }
        self.indices.iter().for_each(|&v| o.put_u32(v));
        for (s, i) in &self.paths {
            o.put_cstr_latin1(s);
            o.put_u32(*i);
        }
        o
    }
}
