//! Lazily built grid navigation over walkable collision.
//!
//! The original navigation data (`.OCT`, `.SGP` graph, per-level AI waypoints) is not decoded. Instead,
//! walkability is probed on demand with ground rays and clearance rays against the collision mesh, grid
//! cells are expanded by A* and cached in a [`NavCache`], so frequently used routes (patrols) become cheap.
//! Everything is deterministic: no hashing order leaks into results (open set ordered by cost then sequence).

use crate::collision::CollisionWorld;
use crate::math::{Vec3, v3};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Debug)]
pub struct NavConfig {
    /// Grid cell size in cm.
    pub cell: f32,
    pub step_height: f32,
    pub max_drop: f32,
    /// Height of the clearance ray above the floor (must be free of geometry).
    pub clearance: f32,
    pub knee: f32,
    pub max_expand: usize,
    pub walkable_ny: f32,
}

impl Default for NavConfig {
    fn default() -> Self {
        NavConfig { cell: 60.0, step_height: 40.0, max_drop: 100.0, clearance: 150.0, knee: 50.0, max_expand: 6000, walkable_ny: 0.65 }
    }
}

type Key = (i32, i32, i32);

#[derive(Default)]
pub struct NavCache {
    /// Expanded neighbours of a node: `(key, y, step cost)`.
    nbrs: HashMap<Key, Vec<(Key, f32, f32)>>,
    pub expansions: u64,
}

impl NavCache {
    pub fn clear(&mut self) {
        self.nbrs.clear();
    }
    pub fn cached_nodes(&self) -> usize {
        self.nbrs.len()
    }
}

#[derive(Clone, Debug)]
pub struct NavPath {
    pub points: Vec<Vec3>,
    /// False when the search ran out of budget / no route exists and the path only leads towards the goal.
    pub complete: bool,
}

fn key_of(cfg: &NavConfig, p: Vec3) -> Key {
    ((p.x / cfg.cell).round() as i32, (p.z / cfg.cell).round() as i32, (p.y / 50.0).round() as i32)
}

fn center(cfg: &NavConfig, k: Key, y: f32) -> Vec3 {
    v3(k.0 as f32 * cfg.cell, y, k.1 as f32 * cfg.cell)
}

const DIRS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

/// True if a body can walk the straight line `a -> b`: continuous floor within the step height and no walls
/// at knee / head height. Doors are ignored (NPCs open them).
pub fn segment_walkable(cw: &CollisionWorld, cfg: &NavConfig, a: Vec3, b: Vec3) -> bool {
    let d = b - a;
    let len = d.flat().len();
    let n = ((len / 40.0).ceil() as usize).max(1);
    let mut prev_y = a.y;
    let mut prev = a;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let p = a + d * t;
        // floor under the sample, searched around the previous floor height
        let Some(g) = cw.ground_below(v3(p.x, prev_y + cfg.step_height, p.z), cfg.step_height + cfg.max_drop, cfg.walkable_ny, false) else {
            return false;
        };
        let gy = g.point.y;
        if gy - prev_y > cfg.step_height + 1.0 || prev_y - gy > cfg.max_drop {
            return false;
        }
        let q = v3(p.x, gy, p.z);
        for h in [cfg.knee, cfg.clearance] {
            let off = v3(0.0, h, 0.0);
            if cw.segment_hit(prev + off, q + off, false).is_some() {
                return false;
            }
        }
        prev = q;
        prev_y = gy;
    }
    true
}

fn expand(cw: &CollisionWorld, cfg: &NavConfig, cache: &mut NavCache, k: Key, y: f32) -> Vec<(Key, f32, f32)> {
    if let Some(v) = cache.nbrs.get(&k) {
        return v.clone();
    }
    cache.expansions += 1;
    let here = center(cfg, k, y);
    let mut out = Vec::new();
    for (dx, dz) in DIRS {
        let nx = k.0 + dx;
        let nz = k.1 + dz;
        let probe = v3(nx as f32 * cfg.cell, y, nz as f32 * cfg.cell);
        let Some(g) = cw.ground_below(
            probe + v3(0.0, cfg.step_height, 0.0),
            cfg.step_height + cfg.max_drop,
            cfg.walkable_ny,
            false,
        ) else {
            continue;
        };
        let to = v3(probe.x, g.point.y, probe.z);
        if to.y - y > cfg.step_height + 1.0 {
            continue;
        }
        let mut blocked = false;
        for h in [cfg.knee, cfg.clearance] {
            let off = v3(0.0, h, 0.0);
            if cw.segment_hit(here + off, to + off, false).is_some() {
                blocked = true;
                break;
            }
        }
        if blocked {
            continue;
        }
        let nk = key_of(cfg, to);
        let nk = (nx, nz, nk.2);
        out.push((nk, to.y, here.dist(to)));
    }
    cache.nbrs.insert(k, out.clone());
    out
}

/// Snaps a point onto the floor below it (searching up to `up` above and `down` below).
pub fn snap_to_floor(cw: &CollisionWorld, cfg: &NavConfig, p: Vec3, up: f32, down: f32) -> Option<Vec3> {
    let g = cw.ground_below(p + v3(0.0, up, 0.0), up + down, cfg.walkable_ny, false)?;
    Some(g.point)
}

/// A* from `from` to `to`; both are snapped to the floor first. Returns a string-pulled path (first point is
/// the start cell, last is `to` when complete).
pub fn find_path(cw: &CollisionWorld, cfg: &NavConfig, cache: &mut NavCache, from: Vec3, to: Vec3) -> Option<NavPath> {
    let start = snap_to_floor(cw, cfg, from, cfg.step_height, 200.0).unwrap_or(from);
    let goal = snap_to_floor(cw, cfg, to, 150.0, 300.0)?;
    if start.dist(goal) < cfg.cell * 0.75 {
        return Some(NavPath { points: vec![goal], complete: true });
    }
    if segment_walkable(cw, cfg, start, goal) {
        return Some(NavPath { points: vec![goal], complete: true });
    }
    let sk = key_of(cfg, start);
    let gk = key_of(cfg, goal);

    #[derive(Clone, Copy)]
    struct Rec {
        y: f32,
        g: f32,
        parent: Option<Key>,
        closed: bool,
    }
    let mut recs: HashMap<Key, Rec> = HashMap::new();
    let mut open: BinaryHeap<Reverse<(u32, u64, Key)>> = BinaryHeap::new();
    let mut seq = 0u64;
    let h = |p: Vec3| p.dist(goal);
    recs.insert(sk, Rec { y: start.y, g: 0.0, parent: None, closed: false });
    open.push(Reverse((h(start).to_bits(), 0, sk)));
    let mut expanded = 0usize;
    let mut best: (f32, Key) = (h(start), sk);
    let mut found = false;
    while let Some(Reverse((_, _, k))) = open.pop() {
        let rec = recs[&k];
        if rec.closed {
            continue;
        }
        recs.get_mut(&k).unwrap().closed = true;
        let here = center(cfg, k, rec.y);
        let dh = h(here);
        if dh < best.0 {
            best = (dh, k);
        }
        if (k.0, k.1) == (gk.0, gk.1) && (rec.y - goal.y).abs() < 100.0 || dh < cfg.cell * 0.8 {
            best = (dh, k);
            found = true;
            break;
        }
        expanded += 1;
        if expanded > cfg.max_expand {
            break;
        }
        for (nk, ny, cost) in expand(cw, cfg, cache, k, rec.y) {
            let ng = rec.g + cost;
            let better = match recs.get(&nk) {
                None => true,
                Some(r) => !r.closed && ng < r.g,
            };
            if better {
                recs.insert(nk, Rec { y: ny, g: ng, parent: Some(k), closed: false });
                seq += 1;
                let f = ng + h(center(cfg, nk, ny));
                open.push(Reverse((f.to_bits(), seq, nk)));
            }
        }
    }
    // reconstruct
    let mut chain = Vec::new();
    let mut cur = Some(best.1);
    while let Some(k) = cur {
        let r = recs[&k];
        chain.push(center(cfg, k, r.y));
        cur = r.parent;
    }
    chain.reverse();
    if found {
        chain.push(goal);
    }
    // string pulling
    let mut pts = vec![start];
    pts.extend(chain.iter().skip(1).copied());
    let mut out = Vec::new();
    let mut anchor = pts[0];
    let mut i = 1;
    while i < pts.len() {
        // furthest point visible from the anchor
        let mut j = i;
        let limit = (i + 12).min(pts.len());
        let mut k = limit;
        while k > i {
            if segment_walkable(cw, cfg, anchor, pts[k - 1]) {
                j = k - 1;
                break;
            }
            k -= 1;
        }
        out.push(pts[j]);
        anchor = pts[j];
        i = j + 1;
    }
    if out.is_empty() {
        return None;
    }
    Some(NavPath { points: out, complete: found })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{Surface, Tri};

    fn floor(x0: f32, x1: f32, z0: f32, z1: f32) -> Vec<Tri> {
        let (a, b, c, d) = (v3(x0, 0.0, z0), v3(x1, 0.0, z0), v3(x1, 0.0, z1), v3(x0, 0.0, z1));
        vec![Tri::new(a, b, c, Surface::Stone), Tri::new(a, c, d, Surface::Stone)]
    }
    fn wall_z(z: f32, x0: f32, x1: f32) -> Vec<Tri> {
        let (a, b, c, d) = (v3(x0, 0.0, z), v3(x1, 0.0, z), v3(x1, 300.0, z), v3(x0, 300.0, z));
        vec![Tri::new(a, b, c, Surface::Stone), Tri::new(a, c, d, Surface::Stone)]
    }

    #[test]
    fn routes_around_a_wall_through_the_gap() {
        let mut tris = floor(-1500.0, 1500.0, -1500.0, 1500.0);
        // wall along z = 0 from x = -1000 to x = 400 (open on both ends)
        tris.extend(wall_z(0.0, -1000.0, 400.0));
        let cw = CollisionWorld::new(tris);
        let cfg = NavConfig::default();
        let mut cache = NavCache::default();
        let p = find_path(&cw, &cfg, &mut cache, v3(-500.0, 0.0, -600.0), v3(-500.0, 0.0, 600.0)).unwrap();
        assert!(p.complete);
        assert!(p.points.iter().any(|q| q.x > 400.0 || q.x < -1000.0), "path must go around the wall: {:?}", p.points);
        // every leg is walkable
        let mut prev = v3(-500.0, 0.0, -600.0);
        for q in &p.points {
            assert!(segment_walkable(&cw, &cfg, prev, *q), "{prev:?} -> {q:?}");
            prev = *q;
        }
        // deterministic
        let mut cache2 = NavCache::default();
        let p2 = find_path(&cw, &cfg, &mut cache2, v3(-500.0, 0.0, -600.0), v3(-500.0, 0.0, 600.0)).unwrap();
        assert_eq!(p.points, p2.points);
    }

    #[test]
    fn unreachable_goal_gives_partial_path() {
        let mut tris = floor(-500.0, 500.0, -500.0, 500.0);
        tris.extend(floor(2000.0, 2500.0, -500.0, 500.0));
        let cw = CollisionWorld::new(tris);
        let mut cfg = NavConfig::default();
        cfg.max_expand = 400;
        let mut cache = NavCache::default();
        let p = find_path(&cw, &cfg, &mut cache, v3(0.0, 0.0, 0.0), v3(2200.0, 0.0, 0.0)).unwrap();
        assert!(!p.complete);
    }
}
