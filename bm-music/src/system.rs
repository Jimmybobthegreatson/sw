//! Game-facing music API: [`MusicSystem`] (state machine control) and [`MusicMixer`] (the `PcmSource`
//! that plays on `Bus::Music`).

use crate::conductor::{Conductor, ConductorConfig, MusicEvent, MusicStats, MusicStatus};
use crate::def::MusicDef;
use crate::error::{MusicError, Result};
use crate::feed::Prefetch;
use crate::source::TrackResolver;
use bm_sound::mixer::{Bus, Ducking, PcmSource, VoiceHandle, VoiceParams};
use bm_sound::AudioEngine;
use std::sync::{Arc, Mutex, MutexGuard};

/// Standard gameplay moments a level script can report. Each maps to an event name that a
/// [`MusicDef`] binds in `events` (the generated defaults bind all of them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionEvent {
    MissionStart,
    /// Back to normal play (no suspicion).
    Calm,
    /// Guards suspicious / trespassing.
    Suspicious,
    /// Cover blown, pursuit.
    Alert,
    Combat,
    /// Target eliminated.
    TargetKilled,
    MissionComplete,
    MissionFailed,
    PlayerDead,
    /// Pause menu / briefing screens.
    Paused,
    Resumed,
}

impl MissionEvent {
    pub const ALL: [MissionEvent; 11] = [
        MissionEvent::MissionStart,
        MissionEvent::Calm,
        MissionEvent::Suspicious,
        MissionEvent::Alert,
        MissionEvent::Combat,
        MissionEvent::TargetKilled,
        MissionEvent::MissionComplete,
        MissionEvent::MissionFailed,
        MissionEvent::PlayerDead,
        MissionEvent::Paused,
        MissionEvent::Resumed,
    ];

    /// Event name used in `MusicDef::events`.
    pub fn name(self) -> &'static str {
        match self {
            MissionEvent::MissionStart => "mission_start",
            MissionEvent::Calm => "calm",
            MissionEvent::Suspicious => "suspicious",
            MissionEvent::Alert => "alert",
            MissionEvent::Combat => "combat",
            MissionEvent::TargetKilled => "target_killed",
            MissionEvent::MissionComplete => "mission_complete",
            MissionEvent::MissionFailed => "mission_failed",
            MissionEvent::PlayerDead => "player_dead",
            MissionEvent::Paused => "paused",
            MissionEvent::Resumed => "resumed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MusicOptions {
    /// Rate of the mix; use the engine's rate so the mixer does not resample again.
    pub sample_rate: u32,
    /// Background read-ahead thread. `false` = manual mode (reads happen inside `render`, fully
    /// deterministic; used by offline rendering and tests).
    pub threaded: bool,
    /// Seconds of decoded audio kept ahead per stem.
    pub read_ahead_secs: f32,
    /// Seconds that must be buffered before a prepared cue is started.
    pub prebuffer_secs: f32,
    /// Longest wait for a prepared cue before it starts anyway.
    pub max_prepare_secs: f32,
}

impl Default for MusicOptions {
    fn default() -> Self {
        MusicOptions { sample_rate: 48_000, threaded: true, read_ahead_secs: 1.5, prebuffer_secs: 0.15, max_prepare_secs: 1.0 }
    }
}

impl MusicOptions {
    /// Deterministic, thread-free options for offline rendering and tests.
    pub fn offline(sample_rate: u32) -> MusicOptions {
        MusicOptions { sample_rate, threaded: false, read_ahead_secs: 0.5, prebuffer_secs: 0.1, max_prepare_secs: 1.0 }
    }
    pub fn for_engine(engine: &dyn AudioEngine) -> MusicOptions {
        MusicOptions { sample_rate: engine.sample_rate(), ..Default::default() }
    }
}

/// The audio-thread end: a never-ending stereo `PcmSource` pulling from the conductor.
#[derive(Clone)]
pub struct MusicMixer {
    cond: Arc<Mutex<Conductor>>,
    rate: u32,
}

impl PcmSource for MusicMixer {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u16 {
        2
    }
    fn read(&mut self, out: &mut [f32]) -> usize {
        let frames = out.len() / 2;
        let mut c = self.cond.lock().unwrap_or_else(|p| p.into_inner());
        c.render(&mut out[..frames * 2]);
        frames
    }
}

pub struct MusicSystem {
    cond: Arc<Mutex<Conductor>>,
    prefetch: Arc<Prefetch>,
    opts: MusicOptions,
    voice: Option<VoiceHandle>,
}

impl MusicSystem {
    /// Validates `def`, enters its initial state and starts reading ahead. Nothing is audible until
    /// the mixer is attached to an engine ([`MusicSystem::attach`]) or rendered offline.
    pub fn new(def: MusicDef, resolver: Arc<dyn TrackResolver>, opts: MusicOptions) -> Result<MusicSystem> {
        def.validate().map_err(MusicError::Invalid)?;
        let rate = opts.sample_rate;
        let prefetch = Arc::new(Prefetch::new(opts.threaded));
        let cfg = ConductorConfig {
            sample_rate: rate,
            read_ahead_frames: (opts.read_ahead_secs * rate as f32) as usize,
            prebuffer_frames: (opts.prebuffer_secs * rate as f32) as usize,
            max_prepare_secs: opts.max_prepare_secs,
        };
        let mut c = Conductor::new(Arc::new(def), resolver, prefetch.clone(), cfg);
        c.start();
        Ok(MusicSystem { cond: Arc::new(Mutex::new(c)), prefetch, opts, voice: None })
    }

    fn c(&self) -> MutexGuard<'_, Conductor> {
        self.cond.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn options(&self) -> &MusicOptions {
        &self.opts
    }

    /// The `PcmSource` for custom hosts (offline tools, other engines).
    pub fn mixer(&self) -> MusicMixer {
        MusicMixer { cond: self.cond.clone(), rate: self.opts.sample_rate }
    }

    /// Starts the music stream on `Bus::Music` and installs the definition's voice ducking rule.
    pub fn attach(&mut self, engine: &mut dyn AudioEngine) -> Result<VoiceHandle> {
        let mut p = VoiceParams::on_bus(Bus::Music);
        p.priority = 255;
        let h = engine.play_stream(Box::new(self.mixer()), p).ok_or(MusicError::NoVoice)?;
        if let Some(d) = self.c().def().ducking.clone() {
            engine.set_ducking(Some(Ducking { trigger: Bus::Voice, target: Bus::Music, depth: d.voice_depth, attack: d.attack_secs, release: d.release_secs }));
        }
        self.voice = Some(h);
        Ok(h)
    }

    pub fn detach(&mut self, engine: &mut dyn AudioEngine, fade_secs: f32) {
        if let Some(h) = self.voice.take() {
            engine.stop(h, fade_secs);
        }
    }

    /// Offline: renders `frames` stereo frames directly from the conductor.
    pub fn render(&self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        self.c().render(&mut out);
        out
    }

    // ---- control

    /// Requests a state (uses the matching transition rule). Returns false for unknown states.
    pub fn set_state(&self, name: &str) -> bool {
        self.c().set_state(name, false)
    }
    /// Like `set_state` but ignores the current state's `min_hold_secs`.
    pub fn force_state(&self, name: &str) -> bool {
        self.c().set_state(name, true)
    }
    pub fn state(&self) -> String {
        self.c().state().to_string()
    }
    /// State a transition is heading to, if one is scheduled.
    pub fn pending_state(&self) -> Option<String> {
        self.c().pending_state().map(|s| s.to_string())
    }
    /// Enters the state bound to a `MusicLocationDef` zone id.
    pub fn set_zone(&self, zone: i32) -> bool {
        self.c().set_zone(zone)
    }
    pub fn event(&self, name: &str) -> bool {
        self.c().event(name)
    }
    pub fn mission_event(&self, e: MissionEvent) -> bool {
        match e {
            MissionEvent::Paused => {
                self.pause(0.4);
                true
            }
            MissionEvent::Resumed => {
                self.resume(0.4);
                true
            }
            _ => self.c().event(e.name()),
        }
    }
    /// 0..1, smoothed according to `IntensityDef::smooth_secs`.
    pub fn set_intensity(&self, v: f32) {
        self.c().set_intensity(v)
    }
    pub fn intensity(&self) -> f32 {
        self.c().intensity()
    }
    pub fn trigger_stinger(&self, name: &str) -> bool {
        self.c().trigger_stinger(name)
    }
    /// Ducks the music to `depth` (linear) until `unduck`.
    pub fn duck(&self, depth: f32, attack_secs: f32, release_secs: f32) {
        self.c().duck(depth, attack_secs, release_secs)
    }
    pub fn unduck(&self) {
        self.c().unduck()
    }
    pub fn pause(&self, fade_secs: f32) {
        self.c().pause(fade_secs)
    }
    pub fn resume(&self, fade_secs: f32) {
        self.c().resume(fade_secs)
    }
    pub fn is_paused(&self) -> bool {
        self.c().is_paused()
    }
    /// Music volume multiplier (on top of the `Bus::Music` volume), faded.
    pub fn set_volume(&self, v: f32, fade_secs: f32) {
        self.c().set_volume(v, fade_secs)
    }
    /// Hot-reloads a definition (validated). The playing audio keeps going; use `set_state` to move.
    pub fn replace_def(&self, def: MusicDef) -> Result<()> {
        def.validate().map_err(MusicError::Invalid)?;
        self.c().replace_def(Arc::new(def));
        Ok(())
    }
    pub fn def(&self) -> Arc<MusicDef> {
        self.c().def().clone()
    }
    pub fn poll_events(&self) -> Vec<MusicEvent> {
        self.c().poll_events()
    }
    pub fn stats(&self) -> MusicStats {
        self.c().stats()
    }
    pub fn status(&self) -> MusicStatus {
        self.c().status()
    }
    /// Manual mode only: fill all read-ahead rings now.
    pub fn pump(&self) {
        self.prefetch.pump();
    }
}
