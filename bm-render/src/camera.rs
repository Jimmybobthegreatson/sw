//! Camera, projection and frustum culling.
//!
//! Conventions: native Glacier space (left-handed, Y-up, centimetres). The view looks down +Z for
//! `yaw = 0`, `+X` is right. The projection is *reverse-Z with an infinite far plane* (depth 1 at the near
//! plane, 0 at infinity) which keeps depth precision usable for the 100 km wide levels.
//! Because the data is left-handed and clockwise-wound exactly like Direct3D, no mirroring is needed:
//! the pipelines simply use `FrontFace::Cw`.

use bm_models::math::{Aabb, Mat4};

pub type Vec3 = [f32; 3];

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
pub fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l < 1e-12 { [0.0, 0.0, 1.0] } else { scale(a, 1.0 / l) }
}

/// Free camera described by position + yaw/pitch (radians).
#[derive(Clone, Debug)]
pub struct Camera {
    pub position: Vec3,
    /// Rotation about +Y; 0 looks along +Z, positive turns towards +X.
    pub yaw: f32,
    /// Positive looks up.
    pub pitch: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    /// Near plane in cm.
    pub near: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { position: [0.0, 170.0, 0.0], yaw: 0.0, pitch: 0.0, fov_y: 60f32.to_radians(), near: 10.0 }
    }
}

impl Camera {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [sy * cp, sp, cy * cp]
    }

    /// Right vector (left-handed: `cross(up, forward)`).
    pub fn right(&self) -> Vec3 {
        normalize(cross([0.0, 1.0, 0.0], self.forward()))
    }

    pub fn up(&self) -> Vec3 {
        cross(self.forward(), self.right())
    }

    /// Points the camera at `target`.
    pub fn look_at(&mut self, target: Vec3) {
        let d = normalize(sub(target, self.position));
        self.yaw = d[0].atan2(d[2]);
        self.pitch = d[1].clamp(-1.0, 1.0).asin();
    }

    /// World -> view (left-handed look-to).
    pub fn view_matrix(&self) -> Mat4 {
        let f = self.forward();
        let r = self.right();
        let u = self.up();
        let p = self.position;
        Mat4([
            [r[0], u[0], f[0], 0.0],
            [r[1], u[1], f[1], 0.0],
            [r[2], u[2], f[2], 0.0],
            [-dot(r, p), -dot(u, p), -dot(f, p), 1.0],
        ])
    }

    /// Reverse-Z infinite-far projection for wgpu clip space (x,y in -1..1, z in 0..1).
    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        let f = 1.0 / (self.fov_y * 0.5).tan();
        Mat4([[f / aspect, 0.0, 0.0, 0.0], [0.0, f, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, self.near, 0.0]])
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection_matrix(aspect).mul(&self.view_matrix())
    }

    pub fn frustum(&self, aspect: f32) -> Frustum {
        Frustum::from_view_proj(&self.view_proj(aspect))
    }
}

/// Five planes (left, right, bottom, top, near); the far plane is at infinity.
#[derive(Clone, Debug)]
pub struct Frustum {
    pub planes: [[f32; 4]; 5],
}

impl Frustum {
    pub fn from_view_proj(vp: &Mat4) -> Frustum {
        let m = &vp.0;
        let row = |r: usize| [m[0][r], m[1][r], m[2][r], m[3][r]];
        let (r0, r1, r3) = (row(0), row(1), row(3));
        let comb = |a: [f32; 4], b: [f32; 4], s: f32| {
            let mut p = [a[0] + s * b[0], a[1] + s * b[1], a[2] + s * b[2], a[3] + s * b[3]];
            let l = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-12);
            for v in &mut p {
                *v /= l;
            }
            p
        };
        // clip.w = view z, so "w >= 0" is the near half space.
        let mut near = r3;
        let l = (near[0] * near[0] + near[1] * near[1] + near[2] * near[2]).sqrt().max(1e-12);
        for v in &mut near {
            *v /= l;
        }
        Frustum { planes: [comb(r3, r0, 1.0), comb(r3, r0, -1.0), comb(r3, r1, 1.0), comb(r3, r1, -1.0), near] }
    }

    /// Conservative AABB test (`false` = certainly outside).
    pub fn intersects_aabb(&self, b: &Aabb) -> bool {
        if b.is_empty() {
            return true;
        }
        for p in &self.planes {
            let v = [
                if p[0] >= 0.0 { b.max[0] } else { b.min[0] },
                if p[1] >= 0.0 { b.max[1] } else { b.min[1] },
                if p[2] >= 0.0 { b.max[2] } else { b.min[2] },
            ];
            if p[0] * v[0] + p[1] * v[1] + p[2] * v[2] + p[3] < 0.0 {
                return false;
            }
        }
        true
    }
}

/// Keyboard/mouse driven fly controller (feed it from winit events; no winit dependency here).
#[derive(Clone, Debug, Default)]
pub struct FlyController {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub fast: bool,
    /// Base speed in cm/s (0 = default 800).
    pub speed: f32,
}

impl FlyController {
    pub fn update(&self, cam: &mut Camera, dt: f32) {
        let base = if self.speed > 0.0 { self.speed } else { 800.0 };
        let v = base * if self.fast { 5.0 } else { 1.0 } * dt;
        let f = cam.forward();
        let r = cam.right();
        let ax = |p: bool, n: bool| (p as i32 - n as i32) as f32;
        let mv = add(add(scale(f, ax(self.forward, self.back)), scale(r, ax(self.right, self.left))), [0.0, ax(self.up, self.down), 0.0]);
        cam.position = add(cam.position, scale(mv, v));
    }

    /// Mouse look; `dx`, `dy` in pixels.
    pub fn look(cam: &mut Camera, dx: f32, dy: f32) {
        cam.yaw += dx * 0.0025;
        cam.pitch = (cam.pitch - dy * 0.0025).clamp(-1.55, 1.55);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basis_is_left_handed_x_right() {
        let c = Camera::default();
        assert!((c.right()[0] - 1.0).abs() < 1e-6);
        assert!((c.up()[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn point_in_front_projects_inside() {
        let c = Camera::default();
        let vp = c.view_proj(1.0);
        let p = vp.transform_vector([0.0, 0.0, 0.0]);
        let _ = p;
        let fr = c.frustum(1.0);
        let inside = Aabb { min: [-10.0, 160.0, 500.0], max: [10.0, 180.0, 520.0] };
        let behind = Aabb { min: [-10.0, 160.0, -520.0], max: [10.0, 180.0, -500.0] };
        assert!(fr.intersects_aabb(&inside));
        assert!(!fr.intersects_aabb(&behind));
    }
}
