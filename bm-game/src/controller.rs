//! Kinematic character controller: three stacked spheres (a capsule), gravity, ground snapping, step-up
//! and wall sliding against a [`CollisionWorld`]. Used by the player and by NPCs.

use crate::collision::{CollisionWorld, Contact, Surface};
use crate::math::{Vec3, v3};

#[derive(Clone, Debug)]
pub struct BodyParams {
    pub radius: f32,
    pub height: f32,
    pub crouch_height: f32,
    pub step_height: f32,
    /// Minimum `normal.y` of a walkable surface (0.65 ~ 49 degrees).
    pub walkable_ny: f32,
    pub gravity: f32,
    pub terminal_speed: f32,
    /// How far the body is pulled down onto the floor when walking off a small drop.
    pub snap_down: f32,
}

impl Default for BodyParams {
    fn default() -> Self {
        BodyParams {
            radius: 30.0,
            height: 175.0,
            crouch_height: 120.0,
            step_height: 40.0,
            walkable_ny: 0.65,
            gravity: 981.0,
            terminal_speed: 3500.0,
            snap_down: 30.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    /// Feet position.
    pub pos: Vec3,
    pub vel_y: f32,
    pub grounded: bool,
    pub ground_normal: Vec3,
    pub surface: Surface,
    pub crouching: bool,
    /// Last position that was standing on walkable ground (fall-out-of-world recovery).
    pub last_safe: Vec3,
    /// Horizontal distance walked since the last footstep event.
    pub step_accum: f32,
}

impl Body {
    pub fn new(pos: Vec3) -> Body {
        Body {
            pos,
            vel_y: 0.0,
            grounded: false,
            ground_normal: Vec3::UP,
            surface: Surface::Default,
            crouching: false,
            last_safe: pos,
            step_accum: 0.0,
        }
    }
    pub fn height(&self, p: &BodyParams) -> f32 {
        if self.crouching { p.crouch_height } else { p.height }
    }
    /// Eye position for line-of-sight checks.
    pub fn eye(&self, p: &BodyParams) -> Vec3 {
        self.pos + v3(0.0, self.height(p) - 12.0, 0.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StepResult {
    pub moved: f32,
    pub landed: bool,
    /// Impact speed (cm/s) when `landed`.
    pub land_speed: f32,
    pub blocked: bool,
}

/// Pushes the capsule at `pos` out of walls. Floors (`normal.y >= walkable`) are ignored, they are handled
/// by the ground probe. Returns the corrected position and whether a wall pushed back.
fn depenetrate(cw: &CollisionWorld, p: &BodyParams, height: f32, mut pos: Vec3, dynamics: bool) -> (Vec3, bool) {
    let r = p.radius;
    let mut pushed = false;
    let mut contacts: Vec<Contact> = Vec::new();
    for _ in 0..4 {
        let mut push = Vec3::ZERO;
        let mut any = false;
        // lowest sphere starts above the step height so stairs / curbs do not count as walls
        let centers = [p.step_height + r * 0.25, (height * 0.5).max(p.step_height + r), (height - r).max(p.step_height + r)];
        for (si, cy) in centers.iter().enumerate() {
            contacts.clear();
            cw.sphere_contacts(pos + v3(0.0, *cy, 0.0), r, dynamics, &mut contacts);
            let mut best: Option<Contact> = None;
            for c in &contacts {
                let walkable = c.normal.y >= p.walkable_ny;
                if walkable && si == 0 {
                    continue;
                }
                if walkable {
                    continue;
                }
                if best.map_or(true, |b| c.depth > b.depth) {
                    best = Some(*c);
                }
            }
            if let Some(c) = best {
                any = true;
                let n = if c.normal.y < -0.7 { c.normal } else { v3(c.normal.x, 0.0, c.normal.z).norm_or(c.normal) };
                let d = if c.normal.y < -0.7 { c.depth } else { c.depth / (c.normal.x.hypot(c.normal.z)).max(0.3) };
                push += n * d.min(r);
            }
        }
        if !any {
            break;
        }
        pushed = true;
        pos += push;
    }
    (pos, pushed)
}

fn slide(cw: &CollisionWorld, p: &BodyParams, height: f32, start: Vec3, disp: Vec3, dynamics: bool) -> (Vec3, bool) {
    // sub-step so a fast mover cannot skip through thin walls
    let n = ((disp.len() / (p.radius * 0.5)).ceil() as usize).clamp(1, 8);
    let mut pos = start;
    let mut blocked = false;
    for _ in 0..n {
        let (np, b) = depenetrate(cw, p, height, pos + disp * (1.0 / n as f32), dynamics);
        blocked |= b;
        pos = np;
    }
    (pos, blocked)
}

/// Highest walkable floor within reach around `pos` (centre + ring of four rays).
fn probe_ground(cw: &CollisionWorld, p: &BodyParams, pos: Vec3, up: f32, len: f32, dynamics: bool) -> Option<crate::collision::Hit> {
    let mut best: Option<crate::collision::Hit> = None;
    let rr = p.radius * 0.6;
    let offs = [v3(0.0, 0.0, 0.0), v3(rr, 0.0, 0.0), v3(-rr, 0.0, 0.0), v3(0.0, 0.0, rr), v3(0.0, 0.0, -rr)];
    for (i, o) in offs.iter().enumerate() {
        let from = pos + *o + v3(0.0, up, 0.0);
        if let Some(h) = cw.ground_below(from, len, p.walkable_ny, dynamics) {
            // ring rays only count when they are not a ledge above the step height
            if i > 0 && h.point.y > pos.y + p.step_height {
                continue;
            }
            if best.as_ref().map_or(true, |b| h.point.y > b.point.y) {
                best = Some(h);
            }
        }
    }
    best
}

/// Advances `body` by `dt` seconds. `desired_vel` is the horizontal velocity request in cm/s (y ignored).
pub fn step_body(cw: &CollisionWorld, p: &BodyParams, body: &mut Body, desired_vel: Vec3, dt: f32, dynamics: bool) -> StepResult {
    let mut res = StepResult::default();
    let height = body.height(p);
    let start = body.pos;
    let disp = v3(desired_vel.x, 0.0, desired_vel.z) * dt;

    // ---- horizontal ----
    let mut pos = start;
    if disp.len_sq() > 1e-8 {
        let (p1, blocked) = slide(cw, p, height, start, disp, dynamics);
        pos = p1;
        res.blocked = blocked;
        let progress = (p1 - start).flat().dot(disp.norm());
        if blocked && body.grounded && progress < disp.len() * 0.6 {
            // try stepping up onto a ledge: lift, move, drop
            let lift = v3(0.0, p.step_height, 0.0);
            let (p2, _) = slide(cw, p, height, start + lift, disp, dynamics);
            let progress2 = (p2 - (start + lift)).flat().dot(disp.norm());
            if progress2 > progress + 0.5 {
                if let Some(g) = probe_ground(cw, p, p2 - lift, p.step_height + 2.0, p.step_height * 2.0 + 4.0, dynamics) {
                    if g.point.y - start.y <= p.step_height + 1.0 {
                        pos = v3(p2.x, g.point.y, p2.z);
                        res.blocked = false;
                    }
                }
            }
        }
    }

    // ---- vertical ----
    if body.grounded {
        body.vel_y = 0.0;
    } else {
        body.vel_y = (body.vel_y - p.gravity * dt).max(-p.terminal_speed);
    }
    let new_y = pos.y + body.vel_y * dt;
    let reach_down = if body.grounded { p.snap_down } else { (pos.y - new_y).max(0.0) };
    let ground = probe_ground(cw, p, pos, p.step_height, p.step_height + reach_down + 1.0, dynamics);
    match &ground {
        Some(g) if body.grounded => {
            pos.y = g.point.y;
            body.ground_normal = g.normal;
            body.surface = g.surface;
        }
        Some(g) if g.point.y >= new_y - 0.01 && body.vel_y <= 0.0 => {
            res.landed = true;
            res.land_speed = -body.vel_y;
            pos.y = g.point.y;
            body.vel_y = 0.0;
            body.grounded = true;
            body.ground_normal = g.normal;
            body.surface = g.surface;
        }
        _ => {
            body.grounded = false;
            pos.y = new_y;
        }
    }
    if body.grounded && ground.is_none() {
        body.grounded = false;
    }
    // ceiling bump while rising is not needed (no jumping); keep the head sphere out of ceilings when falling
    if body.grounded {
        body.last_safe = pos;
    }
    res.moved = (pos - start).flat().len();
    body.step_accum += res.moved;
    body.pos = pos;
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::Tri;

    fn floor(y: f32, x0: f32, x1: f32, z0: f32, z1: f32) -> Vec<Tri> {
        let (a, b, c, d) = (v3(x0, y, z0), v3(x1, y, z0), v3(x1, y, z1), v3(x0, y, z1));
        vec![Tri::new(a, b, c, Surface::Stone), Tri::new(a, c, d, Surface::Stone)]
    }
    fn wall_x(x: f32, y0: f32, y1: f32, z0: f32, z1: f32) -> Vec<Tri> {
        let (a, b, c, d) = (v3(x, y0, z0), v3(x, y1, z0), v3(x, y1, z1), v3(x, y0, z1));
        vec![Tri::new(a, b, c, Surface::Stone), Tri::new(a, c, d, Surface::Stone)]
    }

    #[test]
    fn falls_and_lands_on_floor() {
        let cw = CollisionWorld::new(floor(0.0, -500.0, 500.0, -500.0, 500.0));
        let p = BodyParams::default();
        let mut b = Body::new(v3(0.0, 300.0, 0.0));
        let mut landed = false;
        for _ in 0..120 {
            landed |= step_body(&cw, &p, &mut b, Vec3::ZERO, 1.0 / 60.0, true).landed;
        }
        assert!(landed && b.grounded && b.pos.y.abs() < 0.5, "{:?}", b);
    }

    #[test]
    fn blocked_by_wall_and_slides() {
        let mut tris = floor(0.0, -1000.0, 1000.0, -1000.0, 1000.0);
        tris.extend(wall_x(200.0, 0.0, 400.0, -1000.0, 1000.0));
        let cw = CollisionWorld::new(tris);
        let p = BodyParams::default();
        let mut b = Body::new(v3(0.0, 0.0, 0.0));
        for _ in 0..240 {
            step_body(&cw, &p, &mut b, v3(300.0, 0.0, 100.0), 1.0 / 60.0, true);
        }
        assert!(b.pos.x < 200.0 - p.radius + 2.0, "x={}", b.pos.x);
        assert!(b.pos.z > 100.0, "slid along wall, z={}", b.pos.z);
    }

    #[test]
    fn climbs_a_small_step_but_not_a_big_one() {
        let mut tris = floor(0.0, -1000.0, 300.0, -1000.0, 1000.0);
        tris.extend(floor(25.0, 300.0, 1000.0, -1000.0, 1000.0));
        tris.extend(wall_x(300.0, 0.0, 25.0, -1000.0, 1000.0));
        let cw = CollisionWorld::new(tris);
        let p = BodyParams::default();
        let mut b = Body::new(v3(0.0, 0.0, 0.0));
        for _ in 0..180 {
            step_body(&cw, &p, &mut b, v3(300.0, 0.0, 0.0), 1.0 / 60.0, true);
        }
        assert!(b.pos.x > 400.0 && (b.pos.y - 25.0).abs() < 1.0, "{:?}", b.pos);

        let mut tris = floor(0.0, -1000.0, 300.0, -1000.0, 1000.0);
        tris.extend(floor(120.0, 300.0, 1000.0, -1000.0, 1000.0));
        tris.extend(wall_x(300.0, 0.0, 120.0, -1000.0, 1000.0));
        let cw = CollisionWorld::new(tris);
        let mut b = Body::new(v3(0.0, 0.0, 0.0));
        for _ in 0..180 {
            step_body(&cw, &p, &mut b, v3(300.0, 0.0, 0.0), 1.0 / 60.0, true);
        }
        assert!(b.pos.x < 300.0 && b.pos.y < 1.0, "{:?}", b.pos);
    }
}
