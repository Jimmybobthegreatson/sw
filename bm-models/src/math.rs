//! Tiny math helpers (column-major 4x4 matrices, AABBs, quaternions). Renderer-agnostic and
//! dependency-free; convert with `Mat4::to_cols_array` into your math library of choice.

/// Column-major 4x4 matrix: `m[col][row]`, i.e. `m[3]` is the translation column. Transforms
/// column vectors: `p' = M * p`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [[f32; 4]; 4]);

impl Mat4 {
    pub const IDENTITY: Mat4 =
        Mat4([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);

    /// From a row-major 3x3 rotation (`r[row*3+col]`) and a translation.
    pub fn from_rotation_translation(r: &[f32; 9], t: [f32; 3]) -> Mat4 {
        Mat4([
            [r[0], r[3], r[6], 0.0],
            [r[1], r[4], r[7], 0.0],
            [r[2], r[5], r[8], 0.0],
            [t[0], t[1], t[2], 1.0],
        ])
    }

    pub fn from_translation(t: [f32; 3]) -> Mat4 {
        Mat4::from_rotation_translation(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], t)
    }

    /// Flat column-major array (what glTF / OpenGL / wgpu expect).
    pub fn to_cols_array(&self) -> [f32; 16] {
        let m = &self.0;
        [
            m[0][0], m[0][1], m[0][2], m[0][3], m[1][0], m[1][1], m[1][2], m[1][3], m[2][0], m[2][1], m[2][2],
            m[2][3], m[3][0], m[3][1], m[3][2], m[3][3],
        ]
    }

    pub fn from_cols_array(a: &[f32; 16]) -> Mat4 {
        Mat4([[a[0], a[1], a[2], a[3]], [a[4], a[5], a[6], a[7]], [a[8], a[9], a[10], a[11]], [a[12], a[13], a[14], a[15]]])
    }

    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut r = [[0f32; 4]; 4];
        for c in 0..4 {
            for row in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += self.0[k][row] * o.0[c][k];
                }
                r[c][row] = s;
            }
        }
        Mat4(r)
    }

    pub fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
            m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
            m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
        ]
    }

    pub fn transform_vector(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2],
            m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2],
            m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2],
        ]
    }

    pub fn translation(&self) -> [f32; 3] {
        [self.0[3][0], self.0[3][1], self.0[3][2]]
    }

    /// Determinant of the upper 3x3 (negative = mirrored / winding flips).
    pub fn det3(&self) -> f32 {
        let m = &self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[2][1] * m[1][2]) - m[1][0] * (m[0][1] * m[2][2] - m[2][1] * m[0][2])
            + m[2][0] * (m[0][1] * m[1][2] - m[1][1] * m[0][2])
    }

    /// Inverse of an affine matrix (last row 0,0,0,1). Returns identity if singular.
    pub fn inverse_affine(&self) -> Mat4 {
        let m = &self.0;
        let det = self.det3();
        if det.abs() < 1e-12 {
            return Mat4::IDENTITY;
        }
        let id = 1.0 / det;
        let a = |c: usize, r: usize| m[c][r];
        let mut inv = [[0f32; 4]; 4];
        inv[0][0] = (a(1, 1) * a(2, 2) - a(2, 1) * a(1, 2)) * id;
        inv[0][1] = (a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2)) * id;
        inv[0][2] = (a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1)) * id;
        inv[1][0] = (a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2)) * id;
        inv[1][1] = (a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0)) * id;
        inv[1][2] = (a(1, 0) * a(0, 2) - a(0, 0) * a(1, 2)) * id;
        inv[2][0] = (a(1, 0) * a(2, 1) - a(2, 0) * a(1, 1)) * id;
        inv[2][1] = (a(2, 0) * a(0, 1) - a(0, 0) * a(2, 1)) * id;
        inv[2][2] = (a(0, 0) * a(1, 1) - a(1, 0) * a(0, 1)) * id;
        let t = [m[3][0], m[3][1], m[3][2]];
        for r in 0..3 {
            inv[3][r] = -(inv[0][r] * t[0] + inv[1][r] * t[1] + inv[2][r] * t[2]);
        }
        inv[3][3] = 1.0;
        Mat4(inv)
    }

    /// Compose from translation, unit quaternion `[x,y,z,w]` and scale.
    pub fn from_trs(t: [f32; 3], q: [f32; 4], s: [f32; 3]) -> Mat4 {
        let [x, y, z, w] = q;
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, xy, xz) = (x * x2, x * y2, x * z2);
        let (yy, yz, zz) = (y * y2, y * z2, z * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);
        Mat4([
            [(1.0 - (yy + zz)) * s[0], (xy + wz) * s[0], (xz - wy) * s[0], 0.0],
            [(xy - wz) * s[1], (1.0 - (xx + zz)) * s[1], (yz + wx) * s[1], 0.0],
            [(xz + wy) * s[2], (yz - wx) * s[2], (1.0 - (xx + yy)) * s[2], 0.0],
            [t[0], t[1], t[2], 1.0],
        ])
    }

    /// Decompose an affine matrix into translation, rotation quaternion `[x,y,z,w]` and scale
    /// (no shear support; mirrored matrices fold the sign into the X scale).
    pub fn decompose(&self) -> ([f32; 3], [f32; 4], [f32; 3]) {
        let m = &self.0;
        let len = |c: [f32; 4]| (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        let mut sx = len(m[0]);
        let sy = len(m[1]);
        let sz = len(m[2]);
        if self.det3() < 0.0 {
            sx = -sx;
        }
        let inv = |s: f32| if s.abs() > 1e-12 { 1.0 / s } else { 0.0 };
        let r = [
            [m[0][0] * inv(sx), m[0][1] * inv(sx), m[0][2] * inv(sx)],
            [m[1][0] * inv(sy), m[1][1] * inv(sy), m[1][2] * inv(sy)],
            [m[2][0] * inv(sz), m[2][1] * inv(sz), m[2][2] * inv(sz)],
        ];
        let (m00, m10, m20) = (r[0][0], r[0][1], r[0][2]);
        let (m01, m11, m21) = (r[1][0], r[1][1], r[1][2]);
        let (m02, m12, m22) = (r[2][0], r[2][1], r[2][2]);
        let tr = m00 + m11 + m22;
        let q = if tr > 0.0 {
            let s = (tr + 1.0).sqrt() * 2.0;
            [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
        } else if m00 > m11 && m00 > m22 {
            let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
            [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
        } else if m11 > m22 {
            let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
            [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
        } else {
            let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
            [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
        };
        (self.translation(), quat_normalize(q), [sx, sy, sz])
    }
}

impl Default for Mat4 {
    fn default() -> Self {
        Mat4::IDENTITY
    }
}

pub fn quat_normalize(q: [f32; 4]) -> [f32; 4] {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l < 1e-12 { [0.0, 0.0, 0.0, 1.0] } else { [q[0] / l, q[1] / l, q[2] / l, q[3] / l] }
}

/// Spherical (shortest path, normalised-lerp for small angles) interpolation.
pub fn quat_slerp(a: [f32; 4], mut b: [f32; 4], t: f32) -> [f32; 4] {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if d < 0.0 {
        d = -d;
        b = [-b[0], -b[1], -b[2], -b[3]];
    }
    if d > 0.9995 {
        return quat_normalize([
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
            a[3] + (b[3] - a[3]) * t,
        ]);
    }
    let th = d.clamp(-1.0, 1.0).acos();
    let s = th.sin();
    let (wa, wb) = (((1.0 - t) * th).sin() / s, (t * th).sin() / s);
    [a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb, a[2] * wa + b[2] * wb, a[3] * wa + b[3] * wb]
}

pub fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Axis aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: [f32::INFINITY; 3], max: [f32::NEG_INFINITY; 3] };

    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }

    pub fn extend(&mut self, p: [f32; 3]) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }

    pub fn union(&self, o: &Aabb) -> Aabb {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let mut r = *self;
        r.extend(o.min);
        r.extend(o.max);
        r
    }

    pub fn from_points<'a>(it: impl IntoIterator<Item = &'a [f32; 3]>) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in it {
            b.extend(*p);
        }
        b
    }

    pub fn center(&self) -> [f32; 3] {
        [(self.min[0] + self.max[0]) * 0.5, (self.min[1] + self.max[1]) * 0.5, (self.min[2] + self.max[2]) * 0.5]
    }

    pub fn size(&self) -> [f32; 3] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1], self.max[2] - self.min[2]]
    }

    /// World-space box of this box under `m` (transforms all 8 corners).
    pub fn transformed(&self, m: &Mat4) -> Aabb {
        let mut b = Aabb::EMPTY;
        if self.is_empty() {
            return b;
        }
        for i in 0..8 {
            let p = [
                if i & 1 == 0 { self.min[0] } else { self.max[0] },
                if i & 2 == 0 { self.min[1] } else { self.max[1] },
                if i & 4 == 0 { self.min[2] } else { self.max[2] },
            ];
            b.extend(m.transform_point(p));
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_roundtrip() {
        let m = Mat4::from_trs([1.0, 2.0, 3.0], quat_normalize([0.2, 0.5, -0.1, 0.8]), [1.0, 2.0, 0.5]);
        let i = m.inverse_affine();
        let p = [4.0, -1.0, 2.5];
        let q = i.transform_point(m.transform_point(p));
        for k in 0..3 {
            assert!((p[k] - q[k]).abs() < 1e-4);
        }
    }

    #[test]
    fn decompose_roundtrip() {
        let q = quat_normalize([0.3, -0.2, 0.6, 0.7]);
        let m = Mat4::from_trs([5.0, 6.0, 7.0], q, [1.0, 1.0, 1.0]);
        let (t, q2, s) = m.decompose();
        let m2 = Mat4::from_trs(t, q2, s);
        for c in 0..4 {
            for r in 0..4 {
                assert!((m.0[c][r] - m2.0[c][r]).abs() < 1e-4);
            }
        }
    }
}
