//! Triangle-mesh collision: a static BVH built from level geometry plus a few dynamic colliders (doors).
//!
//! Everything is in native engine space (left-handed, Y up, cm). Triangles are treated as double sided.
//! The original engine's `.OCT` collision/visibility trees are *not* decoded (see `docs/game.md`); the
//! level builder in [`crate::level`] feeds visible geometry and the `_Glacier/WorldColi` proxies here.

use crate::math::{Vec3, v3};
use std::sync::Arc;

/// Physical material of a triangle (footsteps, impact effects).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum Surface {
    #[default]
    Default,
    Stone,
    Wood,
    Metal,
    Glass,
    Carpet,
    Grass,
    Dirt,
    Gravel,
    Snow,
    Water,
    Flesh,
}

impl Surface {
    /// Best-effort classification from a `.MAT` material name: the `_Glacier/WorldColi/WC_<n>_<Kind>`
    /// proxies carry the kind explicitly, everything else is guessed from words in the name.
    pub fn from_material_name(name: &str) -> Surface {
        let n = name.to_ascii_lowercase();
        let has = |s: &str| n.contains(s);
        if has("worldcoli") {
            return if has("wood") {
                Surface::Wood
            } else if has("stone") || has("concrete") || has("brick") {
                Surface::Stone
            } else if has("metal") {
                Surface::Metal
            } else if has("glass") {
                Surface::Glass
            } else if has("grass") {
                Surface::Grass
            } else if has("carpet") {
                Surface::Carpet
            } else if has("gravel") {
                Surface::Gravel
            } else if has("snow") {
                Surface::Snow
            } else if has("dirt") || has("sand") {
                Surface::Dirt
            } else if has("water") {
                Surface::Water
            } else {
                Surface::Default
            };
        }
        if has("water") {
            Surface::Water
        } else if has("carpet") || has("rug") {
            Surface::Carpet
        } else if has("wood") || has("parquet") || has("plank") || has("door") || has("crate") {
            Surface::Wood
        } else if has("metal") || has("steel") || has("iron") {
            Surface::Metal
        } else if has("glass") || has("window") {
            Surface::Glass
        } else if has("grass") || has("jungle") || has("landscape") {
            Surface::Grass
        } else if has("gravel") || has("asphalt") {
            Surface::Gravel
        } else if has("dirt") || has("sand") || has("mud") {
            Surface::Dirt
        } else if has("snow") {
            Surface::Snow
        } else if has("stone") || has("tile") || has("marble") || has("brick") || has("concrete") || has("plaster") || has("wall") {
            Surface::Stone
        } else {
            Surface::Default
        }
    }

    /// Directory under `Character/Footsteps/` in the sound banks (falls back to `Stone`).
    pub fn footstep_dir(self) -> &'static str {
        match self {
            Surface::Carpet => "Carpet",
            Surface::Gravel => "Asphalt",
            _ => "Stone",
        }
    }

    /// Directory under `Impacts_Bullet/` in the sound banks.
    pub fn impact_dir(self) -> &'static str {
        match self {
            Surface::Metal => "Metal",
            Surface::Snow => "Snow",
            Surface::Glass => "Glass",
            Surface::Wood => "Wood",
            _ => "Stone",
        }
    }

    /// Particle template name (scene `ZParticleTemplate`) for a bullet impact on this surface.
    pub fn impact_particle(self) -> &'static str {
        match self {
            Surface::Wood => "Wood_Debris",
            Surface::Metal => "Metal_Sparks",
            Surface::Glass => "Glass_Debris",
            Surface::Grass => "Grass_Debris",
            Surface::Dirt | Surface::Gravel => "Default_Debris",
            Surface::Snow => "Snow_Debris",
            Surface::Water => "Water_Debris",
            Surface::Flesh => "Blood_Smoke",
            Surface::Carpet => "Plaster_Debris",
            Surface::Stone | Surface::Default => "Stone_Debris",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub v: [Vec3; 3],
    pub surface: Surface,
}

impl Tri {
    pub fn new(a: Vec3, b: Vec3, c: Vec3, surface: Surface) -> Tri {
        Tri { v: [a, b, c], surface }
    }
    /// Unit normal following the vertex order (zero for degenerate triangles).
    pub fn normal(&self) -> Vec3 {
        (self.v[1] - self.v[0]).cross(self.v[2] - self.v[0]).norm()
    }
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = self.v[0];
        let mut hi = self.v[0];
        for p in &self.v[1..] {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        (lo, hi)
    }
    pub fn centroid(&self) -> Vec3 {
        (self.v[0] + self.v[1] + self.v[2]) * (1.0 / 3.0)
    }
    pub fn transformed(&self, m: &bm_models::Mat4) -> Tri {
        Tri {
            v: [crate::math::xform_point(m, self.v[0]), crate::math::xform_point(m, self.v[1]), crate::math::xform_point(m, self.v[2])],
            surface: self.surface,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Distance along the ray.
    pub t: f32,
    pub point: Vec3,
    /// Unit normal facing the ray origin.
    pub normal: Vec3,
    pub surface: Surface,
    /// `Some(handle)` when a dynamic collider (door) was hit.
    pub collider: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct Contact {
    /// Direction to push the sphere out along (unit).
    pub normal: Vec3,
    pub depth: f32,
    pub surface: Surface,
}

#[derive(Clone, Debug)]
struct Node {
    lo: [f32; 3],
    hi: [f32; 3],
    /// Interior: child node indices. Leaf (`count > 0`): `left` = first index into `order`.
    left: u32,
    right: u32,
    count: u32,
}

/// Bounding volume hierarchy over a triangle soup (median split along the longest axis).
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn build(tris: &[Tri]) -> Bvh {
        let mut bvh = Bvh { nodes: Vec::new(), order: (0..tris.len() as u32).collect() };
        if tris.is_empty() {
            return bvh;
        }
        let cent: Vec<Vec3> = tris.iter().map(|t| t.centroid()).collect();
        let bounds: Vec<(Vec3, Vec3)> = tris.iter().map(|t| t.bounds()).collect();
        let mut order = std::mem::take(&mut bvh.order);
        bvh.build_rec(&mut order, 0, tris.len(), &cent, &bounds);
        bvh.order = order;
        bvh
    }

    fn build_rec(&mut self, order: &mut [u32], lo: usize, hi: usize, cent: &[Vec3], bounds: &[(Vec3, Vec3)]) -> u32 {
        let mut bmin = v3(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut bmax = v3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        let mut cmin = bmin;
        let mut cmax = bmax;
        for &i in &order[lo..hi] {
            let (a, b) = bounds[i as usize];
            bmin = bmin.min(a);
            bmax = bmax.max(b);
            cmin = cmin.min(cent[i as usize]);
            cmax = cmax.max(cent[i as usize]);
        }
        let idx = self.nodes.len() as u32;
        self.nodes.push(Node { lo: bmin.to_array(), hi: bmax.to_array(), left: 0, right: 0, count: 0 });
        let n = hi - lo;
        let ext = cmax - cmin;
        if n <= LEAF || (ext.x <= 1e-6 && ext.y <= 1e-6 && ext.z <= 1e-6) {
            let nd = &mut self.nodes[idx as usize];
            nd.left = lo as u32;
            nd.count = n as u32;
            return idx;
        }
        let axis = if ext.x >= ext.y && ext.x >= ext.z {
            0
        } else if ext.y >= ext.z {
            1
        } else {
            2
        };
        let mid = lo + n / 2;
        order[lo..hi].select_nth_unstable_by(n / 2, |&a, &b| {
            cent[a as usize].axis(axis).total_cmp(&cent[b as usize].axis(axis)).then(a.cmp(&b))
        });
        let l = self.build_rec(order, lo, mid, cent, bounds);
        let r = self.build_rec(order, mid, hi, cent, bounds);
        let nd = &mut self.nodes[idx as usize];
        nd.left = l;
        nd.right = r;
        idx
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.nodes.first().map(|n| (Vec3::from_array(n.lo), Vec3::from_array(n.hi)))
    }

    /// Calls `f(triangle_index)` for every triangle whose leaf box overlaps `[lo, hi]`.
    pub fn query_aabb(&self, lo: Vec3, hi: Vec3, mut f: impl FnMut(u32)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            if n.lo[0] > hi.x || n.hi[0] < lo.x || n.lo[1] > hi.y || n.hi[1] < lo.y || n.lo[2] > hi.z || n.hi[2] < lo.z {
                continue;
            }
            if n.count > 0 {
                for k in 0..n.count {
                    f(self.order[(n.left + k) as usize]);
                }
            } else {
                stack.push(n.left);
                stack.push(n.right);
            }
        }
    }

    /// Nearest hit along `o + t d`, `0 <= t <= max_t`, `d` unit length. `accept` can reject triangles.
    pub fn raycast(&self, tris: &[Tri], o: Vec3, d: Vec3, max_t: f32, accept: &dyn Fn(&Tri) -> bool) -> Option<(f32, u32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = v3(1.0 / nz(d.x), 1.0 / nz(d.y), 1.0 / nz(d.z));
        let mut best = max_t;
        let mut best_i = None;
        let mut stack: Vec<u32> = vec![0];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            if slab(n, o, inv, best).is_none() {
                continue;
            }
            if n.count > 0 {
                for k in 0..n.count {
                    let ti = self.order[(n.left + k) as usize];
                    let tri = &tris[ti as usize];
                    if !accept(tri) {
                        continue;
                    }
                    if let Some(t) = ray_tri(o, d, tri) {
                        if t < best {
                            best = t;
                            best_i = Some(ti);
                        }
                    }
                }
            } else {
                let a = &self.nodes[n.left as usize];
                let b = &self.nodes[n.right as usize];
                let ta = slab(a, o, inv, best).unwrap_or(f32::INFINITY);
                let tb = slab(b, o, inv, best).unwrap_or(f32::INFINITY);
                if ta <= tb {
                    stack.push(n.right);
                    stack.push(n.left);
                } else {
                    stack.push(n.left);
                    stack.push(n.right);
                }
            }
        }
        best_i.map(|i| (best, i))
    }
}

fn nz(x: f32) -> f32 {
    if x.abs() < 1e-12 { 1e-12f32.copysign(x) } else { x }
}

fn slab(n: &Node, o: Vec3, inv: Vec3, max_t: f32) -> Option<f32> {
    let mut t0 = 0.0f32;
    let mut t1 = max_t;
    for a in 0..3 {
        let (oa, ia) = (o.axis(a), inv.axis(a));
        let (mut ta, mut tb) = ((n.lo[a] - oa) * ia, (n.hi[a] - oa) * ia);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// Moeller-Trumbore, double sided. Returns `t >= 0`.
pub fn ray_tri(o: Vec3, d: Vec3, tri: &Tri) -> Option<f32> {
    let e1 = tri.v[1] - tri.v[0];
    let e2 = tri.v[2] - tri.v[0];
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - tri.v[0];
    let u = s.dot(p) * inv;
    if !(-1e-5..=1.0 + 1e-5).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < -1e-5 || u + v > 1.0 + 1e-5 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

/// Closest point on a triangle to `p` (Ericson, Real-Time Collision Detection).
pub fn closest_point_on_tri(p: Vec3, t: &Tri) -> Vec3 {
    let (a, b, c) = (t.v[0], t.v[1], t.v[2]);
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return a + ab * v;
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return a + ac * w;
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return b + (c - b) * w;
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    a + ab * v + ac * w
}

/// Immutable level geometry shared between worlds.
#[derive(Debug, Default)]
pub struct StaticMesh {
    pub tris: Vec<Tri>,
    pub bvh: Bvh,
}

impl StaticMesh {
    pub fn new(tris: Vec<Tri>) -> StaticMesh {
        let bvh = Bvh::build(&tris);
        StaticMesh { tris, bvh }
    }
}

/// A movable collider (door leaf): triangles stored in local space and re-posed with [`DynCollider::set_transform`].
#[derive(Clone, Debug)]
pub struct DynCollider {
    pub handle: u32,
    pub enabled: bool,
    local: Vec<Tri>,
    world: Vec<Tri>,
    lo: Vec3,
    hi: Vec3,
}

impl DynCollider {
    pub fn new(handle: u32, local: Vec<Tri>, xf: &bm_models::Mat4) -> DynCollider {
        let mut d = DynCollider { handle, enabled: true, world: Vec::new(), local, lo: Vec3::ZERO, hi: Vec3::ZERO };
        d.set_transform(xf);
        d
    }
    pub fn set_transform(&mut self, xf: &bm_models::Mat4) {
        self.world = self.local.iter().map(|t| t.transformed(xf)).collect();
        let mut lo = v3(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut hi = -lo;
        for t in &self.world {
            let (a, b) = t.bounds();
            lo = lo.min(a);
            hi = hi.max(b);
        }
        self.lo = lo;
        self.hi = hi;
    }
    pub fn aabb(&self) -> (Vec3, Vec3) {
        (self.lo, self.hi)
    }
}

/// Static BVH + dynamic colliders; the single entry point for queries.
#[derive(Clone, Debug, Default)]
pub struct CollisionWorld {
    pub statics: Arc<StaticMesh>,
    pub dynamics: Vec<DynCollider>,
}

impl CollisionWorld {
    pub fn new(tris: Vec<Tri>) -> CollisionWorld {
        CollisionWorld { statics: Arc::new(StaticMesh::new(tris)), dynamics: Vec::new() }
    }

    pub fn from_static(statics: Arc<StaticMesh>) -> CollisionWorld {
        CollisionWorld { statics, dynamics: Vec::new() }
    }

    pub fn triangle_count(&self) -> usize {
        self.statics.tris.len()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.statics.bvh.bounds()
    }

    pub fn add_dynamic(&mut self, local: Vec<Tri>, xf: &bm_models::Mat4) -> u32 {
        let handle = self.dynamics.len() as u32;
        self.dynamics.push(DynCollider::new(handle, local, xf));
        handle
    }

    pub fn set_dynamic_transform(&mut self, handle: u32, xf: &bm_models::Mat4) {
        if let Some(d) = self.dynamics.get_mut(handle as usize) {
            d.set_transform(xf);
        }
    }

    pub fn set_dynamic_enabled(&mut self, handle: u32, on: bool) {
        if let Some(d) = self.dynamics.get_mut(handle as usize) {
            d.enabled = on;
        }
    }

    /// Nearest hit along a unit-direction ray. `dynamics` selects whether door colliders count.
    pub fn raycast(&self, o: Vec3, d: Vec3, max_t: f32, dynamics: bool) -> Option<Hit> {
        self.raycast_filtered(o, d, max_t, dynamics, &|_| true)
    }

    pub fn raycast_filtered(&self, o: Vec3, d: Vec3, max_t: f32, dynamics: bool, accept: &dyn Fn(&Tri) -> bool) -> Option<Hit> {
        let mut best: Option<(f32, Tri, Option<u32>)> = None;
        if let Some((t, i)) = self.statics.bvh.raycast(&self.statics.tris, o, d, max_t, accept) {
            best = Some((t, self.statics.tris[i as usize], None));
        }
        if dynamics {
            for dc in self.dynamics.iter().filter(|c| c.enabled) {
                if !ray_hits_aabb(o, d, max_t, dc.lo, dc.hi) {
                    continue;
                }
                for tri in &dc.world {
                    if !accept(tri) {
                        continue;
                    }
                    if let Some(t) = ray_tri(o, d, tri) {
                        if t <= best.as_ref().map(|b| b.0).unwrap_or(max_t) && t <= max_t {
                            best = Some((t, *tri, Some(dc.handle)));
                        }
                    }
                }
            }
        }
        best.map(|(t, tri, collider)| {
            let mut n = tri.normal();
            if n.dot(d) > 0.0 {
                n = -n;
            }
            Hit { t, point: o + d * t, normal: n, surface: tri.surface, collider }
        })
    }

    /// Ray between two points; `None` if nothing is in the way.
    pub fn segment_hit(&self, a: Vec3, b: Vec3, dynamics: bool) -> Option<Hit> {
        let d = b - a;
        let l = d.len();
        if l < 1e-4 {
            return None;
        }
        self.raycast(a, d * (1.0 / l), l, dynamics)
    }

    pub fn line_of_sight(&self, a: Vec3, b: Vec3) -> bool {
        self.segment_hit(a, b, true).is_none()
    }

    /// Highest walkable surface below `from` within `max_drop` (surfaces steeper than `min_ny` are skipped).
    pub fn ground_below(&self, from: Vec3, max_drop: f32, min_ny: f32, dynamics: bool) -> Option<Hit> {
        let mut origin = from;
        let mut left = max_drop;
        for _ in 0..4 {
            let h = self.raycast(origin, v3(0.0, -1.0, 0.0), left, dynamics)?;
            if h.normal.y >= min_ny {
                return Some(Hit { t: max_drop - left + h.t, ..h });
            }
            // steep face: continue below it
            let adv = h.t + 0.5;
            origin = origin + v3(0.0, -adv, 0.0);
            left -= adv;
            if left <= 0.0 {
                return None;
            }
        }
        None
    }

    /// Collects penetrations of a sphere against all triangles.
    pub fn sphere_contacts(&self, c: Vec3, r: f32, dynamics: bool, out: &mut Vec<Contact>) {
        let lo = c - v3(r, r, r);
        let hi = c + v3(r, r, r);
        let tris = &self.statics.tris;
        self.statics.bvh.query_aabb(lo, hi, |i| push_contact(&tris[i as usize], c, r, out));
        if dynamics {
            for dc in self.dynamics.iter().filter(|d| d.enabled) {
                if dc.lo.x > hi.x || dc.hi.x < lo.x || dc.lo.y > hi.y || dc.hi.y < lo.y || dc.lo.z > hi.z || dc.hi.z < lo.z {
                    continue;
                }
                for t in &dc.world {
                    push_contact(t, c, r, out);
                }
            }
        }
    }
}

/// Slab test: does the ray `o + t d` (0 <= t <= max_t) touch the box?
fn ray_hits_aabb(o: Vec3, d: Vec3, max_t: f32, lo: Vec3, hi: Vec3) -> bool {
    let mut t0 = 0.0f32;
    let mut t1 = max_t;
    for k in 0..3 {
        let (oo, dd, l, h) = (o.axis(k), d.axis(k), lo.axis(k), hi.axis(k));
        if dd.abs() < 1e-9 {
            if oo < l || oo > h {
                return false;
            }
        } else {
            let inv = 1.0 / dd;
            let (mut a, mut b) = ((l - oo) * inv, (h - oo) * inv);
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            t0 = t0.max(a);
            t1 = t1.min(b);
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

fn push_contact(t: &Tri, c: Vec3, r: f32, out: &mut Vec<Contact>) {
    let p = closest_point_on_tri(c, t);
    let d = c - p;
    let dist2 = d.len_sq();
    if dist2 >= r * r {
        return;
    }
    let dist = dist2.sqrt();
    let normal = if dist > 1e-5 {
        d * (1.0 / dist)
    } else {
        let n = t.normal();
        if n.len_sq() < 0.5 {
            return;
        }
        n
    };
    out.push(Contact { normal, depth: r - dist, surface: t.surface });
}

/// Ray vs a vertical capsule approximated by a cylinder with spherical caps. `base` = feet position.
pub fn ray_vs_capsule(o: Vec3, d: Vec3, base: Vec3, height: f32, radius: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    let (ox, oz) = (o.x - base.x, o.z - base.z);
    let a = d.x * d.x + d.z * d.z;
    if a > 1e-9 {
        let b = 2.0 * (ox * d.x + oz * d.z);
        let c = ox * ox + oz * oz - radius * radius;
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let s = disc.sqrt();
            for t in [(-b - s) / (2.0 * a), (-b + s) / (2.0 * a)] {
                if t >= 0.0 {
                    let y = o.y + d.y * t;
                    if y >= base.y + radius && y <= base.y + height - radius {
                        best = Some(best.map_or(t, |b| b.min(t)));
                    }
                }
            }
        }
    }
    for cy in [base.y + radius, base.y + height - radius] {
        let c = v3(base.x, cy, base.z);
        let oc = o - c;
        let b = oc.dot(d);
        let cc = oc.dot(oc) - radius * radius;
        let disc = b * b - cc;
        if disc >= 0.0 {
            let s = disc.sqrt();
            for t in [-b - s, -b + s] {
                if t >= 0.0 {
                    best = Some(best.map_or(t, |bb| bb.min(t)));
                    break;
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32, half: f32) -> Vec<Tri> {
        let (a, b, c, d) = (v3(-half, y, -half), v3(half, y, -half), v3(half, y, half), v3(-half, y, half));
        vec![Tri::new(a, b, c, Surface::Stone), Tri::new(a, c, d, Surface::Stone)]
    }

    #[test]
    fn ray_hits_floor_and_misses_outside() {
        let w = CollisionWorld::new(quad(0.0, 100.0));
        let h = w.raycast(v3(10.0, 50.0, 10.0), v3(0.0, -1.0, 0.0), 200.0, false).unwrap();
        assert!((h.t - 50.0).abs() < 1e-3 && h.normal.y > 0.99);
        assert!(w.raycast(v3(500.0, 50.0, 0.0), v3(0.0, -1.0, 0.0), 200.0, false).is_none());
    }

    #[test]
    fn bvh_matches_brute_force() {
        let mut tris = Vec::new();
        let mut rng = crate::math::Rng::new(5);
        for _ in 0..500 {
            let c = v3(rng.range(-500.0, 500.0), rng.range(-50.0, 50.0), rng.range(-500.0, 500.0));
            tris.push(Tri::new(
                c,
                c + v3(rng.range(5.0, 40.0), 0.0, 0.0),
                c + v3(0.0, rng.range(5.0, 40.0), rng.range(-10.0, 30.0)),
                Surface::Default,
            ));
        }
        let w = CollisionWorld::new(tris.clone());
        for _ in 0..200 {
            let o = v3(rng.range(-600.0, 600.0), rng.range(-80.0, 80.0), rng.range(-600.0, 600.0));
            let d = v3(rng.range(-1.0, 1.0), rng.range(-0.3, 0.3), rng.range(-1.0, 1.0)).norm();
            let brute = tris.iter().filter_map(|t| ray_tri(o, d, t)).fold(f32::INFINITY, f32::min);
            match w.raycast(o, d, 5000.0, false) {
                Some(h) => assert!((h.t - brute).abs() < 1e-2, "{} vs {}", h.t, brute),
                None => assert!(brute > 5000.0),
            }
        }
    }

    #[test]
    fn sphere_contact_pushes_out_of_floor() {
        let w = CollisionWorld::new(quad(0.0, 100.0));
        let mut out = Vec::new();
        w.sphere_contacts(v3(0.0, 20.0, 0.0), 30.0, false, &mut out);
        assert!(!out.is_empty());
        assert!((out[0].depth - 10.0).abs() < 1e-3 && out[0].normal.y > 0.99);
    }

    #[test]
    fn capsule_ray() {
        let t = ray_vs_capsule(v3(0.0, 100.0, -200.0), v3(0.0, 0.0, 1.0), v3(0.0, 0.0, 0.0), 175.0, 30.0).unwrap();
        assert!((t - 170.0).abs() < 1e-2);
        assert!(ray_vs_capsule(v3(100.0, 100.0, -200.0), v3(0.0, 0.0, 1.0), Vec3::ZERO, 175.0, 30.0).is_none());
    }
}
