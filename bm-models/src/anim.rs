//! Animation clips, sampling and `.ANM` indexing.
//!
//! **Status.** `.ANM` key data is decoded (see `docs/anim.md`): [`AnmBank::parse`] finds the clip table of a
//! level's `.ANM`, [`AnmBank::raw_clip`] decodes the bit packed streams ([`crate::anm_decode`]) and
//! [`AnmBank::animation`] turns a clip into an [`Animation`] for a given skeleton through the humanoid rig
//! solver ([`crate::rig`]). Also implemented and tested:
//!
//! * the runtime data model ([`Animation`], [`Track`]) with exact sampling (binary search +
//!   lerp/slerp, looping) producing a [`Pose`] and skinning matrices via
//!   [`Skeleton::skinning_matrices`](crate::skeleton::Skeleton::skinning_matrices);
//! * [`AnmIndex`]: the name tables of a `.ANM` (animation databases, clip paths, `anim:` object names).

use bm_formats::anm::Anm;

use crate::anm_decode::{decode_stream, AnmError, Channel};
use crate::math::lerp3;
use crate::math::quat_slerp;
use crate::rig::{RigSolver, PARAM_COUNT};
use crate::skeleton::{BoneTransform, Pose, Skeleton};

/// Keyframes of one bone. Any of the channels may be empty (the bind pose value is used then).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    /// Skeleton bone index.
    pub bone: usize,
    /// `(time in seconds, translation relative to the parent)`, ascending in time.
    pub translations: Vec<(f32, [f32; 3])>,
    /// `(time, unit quaternion xyzw)`.
    pub rotations: Vec<(f32, [f32; 4])>,
    pub scales: Vec<(f32, [f32; 3])>,
}

fn find_segment<T>(keys: &[(f32, T)], t: f32) -> (usize, usize, f32) {
    // keys is non-empty
    if t <= keys[0].0 {
        return (0, 0, 0.0);
    }
    let last = keys.len() - 1;
    if t >= keys[last].0 {
        return (last, last, 0.0);
    }
    let hi = keys.partition_point(|k| k.0 <= t);
    let lo = hi - 1;
    let span = keys[hi].0 - keys[lo].0;
    (lo, hi, if span > 1e-9 { (t - keys[lo].0) / span } else { 0.0 })
}

impl Track {
    /// Samples the track at `t`, using `base` for channels without keys.
    pub fn sample(&self, t: f32, base: &BoneTransform) -> BoneTransform {
        let mut out = *base;
        if !self.translations.is_empty() {
            let (a, b, f) = find_segment(&self.translations, t);
            out.translation = lerp3(self.translations[a].1, self.translations[b].1, f);
        }
        if !self.rotations.is_empty() {
            let (a, b, f) = find_segment(&self.rotations, t);
            out.rotation = quat_slerp(self.rotations[a].1, self.rotations[b].1, f);
        }
        if !self.scales.is_empty() {
            let (a, b, f) = find_segment(&self.scales, t);
            out.scale = lerp3(self.scales[a].1, self.scales[b].1, f);
        }
        out
    }

    /// Time of the last key of any channel.
    pub fn end_time(&self) -> f32 {
        let a = self.translations.last().map(|k| k.0).unwrap_or(0.0);
        let b = self.rotations.last().map(|k| k.0).unwrap_or(0.0);
        let c = self.scales.last().map(|k| k.0).unwrap_or(0.0);
        a.max(b).max(c)
    }
}

/// A skeletal animation clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Animation {
    pub name: String,
    /// Length in seconds.
    pub duration: f32,
    pub looping: bool,
    pub tracks: Vec<Track>,
    /// Translation of the `GROUND` (root) bone over time, `(seconds, cm)`: the motion the character is
    /// supposed to travel while the clip plays. Not part of `tracks`, because the game moves the body itself.
    pub root_motion: Vec<(f32, [f32; 3])>,
}

impl Animation {
    pub fn new(name: &str, tracks: Vec<Track>, looping: bool) -> Animation {
        let duration = tracks.iter().map(|t| t.end_time()).fold(0.0, f32::max);
        Animation { name: name.to_string(), duration, looping, tracks, root_motion: Vec::new() }
    }

    /// Average horizontal root speed over the clip in cm/s (0 without root motion).
    pub fn root_speed(&self) -> f32 {
        match (self.root_motion.first(), self.root_motion.last()) {
            (Some(a), Some(b)) if b.0 > a.0 => {
                let dx = b.1[0] - a.1[0];
                let dz = b.1[2] - a.1[2];
                (dx * dx + dz * dz).sqrt() / (b.0 - a.0)
            }
            _ => 0.0,
        }
    }

    /// Wraps / clamps `t` into `[0, duration]` according to `looping`.
    pub fn normalize_time(&self, t: f32) -> f32 {
        if self.duration <= 0.0 {
            0.0
        } else if self.looping {
            t.rem_euclid(self.duration)
        } else {
            t.clamp(0.0, self.duration)
        }
    }

    /// Samples the clip at time `t` (seconds). Bones without a track keep the skeleton's bind pose.
    pub fn sample(&self, skeleton: &Skeleton, t: f32) -> Pose {
        let mut pose = Pose::bind(skeleton);
        self.sample_into(skeleton, t, &mut pose);
        pose
    }

    /// Like [`Animation::sample`] but overwrites only the animated bones of an existing pose
    /// (useful for layering a clip over another).
    pub fn sample_into(&self, _skeleton: &Skeleton, t: f32, pose: &mut Pose) {
        let t = self.normalize_time(t);
        for tr in &self.tracks {
            if let Some(slot) = pose.bones.get_mut(tr.bone) {
                *slot = tr.sample(t, slot);
            }
        }
    }
}

/// Everything known about the structure of an `.ANM` file without decoding its key data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnmIndex {
    /// `anmcol:animationdatabase#<name>` objects (per character / weapon animation databases).
    pub databases: Vec<String>,
    /// Animation set paths (`/Movement/Run/Forward`, `/Sit/GetUpLeftFast`, ...).
    pub clip_paths: Vec<String>,
    /// Animation object names (`anim:Male_Reg/Male_Reg_Run#Run_Backward`, ...), in file order.
    pub anim_objects: Vec<String>,
    /// Bone names of the skeleton block (small skeleton ANMs such as `Loader_Sequence`).
    pub bones: Vec<String>,
    /// Facial pose names of the skeleton block.
    pub poses: Vec<String>,
}

/// Collects the decoded name tables of an [`Anm`].
pub fn anm_index(anm: &Anm) -> AnmIndex {
    let mut idx = AnmIndex::default();
    for d in &anm.databases {
        if !idx.databases.contains(&d.name) {
            idx.databases.push(d.name.clone());
        }
    }
    if let Some(sk) = &anm.skeleton {
        idx.bones = sk.bones.clone();
        idx.poses = sk.poses.clone();
    }
    let d = &anm.raw;
    let mut i = 0;
    while i < d.len() {
        if (0x20..0x7f).contains(&d[i]) {
            let s = i;
            while i < d.len() && (0x20..0x7f).contains(&d[i]) {
                i += 1;
            }
            if i < d.len() && d[i] == 0 && i - s >= 4 {
                let st = String::from_utf8_lossy(&d[s..i]).into_owned();
                if st.starts_with("anim:") {
                    idx.anim_objects.push(st);
                } else if st.starts_with('/') && !st.contains(' ') {
                    idx.clip_paths.push(st);
                }
            }
        }
        i += 1;
    }
    idx
}

/// One entry of the clip table of a `.ANM`.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipHeader {
    pub index: usize,
    /// Full name (`anim:Male_Reg/Male_Reg_Walk#Walk`) or `extra` for alias entries without a name.
    pub name: String,
    pub frames: u32,
    pub fps: u32,
    /// Size of the clip's data in the pool (all streams).
    pub size: u32,
    /// Offsets of streams A (rig parameters), B (extra bones), C (root), D (scalars) in the key data pool.
    pub streams: [Option<u32>; 4],
}

impl ClipHeader {
    /// Name without the `anim:` prefix, e.g. `Male_Reg/Male_Reg_Walk#Walk`.
    pub fn short_name(&self) -> &str {
        self.name.strip_prefix("anim:").unwrap_or(&self.name)
    }

    pub fn duration(&self) -> f32 {
        self.frames.saturating_sub(1) as f32 / self.fps.max(1) as f32
    }
}

/// A transform channel of a bone outside the rig parameters (stream B/C): hand attachers, camera, weapon parts, root.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtraTrack {
    /// Index into [`AnmBank::bone_names`].
    pub id: u16,
    pub bone: String,
    pub rotations: Vec<[f32; 4]>,
    /// Translations in cm (empty for pure quaternion channels).
    pub translations: Vec<[f32; 3]>,
}

/// A decoded clip: the rig parameters of every frame plus the extra channels.
#[derive(Clone, Debug, PartialEq)]
pub struct RawClip {
    pub frames: usize,
    pub fps: f32,
    /// `frames` vectors of [`PARAM_COUNT`] floats (empty for clips without a stream A, e.g. weapon animations).
    pub params: Vec<[f32; PARAM_COUNT]>,
    /// Which parameter floats the clip actually animates (partial clips such as upper-body layers leave the rest
    /// zero; [`AnmBank::animation_with`] fills them from the base pose).
    pub mask: Vec<bool>,
    pub extras: Vec<ExtraTrack>,
    /// `GROUND` motion (stream C).
    pub root: Option<ExtraTrack>,
}

/// The decoded clip table of one `.ANM` file (all animations of a level).
#[derive(Clone, Debug)]
pub struct AnmBank {
    raw: Vec<u8>,
    pool: usize,
    pub clips: Vec<ClipHeader>,
    /// Bone names by animation bone id (`PELVIS`, `SPINE`, ... `GROUND`, `camera_bone`, ... `w_bone_*`).
    pub bone_names: Vec<String>,
    /// Facial pose names that follow the clip names.
    pub facial_poses: Vec<String>,
    base: std::sync::OnceLock<Option<[f32; PARAM_COUNT]>>,
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

impl AnmBank {
    /// Locates the clip table of a big `.ANM` (returns `None` for small skeleton / cutscene files).
    ///
    /// The bone name list (starting `PELVIS\0SPINE\0`) is followed by one name per clip header, so the number
    /// of 64 byte headers is known; they sit directly in front of the key data pool and are found as the run
    /// of plausible records that ends the table.
    pub fn parse(raw: &[u8]) -> Option<AnmBank> {
        let key = b"PELVIS\0SPINE\0";
        let bl = raw.windows(key.len()).position(|w| w == key)?;
        let mut pos = bl;
        let mut bone_names = Vec::new();
        let mut clip_names: Vec<String> = Vec::new();
        let mut facial = Vec::new();
        let mut stage = 0;
        while pos < raw.len() {
            let e = raw[pos..].iter().position(|&b| b == 0)? + pos;
            let s = String::from_utf8_lossy(&raw[pos..e]).into_owned();
            pos = e + 1;
            let is_clip = s.starts_with("anim:") || s == "extra";
            match stage {
                0 if is_clip => {
                    stage = 1;
                    clip_names.push(s)
                }
                0 => bone_names.push(s),
                1 if is_clip => clip_names.push(s),
                1 => {
                    stage = 2;
                    facial.push(s)
                }
                _ => facial.push(s),
            }
        }
        let n = clip_names.len();
        if n == 0 || bone_names.len() < 60 {
            return None;
        }
        // plausibility of a header at word index `i`
        let limit = bl;
        let good = |i: usize| -> bool {
            if (i + 16) * 4 > raw.len() {
                return false;
            }
            let w = |k: usize| u32_at(raw, (i + k) * 4);
            let fps = w(1) >> 16;
            let nfr = w(1) & 0xffff;
            let off_ok = |v: u32| v == 0xffff_ffff || (v as usize) < limit;
            (1..=64).contains(&fps)
                && (1..20000).contains(&nfr)
                && w(3) < 0x100_0000
                && w(7) % 16 == 0
                && w(7) < 0x4_0000
                && off_ok(w(4))
                && off_ok(w(5))
                && off_ok(w(6))
        };
        let goodv: Vec<bool> = (0..limit / 4).map(good).collect();
        // longest run with stride 16 words that is at least n long; the table is the last n entries of the run
        let mut best: Option<(usize, usize)> = None; // (run_start_word, count)
        for i in 0..goodv.len() {
            if !goodv[i] || (i >= 16 && goodv[i - 16]) {
                continue;
            }
            let mut j = i;
            let mut c = 0;
            while j < goodv.len() && goodv[j] {
                c += 1;
                j += 16;
            }
            if c >= n && best.is_none_or(|b| c > b.1) {
                best = Some((i, c));
            }
        }
        let (start, count) = best?;
        let h0 = (start + 16 * (count - n)) * 4;
        let pool = h0 + 64 * n;
        let mut clips = Vec::with_capacity(n);
        for (i, name) in clip_names.into_iter().enumerate() {
            let o = h0 + 64 * i;
            let w = |k: usize| u32_at(raw, o + 4 * k);
            let off = |v: u32| if v == 0xffff_ffff { None } else { Some(v) };
            clips.push(ClipHeader {
                index: i,
                name,
                frames: w(1) & 0xffff,
                fps: w(1) >> 16,
                size: w(3),
                streams: [off(w(4)), off(w(5)), off(w(6)), off(w(8))],
            });
        }
        Some(AnmBank { raw: raw.to_vec(), pool, clips, bone_names, facial_poses: facial, base: std::sync::OnceLock::new() })
    }

    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    /// Index of the clip with exactly this name (with or without the `anim:` prefix, ASCII case-insensitive).
    pub fn find(&self, name: &str) -> Option<usize> {
        let want = name.strip_prefix("anim:").unwrap_or(name);
        self.clips.iter().position(|c| c.short_name().eq_ignore_ascii_case(want))
    }

    /// First clip whose name ends with `suffix` (e.g. `Male_Reg_Walk#Walk` or `#Walk`).
    pub fn find_suffix(&self, suffix: &str) -> Option<usize> {
        let sfx = suffix.to_ascii_lowercase();
        self.clips.iter().position(|c| c.short_name().to_ascii_lowercase().ends_with(&sfx))
    }

    /// All clip indices whose name contains `needle` (case-insensitive).
    pub fn search(&self, needle: &str) -> Vec<usize> {
        let n = needle.to_ascii_lowercase();
        self.clips.iter().filter(|c| c.name.to_ascii_lowercase().contains(&n)).map(|c| c.index).collect()
    }

    fn stream(&self, h: &ClipHeader, which: usize) -> Result<Option<Vec<Channel>>, AnmError> {
        let Some(off) = h.streams[which] else { return Ok(None) };
        let start = self.pool + off as usize;
        if start >= self.raw.len() {
            return Err(AnmError::OutOfRange);
        }
        let end = (start + h.size as usize + 64).min(self.raw.len());
        decode_stream(&self.raw[start..end], h.frames as usize).map(Some)
    }

    /// Decodes all streams of clip `index`.
    pub fn raw_clip(&self, index: usize) -> Result<RawClip, AnmError> {
        let h = self.clips.get(index).ok_or(AnmError::OutOfRange)?;
        let frames = h.frames as usize;
        let mut params = Vec::new();
        let mut mask = vec![false; PARAM_COUNT];
        if let Some(chs) = self.stream(h, 0)? {
            params = vec![[0f32; PARAM_COUNT]; frames];
            for c in &chs {
                let id = c.id as usize;
                if id >= 74 {
                    continue;
                }
                // ten quaternions first (4 floats each), then one float per scalar id
                let slot = if id < 10 { 4 * id } else { 30 + id };
                let width = if id < 10 { 4 } else { 1 };
                mask[slot..slot + width].iter_mut().for_each(|m| *m = true);
                for (f, param) in params.iter_mut().enumerate() {
                    let mut s = slot;
                    if let Some(q) = c.quats.get(f) {
                        param[s..s + 4].copy_from_slice(q);
                        s += 4;
                    }
                    for sc in &c.scalars {
                        if let (Some(v), true) = (sc.get(f), s < PARAM_COUNT) {
                            param[s] = *v;
                        }
                        s += 1;
                    }
                }
            }
        }
        let to_extra = |c: &Channel, names: &[String]| ExtraTrack {
            id: c.id,
            bone: names.get(c.id as usize).cloned().unwrap_or_default(),
            rotations: c.quats.clone(),
            translations: if c.scalars.len() == 3 {
                (0..frames.min(c.scalars[0].len())).map(|f| [c.scalars[0][f], c.scalars[1][f], c.scalars[2][f]]).collect()
            } else {
                Vec::new()
            },
        };
        let extras = self
            .stream(h, 1)?
            .map(|v| v.iter().filter(|c| !c.quats.is_empty()).map(|c| to_extra(c, &self.bone_names)).collect())
            .unwrap_or_default();
        let root = self.stream(h, 2)?.and_then(|v| v.iter().find(|c| !c.quats.is_empty()).map(|c| to_extra(c, &self.bone_names)));
        Ok(RawClip { frames, fps: h.fps.max(1) as f32, params, mask, extras, root })
    }

    /// Rig parameters of the neutral standing pose (`Base_Pose`, frame 0), used to fill channels a partial clip
    /// does not animate.
    pub fn base_params(&self) -> Option<[f32; PARAM_COUNT]> {
        *self.base.get_or_init(|| {
            let i = self.find_suffix("Male_Reg#Base_Pose").or_else(|| self.find_suffix("#Base_Pose"))?;
            let raw = self.raw_clip(i).ok()?;
            raw.params.first().copied()
        })
    }

    /// Builds an [`Animation`] for `skeleton` from clip `index`. Humanoid skeletons get the full rig solution;
    /// any skeleton gets the extra bones (attachers, weapon parts) that it contains by name. `None` when the clip
    /// cannot be decoded or drives no bone of the skeleton.
    pub fn animation(&self, index: usize, skeleton: &Skeleton) -> Option<Animation> {
        let solver = RigSolver::new(skeleton);
        self.animation_with(index, skeleton, solver.as_ref())
    }

    /// Like [`AnmBank::animation`] with a prebuilt solver (reuse it for many clips of one skeleton).
    pub fn animation_with(&self, index: usize, skeleton: &Skeleton, solver: Option<&RigSolver>) -> Option<Animation> {
        let raw = self.raw_clip(index).ok()?;
        let h = &self.clips[index];
        let dt = 1.0 / raw.fps;
        let n = raw.frames;
        let mut tracks: Vec<Track> = Vec::new();
        let mut slot_of: Vec<Option<usize>> = vec![None; skeleton.bones.len()];
        let mut track_for = |bone: usize, tracks: &mut Vec<Track>| -> usize {
            *slot_of[bone].get_or_insert_with(|| {
                tracks.push(Track { bone, ..Default::default() });
                tracks.len() - 1
            })
        };
        if let (Some(solver), false) = (solver, raw.params.is_empty()) {
            let base = self.base_params();
            for (f, p) in raw.params.iter().enumerate() {
                let t = f as f32 * dt;
                let mut p = *p;
                if let Some(base) = base {
                    for k in 0..PARAM_COUNT {
                        if !raw.mask[k] {
                            p[k] = base[k];
                        }
                    }
                }
                for s in solver.solve(&p) {
                    let ti = track_for(s.bone, &mut tracks);
                    tracks[ti].rotations.push((t, s.rotation));
                    if let Some(tr) = s.translation {
                        tracks[ti].translations.push((t, tr));
                    }
                }
            }
        }
        for e in &raw.extras {
            let Some(bone) = skeleton.bone_index(&e.bone) else { continue };
            let ti = track_for(bone, &mut tracks);
            tracks[ti].rotations = e.rotations.iter().enumerate().map(|(f, q)| (f as f32 * dt, *q)).collect();
            tracks[ti].translations = e.translations.iter().enumerate().map(|(f, v)| (f as f32 * dt, *v)).collect();
        }
        let root_motion: Vec<(f32, [f32; 3])> = raw
            .root
            .as_ref()
            .map(|r| r.translations.iter().enumerate().map(|(f, v)| (f as f32 * dt, *v)).collect())
            .unwrap_or_default();
        if tracks.is_empty() {
            return None;
        }
        let duration = n.saturating_sub(1) as f32 * dt;
        // a clip loops when its last frame matches its first (walk / run / idle cycles)
        let looping = n > 2
            && tracks.iter().all(|t| match (t.rotations.first(), t.rotations.last()) {
                (Some(x), Some(y)) => {
                    let d: f32 = x.1.iter().zip(y.1.iter()).map(|(p, q)| (p - q).abs()).sum();
                    let s: f32 = x.1.iter().zip(y.1.iter()).map(|(p, q)| (p + q).abs()).sum();
                    d < 0.05 || s < 0.05
                }
                _ => true,
            });
        Some(Animation { name: h.short_name().to_string(), duration, looping, tracks, root_motion })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Mat4, quat_normalize};
    use crate::skeleton::Bone;

    fn two_bone_skeleton() -> Skeleton {
        Skeleton {
            bones: vec![
                Bone { name: "ROOT".into(), parent: None, bind_local: Mat4::IDENTITY, bind_world: Mat4::IDENTITY },
                Bone {
                    name: "CHILD".into(),
                    parent: Some(0),
                    bind_local: Mat4::from_translation([0.0, 10.0, 0.0]),
                    bind_world: Mat4::from_translation([0.0, 10.0, 0.0]),
                },
            ],
            facial_poses: vec![],
            face: vec![],
        }
    }

    #[test]
    fn sampling_interpolates_and_loops() {
        let sk = two_bone_skeleton();
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let anim = Animation::new(
            "spin",
            vec![Track {
                bone: 0,
                translations: vec![(0.0, [0.0, 0.0, 0.0]), (2.0, [4.0, 0.0, 0.0])],
                rotations: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (2.0, quat_normalize([0.0, half, 0.0, half]))],
                scales: vec![],
            }],
            true,
        );
        assert_eq!(anim.duration, 2.0);
        let p = anim.sample(&sk, 1.0);
        assert!((p.bones[0].translation[0] - 2.0).abs() < 1e-5);
        // halfway between identity and 90 degrees about Y = 45 degrees
        let a = (std::f32::consts::FRAC_PI_4 / 2.0).sin();
        assert!((p.bones[0].rotation[1] - a).abs() < 1e-4);
        // untouched bone keeps its bind pose
        assert_eq!(p.bones[1].translation, [0.0, 10.0, 0.0]);
        // looping wraps, clamping does not
        let q = anim.sample(&sk, 3.0);
        assert!((q.bones[0].translation[0] - 2.0).abs() < 1e-5);
        let mut once = anim.clone();
        once.looping = false;
        assert!((once.sample(&sk, 3.0).bones[0].translation[0] - 4.0).abs() < 1e-5);
        // skinning: bind pose gives identity, a translated root moves the child
        let skin = sk.skinning_matrices(&Pose::bind(&sk));
        assert!(skin.iter().all(|m| (m.0[3][1]).abs() < 1e-5));
        let moved = sk.skinning_matrices(&p);
        let c = moved[1].transform_point([0.0, 10.0, 0.0]);
        assert!((c[0] - 2.0).abs() < 1e-4 && c[1] > 9.0, "{c:?}");
    }
}
