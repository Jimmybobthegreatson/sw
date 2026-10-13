//! Facial animation of characters: rest expression, blinking, lip sync and debug overrides.
//!
//! The pose bank (`Skeleton::face`, decoded in `bm-models`, see `docs/anim.md` section 7) holds additive bone deltas for the
//! pose names of the `.ANM` (`Neutral`, `D_Closed`, `B_Blink`, `D_OO:Q:W` ...). This module decides the weights per frame:
//!
//! * rest: `D_Closed` at full weight (the bind pose has slightly parted lips),
//! * blink: `B_Blink` for ~0.18 s every 2.5..6 s (deterministic per entity),
//! * speech: the `LIP ` channels of a dialogue line are pose indices (verified: ids 14..24 are the `D_*` visemes, 40..43 the
//!   jaw / `D_D_L_N_T` / `D_TH` poses); their 0..1 weights drive the poses and relax `D_Closed`.

use std::sync::Arc;

use bm_models::Skeleton;
use bm_sound::LipSync;

/// Pose names used by the controller.
pub const POSE_CLOSED: &str = "D_Closed";
pub const POSE_BLINK: &str = "B_Blink";

#[derive(Clone, Debug, Default)]
pub struct FaceController {
    blink_phase: f32,
    next_blink: f32,
    lip: Option<(Arc<LipSync>, f32)>,
    /// Extra weights by pose name (expressions such as `C_Smile`, `C_Pain_01`).
    pub expression: Vec<(String, f32)>,
}

fn hash01(id: u64, salt: u64) -> f32 {
    let mut x = id.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x & 0xffff) as f32 / 65535.0
}

impl FaceController {
    pub fn new(id: u64) -> FaceController {
        FaceController { blink_phase: -1.0, next_blink: 1.0 + hash01(id, 1) * 5.0, lip: None, expression: Vec::new() }
    }

    /// Starts a dialogue line at game time `now` (seconds).
    pub fn speak(&mut self, lip: Arc<LipSync>, now: f32) {
        self.lip = Some((lip, now));
    }

    pub fn is_speaking(&self, now: f32) -> bool {
        self.lip.as_ref().is_some_and(|(l, s)| now - s < l.duration_secs() + 0.1)
    }

    /// Pose weights `(pose index, weight)` at game time `now`; `id` seeds the blink timing. Stateless apart from the
    /// blink schedule, which is advanced here.
    pub fn weights(&mut self, skel: &Skeleton, id: u64, now: f32) -> Vec<(usize, f32)> {
        let mut out: Vec<(usize, f32)> = Vec::with_capacity(8);
        if skel.face.is_empty() {
            return out;
        }
        let idx = |n: &str| skel.face_pose_index(n);
        // blink schedule
        if self.blink_phase < 0.0 && now >= self.next_blink {
            self.blink_phase = 0.0;
        }
        let mut blink = 0.0;
        if self.blink_phase >= 0.0 {
            let t = (now - self.next_blink) / 0.18;
            if t >= 1.0 {
                self.blink_phase = -1.0;
                self.next_blink = now + 2.5 + hash01(id, (now * 10.0) as u64) * 3.5;
            } else {
                blink = 1.0 - (2.0 * t - 1.0).abs();
            }
        }
        let mut closed = 1.0f32;
        if let Some((lip, start)) = &self.lip {
            let t = now - start;
            if t > lip.duration_secs() + 0.1 {
                self.lip = None;
            } else {
                let mut open = 0.0f32;
                for (cid, w) in lip.frame_at(t.max(0.0)) {
                    if w > 0.0 && (cid as usize) < skel.face.len() && cid != 0 {
                        out.push((cid as usize, w));
                        open = open.max(w);
                    }
                }
                closed = 1.0 - open;
            }
        }
        if let Some(i) = idx(POSE_CLOSED) {
            out.push((i, closed));
        }
        if blink > 0.0 {
            if let Some(i) = idx(POSE_BLINK) {
                out.push((i, blink));
            }
        }
        for (n, w) in &self.expression {
            if let Some(i) = idx(n) {
                out.push((i, *w));
            }
        }
        out
    }
}
