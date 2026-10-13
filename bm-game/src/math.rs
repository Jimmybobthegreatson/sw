//! Small vector math in native engine space (left-handed, Y up, centimetres).
//!
//! Heading convention: `yaw = 0` faces `+Z`, a positive yaw turns towards `+X`
//! (`forward(yaw) = (sin yaw, 0, cos yaw)`). The rotation matrix that maps local `+Z` to `forward(yaw)`
//! is `Ry(yaw)` below, and matches the engine's proper rotation convention (see `docs/models.md`).

use bm_models::Mat4;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

impl Vec3 {
    pub const ZERO: Vec3 = v3(0.0, 0.0, 0.0);
    pub const UP: Vec3 = v3(0.0, 1.0, 0.0);

    pub fn from_array(a: [f32; 3]) -> Vec3 {
        v3(a[0], a[1], a[2])
    }
    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }
    pub fn dist(self, o: Vec3) -> f32 {
        (self - o).len()
    }
    pub fn dist_xz(self, o: Vec3) -> f32 {
        let (dx, dz) = (self.x - o.x, self.z - o.z);
        (dx * dx + dz * dz).sqrt()
    }
    /// Unit vector, or `fallback` for (near) zero input.
    pub fn norm_or(self, fallback: Vec3) -> Vec3 {
        let l = self.len();
        if l < 1e-9 { fallback } else { self * (1.0 / l) }
    }
    pub fn norm(self) -> Vec3 {
        self.norm_or(Vec3::ZERO)
    }
    pub fn lerp(self, o: Vec3, t: f32) -> Vec3 {
        self + (o - self) * t
    }
    pub fn with_y(self, y: f32) -> Vec3 {
        v3(self.x, y, self.z)
    }
    pub fn flat(self) -> Vec3 {
        self.with_y(0.0)
    }
    pub fn min(self, o: Vec3) -> Vec3 {
        v3(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    pub fn max(self, o: Vec3) -> Vec3 {
        v3(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
    pub fn axis(self, i: usize) -> f32 {
        match i {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
}
impl Div<f32> for Vec3 {
    type Output = Vec3;
    fn div(self, s: f32) -> Vec3 {
        v3(self.x / s, self.y / s, self.z / s)
    }
}
impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        v3(-self.x, -self.y, -self.z)
    }
}
impl AddAssign for Vec3 {
    fn add_assign(&mut self, o: Vec3) {
        *self = *self + o;
    }
}
impl SubAssign for Vec3 {
    fn sub_assign(&mut self, o: Vec3) {
        *self = *self - o;
    }
}

impl From<bm_script::Vec3> for Vec3 {
    fn from(v: bm_script::Vec3) -> Vec3 {
        v3(v.x, v.y, v.z)
    }
}
impl From<Vec3> for bm_script::Vec3 {
    fn from(v: Vec3) -> bm_script::Vec3 {
        bm_script::Vec3::new(v.x, v.y, v.z)
    }
}

/// Unit forward vector of a heading (see module docs).
pub fn forward(yaw: f32) -> Vec3 {
    v3(yaw.sin(), 0.0, yaw.cos())
}

/// Heading of a horizontal direction (0 for a zero vector).
pub fn yaw_of(d: Vec3) -> f32 {
    if d.x.abs() < 1e-6 && d.z.abs() < 1e-6 { 0.0 } else { d.x.atan2(d.z) }
}

/// Smallest signed difference `b - a` of two angles, in `(-pi, pi]`.
pub fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = (b - a) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d <= -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

/// Turns `a` towards `b` by at most `max_step` radians.
pub fn turn_towards(a: f32, b: f32, max_step: f32) -> f32 {
    let d = angle_diff(a, b);
    a + d.clamp(-max_step, max_step)
}

/// `T(pos) * Ry(yaw)` as an engine matrix (column vectors).
pub fn pose_matrix(pos: Vec3, yaw: f32) -> Mat4 {
    let (s, c) = yaw.sin_cos();
    Mat4([[c, 0.0, -s, 0.0], [0.0, 1.0, 0.0, 0.0], [s, 0.0, c, 0.0], [pos.x, pos.y, pos.z, 1.0]])
}

/// Heading of the local `+Z` axis of a matrix.
pub fn matrix_yaw(m: &Mat4) -> f32 {
    let f = m.transform_vector([0.0, 0.0, 1.0]);
    yaw_of(v3(f[0], 0.0, f[2]))
}

/// Characters face local **-Z** in model space (bind pose toes, walk clip root motion), while the simulation's heading
/// ([`forward`]) is +Z. Spawn yaws of characters come from [`character_yaw`] and the render matrix gets [`character_render`].
pub fn character_yaw(m: &Mat4) -> f32 {
    let f = m.transform_vector([0.0, 0.0, -1.0]);
    yaw_of(v3(f[0], 0.0, f[2]))
}

/// World matrix used to draw a character: the simulation pose turned half a circle (model forward is -Z).
pub fn character_render(world: &Mat4) -> Mat4 {
    world.mul(&pose_matrix(Vec3::ZERO, std::f32::consts::PI))
}

pub fn mat_pos(m: &Mat4) -> Vec3 {
    Vec3::from_array(m.translation())
}

pub fn xform_point(m: &Mat4, p: Vec3) -> Vec3 {
    Vec3::from_array(m.transform_point(p.to_array()))
}

/// Degrees (as scripts use) to a euler triple: x = pitch, y = yaw, z = roll.
pub fn rad_to_deg3(yaw: f32) -> Vec3 {
    v3(0.0, yaw.to_degrees(), 0.0)
}

/// Deterministic xorshift64* generator (no `rand` dependency, state is part of the saved game state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    /// Uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { self.next_u32() as usize % n }
    }
}

#[cfg(test)]
mod facing_tests {
    use super::*;

    #[test]
    fn character_front_is_model_minus_z_along_the_simulation_heading() {
        for yaw in [0.0f32, 0.7, 2.0, -1.3, 3.0] {
            let render = character_render(&pose_matrix(v3(10.0, 0.0, 20.0), yaw));
            let front = render.transform_vector([0.0, 0.0, -1.0]);
            let f = forward(yaw);
            assert!((front[0] - f.x).abs() < 1e-4 && (front[2] - f.z).abs() < 1e-4, "yaw {yaw}: {front:?} vs {f:?}");
            // a node whose model faces -Z gives back the same heading
            assert!((angle_diff(character_yaw(&pose_matrix(Vec3::ZERO, yaw + std::f32::consts::PI)), yaw)).abs() < 1e-4);
        }
    }
}
