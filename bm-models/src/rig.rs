//! The humanoid "rig solver": turns the 74 decoded animation parameters of one frame (stream A of a clip, see
//! [`crate::anm_decode`]) into local bone transforms of a PRM skeleton.
//!
//! The parameter vector has 104 floats (`PARAM_COUNT`): ten quaternions (`q0..q9`, 4 floats each) followed by 64
//! scalars (ids 10..73). Everything below was derived from the data and validated against the original
//! engine's own solver (bit exact quaternions for fingers/feet/hands/neck/head/spine/clavicles, <0.01 for the
//! IK limbs; spine translations within 1 cm):
//!
//! | parameter | meaning |
//! |---|---|
//! | `q0` | `PELVIS` local rotation; `s10..12` = pelvis position in units of the leg length (see [`RigSolver::leg_length`]) |
//! | `q1` | spine rotation, shared in three equal parts by `SPINE`, `SPINE_1`, `SPINE_2`; `s13..15` = position of the `SPINE_2` joint in the pelvis frame (cm) |
//! | `q2`, `q3` | `NECK`, `HEAD` local rotations |
//! | `q4`, `q5` | left / right `FOOT` local rotation; `s19`, `s24` swivel, `s20`, `s25` toe angle |
//! | `s16..18`, `s21..23` | left / right ankle position, absolute, **Z up** (`x, z, y` of the Y-up model space); two-bone IK from the hip |
//! | `q6`, `q8` | left / right `HAND` local rotation (its twist about X is spread over the forearm twist bones, 0.32 each) |
//! | `q7`, `q9` | clavicle rotation relative to the bind pose (`bind * q`) |
//! | `s26..28`, `s30..32` | left / right wrist position in 1/100 cm in the `SPINE_2` frame (axes `x, z, y`); `s29`, `s33` elbow swivel |
//! | `s34..53`, `s54..73` | left / right fingers: 4 fingers x (euler xyz of the base joint + one flex angle for the two other joints), then the thumb |
//!
//! The IK limbs aim the joint plane's hinge axis from a reference frame (pelvis*bind-thigh for legs, the clavicle for
//! arms), twist it by the swivel angle, and solve the knee / elbow with the law of cosines. Bones that the rig
//! does not drive (face, helper "xtra" muscles) stay at the bind pose.

use crate::math::Mat4;
use crate::skeleton::Skeleton;

/// Length of the parameter vector of one frame.
pub const PARAM_COUNT: usize = 104;

type V3 = [f64; 3];
type M3 = [[f64; 3]; 3];
type Q = [f64; 4];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn norm(a: V3) -> V3 {
    let l = len(a);
    if l < 1e-12 { [1.0, 0.0, 0.0] } else { scale(a, 1.0 / l) }
}
fn mv(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}
fn mm(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    r
}
fn tr(m: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = m[j][i];
        }
    }
    r
}
fn col(m: &M3, c: usize) -> V3 {
    [m[0][c], m[1][c], m[2][c]]
}
fn from_cols(x: V3, y: V3, z: V3) -> M3 {
    [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
}

fn qnorm(q: Q) -> Q {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l < 1e-12 { [0.0, 0.0, 0.0, 1.0] } else { [q[0] / l, q[1] / l, q[2] / l, q[3] / l] }
}

fn qmul(a: Q, b: Q) -> Q {
    let [x1, y1, z1, w1] = a;
    let [x2, y2, z2, w2] = b;
    [
        w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
        w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
        w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
        w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
    ]
}

/// Rotation matrix of a unit quaternion `[x, y, z, w]` (column vectors, `m[row][col]`).
fn qmat(q: Q) -> M3 {
    let [x, y, z, w] = qnorm(q);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

fn mat_to_quat(r: &M3) -> Q {
    let t = r[0][0] + r[1][1] + r[2][2];
    let q = if t > 0.0 {
        let s = (t + 1.0).sqrt() * 2.0;
        [(r[2][1] - r[1][2]) / s, (r[0][2] - r[2][0]) / s, (r[1][0] - r[0][1]) / s, s / 4.0]
    } else if r[0][0] >= r[1][1] && r[0][0] >= r[2][2] {
        let s = (1.0 + r[0][0] - r[1][1] - r[2][2]).sqrt() * 2.0;
        [s / 4.0, (r[0][1] + r[1][0]) / s, (r[0][2] + r[2][0]) / s, (r[2][1] - r[1][2]) / s]
    } else if r[1][1] >= r[2][2] {
        let s = (1.0 + r[1][1] - r[0][0] - r[2][2]).sqrt() * 2.0;
        [(r[0][1] + r[1][0]) / s, s / 4.0, (r[1][2] + r[2][1]) / s, (r[0][2] - r[2][0]) / s]
    } else {
        let s = (1.0 + r[2][2] - r[0][0] - r[1][1]).sqrt() * 2.0;
        [(r[0][2] + r[2][0]) / s, (r[1][2] + r[2][1]) / s, s / 4.0, (r[1][0] - r[0][1]) / s]
    };
    let q = qnorm(q);
    if q[3] < 0.0 { [-q[0], -q[1], -q[2], -q[3]] } else { q }
}

/// Rotation by `ang` radians about the (not necessarily unit) `axis`.
fn rot_axis(axis: V3, ang: f64) -> M3 {
    let a = norm(axis);
    let (s, c) = ang.sin_cos();
    let k = [[0.0, -a[2], a[1]], [a[2], 0.0, -a[0]], [-a[1], a[0], 0.0]];
    let kk = mm(&k, &k);
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = if i == j { 1.0 } else { 0.0 } + s * k[i][j] + (1.0 - c) * kk[i][j];
        }
    }
    r
}

/// Shortest-arc rotation taking direction `a` to direction `b`.
fn arc(a: V3, b: V3) -> M3 {
    let (a, b) = (norm(a), norm(b));
    let v = cross(a, b);
    let c = dot(a, b);
    if c < -0.99999 {
        let axis = if a[0].abs() < 0.9 { cross(a, [1.0, 0.0, 0.0]) } else { cross(a, [0.0, 1.0, 0.0]) };
        return rot_axis(axis, std::f64::consts::PI);
    }
    let k = [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]];
    let kk = mm(&k, &k);
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = if i == j { 1.0 } else { 0.0 } + k[i][j] + kk[i][j] / (1.0 + c);
        }
    }
    r
}

/// `q^f` (rotation about the same axis by `f` times the angle).
fn qpow(q: Q, f: f64) -> Q {
    let mut q = qnorm(q);
    if q[3] < 0.0 {
        q = [-q[0], -q[1], -q[2], -q[3]];
    }
    let s = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    if s < 1e-9 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let ang = 2.0 * q[3].clamp(-1.0, 1.0).acos();
    let k = (ang * f / 2.0).sin() / s;
    [q[0] * k, q[1] * k, q[2] * k, (ang * f / 2.0).cos()]
}

/// Quaternion from the engine's "euler" triple (angles negated and halved, order matching the original).
fn euler_q(v: [f64; 3]) -> Q {
    let (hx, hy, hz) = (-0.5 * v[0], -0.5 * v[1], -0.5 * v[2]);
    let (cx, sx, cy, sy, cz, sz) = (hx.cos(), hx.sin(), hy.cos(), hy.sin(), hz.cos(), hz.sin());
    [cx * sy * sz + sx * cy * cz, cx * sy * cz - sx * cy * sz, sx * sy * cz + cx * cy * sz, cy * cz * cx - sy * sz * sx]
}

/// Rotation of `ang` about local Y: the flexion of hinge joints (`(0, sin, 0, cos)` of the half angle).
fn rot_y_q(ang: f64) -> Q {
    [0.0, (ang / 2.0).sin(), 0.0, (ang / 2.0).cos()]
}

/// Which bones of a [`Skeleton`] the solver drives.
#[derive(Clone, Debug)]
struct Bones {
    pelvis: usize,
    spine: [usize; 3],
    neck: usize,
    head: usize,
    thigh: [usize; 2],
    calf: [usize; 2],
    foot: [usize; 2],
    toe: [usize; 2],
    clav: [usize; 2],
    ua: [usize; 2],
    fa: [usize; 2],
    hand: [usize; 2],
    twist: [usize; 2],
    twist1: [usize; 2],
    /// per side: 4 fingers x 3 bones, then the thumb x 3
    fingers: [[usize; 15]; 2],
}

/// Output of [`RigSolver::solve`]: a new local rotation (and optionally translation) for one bone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolvedBone {
    pub bone: usize,
    /// Local rotation `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Local translation, only for the pelvis and the three spine bones.
    pub translation: Option<[f32; 3]>,
}

/// Solver bound to one skeleton (bind pose lengths and frames are taken from it).
#[derive(Clone, Debug)]
pub struct RigSolver {
    b: Bones,
    bind_t: Vec<V3>,
    bind_q: Vec<Q>,
    leg_len: f64,
}

impl RigSolver {
    /// `None` if the skeleton lacks any of the humanoid bones (weapons, props).
    pub fn new(skel: &Skeleton) -> Option<RigSolver> {
        let ix = |n: &str| skel.bone_index(n);
        let pair = |f: &dyn Fn(&str) -> String| -> Option<[usize; 2]> { Some([ix(&f("LEFT"))?, ix(&f("RIGHT"))?]) };
        let mut fingers = [[0usize; 15]; 2];
        for (si, side) in ["LEFT", "RIGHT"].iter().enumerate() {
            let mut n = 0;
            for f in [1, 2, 3, 4, 0] {
                for suffix in ["", "_1", "_2"] {
                    fingers[si][n] = ix(&format!("{side}_FINGER_{f}{suffix}"))?;
                    n += 1;
                }
            }
        }
        let b = Bones {
            pelvis: ix("PELVIS")?,
            spine: [ix("SPINE")?, ix("SPINE_1")?, ix("SPINE_2")?],
            neck: ix("NECK")?,
            head: ix("HEAD")?,
            thigh: pair(&|s| format!("{s}_THIGH"))?,
            calf: pair(&|s| format!("{s}_CALF"))?,
            foot: pair(&|s| format!("{s}_FOOT"))?,
            toe: pair(&|s| format!("{s}_TOE"))?,
            clav: pair(&|s| format!("{s}_CLAVICLE"))?,
            ua: pair(&|s| format!("{s}_UPPER_ARM"))?,
            fa: pair(&|s| format!("{s}_FOREARM"))?,
            hand: pair(&|s| format!("{s}_HAND"))?,
            twist: pair(&|s| format!("{s}_FORETWIST"))?,
            twist1: pair(&|s| format!("{s}_FORETWIST1"))?,
            fingers,
        };
        let mut bind_t = Vec::new();
        let mut bind_q = Vec::new();
        for bone in &skel.bones {
            let m: &Mat4 = &bone.bind_local;
            let t = m.translation();
            let (_, q, _) = m.decompose();
            bind_t.push([t[0] as f64, t[1] as f64, t[2] as f64]);
            bind_q.push([q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64]);
        }
        let l = |i: usize| len(bind_t[i]);
        let leg_len = 0.5 * (l(b.calf[0]) + l(b.foot[0]) + l(b.calf[1]) + l(b.foot[1]));
        Some(RigSolver { b, bind_t, bind_q, leg_len })
    }

    /// Average leg length of the skeleton (cm); the pelvis position parameters are expressed in this unit.
    pub fn leg_length(&self) -> f32 {
        self.leg_len as f32
    }

    /// Bones driven by [`RigSolver::solve`].
    pub fn driven_bones(&self) -> Vec<usize> {
        let b = &self.b;
        let mut v = vec![b.pelvis, b.neck, b.head];
        v.extend(b.spine);
        for s in 0..2 {
            v.extend([b.thigh[s], b.calf[s], b.foot[s], b.toe[s], b.clav[s], b.ua[s], b.fa[s], b.hand[s], b.twist[s], b.twist1[s]]);
            v.extend(b.fingers[s]);
        }
        v
    }

    /// Solves one frame of parameters (`params.len() >= PARAM_COUNT`) into local bone transforms.
    pub fn solve(&self, params: &[f32]) -> Vec<SolvedBone> {
        let p: Vec<f64> = params.iter().take(PARAM_COUNT).map(|&x| x as f64).collect();
        if p.len() < PARAM_COUNT {
            return Vec::new();
        }
        let b = &self.b;
        let quat = |k: usize| -> Q { [p[4 * k], p[4 * k + 1], p[4 * k + 2], p[4 * k + 3]] };
        let sc = |id: usize| p[30 + id];
        let mut out: Vec<SolvedBone> = Vec::with_capacity(80);
        let mut push = |bone: usize, q: Q, t: Option<V3>| {
            let q = qnorm(q);
            out.push(SolvedBone {
                bone,
                rotation: [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32],
                translation: t.map(|t| [t[0] as f32, t[1] as f32, t[2] as f32]),
            });
        };

        // pelvis
        let q0 = qnorm(quat(0));
        let rp = qmat(q0);
        let pelvis_pos = scale([sc(10), sc(11), sc(12)], self.leg_len);
        push(b.pelvis, q0, Some(pelvis_pos));

        // spine: the rotation is shared in thirds, the positions are bent so that the SPINE_2 joint lands on s13..15
        let third = qpow(quat(1), 1.0 / 3.0);
        let r3 = qmat(third);
        let target = [sc(13), sc(14), sc(15)];
        let bs = [self.bind_t[b.spine[0]], self.bind_t[b.spine[1]], self.bind_t[b.spine[2]]];
        let ls = [len(bs[0]), len(bs[1]), len(bs[2])];
        let reach = add(bs[0], mv(&r3, add(bs[1], mv(&r3, bs[2]))));
        let e = sub(target, reach);
        let wsum = (ls[0] + ls[1] + ls[2]).max(1e-9);
        let r3t = tr(&r3);
        let p0 = add(bs[0], scale(e, ls[0] / wsum));
        let p1 = add(bs[1], scale(mv(&r3t, e), ls[1] / wsum));
        let p2 = add(bs[2], scale(mv(&r3t, mv(&r3t, e)), ls[2] / wsum));
        push(b.spine[0], third, Some(p0));
        push(b.spine[1], third, Some(p1));
        push(b.spine[2], third, Some(p2));
        push(b.neck, quat(2), None);
        push(b.head, quat(3), None);
        let rsp2 = mm(&mm(&rp, &r3), &mm(&r3, &r3));
        let sp2_pos = add(pelvis_pos, mv(&rp, target));

        for side in 0..2 {
            // ---- leg
            let (s0, qf) = if side == 0 { (16, 4) } else { (21, 5) };
            let ankle = [sc(s0), sc(s0 + 2), sc(s0 + 1)];
            let swivel = sc(s0 + 3);
            let hip = add(pelvis_pos, mv(&rp, self.bind_t[b.thigh[side]]));
            let rbase = mm(&rp, &qmat(self.bind_q[b.thigh[side]]));
            let l1 = len(self.bind_t[b.calf[side]]);
            let l2 = len(self.bind_t[b.foot[side]]);
            let (rt, beta) = two_bone(hip, &rbase, ankle, l1, l2, swivel);
            push(b.thigh[side], mat_to_quat(&mm(&tr(&rp), &rt)), None);
            push(b.calf[side], rot_y_q(beta), None);
            push(b.foot[side], quat(qf), None);
            push(b.toe[side], rot_y_q(-sc(s0 + 4) - std::f64::consts::FRAC_PI_2), None);

            // ---- arm
            let (s0, qh, qc) = if side == 0 { (26, 6, 7) } else { (30, 8, 9) };
            let q_cl = qmul(qnorm(self.bind_q[b.clav[side]]), quat(qc));
            let r_cl = mm(&rsp2, &qmat(q_cl));
            let cl_pos = add(sp2_pos, mv(&rsp2, self.bind_t[b.clav[side]]));
            let shoulder = add(cl_pos, mv(&r_cl, self.bind_t[b.ua[side]]));
            let wrist = add(sp2_pos, mv(&rsp2, scale([sc(s0), sc(s0 + 2), sc(s0 + 1)], 100.0)));
            let l1 = len(self.bind_t[b.fa[side]]);
            let l2 = len(self.bind_t[b.hand[side]]);
            let swivel = sc(s0 + 3);
            let ang = if side == 0 { swivel + std::f64::consts::FRAC_PI_2 } else { -swivel - std::f64::consts::FRAC_PI_2 };
            let (rua, beta) = two_bone(shoulder, &r_cl, wrist, l1, l2, ang);
            push(b.clav[side], q_cl, None);
            push(b.ua[side], mat_to_quat(&mm(&tr(&r_cl), &rua)), None);
            let q_fa = rot_y_q(beta);
            push(b.fa[side], q_fa, None);
            let hq = qnorm(quat(qh));
            push(b.hand[side], hq, None);
            // hand twist about X (relative to the bind twist) is spread over the two forearm twist bones
            let bind_twist = if side == 0 { std::f64::consts::FRAC_PI_2 } else { -std::f64::consts::FRAC_PI_2 };
            let hq_pos = if hq[3] < 0.0 { [-hq[0], -hq[1], -hq[2], -hq[3]] } else { hq };
            let mut delta = 2.0 * hq_pos[0].atan2(hq_pos[3]) - bind_twist;
            delta = (delta + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
            let tau = 0.32 * delta;
            let rx = [(tau / 2.0).sin(), 0.0, 0.0, (tau / 2.0).cos()];
            push(b.twist[side], qmul(q_fa, rx), None);
            push(b.twist1[side], rx, None);

            // ---- fingers
            let f0 = if side == 0 { 34 } else { 54 };
            for k in 0..5 {
                let base = f0 + 4 * k;
                let q = euler_q([sc(base), sc(base + 1), sc(base + 2)]);
                let flex = rot_y_q(-sc(base + 3));
                let [a, bb, c] = [b.fingers[side][3 * k], b.fingers[side][3 * k + 1], b.fingers[side][3 * k + 2]];
                push(a, q, None);
                push(bb, flex, None);
                push(c, flex, None);
            }
        }
        out
    }
}

/// Two bone IK. `origin` is the root joint position, `rbase` the reference frame whose X axis is aimed at the
/// target (its Y axis becomes the hinge), `swivel` rotates the joint plane about the root-target axis.
/// Returns the root bone's world rotation (X along the first bone) and the hinge flexion angle of the second bone.
fn two_bone(origin: V3, rbase: &M3, target: V3, l1: f64, l2: f64, swivel: f64) -> (M3, f64) {
    let v = sub(target, origin);
    let d = len(v).max(1e-6);
    let ax = scale(v, 1.0 / d);
    let q = arc(col(rbase, 0), ax);
    let rt0 = mm(&mm(&rot_axis(ax, swivel), &q), rbase);
    let hinge = col(&rt0, 1);
    let dd = d.min(l1 + l2 - 1e-4).max((l1 - l2).abs() + 1e-4);
    let a1 = ((l1 * l1 + dd * dd - l2 * l2) / (2.0 * l1 * dd)).clamp(-1.0, 1.0).acos();
    let xk = mv(&rot_axis(hinge, -a1), ax);
    let rroot = from_cols(xk, hinge, cross(xk, hinge));
    let knee = add(origin, scale(xk, l1));
    let d2 = norm(sub(target, knee));
    let loc = mv(&tr(&rroot), d2);
    let beta = (-loc[2]).atan2(loc[0]);
    (rroot, beta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::quat_normalize;
    use crate::skeleton::Bone;

    #[test]
    fn quaternion_matrix_roundtrip() {
        let q = qnorm([0.3, -0.5, 0.2, 0.7]);
        let r = qmat(q);
        let q2 = mat_to_quat(&r);
        let d: f64 = q.iter().zip(q2.iter()).map(|(a, b)| (a - b).abs()).sum();
        let d2: f64 = q.iter().zip(q2.iter()).map(|(a, b)| (a + b).abs()).sum();
        assert!(d < 1e-9 || d2 < 1e-9);
    }

    #[test]
    fn arc_and_axis_rotations() {
        let a = norm([1.0, 2.0, -0.5]);
        let b = norm([-0.3, 0.4, 1.0]);
        let r = arc(a, b);
        let ra = mv(&r, a);
        for i in 0..3 {
            assert!((ra[i] - b[i]).abs() < 1e-9);
        }
        let r = rot_axis([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2);
        let v = mv(&r, [1.0, 0.0, 0.0]);
        assert!((v[1] - 1.0).abs() < 1e-12 && v[0].abs() < 1e-12);
        // opposite directions
        let r = arc([1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]);
        let v = mv(&r, [1.0, 0.0, 0.0]);
        assert!((v[0] + 1.0).abs() < 1e-9);
    }

    #[test]
    fn two_bone_reaches_the_target() {
        let rbase = qmat([0.0, 0.0, 0.0, 1.0]);
        let origin = [0.0, 0.0, 0.0];
        let target = [30.0, 20.0, 10.0];
        let (l1, l2) = (26.0, 23.0);
        let (rt, beta) = two_bone(origin, &rbase, target, l1, l2, 0.3);
        // forward kinematics: knee = origin + rt * (l1,0,0); tip = knee + rt*Ry(beta)*(l2,0,0)
        let knee = add(origin, mv(&rt, [l1, 0.0, 0.0]));
        let ry = qmat(rot_y_q(beta));
        let tip = add(knee, mv(&mm(&rt, &ry), [l2, 0.0, 0.0]));
        for i in 0..3 {
            assert!((tip[i] - target[i]).abs() < 1e-6, "{tip:?}");
        }
    }

    #[test]
    fn spine_distribution_hits_the_target() {
        // synthetic skeleton with the bones the solver needs
        let mut bones: Vec<Bone> = Vec::new();
        let mut add_bone = |name: &str, parent: Option<usize>, t: [f32; 3]| {
            let m = Mat4::from_translation(t);
            bones.push(Bone { name: name.into(), parent, bind_local: m, bind_world: m });
            bones.len() - 1
        };
        let g = add_bone("GROUND", None, [0.0; 3]);
        let pel = add_bone("PELVIS", Some(g), [0.0, 97.0, 0.0]);
        let s = add_bone("SPINE", Some(pel), [9.0, 0.0, 0.0]);
        let s1 = add_bone("SPINE_1", Some(s), [10.0, 0.0, 0.0]);
        let s2 = add_bone("SPINE_2", Some(s1), [10.0, 0.0, 0.0]);
        let neck = add_bone("NECK", Some(s2), [27.0, 0.0, 0.0]);
        add_bone("HEAD", Some(neck), [8.0, 0.0, 0.0]);
        for side in ["LEFT", "RIGHT"] {
            let th = add_bone(&format!("{side}_THIGH"), Some(pel), [0.0, 9.0, 0.0]);
            let ca = add_bone(&format!("{side}_CALF"), Some(th), [45.0, 0.0, 0.0]);
            let ft = add_bone(&format!("{side}_FOOT"), Some(ca), [42.0, 0.0, 0.0]);
            add_bone(&format!("{side}_TOE"), Some(ft), [10.0, 0.0, 0.0]);
            let cl = add_bone(&format!("{side}_CLAVICLE"), Some(s2), [24.0, 3.0, 0.0]);
            let ua = add_bone(&format!("{side}_UPPER_ARM"), Some(cl), [17.0, 0.0, 0.0]);
            let fa = add_bone(&format!("{side}_FOREARM"), Some(ua), [26.0, 0.0, 0.0]);
            let hd = add_bone(&format!("{side}_HAND"), Some(fa), [23.0, 0.0, 0.0]);
            let tw = add_bone(&format!("{side}_FORETWIST"), Some(fa), [0.0; 3]);
            add_bone(&format!("{side}_FORETWIST1"), Some(tw), [11.0, 0.0, 0.0]);
            for f in [1, 2, 3, 4, 0] {
                let a = add_bone(&format!("{side}_FINGER_{f}"), Some(hd), [3.0, 0.0, 0.0]);
                let b = add_bone(&format!("{side}_FINGER_{f}_1"), Some(a), [3.0, 0.0, 0.0]);
                add_bone(&format!("{side}_FINGER_{f}_2"), Some(b), [3.0, 0.0, 0.0]);
            }
        }
        let skel = Skeleton { bones, facial_poses: vec![], face: vec![] };
        let solver = RigSolver::new(&skel).expect("humanoid");
        assert!((solver.leg_length() - 87.0).abs() < 1e-3);
        let mut p = vec![0.0f32; PARAM_COUNT];
        for k in 0..10 {
            p[4 * k + 3] = 1.0;
        }
        // spine rotation + a target slightly off the straight chain
        p[4..8].copy_from_slice(&quat_normalize([0.0, 0.2, 0.1, 0.9]));
        p[40 + 1] = 1.1; // pelvis y
        p[40 + 3] = 28.5;
        p[40 + 4] = 1.0;
        p[40 + 5] = 2.0;
        // standing ankles
        p[30 + 16] = 8.8;
        p[30 + 17] = 0.0;
        p[30 + 18] = 9.7;
        p[30 + 21] = -8.8;
        p[30 + 23] = 9.7;
        let out = solver.solve(&p);
        let get = |name: &str| out.iter().find(|s| s.bone == skel.bone_index(name).unwrap()).unwrap();
        let q = |s: &SolvedBone| [s.rotation[0] as f64, s.rotation[1] as f64, s.rotation[2] as f64, s.rotation[3] as f64];
        let r3 = qmat(q(get("SPINE")));
        let t = |s: &SolvedBone| s.translation.map(|v| [v[0] as f64, v[1] as f64, v[2] as f64]).unwrap();
        let reach = add(t(get("SPINE")), mv(&r3, add(t(get("SPINE_1")), mv(&r3, t(get("SPINE_2"))))));
        assert!((reach[0] - 28.5).abs() < 1e-3 && (reach[1] - 1.0).abs() < 1e-3 && (reach[2] - 2.0).abs() < 1e-3, "{reach:?}");
        // every driven bone got exactly one result
        let mut ids: Vec<usize> = out.iter().map(|s| s.bone).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), out.len());
        assert_eq!(ids.len(), solver.driven_bones().len());
    }
}
