//! The music state machine and mixer. Lives behind a mutex shared by the game-facing
//! [`crate::MusicSystem`] and the audio-facing [`crate::MusicMixer`] (a `PcmSource`). All timing is
//! in output frames of the render clock, so transitions are sample accurate and offline renders are
//! deterministic.

use crate::curve::{Curve, FadeDef, Ramp};
use crate::def::{CueDef, LayerDef, MusicDef, StateDef, StemDef, SyncMode, TransitionRule};
use crate::feed::{Prefetch, Ring, StemFeed};
use crate::segment::{Frame, StemGen, StemInfo};
use crate::source::{OpenContext, TrackResolver};
use std::collections::VecDeque;
use std::sync::Arc;

const MAX_CHUNK: usize = 256;

/// Things the game may want to react to (poll with `MusicSystem::poll_events`).
#[derive(Clone, Debug, PartialEq)]
pub enum MusicEvent {
    /// A transition was requested; it fires `in_secs` from now (sync point / min hold).
    TransitionScheduled { from: String, to: String, in_secs: f32 },
    /// The state machine switched state (the audio fade starts here).
    StateChanged { from: String, to: String },
    CueStarted { cue: String },
    /// A non-looping cue ran to its end.
    CueFinished { cue: String },
    StingerStarted { stinger: String },
    /// A track could not be opened; the stem plays silence.
    TrackError { cue: String, stem: String, message: String },
    /// A request named an unknown state / zone / event.
    Ignored { what: String },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MusicStats {
    pub frames_rendered: u64,
    /// Number of render chunks in which a started cue had less data than needed.
    pub underruns: u64,
    pub active_cues: usize,
}

/// Snapshot for debugging / UI.
#[derive(Clone, Debug)]
pub struct MusicStatus {
    pub state: String,
    pub pending: Option<String>,
    pub intensity: f32,
    pub paused: bool,
    pub clock_secs: f64,
    pub cues: Vec<CueStatus>,
}

#[derive(Clone, Debug)]
pub struct CueStatus {
    pub cue: String,
    pub started: bool,
    pub fading_out: bool,
    pub gain: f32,
    pub age_secs: f64,
    pub stinger: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cue,
    Stinger,
}

struct ActiveStem {
    name: String,
    ring: Arc<Ring>,
    base_gain: f32,
    layer: Option<LayerDef>,
    mult: f32,
    gain: Ramp,
    last_target: f32,
    reported_error: bool,
}

struct ActiveCue {
    cue: String,
    kind: Kind,
    stems: Vec<ActiveStem>,
    env: Ramp,
    cue_gain: f32,
    started: bool,
    start_at: u64,
    created: u64,
    fading_out: bool,
    age: u64,
    fade_in: FadeDef,
    /// Stinger ducking (depth, attack, release secs).
    duck: Option<(f32, f32, f32)>,
    slew_until: u64,
    slew_frames: u64,
    stinger_name: Option<String>,
    advanced: bool,
}

struct Pending {
    to: String,
    rule: TransitionRule,
    fire_at: u64,
    deadline: u64,
    prepared: Option<ActiveCue>,
    mult: Vec<(String, f32)>,
    state_gain: f32,
    playlist_pos: usize,
}

#[derive(Clone, Debug)]
pub struct ConductorConfig {
    pub sample_rate: u32,
    /// Frames each ring is kept filled to.
    pub read_ahead_frames: usize,
    /// Frames that must be buffered before a prepared cue may start.
    pub prebuffer_frames: usize,
    /// How long a prepared cue may take to become ready before it starts anyway.
    pub max_prepare_secs: f32,
}

pub struct Conductor {
    def: Arc<MusicDef>,
    resolver: Arc<dyn TrackResolver>,
    prefetch: Arc<Prefetch>,
    cfg: ConductorConfig,
    clock: u64,
    state: String,
    state_since: u64,
    cues: Vec<ActiveCue>,
    pending: Option<Pending>,
    intensity: f32,
    intensity_target: f32,
    master: Ramp,
    pause: Ramp,
    pause_requested: bool,
    frozen: bool,
    duck: Ramp,
    duck_target: f32,
    manual_duck: Option<(f32, f32, f32)>,
    events: VecDeque<MusicEvent>,
    stats: MusicStats,
    music_buf: Vec<f32>,
    sting_buf: Vec<f32>,
    pop_buf: Vec<Frame>,
    started: bool,
    playlist_pos: usize,
    rng: u64,
    zone: Option<i32>,
}

fn secs_to_frames(s: f32, rate: u32) -> u64 {
    (s.max(0.0) as f64 * rate as f64).round() as u64
}

impl Conductor {
    pub fn new(def: Arc<MusicDef>, resolver: Arc<dyn TrackResolver>, prefetch: Arc<Prefetch>, cfg: ConductorConfig) -> Conductor {
        let intensity = def.intensity.as_ref().map(|i| i.initial).unwrap_or(0.0).clamp(0.0, 1.0);
        let master = def.master_gain;
        Conductor {
            def,
            resolver,
            prefetch,
            cfg,
            clock: 0,
            state: String::new(),
            state_since: 0,
            cues: Vec::new(),
            pending: None,
            intensity,
            intensity_target: intensity,
            master: Ramp::constant(master),
            pause: Ramp::constant(1.0),
            pause_requested: false,
            frozen: false,
            duck: Ramp::constant(1.0),
            duck_target: 1.0,
            manual_duck: None,
            events: VecDeque::new(),
            stats: MusicStats::default(),
            music_buf: vec![0.0; MAX_CHUNK * 2],
            sting_buf: vec![0.0; MAX_CHUNK * 2],
            pop_buf: vec![[0.0; 2]; MAX_CHUNK],
            started: false,
            playlist_pos: 0,
            rng: 0x2545_F491_4F6C_DD1D,
            zone: None,
        }
    }

    pub fn def(&self) -> &Arc<MusicDef> {
        &self.def
    }

    pub fn sample_rate(&self) -> u32 {
        self.cfg.sample_rate
    }

    // ------------------------------------------------------------------ commands

    /// Enters the initial state without a transition (call once).
    pub fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let init = if !self.def.initial_state.is_empty() {
            self.def.initial_state.clone()
        } else {
            self.def.states.keys().next().cloned().unwrap_or_default()
        };
        let Some(st) = self.def.states.get(&init).cloned() else { return };
        self.state = init.clone();
        self.state_since = self.clock;
        if let Some(v) = st.intensity {
            self.intensity = v.clamp(0.0, 1.0);
            self.intensity_target = self.intensity;
        }
        if let Some(cue) = &st.cue {
            let mut c = self.make_cue(cue, Kind::Cue, &st, None);
            c.started = false;
            c.fade_in = FadeDef::new(0.02, Curve::Linear);
            c.start_at = self.clock;
            self.cues.push(c);
            self.events.push_back(MusicEvent::StateChanged { from: String::new(), to: init });
        }
        self.prefetch.pump();
    }

    pub fn state(&self) -> &str {
        &self.state
    }

    pub fn pending_state(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.to.as_str())
    }

    /// Requests a state change using the matching transition rule. `force` ignores `min_hold_secs`.
    fn next_rand(&mut self) -> u64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `[cue] + playlist` of a state.
    fn cue_list(st: &StateDef) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(c) = &st.cue {
            v.push(c.clone());
        }
        v.extend(st.playlist.iter().cloned());
        v
    }

    pub fn set_state(&mut self, name: &str, force: bool) -> bool {
        let Some(st0) = self.def.states.get(name) else {
            self.events.push_back(MusicEvent::Ignored { what: format!("state `{name}`") });
            return false;
        };
        // "follow_zone" states hand over to the remembered zone's state
        let mut name = name.to_string();
        if st0.follow_zone {
            if let Some(z) = self.zone.and_then(|z| self.def.zones.get(&z)) {
                if self.def.states.contains_key(z) {
                    name = z.clone();
                }
            }
        }
        let name = name.as_str();
        let st = self.def.states[name].clone();
        let effective = self.pending.as_ref().map(|p| p.to.clone()).unwrap_or_else(|| self.state.clone());
        if name == effective {
            return true;
        }
        if name == self.state {
            // back to where we are: cancel the pending move
            self.pending = None;
            return true;
        }
        self.pending = None;
        let rule = self.def.rule_for(&self.state, name).clone();
        let playing = self.playing_cue_name();
        let list = Conductor::cue_list(&st);
        let pos = if st.shuffle && list.len() > 1 { (self.next_rand() % list.len() as u64) as usize } else { 0 };
        let first = list.get(pos).cloned();
        let same_cue = first == playing;
        let rate = self.cfg.sample_rate;

        let mut fire_at = self.clock;
        if rule.sync != SyncMode::Immediate {
            if let Some(w) = self.frames_to_sync(rule.sync) {
                fire_at = self.clock + w.min(secs_to_frames(rule.max_wait_secs, rate));
            }
        }
        if !force {
            if let Some(cur) = self.def.states.get(&self.state) {
                fire_at = fire_at.max(self.state_since + secs_to_frames(cur.min_hold_secs, rate));
            }
        }
        let prepared = match (&first, same_cue) {
            (Some(c), false) => Some(self.make_cue(c, Kind::Cue, &st, None)),
            _ => None,
        };
        let mult: Vec<(String, f32)> = st.stem_gains.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let deadline = fire_at + secs_to_frames(self.cfg.max_prepare_secs, rate);
        let in_secs = (fire_at - self.clock) as f32 / rate as f32;
        self.events.push_back(MusicEvent::TransitionScheduled { from: self.state.clone(), to: name.to_string(), in_secs });
        self.pending = Some(Pending { to: name.to_string(), rule, fire_at, deadline, prepared, mult, state_gain: st.gain, playlist_pos: pos });
        self.prefetch.pump();
        self.try_fire();
        true
    }

    fn playing_cue_name(&self) -> Option<String> {
        self.cues.iter().rev().find(|c| c.kind == Kind::Cue && !c.fading_out).map(|c| c.cue.clone())
    }

    /// Frames until the next sync point of the playing cue (None if unknown).
    fn frames_to_sync(&self, mode: SyncMode) -> Option<u64> {
        let c = self.cues.iter().rev().find(|c| c.kind == Kind::Cue && !c.fading_out && c.started)?;
        let info: StemInfo = c.stems.first()?.ring.info()?;
        let t = c.age;
        let b = match mode {
            SyncMode::Immediate => return None,
            SyncMode::LoopEnd => info.next_boundary(t)?,
            SyncMode::Beat => info.next_beat(t, false)?,
            SyncMode::Bar => info.next_beat(t, true)?,
        };
        Some(b.saturating_sub(t))
    }

    pub fn set_zone(&mut self, zone: i32) -> bool {
        match self.def.zones.get(&zone).cloned() {
            Some(s) => {
                self.zone = Some(zone);
                let cur = self.pending.as_ref().map(|p| p.to.as_str()).unwrap_or(self.state.as_str());
                if self.def.states.get(cur).map(|c| c.zone_locked).unwrap_or(false) {
                    return true; // remembered; applied when a follow_zone state is entered
                }
                self.set_state(&s, false)
            }
            None => {
                self.events.push_back(MusicEvent::Ignored { what: format!("zone {zone}") });
                false
            }
        }
    }

    /// Runs a named event (mission hook / custom): state change, stinger, intensity.
    pub fn event(&mut self, name: &str) -> bool {
        let Some(a) = self.def.events.get(name).cloned() else {
            self.events.push_back(MusicEvent::Ignored { what: format!("event `{name}`") });
            return false;
        };
        if let Some(v) = a.intensity {
            self.set_intensity(v);
        }
        if let Some(d) = a.intensity_delta {
            self.set_intensity(self.intensity_target + d);
        }
        if let Some(s) = &a.stinger {
            self.trigger_stinger(s);
        }
        if let Some(s) = &a.state {
            self.set_state(s, false);
        }
        true
    }

    pub fn set_intensity(&mut self, v: f32) {
        self.intensity_target = v.clamp(0.0, 1.0);
    }

    pub fn intensity(&self) -> f32 {
        self.intensity
    }

    pub fn set_volume(&mut self, v: f32, fade_secs: f32) {
        let f = secs_to_frames(fade_secs, self.cfg.sample_rate);
        self.master.retarget(v.max(0.0), f, Curve::Linear);
    }

    pub fn pause(&mut self, fade_secs: f32) {
        self.pause_requested = true;
        let f = secs_to_frames(fade_secs, self.cfg.sample_rate);
        self.pause.retarget(0.0, f, Curve::EqualPower);
    }

    pub fn resume(&mut self, fade_secs: f32) {
        self.pause_requested = false;
        self.frozen = false;
        let f = secs_to_frames(fade_secs, self.cfg.sample_rate);
        self.pause.retarget(1.0, f, Curve::EqualPower);
    }

    pub fn is_paused(&self) -> bool {
        self.pause_requested
    }

    /// Ducks the (non-stinger) music to `depth` (linear gain, 1 = off) until [`Conductor::unduck`].
    pub fn duck(&mut self, depth: f32, attack_secs: f32, release_secs: f32) {
        self.manual_duck = Some((depth.clamp(0.0, 1.0), attack_secs, release_secs));
    }

    pub fn unduck(&mut self) {
        self.manual_duck = None;
    }

    pub fn trigger_stinger(&mut self, name: &str) -> bool {
        let Some(s) = self.def.stingers.get(name).cloned() else {
            self.events.push_back(MusicEvent::Ignored { what: format!("stinger `{name}`") });
            return false;
        };
        let cue = CueDef {
            stems: vec![StemDef { name: "main".into(), track: s.track.clone(), intro: None, gain: s.gain, layer: None }],
            gain: 1.0,
            start_secs: 0.0,
        };
        let st = StateDef::default();
        let mut c = self.make_cue_from(&format!("stinger:{name}"), &cue, Kind::Stinger, &st, Some(name.to_string()));
        c.duck = (s.duck_depth < 0.999).then_some((s.duck_depth, s.duck_attack_secs, s.duck_release_secs));
        c.fade_in = FadeDef::new(s.fade_in_secs, Curve::Linear);
        c.start_at = self.clock;
        self.cues.push(c);
        self.prefetch.pump();
        true
    }

    pub fn poll_events(&mut self) -> Vec<MusicEvent> {
        self.events.drain(..).collect()
    }

    pub fn stats(&self) -> MusicStats {
        MusicStats { active_cues: self.cues.len(), ..self.stats }
    }

    pub fn status(&self) -> MusicStatus {
        MusicStatus {
            state: self.state.clone(),
            pending: self.pending.as_ref().map(|p| p.to.clone()),
            intensity: self.intensity,
            paused: self.pause_requested,
            clock_secs: self.clock as f64 / self.cfg.sample_rate as f64,
            cues: self
                .cues
                .iter()
                .map(|c| CueStatus {
                    cue: c.cue.clone(),
                    started: c.started,
                    fading_out: c.fading_out,
                    gain: c.env.value(),
                    age_secs: c.age as f64 / self.cfg.sample_rate as f64,
                    stinger: c.kind == Kind::Stinger,
                })
                .collect(),
        }
    }

    /// Replaces the definition (hot reload). The playing cue keeps playing; state names that vanished
    /// keep their last cue until the next transition.
    pub fn replace_def(&mut self, def: Arc<MusicDef>) {
        self.def = def;
        self.pending = None;
        let m = self.def.master_gain;
        self.master.retarget(m, 0, Curve::Linear);
    }

    // ------------------------------------------------------------------ internals

    fn make_cue(&mut self, name: &str, kind: Kind, st: &StateDef, stinger: Option<String>) -> ActiveCue {
        let cue = self.def.cues.get(name).cloned().unwrap_or_default();
        self.make_cue_from(name, &cue, kind, st, stinger)
    }

    fn make_cue_from(&mut self, name: &str, cue: &CueDef, kind: Kind, st: &StateDef, stinger: Option<String>) -> ActiveCue {
        let rate = self.cfg.sample_rate;
        let mut stems = Vec::new();
        for (i, s) in cue.stems.iter().enumerate() {
            let Some(track) = self.def.tracks.get(&s.track).cloned() else {
                self.events.push_back(MusicEvent::TrackError { cue: name.to_string(), stem: s.name.clone(), message: format!("unknown track `{}`", s.track) });
                continue;
            };
            let mut track = track;
            if kind == Kind::Stinger {
                track.looping = false;
            }
            let intro = s.intro.as_ref().and_then(|k| self.def.tracks.get(k).cloned().map(|t| (k.clone(), t)));
            let resolver = self.resolver.clone();
            let ctx = OpenContext { cue: name.trim_start_matches("stinger:").to_string(), stem: s.name.clone(), primary: i == 0, track_key: s.track.clone() };
            let start_secs = cue.start_secs;
            let ring = Arc::new(Ring::default());
            let opener: Box<dyn FnOnce() -> crate::error::Result<StemGen> + Send> = Box::new(move || {
                let body = resolver.open(&ctx, &track)?;
                let intro_src = match &intro {
                    Some((k, t)) => {
                        let ictx = OpenContext { cue: ctx.cue.clone(), stem: format!("{}.intro", ctx.stem), primary: false, track_key: k.clone() };
                        Some((resolver.open(&ictx, t)?, t))
                    }
                    None => None,
                };
                Ok(StemGen::new(intro_src, (body, &track), rate, start_secs))
            });
            self.prefetch.add(StemFeed::new(ring.clone(), opener, self.cfg.read_ahead_frames));
            let mult = st.stem_gains.get(&s.name).copied().unwrap_or(1.0) * st.gain;
            stems.push(ActiveStem {
                name: s.name.clone(),
                ring,
                base_gain: s.gain,
                layer: s.layer.clone(),
                mult,
                gain: Ramp::constant(0.0),
                last_target: f32::NAN,
                reported_error: false,
            });
        }
        ActiveCue {
            cue: name.to_string(),
            kind,
            stems,
            env: Ramp::constant(0.0),
            cue_gain: cue.gain,
            started: false,
            start_at: u64::MAX,
            created: self.clock,
            fading_out: false,
            age: 0,
            fade_in: FadeDef::new(0.0, Curve::Linear),
            duck: None,
            slew_until: 0,
            slew_frames: 0,
            stinger_name: stinger,
            advanced: false,
        }
    }

    /// Starts the next entry of the current state's playlist; the playing cue fades out over
    /// `overlap_secs` (0 = it has already ended).
    fn advance_playlist(&mut self, overlap_secs: f32) {
        let Some(st) = self.def.states.get(&self.state).cloned() else { return };
        let list = Conductor::cue_list(&st);
        if list.len() < 2 {
            return;
        }
        let next = if st.shuffle {
            let mut n = (self.next_rand() % list.len() as u64) as usize;
            if n == self.playlist_pos {
                n = (n + 1) % list.len();
            }
            n
        } else {
            (self.playlist_pos + 1) % list.len()
        };
        self.playlist_pos = next;
        let rate = self.cfg.sample_rate;
        let ov = secs_to_frames(overlap_secs, rate);
        for c in self.cues.iter_mut().filter(|c| c.kind == Kind::Cue && !c.fading_out) {
            c.advanced = true;
            if ov > 0 {
                c.fading_out = true;
                c.env.retarget(0.0, ov, Curve::EqualPower);
            }
        }
        let mut c = self.make_cue(&list[next], Kind::Cue, &st, None);
        c.start_at = self.clock;
        c.fade_in = FadeDef::new(st.playlist_fade_secs.max(overlap_secs), Curve::EqualPower);
        self.cues.push(c);
        self.prefetch.pump();
    }

    fn cue_ready(&self, c: &ActiveCue) -> bool {
        c.stems.iter().all(|s| s.ring.ready(self.cfg.prebuffer_frames))
    }

    fn try_fire(&mut self) {
        let Some(p) = self.pending.as_ref() else { return };
        if self.clock < p.fire_at {
            return;
        }
        let ready = match &p.prepared {
            Some(c) => self.cue_ready(c) || self.clock >= p.deadline,
            None => true,
        };
        if !ready {
            return;
        }
        let mut p = self.pending.take().unwrap();
        let rate = self.cfg.sample_rate;
        let from = std::mem::replace(&mut self.state, p.to.clone());
        self.state_since = self.clock;
        self.playlist_pos = p.playlist_pos;
        let fo = secs_to_frames(p.rule.fade_out.secs, rate);
        let fi = secs_to_frames(p.rule.fade_in.secs, rate);
        let st = self.def.states.get(&p.to).cloned().unwrap_or_default();
        let replacing = p.prepared.is_some() || st.cue.is_none();
        if replacing {
            for c in self.cues.iter_mut().filter(|c| c.kind == Kind::Cue && !c.fading_out) {
                c.fading_out = true;
                c.env.retarget(0.0, fo, p.rule.fade_out.curve.clone());
            }
        } else if let Some(c) = self.cues.iter_mut().rev().find(|c| c.kind == Kind::Cue && !c.fading_out) {
            // same cue, different state: re-balance the stems
            for s in &mut c.stems {
                s.mult = p.mult.iter().find(|(k, _)| *k == s.name).map(|(_, v)| *v).unwrap_or(1.0) * p.state_gain;
            }
            c.slew_frames = fi.max(1);
            c.slew_until = self.clock + fi;
        }
        if let Some(mut c) = p.prepared.take() {
            let delay = secs_to_frames(p.rule.delay_secs, rate);
            c.start_at = self.clock + delay + if p.rule.overlap { 0 } else { fo };
            c.fade_in = p.rule.fade_in.clone();
            c.created = self.clock;
            self.cues.push(c);
        }
        if let Some(v) = st.intensity {
            self.set_intensity(v);
        }
        if let Some(s) = &p.rule.stinger {
            let s = s.clone();
            self.trigger_stinger(&s);
        }
        self.events.push_back(MusicEvent::StateChanged { from, to: p.to });
    }

    fn next_event_in(&self) -> usize {
        let mut n = MAX_CHUNK as u64;
        let c = self.clock;
        let mut consider = |t: u64| {
            if t > c && t - c < n {
                n = t - c;
            }
        };
        if let Some(p) = &self.pending {
            consider(p.fire_at);
        }
        for q in &self.cues {
            if !q.started && q.start_at != u64::MAX {
                consider(q.start_at);
            }
        }
        n as usize
    }

    /// Renders `out.len() / 2` stereo frames, adding into a zeroed `out`.
    pub fn render(&mut self, out: &mut [f32]) {
        for v in out.iter_mut() {
            *v = 0.0;
        }
        if !self.started {
            self.start();
        }
        let total = out.len() / 2;
        let mut done = 0;
        while done < total {
            if self.frozen {
                break;
            }
            if !self.prefetch.is_threaded() {
                self.prefetch.pump();
            }
            self.try_fire();
            let n = (total - done).min(self.next_event_in()).max(1);
            self.render_chunk(&mut out[done * 2..(done + n) * 2], n);
            done += n;
        }
        self.stats.frames_rendered += done as u64;
    }

    fn render_chunk(&mut self, out: &mut [f32], n: usize) {
        let rate = self.cfg.sample_rate;
        // start cues whose time has come
        let clock = self.clock;
        let ready_flags: Vec<bool> = self.cues.iter().map(|c| !c.started && clock >= c.start_at && (self.cue_ready(c) || clock >= c.created + secs_to_frames(self.cfg.max_prepare_secs, rate))).collect();
        for (c, go) in self.cues.iter_mut().zip(ready_flags) {
            if go {
                c.started = true;
                let fi = secs_to_frames(c.fade_in.secs, rate);
                c.env = Ramp::constant(0.0);
                c.env.retarget(1.0, fi, c.fade_in.curve.clone());
                if c.kind == Kind::Cue {
                    self.events.push_back(MusicEvent::CueStarted { cue: c.cue.clone() });
                } else if let Some(s) = &c.stinger_name {
                    self.events.push_back(MusicEvent::StingerStarted { stinger: s.clone() });
                }
            }
        }

        // intensity smoothing
        if (self.intensity_target - self.intensity).abs() > 1e-6 {
            let smooth = self.def.intensity.as_ref().map(|i| i.smooth_secs).unwrap_or(1.0);
            if smooth <= 0.0 {
                self.intensity = self.intensity_target;
            } else {
                let step = n as f32 / (smooth * rate as f32);
                let d = self.intensity_target - self.intensity;
                self.intensity += d.clamp(-step, step);
            }
        }

        // ducking target
        let mut want = 1.0f32;
        let mut attack = 0.05;
        let mut release = 0.5;
        for c in self.cues.iter().filter(|c| c.kind == Kind::Stinger && c.started) {
            if let Some((d, a, r)) = c.duck {
                if d < want {
                    want = d;
                    attack = a;
                    release = r;
                }
            }
        }
        if let Some((d, a, r)) = self.manual_duck {
            if d < want {
                want = d;
                attack = a;
                release = r;
            }
        }
        if (want - self.duck_target).abs() > 1e-4 {
            let secs = if want < self.duck_target { attack } else { release };
            self.duck.retarget(want, secs_to_frames(secs, rate), Curve::SCurve);
            self.duck_target = want;
        }

        for v in self.music_buf[..n * 2].iter_mut() {
            *v = 0.0;
        }
        for v in self.sting_buf[..n * 2].iter_mut() {
            *v = 0.0;
        }

        let intensity = self.intensity;
        let clock = self.clock;
        let mut finished: Vec<usize> = Vec::new();
        for (ci, c) in self.cues.iter_mut().enumerate() {
            if !c.started {
                continue;
            }
            let dest = if c.kind == Kind::Cue { &mut self.music_buf } else { &mut self.sting_buf };
            let live: Vec<usize> = (0..c.stems.len()).filter(|&i| !c.stems[i].ring.drained()).collect();
            for s in c.stems.iter_mut() {
                if !s.reported_error {
                    if let Some(msg) = s.ring.failed() {
                        s.reported_error = true;
                        self.events.push_back(MusicEvent::TrackError { cue: c.cue.clone(), stem: s.name.clone(), message: msg });
                    }
                }
            }
            if live.is_empty() {
                finished.push(ci);
                continue;
            }
            let avail = live.iter().map(|&i| c.stems[i].ring.available()).min().unwrap_or(0);
            let use_n = avail.min(n);
            if use_n < n {
                self.stats.underruns += 1;
            }
            // per-stem gain targets for this chunk
            let slewing = clock < c.slew_until;
            for &i in &live {
                let s = &mut c.stems[i];
                let layer = s.layer.as_ref().map(|l| l.gain_at(intensity)).unwrap_or(1.0);
                let target = c.cue_gain * s.base_gain * layer * s.mult;
                if s.last_target.is_nan() {
                    s.gain = Ramp::constant(target);
                } else if (target - s.last_target).abs() > 1e-5 {
                    let frames = if slewing { c.slew_frames.max(n as u64) } else { n as u64 };
                    s.gain.retarget(target, frames, Curve::Linear);
                }
                s.last_target = target;
            }
            let mut env_vals = [0.0f32; MAX_CHUNK];
            for e in env_vals.iter_mut().take(n) {
                *e = c.env.next();
            }
            for &i in &live {
                let s = &mut c.stems[i];
                let got = s.ring.pop(&mut self.pop_buf[..use_n]);
                for k in 0..got {
                    let g = s.gain.next() * env_vals[k];
                    dest[k * 2] += self.pop_buf[k][0] * g;
                    dest[k * 2 + 1] += self.pop_buf[k][1] * g;
                }
                if got < n {
                    s.gain.advance((n - got) as u64);
                }
            }
            c.age += use_n as u64;
            if c.fading_out && c.env.is_done() && c.env.value() <= 1e-6 {
                finished.push(ci);
            }
        }

        // combine
        for i in 0..n {
            let d = self.duck.next();
            let m = self.master.next() * self.pause.next();
            out[i * 2] = (self.music_buf[i * 2] * d + self.sting_buf[i * 2]) * m;
            out[i * 2 + 1] = (self.music_buf[i * 2 + 1] * d + self.sting_buf[i * 2 + 1]) * m;
        }
        self.clock += n as u64;

        // retire finished cues (indices ascending)
        finished.sort_unstable();
        finished.dedup();
        let mut on_finish: Option<String> = None;
        let mut advance = false;
        for &ci in finished.iter().rev() {
            let c = self.cues.remove(ci);
            if c.kind == Kind::Cue && !c.fading_out {
                self.events.push_back(MusicEvent::CueFinished { cue: c.cue.clone() });
                if let Some(st) = self.def.states.get(&self.state) {
                    let in_state = st.cue.as_deref() == Some(c.cue.as_str()) || st.playlist.iter().any(|p| *p == c.cue);
                    if in_state && !st.playlist.is_empty() {
                        advance = !c.advanced;
                    } else if in_state {
                        on_finish = st.on_finish.clone();
                    }
                }
            }
        }
        if advance && self.pending.is_none() {
            self.advance_playlist(0.0);
        }
        if let Some(s) = on_finish {
            self.set_state(&s, true);
        }
        // start the next playlist entry early (crossfade) when the state asks for it
        if self.pending.is_none() {
            if let Some(st) = self.def.states.get(&self.state) {
                if !st.playlist.is_empty() && st.playlist_overlap_secs > 0.0 {
                    let ov = secs_to_frames(st.playlist_overlap_secs, rate);
                    let due = self.cues.iter().any(|c| {
                        c.kind == Kind::Cue
                            && c.started
                            && !c.fading_out
                            && !c.advanced
                            && c.stems.first().and_then(|s| s.ring.info()).and_then(|i| i.total).map(|t| t.saturating_sub(c.age) <= ov).unwrap_or(false)
                    });
                    if due {
                        self.advance_playlist(st.playlist_overlap_secs);
                    }
                }
            }
        }

        if self.pause_requested && self.pause.is_done() && self.pause.value() <= 1e-6 {
            self.frozen = true;
        }
        // drop fully faded cues even while their rings still have data
        self.cues.retain(|c| !(c.fading_out && c.started && c.env.is_done() && c.env.value() <= 1e-6));
    }
}
