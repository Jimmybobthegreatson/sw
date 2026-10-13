//! Device independent software mixer: voices, 3D positioning, buses, ducking, reverb send and a
//! soft limiter. [`Mixer::render`] produces interleaved stereo `f32`; the output backend (cpal) or the
//! headless renderer simply calls it.
//!
//! # Coordinates
//!
//! Positions, `Listener` and the distances of [`Spatial`] all use the *same* (arbitrary) world unit.
//! Glacier's native unit is the centimetre, [`Spatial::from_meters`] converts. The listener basis follows
//! the left-handed Y-up Glacier convention: with `forward = +Z` and `up = +Y` the listener's right-hand side
//! is `+X`.
//!
//! # 3D model (no HRTF, no Doppler)
//!
//! * gain: inverse-distance rolloff clamped between `min_distance` (full volume) and `max_distance`
//!   (silent), with a smooth fade over the last quarter of the range;
//! * pan: constant-power pan from the source azimuth (`sin(azimuth)`), narrowed when the source is very close
//!   to the listener, sources behind lose some level and high frequencies (one-pole low-pass, which also
//!   models air absorption and the per-voice `occlusion` amount);
//! * reverb send: per voice amount, scaled by distance gain.

use crate::pcm::Pcm;
use crate::reverb::{Reverb, ReverbParams};
use std::sync::Arc;

/// Mixing buses (sub-mixes with their own volume).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bus {
    Sfx = 0,
    Voice = 1,
    Ambient = 2,
    /// Hook for `bm-music`: play a [`PcmSource`] stream on this bus.
    Music = 3,
    Ui = 4,
}

pub const BUS_COUNT: usize = 5;

impl Bus {
    pub const ALL: [Bus; BUS_COUNT] = [Bus::Sfx, Bus::Voice, Bus::Ambient, Bus::Music, Bus::Ui];
}

/// Listener (usually the camera).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
}

impl Default for Listener {
    fn default() -> Self {
        Listener { position: [0.0; 3], forward: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0] }
    }
}

impl Listener {
    /// Builds a listener from a camera-to-world matrix (columns/rows as in `bm_models::Mat4`): forward is the
    /// transformed `+Z` axis, up the transformed `+Y` axis.
    pub fn from_matrix(m: &bm_models::Mat4) -> Listener {
        Listener { position: m.translation(), forward: m.transform_vector([0.0, 0.0, 1.0]), up: m.transform_vector([0.0, 1.0, 0.0]) }
    }

    fn basis(&self) -> ([f32; 3], [f32; 3]) {
        let f = norm(self.forward);
        let u = norm(self.up);
        // left-handed: right = up x forward
        let r = norm(cross(u, f));
        (r, f)
    }
}

/// 3D placement of a voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spatial {
    pub position: [f32; 3],
    /// Within this distance the voice is at full volume.
    pub min_distance: f32,
    /// Beyond this distance the voice is silent.
    pub max_distance: f32,
    /// Rolloff multiplier (1.0 = physical 1/d).
    pub rolloff: f32,
}

impl Spatial {
    /// `position` in native centimetres, distances in metres.
    pub fn from_meters(position_cm: [f32; 3], min_m: f32, max_m: f32) -> Spatial {
        Spatial { position: position_cm, min_distance: min_m * 100.0, max_distance: max_m * 100.0, rolloff: 1.0 }
    }
}

/// Parameters of a voice. All fields can be changed later with [`Mixer::update_voice`].
#[derive(Clone, Debug)]
pub struct VoiceParams {
    pub bus: Bus,
    pub volume: f32,
    /// Playback rate multiplier (changes duration and pitch). Streams ignore values other than 1.
    pub pitch: f32,
    pub looping: bool,
    /// Higher wins when voices are stolen.
    pub priority: u8,
    pub spatial: Option<Spatial>,
    /// Stereo balance for non-spatial voices, -1 (left) .. 1 (right).
    pub pan: f32,
    /// 0..1 amount sent to the shared reverb.
    pub reverb_send: f32,
    /// 0..1: 0 = open, 1 = fully occluded (reduces level and brightness).
    pub occlusion: f32,
    /// Fade-in in seconds.
    pub fade_in: f32,
}

impl Default for VoiceParams {
    fn default() -> Self {
        VoiceParams {
            bus: Bus::Sfx,
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            priority: 128,
            spatial: None,
            pan: 0.0,
            reverb_send: 0.0,
            occlusion: 0.0,
            fade_in: 0.0,
        }
    }
}

impl VoiceParams {
    pub fn on_bus(bus: Bus) -> VoiceParams {
        VoiceParams { bus, ..Default::default() }
    }
    pub fn at(mut self, s: Spatial) -> Self {
        self.spatial = Some(s);
        self
    }
    pub fn looped(mut self) -> Self {
        self.looping = true;
        self
    }
    pub fn volume(mut self, v: f32) -> Self {
        self.volume = v;
        self
    }
    pub fn pitch(mut self, p: f32) -> Self {
        self.pitch = p;
        self
    }
    pub fn priority(mut self, p: u8) -> Self {
        self.priority = p;
        self
    }
    pub fn reverb(mut self, s: f32) -> Self {
        self.reverb_send = s;
        self
    }
}

/// Partial update of a running voice.
#[derive(Clone, Debug, Default)]
pub struct VoiceUpdate {
    pub volume: Option<f32>,
    pub pitch: Option<f32>,
    pub position: Option<[f32; 3]>,
    pub pan: Option<f32>,
    pub occlusion: Option<f32>,
    pub reverb_send: Option<f32>,
    pub looping: Option<bool>,
}

/// Generic streaming source (decoded music, procedurally generated audio ...). Interleaved `f32`.
pub trait PcmSource: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Fills `out` (`frames * channels` samples) and returns the number of frames written;
    /// fewer frames than requested means the stream ended.
    fn read(&mut self, out: &mut [f32]) -> usize;
}

/// Opaque handle of a playing voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoiceHandle(pub u64);

/// Ducking: while `trigger` is audible the `target` bus is reduced to `depth` (linear gain).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ducking {
    pub trigger: Bus,
    pub target: Bus,
    pub depth: f32,
    pub attack: f32,
    pub release: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MixerStats {
    pub active_voices: usize,
    pub audible_voices: usize,
    pub stolen: u64,
    pub rejected: u64,
    pub rendered_frames: u64,
    pub peak: f32,
}

enum Source {
    Buffer(Arc<Pcm>),
    Stream { src: Box<dyn PcmSource>, queue: Vec<f32>, base: i64, ended: bool },
}

struct Voice {
    id: u64,
    source: Source,
    p: VoiceParams,
    pos: f64,
    gain: [f32; 2],
    env: f32,
    env_target: f32,
    env_step: f32,
    lp: [f32; 2],
    age: u64,
    audible: f32,
    finished: bool,
    first_block: bool,
}

pub struct Mixer {
    rate: u32,
    voices: Vec<Voice>,
    next_id: u64,
    max_voices: usize,
    listener: Listener,
    bus_gain: [f32; BUS_COUNT],
    bus_cur: [f32; BUS_COUNT],
    bus_mute: [bool; BUS_COUNT],
    bus_env: [f32; BUS_COUNT],
    master: f32,
    duck: Option<Ducking>,
    duck_cur: f32,
    reverb: Reverb,
    stats: MixerStats,
    bus_buf: Vec<Vec<f32>>,
    send_buf: Vec<f32>,
    mix_buf: Vec<f32>,
}

const BLOCK: usize = 256;

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l < 1e-9 { [0.0, 0.0, 1.0] } else { [a[0] / l, a[1] / l, a[2] / l] }
}

/// Result of the 3D computation for one voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spatialized {
    pub gain_l: f32,
    pub gain_r: f32,
    /// One-pole low-pass coefficient (1 = no filtering).
    pub lp: f32,
    /// Distance attenuation alone (0..1).
    pub distance_gain: f32,
    pub pan: f32,
}

/// Distance + pan model used by the mixer (public so games/tests can query it).
pub fn spatialize(listener: &Listener, s: &Spatial, occlusion: f32) -> Spatialized {
    let (right, fwd) = listener.basis();
    let dir = [s.position[0] - listener.position[0], s.position[1] - listener.position[1], s.position[2] - listener.position[2]];
    let d = dot(dir, dir).sqrt();
    let min = s.min_distance.max(1e-3);
    let max = s.max_distance.max(min * 1.0001);
    let mut g = if d <= min { 1.0 } else { min / (min + s.rolloff.max(0.0) * (d - min)) };
    // smooth fade to silence over the last quarter of the range
    let fade_start = min + (max - min) * 0.75;
    if d >= max {
        g = 0.0;
    } else if d > fade_start {
        let t = (max - d) / (max - fade_start);
        g *= t * t * (3.0 - 2.0 * t);
    }
    let (x, z) = if d > 1e-6 { (dot(dir, right) / d, dot(dir, fwd) / d) } else { (0.0, 1.0) };
    // narrow pan when the source is on top of the listener
    let spread = (d / (min * 0.5).max(1e-3)).clamp(0.0, 1.0);
    let pan = (x * spread).clamp(-1.0, 1.0);
    let behind = (-z).max(0.0);
    g *= 1.0 - 0.3 * behind;
    let occ = occlusion.clamp(0.0, 1.0);
    g *= 1.0 - 0.6 * occ;
    let air = (d / max).clamp(0.0, 1.0);
    let lp = ((1.0 - 0.65 * behind) * (1.0 - 0.45 * air) * (1.0 - 0.85 * occ)).clamp(0.03, 1.0);
    let ang = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    Spatialized { gain_l: g * ang.cos(), gain_r: g * ang.sin(), lp, distance_gain: g, pan }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    const T: f32 = 0.8;
    let a = x.abs();
    if a <= T {
        x
    } else {
        x.signum() * (T + (0.995 - T) * ((a - T) / (0.995 - T)).tanh())
    }
}

impl Mixer {
    pub fn new(sample_rate: u32) -> Mixer {
        Mixer {
            rate: sample_rate,
            voices: Vec::new(),
            next_id: 1,
            max_voices: 64,
            listener: Listener::default(),
            bus_gain: [1.0; BUS_COUNT],
            bus_cur: [1.0; BUS_COUNT],
            bus_mute: [false; BUS_COUNT],
            bus_env: [0.0; BUS_COUNT],
            master: 1.0,
            duck: None,
            duck_cur: 1.0,
            reverb: Reverb::new(sample_rate),
            stats: MixerStats::default(),
            bus_buf: (0..BUS_COUNT).map(|_| vec![0.0; BLOCK * 2]).collect(),
            send_buf: vec![0.0; BLOCK],
            mix_buf: vec![0.0; BLOCK * 2],
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn set_max_voices(&mut self, n: usize) {
        self.max_voices = n.max(1);
    }

    pub fn max_voices(&self) -> usize {
        self.max_voices
    }

    pub fn set_listener(&mut self, l: Listener) {
        self.listener = l;
    }

    pub fn listener(&self) -> Listener {
        self.listener
    }

    pub fn set_master_volume(&mut self, v: f32) {
        self.master = v.max(0.0);
    }

    pub fn set_bus_volume(&mut self, bus: Bus, v: f32) {
        self.bus_gain[bus as usize] = v.max(0.0);
    }

    pub fn bus_volume(&self, bus: Bus) -> f32 {
        self.bus_gain[bus as usize]
    }

    pub fn set_bus_muted(&mut self, bus: Bus, m: bool) {
        self.bus_mute[bus as usize] = m;
    }

    pub fn set_ducking(&mut self, d: Option<Ducking>) {
        self.duck = d;
    }

    pub fn set_reverb(&mut self, p: ReverbParams) {
        self.reverb.set_params(p);
    }

    pub fn reverb_params(&self) -> ReverbParams {
        self.reverb.params()
    }

    pub fn stats(&self) -> MixerStats {
        let mut s = self.stats;
        s.active_voices = self.voices.len();
        s.audible_voices = self.voices.iter().filter(|v| v.audible > 1e-4).count();
        s
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    fn alloc_slot(&mut self, priority: u8) -> bool {
        if self.voices.len() < self.max_voices {
            return true;
        }
        // victim: lowest priority, then quietest, then oldest
        let victim = self
            .voices
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (a.p.priority, ordf(a.audible), std::cmp::Reverse(a.age)).cmp(&(b.p.priority, ordf(b.audible), std::cmp::Reverse(b.age)))
            })
            .map(|(i, v)| (i, v.p.priority));
        match victim {
            Some((i, vp)) if vp <= priority => {
                self.voices.swap_remove(i);
                self.stats.stolen += 1;
                true
            }
            _ => {
                self.stats.rejected += 1;
                false
            }
        }
    }

    fn add_voice(&mut self, source: Source, p: VoiceParams) -> Option<VoiceHandle> {
        if !self.alloc_slot(p.priority) {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let (env, step) = if p.fade_in > 0.0 { (0.0, 1.0 / (p.fade_in * self.rate as f32)) } else { (1.0, 0.0) };
        self.voices.push(Voice {
            id,
            source,
            p,
            pos: 0.0,
            gain: [0.0; 2],
            env,
            env_target: 1.0,
            env_step: step,
            lp: [0.0; 2],
            age: 0,
            audible: 1.0,
            finished: false,
            first_block: true,
        });
        Some(VoiceHandle(id))
    }

    /// Starts a one-shot or looping buffer voice. `None` when the voice limit is hit and nothing can be stolen.
    pub fn play(&mut self, pcm: Arc<Pcm>, p: VoiceParams) -> Option<VoiceHandle> {
        if pcm.frames() == 0 {
            return None;
        }
        self.add_voice(Source::Buffer(pcm), p)
    }

    /// Starts a streaming voice (music, procedural audio). Pitch is ignored (always 1.0 relative to the
    /// source rate).
    pub fn play_stream(&mut self, src: Box<dyn PcmSource>, mut p: VoiceParams) -> Option<VoiceHandle> {
        p.pitch = 1.0;
        self.add_voice(Source::Stream { src, queue: Vec::new(), base: 0, ended: false }, p)
    }

    fn find(&mut self, h: VoiceHandle) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|v| v.id == h.0)
    }

    pub fn is_playing(&self, h: VoiceHandle) -> bool {
        self.voices.iter().any(|v| v.id == h.0 && !v.finished)
    }

    /// Stops a voice, optionally with a fade-out.
    pub fn stop(&mut self, h: VoiceHandle, fade_secs: f32) {
        let rate = self.rate as f32;
        if let Some(v) = self.find(h) {
            if fade_secs <= 0.0 {
                v.finished = true;
            } else {
                v.env_target = 0.0;
                v.env_step = -(v.env.max(1e-3)) / (fade_secs * rate);
            }
        }
    }

    pub fn stop_all(&mut self, bus: Option<Bus>) {
        for v in self.voices.iter_mut() {
            if bus.map_or(true, |b| v.p.bus == b) {
                v.finished = true;
            }
        }
    }

    pub fn update_voice(&mut self, h: VoiceHandle, u: &VoiceUpdate) {
        if let Some(v) = self.find(h) {
            if let Some(x) = u.volume {
                v.p.volume = x.max(0.0);
            }
            if let Some(x) = u.pitch {
                if matches!(v.source, Source::Buffer(_)) {
                    v.p.pitch = x.clamp(0.05, 8.0);
                }
            }
            if let (Some(x), Some(s)) = (u.position, v.p.spatial.as_mut()) {
                s.position = x;
            }
            if let Some(x) = u.pan {
                v.p.pan = x.clamp(-1.0, 1.0);
            }
            if let Some(x) = u.occlusion {
                v.p.occlusion = x.clamp(0.0, 1.0);
            }
            if let Some(x) = u.reverb_send {
                v.p.reverb_send = x.clamp(0.0, 1.0);
            }
            if let Some(x) = u.looping {
                v.p.looping = x;
            }
        }
    }

    /// Current normalised playback position (0..1) of a buffer voice.
    pub fn voice_progress(&self, h: VoiceHandle) -> Option<f32> {
        let v = self.voices.iter().find(|v| v.id == h.0)?;
        match &v.source {
            Source::Buffer(p) => Some((v.pos / p.frames() as f64) as f32),
            _ => None,
        }
    }

    /// Renders `out.len() / 2` frames of interleaved stereo.
    pub fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            self.render_block(&mut out[done * 2..(done + n) * 2], n);
            done += n;
        }
    }

    fn render_block(&mut self, out: &mut [f32], n: usize) {
        for b in self.bus_buf.iter_mut() {
            b[..n * 2].iter_mut().for_each(|s| *s = 0.0);
        }
        self.send_buf[..n].iter_mut().for_each(|s| *s = 0.0);
        let listener = self.listener;
        let rate = self.rate;
        let mut voices = std::mem::take(&mut self.voices);
        for v in voices.iter_mut() {
            let bi = v.p.bus as usize;
            mix_voice(v, n, rate, &listener, &mut self.bus_buf[bi][..n * 2], &mut self.send_buf[..n]);
        }
        voices.retain(|v| !v.finished);
        self.voices = voices;

        // bus envelopes (for ducking) + gains
        for (i, b) in self.bus_buf.iter().enumerate() {
            let peak = b[..n * 2].iter().fold(0f32, |a, &s| a.max(s.abs()));
            let k = if peak > self.bus_env[i] { 0.5 } else { 0.05 };
            self.bus_env[i] += (peak - self.bus_env[i]) * k;
        }
        let mut duck_target = 1.0;
        if let Some(d) = self.duck {
            if self.bus_env[d.trigger as usize] > 0.01 {
                duck_target = d.depth;
            }
        }
        let block_secs = n as f32 / self.rate as f32;
        if let Some(d) = self.duck {
            let tau = if duck_target < self.duck_cur { d.attack } else { d.release }.max(1e-3);
            self.duck_cur += (duck_target - self.duck_cur) * (1.0 - (-block_secs / tau).exp());
        } else {
            self.duck_cur = 1.0;
        }
        let mix = &mut self.mix_buf[..n * 2];
        mix.iter_mut().for_each(|s| *s = 0.0);
        for (i, b) in self.bus_buf.iter().enumerate() {
            let mut target = if self.bus_mute[i] { 0.0 } else { self.bus_gain[i] };
            if self.duck.map_or(false, |d| d.target as usize == i) {
                target *= self.duck_cur;
            }
            let g0 = self.bus_cur[i];
            let g1 = if (target - g0).abs() < 1e-5 { target } else { g0 + (target - g0) * 0.5 };
            for f in 0..n {
                let g = g0 + (g1 - g0) * (f as f32 / n as f32);
                mix[2 * f] += b[2 * f] * g;
                mix[2 * f + 1] += b[2 * f + 1] * g;
            }
            self.bus_cur[i] = g1;
        }
        // reverb return
        self.reverb.process(&self.send_buf[..n], mix);
        let mut peak = 0f32;
        for f in 0..out.len() {
            let s = soft_limit(mix[f] * self.master);
            peak = peak.max(s.abs());
            out[f] = s;
        }
        self.stats.peak = self.stats.peak.max(peak);
        self.stats.rendered_frames += n as u64;
    }
}

fn ordf(f: f32) -> u32 {
    (f.clamp(0.0, 1000.0) * 1000.0) as u32
}

fn mix_voice(v: &mut Voice, n: usize, out_rate: u32, listener: &Listener, bus: &mut [f32], send: &mut [f32]) {
    v.age += 1;
    // 3D / pan -> target gains for this block
    let (mut gl, mut gr, lp_a, dist_gain, stereo_passthrough);
    match &v.p.spatial {
        Some(s) => {
            let sp = spatialize(listener, s, v.p.occlusion);
            gl = sp.gain_l;
            gr = sp.gain_r;
            // mono-fold gain is split by pan; compensate sqrt2 so centred sources keep level
            gl *= std::f32::consts::SQRT_2;
            gr *= std::f32::consts::SQRT_2;
            lp_a = sp.lp;
            dist_gain = sp.distance_gain;
            stereo_passthrough = false;
        }
        None => {
            let ang = (v.p.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            let occ = v.p.occlusion.clamp(0.0, 1.0);
            gl = ang.cos() * std::f32::consts::SQRT_2 * (1.0 - 0.6 * occ);
            gr = ang.sin() * std::f32::consts::SQRT_2 * (1.0 - 0.6 * occ);
            lp_a = (1.0 - 0.85 * occ).clamp(0.03, 1.0);
            dist_gain = 1.0;
            stereo_passthrough = true;
        }
    }
    let vol = v.p.volume;
    gl *= vol;
    gr *= vol;
    v.audible = dist_gain * vol * v.env.max(v.env_target);
    let (src_rate, src_ch, src_frames) = match &v.source {
        Source::Buffer(p) => (p.sample_rate, p.channels as usize, p.frames()),
        Source::Stream { src, .. } => (src.sample_rate(), src.channels() as usize, usize::MAX),
    };
    let ratio = v.p.pitch.clamp(0.05, 8.0) as f64 * src_rate as f64 / out_rate as f64;
    let g_start = if v.first_block { [gl, gr] } else { v.gain };
    v.first_block = false;
    let inaudible = gl.max(gr) < 1e-5 && g_start[0].max(g_start[1]) < 1e-5;
    let send_amt = v.p.reverb_send * dist_gain;
    let looping = v.p.looping;
    let stream_input = matches!(v.source, Source::Stream { .. });

    for f in 0..n {
        // envelope
        if v.env_step != 0.0 {
            v.env += v.env_step;
            if (v.env_step > 0.0 && v.env >= v.env_target) || (v.env_step < 0.0 && v.env <= v.env_target) {
                v.env = v.env_target;
                v.env_step = 0.0;
                if v.env_target <= 0.0 {
                    v.finished = true;
                    break;
                }
            }
        }
        let t = f as f32 / n as f32;
        let (cl, cr) = (g_start[0] + (gl - g_start[0]) * t, g_start[1] + (gr - g_start[1]) * t);
        // fetch interpolated source sample
        let (sl, sr) = if inaudible && !stream_input {
            (0.0, 0.0)
        } else {
            match &mut v.source {
                Source::Buffer(p) => {
                    let i0 = v.pos as usize;
                    let fr = (v.pos - i0 as f64) as f32;
                    let i1 = if i0 + 1 >= src_frames { if looping { 0 } else { i0 } } else { i0 + 1 };
                    let a_l = p.sample_f32(i0, 0);
                    let b_l = p.sample_f32(i1, 0);
                    let l = a_l + (b_l - a_l) * fr;
                    if src_ch >= 2 {
                        let a_r = p.sample_f32(i0, 1);
                        let b_r = p.sample_f32(i1, 1);
                        (l, a_r + (b_r - a_r) * fr)
                    } else {
                        (l, l)
                    }
                }
                Source::Stream { src, queue, base, ended } => {
                    let i0 = v.pos as i64 - *base;
                    // make sure frames i0 and i0+1 are queued
                    while !*ended && (((i0 + 2) as usize) * src_ch.max(1)) > queue.len() {
                        let mut tmp = vec![0.0f32; 1024 * src_ch.max(1)];
                        let got = src.read(&mut tmp);
                        if got == 0 {
                            *ended = true;
                            break;
                        }
                        queue.extend_from_slice(&tmp[..got * src_ch.max(1)]);
                        if got < 1024 {
                            *ended = true;
                        }
                    }
                    let qf = (queue.len() / src_ch.max(1)) as i64;
                    if i0 >= qf {
                        v.finished = true;
                        break;
                    }
                    let i1 = (i0 + 1).min(qf - 1);
                    let fr = (v.pos - v.pos.floor()) as f32;
                    let at = |fi: i64, c: usize| queue[fi as usize * src_ch + c.min(src_ch - 1)];
                    let l = at(i0, 0) + (at(i1, 0) - at(i0, 0)) * fr;
                    if src_ch >= 2 {
                        (l, at(i0, 1) + (at(i1, 1) - at(i0, 1)) * fr)
                    } else {
                        (l, l)
                    }
                }
            }
        };
        let (mut xl, mut xr);
        if stereo_passthrough && src_ch >= 2 {
            xl = sl;
            xr = sr;
        } else {
            let m = (sl + sr) * 0.5;
            xl = m;
            xr = m;
        }
        if lp_a < 0.999 {
            v.lp[0] += lp_a * (xl - v.lp[0]);
            v.lp[1] += lp_a * (xr - v.lp[1]);
            xl = v.lp[0];
            xr = v.lp[1];
        }
        let e = v.env;
        bus[2 * f] += xl * cl * e;
        bus[2 * f + 1] += xr * cr * e;
        if send_amt > 0.0 {
            send[f] += (xl + xr) * 0.5 * send_amt * e * vol;
        }
        // advance
        v.pos += ratio;
        if let Source::Buffer(_) = v.source {
            if v.pos >= src_frames as f64 {
                if looping {
                    v.pos -= src_frames as f64;
                    if v.pos >= src_frames as f64 {
                        v.pos = 0.0;
                    }
                } else {
                    v.finished = true;
                    break;
                }
            }
        }
    }
    v.gain = [gl, gr];
    // drop consumed stream frames
    if let Source::Stream { queue, base, .. } = &mut v.source {
        let consumed = (v.pos as i64 - *base - 1).max(0);
        if consumed > 4096 {
            let drop_frames = consumed as usize;
            queue.drain(..drop_frames * src_ch.max(1));
            *base += consumed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, secs: f32, rate: u32, amp: f32) -> Arc<Pcm> {
        let n = (secs * rate as f32) as usize;
        let s: Vec<i16> = (0..n).map(|i| ((i as f32 * freq * std::f32::consts::TAU / rate as f32).sin() * amp * 32767.0) as i16).collect();
        Arc::new(Pcm::new(1, rate, s))
    }

    fn render(m: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut o = vec![0.0; frames * 2];
        m.render(&mut o);
        o
    }

    fn rms(o: &[f32], ch: usize) -> f32 {
        let v: Vec<f32> = o.iter().skip(ch).step_by(2).copied().collect();
        (v.iter().map(|s| s * s).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    #[test]
    fn one_shot_has_expected_length_and_finishes() {
        let mut m = Mixer::new(44100);
        let h = m.play(sine(440.0, 0.5, 44100, 0.5), VoiceParams::default()).unwrap();
        let o = render(&mut m, 44100);
        assert!(!m.is_playing(h));
        let last_active = o.chunks(2).rposition(|f| f[0].abs() > 1e-4).unwrap();
        assert!((last_active as i64 - 22050).abs() < 200, "ends near 0.5s, got {last_active}");
    }

    #[test]
    fn pitch_two_halves_duration() {
        let mut m = Mixer::new(44100);
        m.play(sine(440.0, 1.0, 44100, 0.5), VoiceParams::default().pitch(2.0)).unwrap();
        let o = render(&mut m, 44100);
        let last = o.chunks(2).rposition(|f| f[0].abs() > 1e-4).unwrap();
        assert!((last as i64 - 22050).abs() < 400, "got {last}");
    }

    #[test]
    fn looping_keeps_playing() {
        let mut m = Mixer::new(44100);
        let h = m.play(sine(220.0, 0.1, 44100, 0.5), VoiceParams::default().looped()).unwrap();
        let o = render(&mut m, 44100 * 2);
        assert!(m.is_playing(h));
        assert!(rms(&o[o.len() - 8000..], 0) > 0.1);
        m.stop(h, 0.0);
        render(&mut m, 256);
        assert!(!m.is_playing(h));
    }

    #[test]
    fn distance_attenuates_monotonically_and_silences_at_max() {
        let l = Listener::default();
        let mut prev = 2.0;
        for d in [0.0, 100.0, 300.0, 600.0, 900.0, 999.0, 1000.0, 2000.0] {
            let s = Spatial { position: [0.0, 0.0, d], min_distance: 100.0, max_distance: 1000.0, rolloff: 1.0 };
            let g = spatialize(&l, &s, 0.0).distance_gain;
            assert!(g <= prev + 1e-6, "d={d} g={g} prev={prev}");
            prev = g;
        }
        assert_eq!(prev, 0.0);
    }

    #[test]
    fn panning_follows_the_source_side() {
        let l = Listener::default();
        let right = spatialize(&l, &Spatial { position: [500.0, 0.0, 0.0], min_distance: 100.0, max_distance: 5000.0, rolloff: 1.0 }, 0.0);
        let left = spatialize(&l, &Spatial { position: [-500.0, 0.0, 0.0], min_distance: 100.0, max_distance: 5000.0, rolloff: 1.0 }, 0.0);
        assert!(right.gain_r > right.gain_l * 3.0);
        assert!(left.gain_l > left.gain_r * 3.0);
        // rendered signal agrees
        let mut m = Mixer::new(44100);
        m.play(sine(440.0, 0.2, 44100, 0.5), VoiceParams::default().at(Spatial { position: [500.0, 0.0, 100.0], min_distance: 100.0, max_distance: 5000.0, rolloff: 1.0 })).unwrap();
        let o = render(&mut m, 8000);
        assert!(rms(&o, 1) > rms(&o, 0) * 2.0);
    }

    #[test]
    fn listener_rotation_swaps_sides() {
        // listener turned to face +X: a source at +Z is now on the left
        let l = Listener { position: [0.0; 3], forward: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0] };
        let s = spatialize(&l, &Spatial { position: [0.0, 0.0, 500.0], min_distance: 100.0, max_distance: 5000.0, rolloff: 1.0 }, 0.0);
        assert!(s.pan < -0.9, "pan {}", s.pan);
    }

    #[test]
    fn bus_volume_scales_output() {
        let mut m = Mixer::new(44100);
        m.play(sine(440.0, 0.5, 44100, 0.3), VoiceParams::on_bus(Bus::Voice).looped()).unwrap();
        let a = render(&mut m, 4410);
        let full = rms(&a[a.len() - 2000..], 0);
        m.set_bus_volume(Bus::Voice, 0.5);
        render(&mut m, 4410);
        let b = render(&mut m, 4410);
        let half = rms(&b[b.len() - 2000..], 0);
        assert!((half / full - 0.5).abs() < 0.05, "ratio {}", half / full);
        m.set_bus_muted(Bus::Voice, true);
        render(&mut m, 4410);
        let c = render(&mut m, 2000);
        assert!(rms(&c, 0) < 1e-4);
    }

    #[test]
    fn voice_stealing_prefers_low_priority() {
        let mut m = Mixer::new(44100);
        m.set_max_voices(2);
        let snd = sine(100.0, 1.0, 44100, 0.1);
        let a = m.play(snd.clone(), VoiceParams::default().priority(10)).unwrap();
        let b = m.play(snd.clone(), VoiceParams::default().priority(200)).unwrap();
        // a new low-priority voice is rejected, a high one steals `a`
        assert!(m.play(snd.clone(), VoiceParams::default().priority(5)).is_none());
        let c = m.play(snd.clone(), VoiceParams::default().priority(100)).unwrap();
        assert!(!m.is_playing(a));
        assert!(m.is_playing(b) && m.is_playing(c));
        assert_eq!(m.stats().stolen, 1);
        assert_eq!(m.stats().rejected, 1);
    }

    #[test]
    fn output_never_clips() {
        let mut m = Mixer::new(44100);
        for i in 0..20 {
            m.play(sine(100.0 + i as f32 * 37.0, 0.5, 44100, 1.0), VoiceParams::default()).unwrap();
        }
        let o = render(&mut m, 22050);
        assert!(o.iter().all(|s| s.abs() < 1.0));
        assert!(o.iter().any(|s| s.abs() > 0.5));
    }

    #[test]
    fn fade_out_stops_voice() {
        let mut m = Mixer::new(44100);
        let h = m.play(sine(220.0, 2.0, 44100, 0.5), VoiceParams::default()).unwrap();
        render(&mut m, 1000);
        m.stop(h, 0.1);
        let o = render(&mut m, 8000);
        assert!(!m.is_playing(h));
        assert!(rms(&o[o.len() - 400..], 0) < 1e-4);
    }

    struct Tone(f32, u64);
    impl PcmSource for Tone {
        fn sample_rate(&self) -> u32 {
            32000
        }
        fn channels(&self) -> u16 {
            2
        }
        fn read(&mut self, out: &mut [f32]) -> usize {
            let n = out.len() / 2;
            for i in 0..n {
                let s = ((self.1 + i as u64) as f32 * self.0 * std::f32::consts::TAU / 32000.0).sin() * 0.4;
                out[2 * i] = s;
                out[2 * i + 1] = s;
            }
            self.1 += n as u64;
            n
        }
    }

    #[test]
    fn stream_source_is_resampled_to_output_rate() {
        let mut m = Mixer::new(48000);
        m.play_stream(Box::new(Tone(500.0, 0)), VoiceParams::on_bus(Bus::Music)).unwrap();
        let o = render(&mut m, 48000);
        // count zero crossings of the left channel: ~1000 per second for 500 Hz
        let l: Vec<f32> = o.iter().step_by(2).copied().collect();
        let zc = l.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((zc as i64 - 500).abs() <= 3, "rising crossings {zc}");
    }

    #[test]
    fn ducking_lowers_target_bus() {
        let mut m = Mixer::new(44100);
        m.set_ducking(Some(Ducking { trigger: Bus::Voice, target: Bus::Music, depth: 0.3, attack: 0.02, release: 0.2 }));
        m.play(sine(300.0, 3.0, 44100, 0.3), VoiceParams::on_bus(Bus::Music).looped()).unwrap();
        let a = render(&mut m, 8000);
        let before = rms(&a[a.len() - 2000..], 0);
        m.play(sine(700.0, 1.0, 44100, 0.01), VoiceParams::on_bus(Bus::Voice).volume(50.0)).unwrap();
        render(&mut m, 8000);
        let b = render(&mut m, 4000);
        // voice adds energy, so compare only the music component by checking overall duck_cur effect
        assert!(m.duck_cur < 0.5, "duck {}", m.duck_cur);
        assert!(before > 0.05 && b.len() > 0);
    }
}
