//! Loop-aware frame generation: [`SegmentGen`] walks one source (intro, once, or looping with an optional
//! equal-power loop crossfade), [`Resampler`] converts it to the engine rate and [`StemGen`] chains an
//! optional intro segment and the looping body into one gapless stereo stream.

use crate::def::TrackDef;
use crate::source::AudioSource;
use std::collections::VecDeque;

pub type Frame = [f32; 2];

const CHUNK: usize = 2048;

/// Loop region of a source in source frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopPlan {
    pub looping: bool,
    pub start: u64,
    /// Exclusive end of the region.
    pub end: u64,
    /// Crossfade length (tail of the region blended into its head).
    pub xfade: u64,
}

impl LoopPlan {
    pub fn from_track(t: &TrackDef, rate: u32, len: u64) -> LoopPlan {
        let secs = |s: f64| (s.max(0.0) * rate as f64).round() as u64;
        let start = t.loop_start_frames.or(t.loop_start_secs.map(secs)).unwrap_or(0).min(len.saturating_sub(1));
        let end = t.loop_end_frames.or(t.loop_end_secs.map(secs)).unwrap_or(len).min(len);
        let looping = t.looping && end > start + 1;
        let region = end.saturating_sub(start);
        let xfade = if looping { (secs(t.loop_crossfade_secs as f64)).min(region / 2) } else { 0 };
        LoopPlan { looping, start, end, xfade }
    }

    /// Frame index at which the plain run ends and the wrap (or the blend) begins.
    pub fn wrap_point(&self) -> u64 {
        self.end - self.xfade
    }

    /// Frames of one loop period in the output stream.
    pub fn period(&self) -> u64 {
        self.end - self.start - self.xfade
    }
}

fn to_stereo(buf: &[f32], ch: usize, n: usize, gain: f32, out: &mut VecDeque<Frame>) {
    for i in 0..n {
        let f = match ch {
            1 => [buf[i], buf[i]],
            2 => [buf[i * 2], buf[i * 2 + 1]],
            _ => {
                // fold extra channels: front pair, centre/LFE ignored
                [buf[i * ch], buf[i * ch + 1]]
            }
        };
        out.push_back([f[0] * gain, f[1] * gain]);
    }
}

/// Walks one source in source-rate frames, applying the loop plan.
pub struct SegmentGen {
    src: Box<dyn AudioSource>,
    ch: usize,
    plan: LoopPlan,
    len: u64,
    pos: u64,
    gain: f32,
    queue: VecDeque<Frame>,
    finished: bool,
    buf: Vec<f32>,
}

impl SegmentGen {
    pub fn new(src: Box<dyn AudioSource>, plan: LoopPlan, gain: f32, start_frame: u64) -> SegmentGen {
        let ch = src.channels().max(1) as usize;
        let len = src.len_frames();
        let mut pos = start_frame.min(len);
        if plan.looping && pos >= plan.wrap_point() {
            // seeked past the wrap point: continue inside the region
            let over = pos - plan.wrap_point();
            pos = plan.start + plan.xfade + over % plan.period().max(1);
        }
        SegmentGen { src, ch, plan, len, pos, gain, queue: VecDeque::new(), finished: false, buf: vec![0.0; CHUNK * ch] }
    }

    pub fn sample_rate(&self) -> u32 {
        self.src.sample_rate()
    }

    fn refill(&mut self) {
        if self.finished {
            return;
        }
        if !self.plan.looping {
            let n = self.src.read(self.pos, &mut self.buf);
            if n == 0 || self.pos >= self.len {
                self.finished = true;
                return;
            }
            self.pos += n as u64;
            to_stereo(&self.buf, self.ch, n, self.gain, &mut self.queue);
            return;
        }
        let wrap = self.plan.wrap_point();
        if self.pos < wrap {
            let want = ((wrap - self.pos) as usize).min(CHUNK);
            let n = self.src.read(self.pos, &mut self.buf[..want * self.ch]);
            if n == 0 {
                // source shorter than reported: treat as wrap
                self.pos = wrap;
                return;
            }
            self.pos += n as u64;
            to_stereo(&self.buf, self.ch, n, self.gain, &mut self.queue);
            return;
        }
        // at the wrap point
        let x = self.plan.xfade as usize;
        if x == 0 {
            self.pos = self.plan.start;
            // guard against zero-length reads looping forever
            if self.src.len_frames() == 0 {
                self.finished = true;
            }
            return;
        }
        let mut tail = vec![0.0f32; x * self.ch];
        let mut head = vec![0.0f32; x * self.ch];
        let nt = self.src.read(self.plan.end - self.plan.xfade, &mut tail);
        let nh = self.src.read(self.plan.start, &mut head);
        let n = nt.min(nh).min(x);
        for k in 0..n {
            let t = (k as f32 + 0.5) / x as f32 * std::f32::consts::FRAC_PI_2;
            let (wt, wh) = (t.cos(), t.sin());
            let mut f = [0.0f32; 2];
            for (c, fc) in f.iter_mut().enumerate() {
                let cc = c.min(self.ch - 1);
                *fc = (tail[k * self.ch + cc] * wt + head[k * self.ch + cc] * wh) * self.gain;
            }
            self.queue.push_back(f);
        }
        self.pos = self.plan.start + self.plan.xfade;
    }

    pub fn next_src(&mut self) -> Option<Frame> {
        let mut guard = 0;
        while self.queue.is_empty() {
            if self.finished {
                return None;
            }
            self.refill();
            guard += 1;
            if guard > 64 {
                self.finished = true;
                return None;
            }
        }
        self.queue.pop_front()
    }
}

/// Pull-based Catmull-Rom resampler. Pass-through (bit exact) when the rates match.
pub struct Resampler {
    step: f64,
    frac: f64,
    h: [Frame; 4],
    primed: bool,
    ended: bool,
    passthrough: bool,
}

impl Resampler {
    pub fn new(src_rate: u32, out_rate: u32) -> Resampler {
        Resampler { step: src_rate as f64 / out_rate as f64, frac: 0.0, h: [[0.0; 2]; 4], primed: false, ended: false, passthrough: src_rate == out_rate }
    }

    fn push(&mut self, f: Frame) {
        self.h = [self.h[1], self.h[2], self.h[3], f];
    }

    /// Next output frame, or `None` when the generator is exhausted.
    pub fn next(&mut self, g: &mut SegmentGen) -> Option<Frame> {
        if self.ended {
            return None;
        }
        if self.passthrough {
            return match g.next_src() {
                Some(f) => Some(f),
                None => {
                    self.ended = true;
                    None
                }
            };
        }
        if !self.primed {
            let f0 = g.next_src()?;
            let f1 = g.next_src().unwrap_or(f0);
            let f2 = g.next_src().unwrap_or(f1);
            self.h = [f0, f0, f1, f2];
            self.primed = true;
        }
        while self.frac >= 1.0 {
            match g.next_src() {
                Some(f) => self.push(f),
                None => {
                    self.ended = true;
                    return None;
                }
            }
            self.frac -= 1.0;
        }
        let t = self.frac as f32;
        let mut o = [0.0f32; 2];
        for (c, oc) in o.iter_mut().enumerate() {
            let (p0, p1, p2, p3) = (self.h[0][c], self.h[1][c], self.h[2][c], self.h[3][c]);
            *oc = p1 + 0.5 * t * (p2 - p0 + t * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + t * (3.0 * (p1 - p2) + p3 - p0)));
        }
        self.frac += self.step;
        Some(o)
    }
}

/// Timing facts about a stem in output frames, used for beat / loop synchronised transitions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StemInfo {
    pub out_rate: u32,
    /// Length of the intro segment (0 if none).
    pub intro_frames: u64,
    /// Output frame (from stem start) of the first loop/end boundary.
    pub first_boundary: u64,
    /// Loop period (0 = plays once).
    pub period: f64,
    /// Frames of the whole stem when it does not loop.
    pub total: Option<u64>,
    pub beat_frames: Option<f64>,
    pub beats_per_bar: u32,
    /// Output frame of the beat grid origin.
    pub beat_origin: i64,
}

impl StemInfo {
    /// Next boundary strictly after `t` (output frames since the stem started).
    pub fn next_boundary(&self, t: u64) -> Option<u64> {
        if t < self.first_boundary {
            return Some(self.first_boundary);
        }
        if self.period <= 0.0 {
            return None;
        }
        let k = ((t - self.first_boundary) as f64 / self.period).floor() + 1.0;
        Some(self.first_boundary + (k * self.period).round() as u64)
    }

    /// Next beat (or bar line) at or after `t`.
    pub fn next_beat(&self, t: u64, bar: bool) -> Option<u64> {
        let b = self.beat_frames?;
        let unit = if bar { b * self.beats_per_bar.max(1) as f64 } else { b };
        let rel = t as f64 - self.beat_origin as f64;
        let k = (rel / unit).ceil().max(0.0);
        Some((self.beat_origin as f64 + k * unit).round().max(0.0) as u64)
    }
}

struct Seg {
    gen_: SegmentGen,
    rs: Resampler,
}

/// A stem's stereo stream at the engine rate: optional intro followed by the (looping) body.
pub struct StemGen {
    segs: VecDeque<Seg>,
    pub info: StemInfo,
}

impl StemGen {
    pub fn new(intro: Option<(Box<dyn AudioSource>, &TrackDef)>, body: (Box<dyn AudioSource>, &TrackDef), out_rate: u32, start_secs: f64) -> StemGen {
        let mut segs = VecDeque::new();
        let mut info = StemInfo { out_rate, beats_per_bar: 4, ..Default::default() };
        let mut start_secs = start_secs;
        if let Some((src, t)) = intro {
            let rate = src.sample_rate();
            let len = src.len_frames();
            let eff = t.gain * if src.replaced() { 1.0 } else { t.trim };
            let mut plan = LoopPlan::from_track(t, rate, len);
            plan.looping = false;
            let start = (start_secs * rate as f64) as u64;
            if start < len {
                let ratio = out_rate as f64 / rate as f64;
                info.intro_frames = ((len - start) as f64 * ratio).round() as u64;
                start_secs = 0.0;
                segs.push_back(Seg { gen_: SegmentGen::new(src, plan, eff, start), rs: Resampler::new(rate, out_rate) });
            } else {
                start_secs -= len as f64 / rate as f64;
            }
        }
        let (src, t) = body;
        let rate = src.sample_rate();
        let len = src.len_frames();
        let eff = t.gain * if src.replaced() { 1.0 } else { t.trim };
        let plan = LoopPlan::from_track(t, rate, len);
        let start = (start_secs.max(0.0) * rate as f64) as u64;
        let ratio = out_rate as f64 / rate as f64;
        let pos0 = start.min(len);
        if plan.looping {
            let wrap = plan.wrap_point();
            info.first_boundary = info.intro_frames + (wrap.saturating_sub(pos0) as f64 * ratio).round() as u64;
            info.period = plan.period() as f64 * ratio;
        } else {
            let total = info.intro_frames + (len.saturating_sub(pos0) as f64 * ratio).round() as u64;
            info.first_boundary = total;
            info.total = Some(total);
        }
        if let Some(bpm) = t.bpm.filter(|b| *b > 0.0) {
            info.beat_frames = Some(out_rate as f64 * 60.0 / bpm as f64);
            info.beats_per_bar = t.beats_per_bar.max(1);
            info.beat_origin = info.intro_frames as i64 + (t.beat_offset_secs as f64 * out_rate as f64).round() as i64 - (pos0 as f64 * ratio).round() as i64;
        }
        segs.push_back(Seg { gen_: SegmentGen::new(src, plan, eff, start), rs: Resampler::new(rate, out_rate) });
        StemGen { segs, info }
    }

    /// Appends up to `n` frames; returns `false` once the stem has ended (no more data will come).
    pub fn produce(&mut self, out: &mut Vec<Frame>, n: usize) -> bool {
        let mut left = n;
        while left > 0 {
            let Some(seg) = self.segs.front_mut() else { return false };
            match seg.rs.next(&mut seg.gen_) {
                Some(f) => {
                    out.push(f);
                    left -= 1;
                }
                None => {
                    self.segs.pop_front();
                }
            }
        }
        !self.segs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemSource;

    fn sine(rate: u32, freq: f32, frames: usize, ch: u16) -> Vec<f32> {
        let mut v = Vec::new();
        for i in 0..frames {
            let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5;
            for _ in 0..ch {
                v.push(s);
            }
        }
        v
    }

    fn collect(g: &mut StemGen, n: usize) -> Vec<Frame> {
        let mut v = Vec::new();
        g.produce(&mut v, n);
        v
    }

    fn max_step(v: &[Frame]) -> f32 {
        v.windows(2).map(|w| (w[1][0] - w[0][0]).abs().max((w[1][1] - w[0][1]).abs())).fold(0.0, f32::max)
    }

    #[test]
    fn hard_loop_of_whole_cycles_is_seamless() {
        // 100 frames at 1000 Hz, 5 Hz sine = exactly 0.5 cycles?  use 10 Hz => 1 cycle per 100 frames
        let t = TrackDef::default();
        let src = MemSource::from_f32(1000, 1, sine(1000, 10.0, 100, 1));
        let mut g = StemGen::new(None, (Box::new(src), &t), 1000, 0.0);
        let v = collect(&mut g, 1000);
        assert_eq!(v.len(), 1000);
        let ideal = 2.0 * std::f32::consts::PI * 10.0 / 1000.0 * 0.5;
        assert!(max_step(&v) < ideal * 1.05, "{} vs {}", max_step(&v), ideal);
    }

    #[test]
    fn loop_region_skips_the_attack_after_first_pass() {
        let t = TrackDef { loop_start_frames: Some(40), loop_end_frames: Some(80), ..Default::default() };
        let data: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let mut g = StemGen::new(None, (Box::new(MemSource::from_f32(1000, 1, data)), &t), 1000, 0.0);
        let v = collect(&mut g, 130);
        assert_eq!(v[79][0], 79.0);
        assert_eq!(v[80][0], 40.0);
        assert_eq!(v[119][0], 79.0);
        assert_eq!(v[120][0], 40.0);
        assert_eq!(g.info.first_boundary, 80);
        assert_eq!(g.info.period.round() as u64, 40);
    }

    #[test]
    fn crossfade_loop_hides_a_discontinuous_wrap() {
        // a 7.3 Hz sine over 1 s does not close; hard wrap clicks, crossfade does not.
        let data = sine(1000, 7.3, 1000, 1);
        let hard = TrackDef::default();
        let mut gh = StemGen::new(None, (Box::new(MemSource::from_f32(1000, 1, data.clone())), &hard), 1000, 0.0);
        let vh = collect(&mut gh, 3000);
        let soft = TrackDef { loop_crossfade_secs: 0.1, ..Default::default() };
        let mut gs = StemGen::new(None, (Box::new(MemSource::from_f32(1000, 1, data)), &soft), 1000, 0.0);
        let vs = collect(&mut gs, 3000);
        let ideal = 2.0 * std::f32::consts::PI * 7.3 / 1000.0 * 0.5;
        assert!(max_step(&vh) > ideal * 3.0, "hard wrap should click ({})", max_step(&vh));
        assert!(max_step(&vs) < ideal * 1.6, "crossfade should be smooth ({} vs {})", max_step(&vs), ideal);
        assert_eq!(gs.info.period.round() as u64, 900);
    }

    #[test]
    fn intro_then_body_is_gapless_and_reports_boundaries() {
        let intro = TrackDef { looping: false, ..Default::default() };
        let body = TrackDef::default();
        let a = MemSource::from_f32(1000, 1, vec![1.0; 50]);
        let b = MemSource::from_f32(1000, 1, vec![2.0; 30]);
        let mut g = StemGen::new(Some((Box::new(a), &intro)), (Box::new(b), &body), 1000, 0.0);
        let v = collect(&mut g, 120);
        assert_eq!(v[49][0], 1.0);
        assert_eq!(v[50][0], 2.0);
        assert_eq!(v[119][0], 2.0);
        assert_eq!(g.info.intro_frames, 50);
        assert_eq!(g.info.first_boundary, 80);
        assert_eq!(g.info.next_boundary(80), Some(110));
        assert_eq!(g.info.next_boundary(10), Some(80));
    }

    #[test]
    fn one_shot_ends() {
        let t = TrackDef { looping: false, ..Default::default() };
        let mut g = StemGen::new(None, (Box::new(MemSource::from_f32(1000, 1, vec![0.5; 40])), &t), 1000, 0.0);
        let mut v = Vec::new();
        assert!(!g.produce(&mut v, 100));
        assert_eq!(v.len(), 40);
        assert_eq!(g.info.total, Some(40));
    }

    #[test]
    fn resampling_keeps_a_sine_continuous_across_loops() {
        // 441 Hz sine at 44100 Hz (100 frames/cycle), loop of 10 cycles -> engine at 48000 Hz
        let t = TrackDef::default();
        let src = MemSource::from_f32(44100, 2, sine(44100, 441.0, 1000, 2));
        let mut g = StemGen::new(None, (Box::new(src), &t), 48000, 0.0);
        let v = collect(&mut g, 20000);
        let ideal = 2.0 * std::f32::consts::PI * 441.0 / 48000.0 * 0.5;
        assert!(max_step(&v) < ideal * 1.1, "{} vs {}", max_step(&v), ideal);
        // amplitude preserved
        let peak = v.iter().map(|f| f[0].abs()).fold(0.0, f32::max);
        assert!((peak - 0.5).abs() < 0.01, "{peak}");
    }

    #[test]
    fn beat_grid() {
        let t = TrackDef { bpm: Some(120.0), beats_per_bar: 4, ..Default::default() };
        let g = StemGen::new(None, (Box::new(MemSource::from_f32(1000, 1, vec![0.0; 10000])), &t), 1000, 0.0);
        assert_eq!(g.info.beat_frames, Some(500.0));
        assert_eq!(g.info.next_beat(1, false), Some(500));
        assert_eq!(g.info.next_beat(500, false), Some(500));
        assert_eq!(g.info.next_beat(1, true), Some(2000));
    }
}
