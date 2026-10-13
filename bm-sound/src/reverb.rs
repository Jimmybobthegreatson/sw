//! A small stereo Schroeder/Freeverb style reverb used as the shared "reverb send" of the mixer.
//!
//! [`ReverbParams::from_eax`] maps the EAX 4 preset blocks stored in the `.SND` room effects
//! (`SndDb::room_effects`) to this simple model: decay time -> comb feedback (RT60), decay HF ratio ->
//! damping, `room + reverb` (mB) -> wet level.

use crate::snd_db::RoomEffect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    /// Linear wet level applied to the reverb return (0 disables the reverb).
    pub wet: f32,
    /// RT60 in seconds.
    pub decay_time: f32,
    /// High-frequency damping 0..1 (1 = very dark tail).
    pub damping: f32,
    /// Pre-delay in seconds.
    pub pre_delay: f32,
    /// Scales the delay line lengths (room size, 1.0 = default).
    pub size: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        ReverbParams { wet: 0.0, decay_time: 1.2, damping: 0.4, pre_delay: 0.01, size: 1.0 }
    }
}

impl ReverbParams {
    /// Reverb that is used when the listener is outside every room zone: effectively off.
    pub fn off() -> ReverbParams {
        ReverbParams::default()
    }

    /// Maps an EAX room effect: `room`/`reverb` are in mB (1/100 dB).
    pub fn from_eax(e: &RoomEffect) -> ReverbParams {
        let level_db = (e.room + e.reverb) as f32 / 100.0;
        let wet = (10f32.powf(level_db / 20.0)).clamp(0.0, 0.6);
        ReverbParams {
            wet,
            decay_time: e.decay_time.clamp(0.1, 20.0),
            damping: (1.0 - e.decay_hf_ratio).clamp(0.05, 0.9),
            pre_delay: e.reflections_delay.clamp(0.0, 0.1),
            size: (e.environment_size / 7.5).clamp(0.4, 2.5),
        }
    }
}

const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
    feedback: f32,
    damp: f32,
}

impl Comb {
    fn new(len: usize) -> Comb {
        Comb { buf: vec![0.0; len.max(2)], idx: 0, store: 0.0, feedback: 0.8, damp: 0.2 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.idx];
        self.store = y * (1.0 - self.damp) + self.store * self.damp;
        self.buf[self.idx] = x + self.store * self.feedback;
        self.idx += 1;
        if self.idx >= self.buf.len() {
            self.idx = 0;
        }
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(len: usize) -> Allpass {
        Allpass { buf: vec![0.0; len.max(2)], idx: 0 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.idx];
        let y = -x + b;
        self.buf[self.idx] = x + b * 0.5;
        self.idx += 1;
        if self.idx >= self.buf.len() {
            self.idx = 0;
        }
        y
    }
}

pub struct Reverb {
    rate: u32,
    params: ReverbParams,
    combs: [Vec<Comb>; 2],
    allpass: [Vec<Allpass>; 2],
    pre: Vec<f32>,
    pre_idx: usize,
    pre_len: usize,
    /// Smoothed wet gain (avoids clicks when the room changes).
    wet_cur: f32,
}

impl Reverb {
    pub fn new(rate: u32) -> Reverb {
        let mut r = Reverb {
            rate,
            params: ReverbParams::default(),
            combs: [Vec::new(), Vec::new()],
            allpass: [Vec::new(), Vec::new()],
            pre: vec![0.0; (rate as usize / 5).max(16)],
            pre_idx: 0,
            pre_len: 1,
            wet_cur: 0.0,
        };
        r.rebuild(ReverbParams::default());
        r
    }

    pub fn params(&self) -> ReverbParams {
        self.params
    }

    fn rebuild(&mut self, p: ReverbParams) {
        let scale = self.rate as f32 / 44100.0 * p.size;
        for ch in 0..2 {
            let spread = if ch == 1 { STEREO_SPREAD } else { 0 };
            self.combs[ch] = COMB_TUNING.iter().map(|&t| Comb::new(((t + spread) as f32 * scale) as usize)).collect();
            self.allpass[ch] = ALLPASS_TUNING.iter().map(|&t| Allpass::new(((t + spread) as f32 * scale) as usize)).collect();
        }
        self.apply_decay(p);
    }

    fn apply_decay(&mut self, p: ReverbParams) {
        for ch in 0..2 {
            for c in self.combs[ch].iter_mut() {
                let delay_s = c.buf.len() as f32 / self.rate as f32;
                // RT60: the signal drops 60 dB after decay_time seconds.
                c.feedback = 10f32.powf(-3.0 * delay_s / p.decay_time.max(0.05)).clamp(0.0, 0.985);
                c.damp = p.damping.clamp(0.0, 0.95);
            }
        }
        self.pre_len = ((p.pre_delay * self.rate as f32) as usize).clamp(1, self.pre.len() - 1);
    }

    /// Changes the parameters. A different `size` rebuilds the delay lines (clears the tail).
    pub fn set_params(&mut self, p: ReverbParams) {
        if (p.size - self.params.size).abs() > 1e-3 {
            self.params = p;
            self.rebuild(p);
        } else {
            self.params = p;
            self.apply_decay(p);
        }
    }

    /// Processes a mono send block into the stereo `out` (interleaved, summed into).
    pub fn process(&mut self, send: &[f32], out: &mut [f32]) {
        let frames = send.len();
        let target = self.params.wet;
        let k = 1.0 - (-1.0 / (0.02 * self.rate as f32)).exp();
        for i in 0..frames {
            self.wet_cur += (target - self.wet_cur) * k;
            // pre-delay
            let x = send[i];
            let rd = (self.pre_idx + self.pre.len() - self.pre_len) % self.pre.len();
            let delayed = self.pre[rd];
            self.pre[self.pre_idx] = x;
            self.pre_idx = (self.pre_idx + 1) % self.pre.len();
            let input = delayed * 0.015 * 8.0; // gain matches the comb bank's 8-way sum
            let mut l = 0.0;
            let mut r = 0.0;
            for c in self.combs[0].iter_mut() {
                l += c.process(input);
            }
            for c in self.combs[1].iter_mut() {
                r += c.process(input);
            }
            for a in self.allpass[0].iter_mut() {
                l = a.process(l);
            }
            for a in self.allpass[1].iter_mut() {
                r = a.process(r);
            }
            out[2 * i] += l * self.wet_cur;
            out[2 * i + 1] += r * self.wet_cur;
        }
    }

    pub fn reset(&mut self) {
        let p = self.params;
        self.rebuild(p);
        self.pre.iter_mut().for_each(|s| *s = 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_produces_decaying_tail() {
        let mut r = Reverb::new(44100);
        r.set_params(ReverbParams { wet: 1.0, decay_time: 1.0, damping: 0.3, pre_delay: 0.0, size: 1.0 });
        let mut send = vec![0.0f32; 44100 * 2];
        send[0] = 1.0;
        let mut out = vec![0.0f32; send.len() * 2];
        r.process(&send, &mut out);
        let early: f32 = out[..8820].iter().map(|s| s * s).sum();
        let late: f32 = out[out.len() - 8820..].iter().map(|s| s * s).sum();
        assert!(early > 0.0, "tail must exist");
        assert!(late < early * 0.5, "tail must decay (early {early}, late {late})");
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn zero_wet_is_silent() {
        let mut r = Reverb::new(48000);
        let mut out = vec![0.0f32; 2000];
        r.process(&vec![1.0; 1000], &mut out);
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }
}
