//! Door behaviour: swing / sliding leaves, groups of leaves that open together, locks, and NPCs that open
//! doors on their way (the original door data is decoded in `bm_formats::gameplay::doors`, see `docs/game.md`).

use crate::math::{Vec3, v3, xform_point};
use crate::world::*;

/// Swing speed (rad/s) and slide speed (cm/s).
pub const SWING_SPEED: f32 = 2.4;
pub const SLIDE_SPEED: f32 = 90.0;
/// An NPC opens a closed door when it is this close and heading for it (cm).
pub const NPC_DOOR_REACH: f32 = 150.0;
/// Closes again after this many idle seconds when an NPC opened it.
pub const NPC_DOOR_AUTO_CLOSE: f32 = 6.0;

impl World {
    /// All leaves that open together with `id` (including `id`).
    pub fn door_group(&self, id: EntityId) -> Vec<EntityId> {
        let Some(g) = self.doors.get(id).map(|d| d.group) else { return Vec::new() };
        if g == 0 {
            return vec![id];
        }
        self.doors.iter().filter(|(_, d)| d.group == g).map(|(i, _)| i).collect()
    }

    /// World centre of a door leaf (collider bounds when it has one, else its transform).
    pub fn door_center(&self, id: EntityId) -> Option<Vec3> {
        let d = self.doors.get(id)?;
        if let Some(c) = d.collider.and_then(|h| self.collision.dynamics.get(h as usize)) {
            let (lo, hi) = c.aabb();
            return Some((lo + hi) * 0.5);
        }
        self.pos(id)
    }

    /// Opens a closed door away from `from`, closes an open one (all leaves of the door). Locked doors stay shut.
    pub fn toggle_door(&mut self, id: EntityId, from: Vec3) {
        let open = self.doors.get(id).is_some_and(|d| d.open);
        self.set_door(id, !open, from, false);
    }

    /// Opens or closes the door `id` belongs to. Returns false when nothing changed (locked, unknown, already there).
    pub fn set_door(&mut self, id: EntityId, open: bool, from: Vec3, by_npc: bool) -> bool {
        let Some(pos) = self.pos(id) else { return false };
        let group = self.door_group(id);
        if group.is_empty() {
            return false;
        }
        if open && group.iter().any(|g| self.doors.get(*g).is_some_and(|d| d.locked)) {
            if !by_npc {
                self.fx.push(FxEvent::Message { text: "Locked".into(), seconds: 1.5 });
            }
            return false;
        }
        let mut changed = false;
        for g in group {
            let Some(d) = self.doors.get_mut(g) else { continue };
            if d.open == open {
                continue;
            }
            changed = true;
            d.open = open;
            d.idle = 0.0;
            if open {
                d.open_count += 1;
                d.opened_by_npc = by_npc;
                d.auto_close = if by_npc { NPC_DOOR_AUTO_CLOSE } else { 0.0 };
                d.target = match d.motion {
                    DoorMotion::Swing { ext, max } => {
                        let local = xform_point(&d.closed.inverse_affine(), from);
                        let side = if local.z > 0.0 { 1.0 } else { -1.0 };
                        side * ext * max
                    }
                    DoorMotion::Slide { dist, .. } => dist,
                };
            } else {
                d.target = 0.0;
                d.opened_by_npc = false;
            }
        }
        if changed {
            self.fx.push(FxEvent::Door { pos, open });
        }
        changed
    }

    /// Animates door leaves towards their targets and closes doors NPCs opened once nobody is near.
    pub fn update_doors(&mut self, dt: f32) {
        let ids = self.doors.ids();
        if ids.is_empty() {
            return;
        }
        // auto close
        let bodies: Vec<Vec3> = self.bodies.iter().filter(|(id, _)| self.is_alive(*id)).map(|(_, b)| b.pos).collect();
        let mut to_close = Vec::new();
        for &id in &ids {
            let Some(c) = self.door_center(id) else { continue };
            let Some(d) = self.doors.get_mut(id) else { continue };
            if d.open && d.auto_close > 0.0 {
                let near = bodies.iter().any(|b| b.dist_xz(c) < 220.0 && (b.y - c.y).abs() < 250.0);
                if near {
                    d.idle = 0.0;
                } else {
                    d.idle += dt;
                    if d.idle >= d.auto_close {
                        to_close.push(id);
                    }
                }
            }
        }
        for id in to_close {
            let from = self.pos(id).unwrap_or(Vec3::ZERO);
            self.set_door(id, false, from, true);
        }
        // motion
        for id in ids {
            let Some(d) = self.doors.get_mut(id) else { continue };
            let diff = d.target - d.angle;
            if diff.abs() < 1e-4 {
                continue;
            }
            let speed = match d.motion {
                DoorMotion::Swing { .. } => SWING_SPEED,
                DoorMotion::Slide { .. } => SLIDE_SPEED,
            };
            d.angle += diff.clamp(-speed * dt, speed * dt);
            let m = d.current_matrix();
            let collider = d.collider;
            if let Some(c) = collider {
                self.collision.set_dynamic_transform(c, &m);
            }
            self.set_world(id, m);
        }
    }

    /// NPC helper: opens the closed, unlocked door that `who` (at `pos`, moving along `dir`) is about to walk into.
    pub fn npc_open_door_ahead(&mut self, pos: Vec3, dir: Vec3) -> bool {
        let mut best: Option<(f32, EntityId)> = None;
        for (id, d) in self.doors.iter() {
            if d.open || d.locked || d.target != 0.0 {
                continue;
            }
            let Some(c) = self.door_center(id) else { continue };
            if (c.y - pos.y).abs() > 250.0 {
                continue;
            }
            let to = c - pos;
            if to.flat().len() > NPC_DOOR_REACH + 120.0 {
                continue;
            }
            // hinged doors: the leaf centre is up to half a leaf (95 cm) beside the path; use the closest collider point
            let reach = self.door_distance(id, pos).unwrap_or_else(|| to.flat().len());
            if reach > 70.0 {
                continue;
            }
            if to.flat().len() > 40.0 && to.flat().norm().dot(dir) < 0.15 {
                continue;
            }
            if best.is_none_or(|(b, _)| reach < b) {
                best = Some((reach, id));
            }
        }
        match best {
            Some((_, id)) => self.set_door(id, true, pos, true),
            None => false,
        }
    }

    /// Horizontal distance from `p` to the closest point of the door leaf's collider box.
    pub fn door_distance(&self, id: EntityId, p: Vec3) -> Option<f32> {
        let d = self.doors.get(id)?;
        let c = d.collider.and_then(|h| self.collision.dynamics.get(h as usize))?;
        let (lo, hi) = c.aabb();
        let q = v3(p.x.clamp(lo.x, hi.x), p.y, p.z.clamp(lo.z, hi.z));
        Some(q.dist_xz(p))
    }
}
