//! Skeletons (bone hierarchy + bind pose) and poses / skinning matrices.
//!
//! Skinned PRM models (`Skinned34` meshes) carry their skeleton in the model's "parts" chunk (see
//! `docs/models.md`, section *Skeleton block*): bone names, parents, world and local bind
//! matrices. The bind data was verified by the closed-loop test `world == parent_world * local`
//! for all 100 bones of the player model and by the anatomical plausibility of the T-pose.

use bm_formats::prm::Prm;

use crate::math::{lerp3, quat_slerp, Mat4};

/// One bone.
#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// Bind pose relative to the parent (or to model space for roots).
    pub bind_local: Mat4,
    /// Bind pose in model space.
    pub bind_world: Mat4,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    /// Facial pose / morph names (from the `.ANM` skeleton block, when known). Not used by PRM.
    pub facial_poses: Vec<String>,
    /// Facial pose bank decoded from the skeleton block (`docs/anim.md` section 7), indexed like `facial_poses`
    /// (the pose names of the `.ANM`). Empty for skeletons without a face.
    pub face: Vec<FacePose>,
}

/// One bone's contribution to a facial pose: a *delta* on top of the bind pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceDelta {
    pub bone: usize,
    /// Rotation as unit axis + angle (radians), applied in the bone's own frame (`R_bind * R_delta`).
    pub axis: [f32; 3],
    pub angle: f32,
    /// Added to the bind translation (parent frame, cm).
    pub translation: [f32; 3],
    /// Multiplied into the bind scale.
    pub scale: [f32; 3],
}

/// A facial pose (`Neutral`, `A_Mouth_Widen`, `D_OO:Q:W`, `B_Blink`, ...): bone deltas that are blended by weight.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FacePose {
    pub deltas: Vec<FaceDelta>,
}

/// Translation / rotation (xyzw) / scale of a bone relative to its parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneTransform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl BoneTransform {
    pub const IDENTITY: BoneTransform =
        BoneTransform { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };

    pub fn from_matrix(m: &Mat4) -> BoneTransform {
        let (t, r, s) = m.decompose();
        BoneTransform { translation: t, rotation: r, scale: s }
    }

    pub fn to_matrix(&self) -> Mat4 {
        Mat4::from_trs(self.translation, self.rotation, self.scale)
    }

    pub fn lerp(&self, o: &BoneTransform, t: f32) -> BoneTransform {
        BoneTransform {
            translation: lerp3(self.translation, o.translation, t),
            rotation: quat_slerp(self.rotation, o.rotation, t),
            scale: lerp3(self.scale, o.scale, t),
        }
    }
}

/// Local (parent relative) transform of every bone of a skeleton.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    pub bones: Vec<BoneTransform>,
}

impl Pose {
    /// The skeleton's bind pose.
    pub fn bind(skel: &Skeleton) -> Pose {
        Pose { bones: skel.bones.iter().map(|b| BoneTransform::from_matrix(&b.bind_local)).collect() }
    }

    pub fn identity(n: usize) -> Pose {
        Pose { bones: vec![BoneTransform::IDENTITY; n] }
    }

    pub fn lerp(&self, o: &Pose, t: f32) -> Pose {
        Pose { bones: self.bones.iter().zip(&o.bones).map(|(a, b)| a.lerp(b, t)).collect() }
    }
}

/// Convert the 9 stored rotation floats (row-major, as in `.PRP` / `.GMS` / PRM bone blocks) to a
/// proper row-major rotation matrix for column vectors.
///
/// Glacier stores the rows in *reversed axis order*: stored row 0 is the image of the Z axis, row
/// 1 of Y, row 2 of X (an unrotated object stores `0,0,1, 0,1,0, 1,0,0`). Hence
/// `R = (J * S)^T = S^T * J` with `J` the anti-diagonal matrix. Verified with the PRM bone
/// hierarchy (`world == parent_world * local`, mean error 0.000 over 100 bones; every other
/// candidate has errors of several cm).
pub fn rotation_from_stored(s: &[f32; 9]) -> [f32; 9] {
    let mut r = [0f32; 9];
    for i in 0..3 {
        for j in 0..3 {
            r[i * 3 + j] = s[(2 - j) * 3 + i];
        }
    }
    r
}

/// Inverse of [`rotation_from_stored`] (for writers).
pub fn rotation_to_stored(r: &[f32; 9]) -> [f32; 9] {
    // R[i][j] = S[2-j][i]  =>  S[a][b] = R[b][2-a]
    let mut s = [0f32; 9];
    for a in 0..3 {
        for b in 0..3 {
            s[a * 3 + b] = r[b * 3 + (2 - a)];
        }
    }
    s
}

/// 4x3 stored matrix (9 rotation floats + translation) to [`Mat4`].
pub fn mat4_from_stored(w: &[f32; 12]) -> Mat4 {
    let mut s = [0f32; 9];
    s.copy_from_slice(&w[..9]);
    Mat4::from_rotation_translation(&rotation_from_stored(&s), [w[9], w[10], w[11]])
}

impl Skeleton {
    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// Model-space matrices of the given pose (parents are guaranteed to precede children in
    /// all shipped skeletons; if not, the bind pose is used for the offending bone).
    pub fn world_matrices(&self, pose: &Pose) -> Vec<Mat4> {
        let mut w: Vec<Mat4> = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let local = pose.bones.get(i).map(|t| t.to_matrix()).unwrap_or(b.bind_local);
            let m = match b.parent {
                Some(p) if p < i => w[p].mul(&local),
                Some(_) => b.bind_world,
                None => local,
            };
            w.push(m);
        }
        w
    }

    /// Inverse bind matrices (model space -> bone space).
    pub fn inverse_bind(&self) -> Vec<Mat4> {
        self.bones.iter().map(|b| b.bind_world.inverse_affine()).collect()
    }

    /// Skinning matrices `world(pose) * inverse_bind`, one per bone. `p' = sum_i w_i * M[joint_i] * p`.
    pub fn skinning_matrices(&self, pose: &Pose) -> Vec<Mat4> {
        let inv = self.inverse_bind();
        self.world_matrices(pose).iter().zip(&inv).map(|(w, i)| w.mul(i)).collect()
    }

    /// Reads the skeleton block of a skinned PRM model from its `parts` chunk index.
    pub fn from_prm(prm: &Prm, parts_chunk: u32) -> Option<Skeleton> {
        let parts = prm.chunk_words(parts_chunk as usize)?;
        let list_chunk = *parts.get(1)? as usize;
        if list_chunk == 0 {
            return None;
        }
        let list = prm.chunk_words(list_chunk)?;
        if list.len() < 4 {
            return None;
        }
        let count = list[0] as usize;
        if count == 0 || count > 512 {
            return None;
        }
        let recs = prm.chunk(list[1] as usize)?;
        let world = prm.chunk(list[2] as usize)?;
        let local = prm.chunk(list[3] as usize)?;
        if recs.len() < count * 64 || world.len() < count * 48 || local.len() < count * 48 {
            return None;
        }
        let f = |b: &[u8], o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let mat = |b: &[u8], i: usize| {
            let mut a = [0f32; 12];
            for (k, v) in a.iter_mut().enumerate() {
                *v = f(b, i * 48 + k * 4);
            }
            mat4_from_stored(&a)
        };
        let mut bones = Vec::with_capacity(count);
        for i in 0..count {
            let r = &recs[i * 64..i * 64 + 64];
            let parent = i32::from_le_bytes(r[12..16].try_into().unwrap());
            let name_bytes = &r[28..60];
            let n = name_bytes.iter().position(|&c| c == 0).unwrap_or(32);
            let name = String::from_utf8_lossy(&name_bytes[..n]).into_owned();
            if parent >= count as i32 || !name_bytes[..n].iter().all(|c| (0x20..0x7f).contains(c)) {
                return None;
            }
            bones.push(Bone {
                name,
                parent: if parent < 0 { None } else { Some(parent as usize) },
                bind_local: mat(local, i),
                bind_world: mat(world, i),
            });
        }
        let face = Self::read_face_bank(prm, &list, count).unwrap_or_default();
        Some(Skeleton { bones, facial_poses: Vec::new(), face })
    }

    /// Pose index of a facial pose name (case-insensitive), if the names are known.
    pub fn face_pose_index(&self, name: &str) -> Option<usize> {
        self.facial_poses.iter().position(|n| n.eq_ignore_ascii_case(name))
    }

    /// Reads the facial pose bank (`list[11]` -> `[records, offsets, record_count, map, pose_count]`).
    ///
    /// `records` is `record_count x 44` bytes: `f32 axis[3], f32 angle, f32 translation[3], f32 scale[3], u32 bone`.
    /// slot `s` owns records `offsets[s]..offsets[s+1]`. `map[pose]` gives the slot of
    /// pose `pose` (u16, `0xffff` = no data). Verified against the pose names (jaw hinge in `A_Mouth_Open`, eye pitch in
    /// `B_Look_Up`, lids in `B_Blink`) and the bone frames (`docs/anim.md`).
    fn read_face_bank(prm: &Prm, list: &[u32], bones: usize) -> Option<Vec<FacePose>> {
        let head = prm.chunk_words(*list.get(11)? as usize)?;
        if head.len() < 5 {
            return None;
        }
        let data = prm.chunk(head[0] as usize)?;
        let offs = prm.chunk_words(head[1] as usize)?;
        let total = head[2] as usize;
        let map = prm.chunk(head[3] as usize)?;
        let poses = head[4] as usize;
        if total == 0 || data.len() < total * 44 || poses == 0 || poses > 255 {
            return None;
        }
        let f = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let mut out = Vec::new();
        for p in 0..(map.len() / 2).min(256) {
            let slot = match map.get(p * 2..p * 2 + 2) {
                Some(m) => u16::from_le_bytes([m[0], m[1]]) as usize,
                None => 0xffff,
            };
            let mut deltas = Vec::new();
            if slot != 0xffff && slot + 1 < offs.len() {
                for r in offs[slot] as usize..(offs[slot + 1] as usize).min(total) {
                    let o = r * 44;
                    let bone = u32::from_le_bytes(data[o + 40..o + 44].try_into().unwrap()) as usize;
                    if bone >= bones {
                        continue;
                    }
                    deltas.push(FaceDelta {
                        bone,
                        axis: [f(o), f(o + 4), f(o + 8)],
                        angle: f(o + 12),
                        translation: [f(o + 16), f(o + 20), f(o + 24)],
                        scale: [f(o + 28), f(o + 32), f(o + 36)],
                    });
                }
            }
            out.push(FacePose { deltas });
        }
        Some(out)
    }

    /// Blends facial poses into `pose` (additive deltas on whatever the bones currently are, weights usually 0..1).
    pub fn apply_face(&self, pose: &mut Pose, weights: &[(usize, f32)]) {
        for &(pi, w) in weights {
            if w.abs() < 1e-4 {
                continue;
            }
            let Some(fp) = self.face.get(pi) else { continue };
            for d in &fp.deltas {
                let Some(t) = pose.bones.get_mut(d.bone) else { continue };
                t.translation = [t.translation[0] + d.translation[0] * w, t.translation[1] + d.translation[1] * w, t.translation[2] + d.translation[2] * w];
                let len = (d.axis[0] * d.axis[0] + d.axis[1] * d.axis[1] + d.axis[2] * d.axis[2]).sqrt();
                if len > 1e-6 && d.angle.abs() > 1e-6 {
                    let h = d.angle * w * 0.5;
                    let (s, c) = h.sin_cos();
                    let k = s / len;
                    let q = [d.axis[0] * k, d.axis[1] * k, d.axis[2] * k, c];
                    t.rotation = quat_mul(t.rotation, q);
                }
                for k in 0..3 {
                    t.scale[k] *= 1.0 + (d.scale[k] - 1.0) * w;
                }
            }
        }
    }
}

/// Hamilton product `a * b` of xyzw quaternions (apply `b` first).
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_rotation_roundtrip() {
        let j = [0., 0., 1., 0., 1., 0., 1., 0., 0.];
        let r = rotation_from_stored(&j);
        assert_eq!(r, [1., 0., 0., 0., 1., 0., 0., 0., 1.]);
        let s = [0.0, 0.0, -1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        assert_eq!(rotation_to_stored(&rotation_from_stored(&s)), s);
        let r = rotation_from_stored(&s);
        // proper rotation: determinant +1
        let d = r[0] * (r[4] * r[8] - r[5] * r[7]) - r[1] * (r[3] * r[8] - r[5] * r[6]) + r[2] * (r[3] * r[7] - r[4] * r[6]);
        assert!((d - 1.0).abs() < 1e-6);
    }
}
