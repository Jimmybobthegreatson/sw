//! Plain-data music definition: tracks, cues (layered stems), states, transitions, zones and events.
//! Everything derives `serde` so modders can author it as RON or JSON (see `docs/music.md` for the full
//! reference and examples). All structs use `#[serde(default)]`, so only the interesting fields need to
//! be written.

use crate::curve::{Curve, FadeDef};
use crate::error::{MusicError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where the audio of a track comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SourceRef {
    /// A sound of the loaded sound banks by name, e.g. `"Music/Exploration.wav"` (streamed PCM from
    /// `pc_eng.str` for the original music). Loose replacements apply, see the modding rules.
    Bank(String),
    /// Slot of the level's 64-entry music table (`_sounddefinitionsbloodmoney/music/music_mXX`).
    Slot(u32),
    /// A file in the VFS (mods): `"Music/MyMod/theme.ogg"` (ogg/flac/wav).
    File(String),
}

/// One piece of audio with loop information.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackDef {
    pub source: SourceRef,
    /// Linear gain applied to the track (original per-sound trim, mod balancing).
    pub gain: f32,
    /// Original per-sound trim of the shipped audio (the generated defaults store `volume byte / 39` here).
    /// Not applied when a loose mod file replaces the audio of the track.
    pub trim: f32,
    /// Loop the track (whole file unless a loop region is given). `false` plays once.
    pub looping: bool,
    /// Loop region start (seconds, default 0). Audio before it plays once ("attack").
    pub loop_start_secs: Option<f64>,
    /// Loop region end (seconds, default end of file).
    pub loop_end_secs: Option<f64>,
    /// Sample-exact alternatives (source frames); win over the `_secs` fields.
    pub loop_start_frames: Option<u64>,
    pub loop_end_frames: Option<u64>,
    /// Equal-power crossfade of the loop tail into the loop head, for material that does not wrap
    /// seamlessly. 0 = hard wrap (sample accurate).
    pub loop_crossfade_secs: f32,
    /// Tempo for beat/bar-quantised transitions.
    pub bpm: Option<f32>,
    pub beats_per_bar: u32,
    /// Time of the first beat (seconds).
    pub beat_offset_secs: f32,
}

impl Default for TrackDef {
    fn default() -> Self {
        TrackDef {
            source: SourceRef::Bank(String::new()),
            gain: 1.0,
            trim: 1.0,
            looping: true,
            loop_start_secs: None,
            loop_end_secs: None,
            loop_start_frames: None,
            loop_end_frames: None,
            loop_crossfade_secs: 0.0,
            bpm: None,
            beats_per_bar: 4,
            beat_offset_secs: 0.0,
        }
    }
}

impl TrackDef {
    pub fn bank(name: &str) -> TrackDef {
        TrackDef { source: SourceRef::Bank(name.to_string()), ..Default::default() }
    }
    pub fn file(path: &str) -> TrackDef {
        TrackDef { source: SourceRef::File(path.to_string()), ..Default::default() }
    }
}

/// Intensity window of a layered stem. `intensity` runs 0..1. The stem fades in between
/// `fade_in.0 .. fade_in.1` and (optionally) out between `fade_out.0 .. fade_out.1`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerDef {
    pub fade_in: (f32, f32),
    pub fade_out: Option<(f32, f32)>,
    pub curve: Curve,
}

impl Default for LayerDef {
    fn default() -> Self {
        LayerDef { fade_in: (0.0, 0.0), fade_out: None, curve: Curve::Linear }
    }
}

impl LayerDef {
    pub fn gain_at(&self, intensity: f32) -> f32 {
        let ramp = |a: f32, b: f32, x: f32| {
            if b <= a {
                if x >= b { 1.0 } else { 0.0 }
            } else {
                ((x - a) / (b - a)).clamp(0.0, 1.0)
            }
        };
        let up = self.curve.apply(ramp(self.fade_in.0, self.fade_in.1, intensity));
        let down = match self.fade_out {
            Some((a, b)) => 1.0 - self.curve.apply(ramp(a, b, intensity)),
            None => 1.0,
        };
        up * down
    }
}

/// One layer (stem) of a cue. All stems of a cue start together and stay sample-aligned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StemDef {
    pub name: String,
    /// Key into `MusicDef::tracks`.
    pub track: String,
    /// Optional track played once before `track` starts looping (e.g. `Hunter_Intro` -> `Hunter_V2`).
    pub intro: Option<String>,
    pub gain: f32,
    /// Intensity window; `None` = always audible.
    pub layer: Option<LayerDef>,
}

impl Default for StemDef {
    fn default() -> Self {
        StemDef { name: "main".into(), track: String::new(), intro: None, gain: 1.0, layer: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CueDef {
    pub stems: Vec<StemDef>,
    /// Cue gain in addition to the state gain.
    pub gain: f32,
    /// Seek into the cue when it (re)starts, seconds.
    pub start_secs: f64,
}

impl Default for CueDef {
    fn default() -> Self {
        CueDef { stems: Vec::new(), gain: 1.0, start_secs: 0.0 }
    }
}

impl CueDef {
    pub fn single(track: &str) -> CueDef {
        CueDef { stems: vec![StemDef { track: track.to_string(), ..Default::default() }], gain: 1.0, start_secs: 0.0 }
    }
}

/// A node of the music state machine ("ambient", "suspense", "combat"...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateDef {
    /// Cue played while the state is active; `None` = silence.
    pub cue: Option<String>,
    /// Intensity to set when entering (otherwise the current value is kept).
    pub intensity: Option<f32>,
    /// Minimum time the state stays active before another transition may start (anti-flapping).
    pub min_hold_secs: f32,
    pub gain: f32,
    /// Per-stem gain overrides for this state (`{"drums": 0.0}` mutes a stem).
    pub stem_gains: BTreeMap<String, f32>,
    /// State entered automatically when the cue ends (non-looping cues).
    pub on_finish: Option<String>,
    /// Further cues played after `cue` ends (non-looping cues), then wrapping around: a playlist.
    pub playlist: Vec<String>,
    /// Pick the first cue at random and the following ones in random order.
    pub shuffle: bool,
    /// Fade-in of the next playlist entry.
    pub playlist_fade_secs: f32,
    /// Start the next entry this long before the current one ends (crossfade); 0 = back to back.
    pub playlist_overlap_secs: f32,
    /// `set_zone` is remembered but does not leave this state (combat, alert...).
    pub zone_locked: bool,
    /// When entered, the state of the remembered zone is used instead (so "calm" returns to the zone music).
    pub follow_zone: bool,
}

impl Default for StateDef {
    fn default() -> Self {
        StateDef {
            cue: None,
            intensity: None,
            min_hold_secs: 0.0,
            gain: 1.0,
            stem_gains: BTreeMap::new(),
            on_finish: None,
            playlist: Vec::new(),
            shuffle: false,
            playlist_fade_secs: 2.0,
            playlist_overlap_secs: 0.0,
            zone_locked: false,
            follow_zone: false,
        }
    }
}

impl StateDef {
    pub fn cue(name: &str) -> StateDef {
        StateDef { cue: Some(name.to_string()), ..Default::default() }
    }
}

/// When a transition may fire relative to the playing cue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncMode {
    Immediate,
    /// Next beat of the playing cue's first stem track (`bpm`); immediate when it has no tempo.
    Beat,
    /// Next bar (`beats_per_bar` beats).
    Bar,
    /// Next loop point of the playing cue (or end of its intro).
    LoopEnd,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionRule {
    pub fade_out: FadeDef,
    pub fade_in: FadeDef,
    pub sync: SyncMode,
    /// Fire early if the sync point is further away than this many seconds.
    pub max_wait_secs: f32,
    /// `true`: new cue fades in while the old fades out. `false`: the new cue starts after the old
    /// one has faded out completely.
    pub overlap: bool,
    /// Extra delay between the sync point and the new cue's start.
    pub delay_secs: f32,
    /// Stinger (key into `stingers`) fired at the sync point.
    pub stinger: Option<String>,
}

impl Default for TransitionRule {
    fn default() -> Self {
        TransitionRule {
            fade_out: FadeDef::new(2.0, Curve::EqualPower),
            fade_in: FadeDef::new(2.0, Curve::EqualPower),
            sync: SyncMode::Immediate,
            max_wait_secs: 12.0,
            overlap: true,
            delay_secs: 0.0,
            stinger: None,
        }
    }
}

impl TransitionRule {
    /// Symmetric equal-power crossfade of `secs`.
    pub fn crossfade(secs: f32) -> TransitionRule {
        TransitionRule { fade_out: FadeDef::new(secs, Curve::EqualPower), fade_in: FadeDef::new(secs, Curve::EqualPower), ..Default::default() }
    }
}

/// `from`/`to` list state names; an empty list matches any state. The first matching entry wins.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionDef {
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub rule: TransitionRule,
}

/// One-shot sound laid over the music (hit, sting, end-of-mission phrase).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StingerDef {
    pub track: String,
    pub gain: f32,
    /// Level (linear, 1 = no ducking) the music is ducked to while the stinger plays.
    pub duck_depth: f32,
    pub duck_attack_secs: f32,
    pub duck_release_secs: f32,
    pub fade_in_secs: f32,
}

impl Default for StingerDef {
    fn default() -> Self {
        StingerDef { track: String::new(), gain: 1.0, duck_depth: 1.0, duck_attack_secs: 0.05, duck_release_secs: 0.8, fade_in_secs: 0.0 }
    }
}

/// Action bound to a named event (see [`crate::MissionEvent`] for the standard names).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventAction {
    pub state: Option<String>,
    pub stinger: Option<String>,
    pub intensity: Option<f32>,
    /// Add to the current intensity (clamped 0..1).
    pub intensity_delta: Option<f32>,
}

/// Engine-level ducking of the music bus by dialogue (`Bus::Voice` -> `Bus::Music`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuckingDef {
    pub voice_depth: f32,
    pub attack_secs: f32,
    pub release_secs: f32,
}

impl Default for DuckingDef {
    fn default() -> Self {
        DuckingDef { voice_depth: 0.5, attack_secs: 0.15, release_secs: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntensityDef {
    /// Intensity moves at most `1/smooth_secs` per second (0 = instant).
    pub smooth_secs: f32,
    pub initial: f32,
}

impl Default for IntensityDef {
    fn default() -> Self {
        IntensityDef { smooth_secs: 1.0, initial: 0.0 }
    }
}

/// A complete music set (one level, or a mod's additions).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicDef {
    pub version: u32,
    pub name: String,
    pub tracks: BTreeMap<String, TrackDef>,
    pub cues: BTreeMap<String, CueDef>,
    pub states: BTreeMap<String, StateDef>,
    pub initial_state: String,
    pub transitions: Vec<TransitionDef>,
    pub default_transition: TransitionRule,
    /// `MusicLocationDef` zone id -> state name.
    pub zones: BTreeMap<i32, String>,
    /// Named events (mission hooks and custom) -> action.
    pub events: BTreeMap<String, EventAction>,
    pub stingers: BTreeMap<String, StingerDef>,
    pub ducking: Option<DuckingDef>,
    pub intensity: Option<IntensityDef>,
    pub master_gain: f32,
}

impl Default for MusicDef {
    fn default() -> Self {
        MusicDef {
            version: 1,
            name: String::new(),
            tracks: BTreeMap::new(),
            cues: BTreeMap::new(),
            states: BTreeMap::new(),
            initial_state: String::new(),
            transitions: Vec::new(),
            default_transition: TransitionRule::default(),
            zones: BTreeMap::new(),
            events: BTreeMap::new(),
            stingers: BTreeMap::new(),
            ducking: None,
            intensity: None,
            master_gain: 1.0,
        }
    }
}

/// Text format of a definition file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Ron,
    Json,
}

impl Format {
    pub fn from_path(p: &str) -> Option<Format> {
        let l = p.to_ascii_lowercase();
        if l.ends_with(".ron") {
            Some(Format::Ron)
        } else if l.ends_with(".json") {
            Some(Format::Json)
        } else {
            None
        }
    }
}

fn ron_options() -> ron::Options {
    ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
}

impl MusicDef {
    pub fn parse(text: &str, fmt: Format) -> Result<MusicDef> {
        match fmt {
            Format::Ron => ron_options().from_str(text).map_err(|e| MusicError::Parse { what: "RON".into(), msg: e.to_string() }),
            Format::Json => serde_json::from_str(text).map_err(|e| MusicError::Parse { what: "JSON".into(), msg: e.to_string() }),
        }
    }

    pub fn to_ron(&self) -> String {
        let cfg = ron::ser::PrettyConfig::new().depth_limit(12).indentor("  ".into());
        ron_options().to_string_pretty(self, cfg).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Overlays `other` (a mod file) on `self`: maps are extended/replaced per key, `transitions` of the
    /// overlay come first (first match wins), scalars replace when the overlay sets them.
    pub fn merge(&mut self, other: MusicDef) {
        self.tracks.extend(other.tracks);
        self.cues.extend(other.cues);
        self.states.extend(other.states);
        self.zones.extend(other.zones);
        self.events.extend(other.events);
        self.stingers.extend(other.stingers);
        let mut t = other.transitions;
        t.append(&mut self.transitions);
        self.transitions = t;
        if !other.initial_state.is_empty() {
            self.initial_state = other.initial_state;
        }
        if other.ducking.is_some() {
            self.ducking = other.ducking;
        }
        if other.intensity.is_some() {
            self.intensity = other.intensity;
        }
        if (other.master_gain - 1.0).abs() > 1e-6 {
            self.master_gain = other.master_gain;
        }
        if other.default_transition != TransitionRule::default() {
            self.default_transition = other.default_transition;
        }
        if !other.name.is_empty() {
            self.name = other.name;
        }
    }

    /// Reference check. Returns every problem found.
    pub fn validate(&self) -> std::result::Result<(), Vec<String>> {
        let mut e = Vec::new();
        for (n, c) in &self.cues {
            if c.stems.is_empty() {
                e.push(format!("cue `{n}` has no stems"));
            }
            for s in &c.stems {
                if !self.tracks.contains_key(&s.track) {
                    e.push(format!("cue `{n}` stem `{}` uses unknown track `{}`", s.name, s.track));
                }
                if let Some(i) = &s.intro {
                    if !self.tracks.contains_key(i) {
                        e.push(format!("cue `{n}` stem `{}` uses unknown intro track `{i}`", s.name));
                    }
                }
            }
            let mut names: Vec<&str> = c.stems.iter().map(|s| s.name.as_str()).collect();
            names.sort();
            if names.windows(2).any(|w| w[0] == w[1]) {
                e.push(format!("cue `{n}` has duplicate stem names"));
            }
        }
        for (n, s) in &self.states {
            if let Some(c) = &s.cue {
                if !self.cues.contains_key(c) {
                    e.push(format!("state `{n}` uses unknown cue `{c}`"));
                }
            }
            for c in &s.playlist {
                if !self.cues.contains_key(c) {
                    e.push(format!("state `{n}` playlist uses unknown cue `{c}`"));
                }
            }
            if let Some(f) = &s.on_finish {
                if !self.states.contains_key(f) {
                    e.push(format!("state `{n}` on_finish -> unknown state `{f}`"));
                }
            }
        }
        if !self.initial_state.is_empty() && !self.states.contains_key(&self.initial_state) {
            e.push(format!("initial_state `{}` is not a state", self.initial_state));
        }
        for (i, t) in self.transitions.iter().enumerate() {
            for s in t.from.iter().chain(&t.to) {
                if !self.states.contains_key(s) {
                    e.push(format!("transition #{i} mentions unknown state `{s}`"));
                }
            }
            if let Some(s) = &t.rule.stinger {
                if !self.stingers.contains_key(s) {
                    e.push(format!("transition #{i} uses unknown stinger `{s}`"));
                }
            }
        }
        for (z, s) in &self.zones {
            if !self.states.contains_key(s) {
                e.push(format!("zone {z} -> unknown state `{s}`"));
            }
        }
        for (n, a) in &self.events {
            if let Some(s) = &a.state {
                if !self.states.contains_key(s) {
                    e.push(format!("event `{n}` -> unknown state `{s}`"));
                }
            }
            if let Some(s) = &a.stinger {
                if !self.stingers.contains_key(s) {
                    e.push(format!("event `{n}` -> unknown stinger `{s}`"));
                }
            }
        }
        for (n, s) in &self.stingers {
            if !self.tracks.contains_key(&s.track) {
                e.push(format!("stinger `{n}` uses unknown track `{}`", s.track));
            }
        }
        if e.is_empty() { Ok(()) } else { Err(e) }
    }

    /// Transition rule for `from -> to` (first matching entry, else the default).
    pub fn rule_for(&self, from: &str, to: &str) -> &TransitionRule {
        self.transitions
            .iter()
            .find(|t| (t.from.is_empty() || t.from.iter().any(|f| f == from)) && (t.to.is_empty() || t.to.iter().any(|x| x == to)))
            .map(|t| &t.rule)
            .unwrap_or(&self.default_transition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MusicDef {
        let mut d = MusicDef::default();
        d.name = "t".into();
        d.tracks.insert("a".into(), TrackDef::bank("Music/A.wav"));
        d.tracks.insert("b".into(), TrackDef::file("Music/Mod/b.ogg"));
        d.cues.insert("calm".into(), CueDef::single("a"));
        d.cues.insert("fight".into(), CueDef::single("b"));
        d.states.insert("calm".into(), StateDef::cue("calm"));
        d.states.insert("fight".into(), StateDef::cue("fight"));
        d.initial_state = "calm".into();
        d.zones.insert(3, "fight".into());
        d
    }

    #[test]
    fn ron_and_json_round_trip() {
        let d = sample();
        let r = MusicDef::parse(&d.to_ron(), Format::Ron).unwrap();
        assert_eq!(r, d);
        let j = MusicDef::parse(&d.to_json(), Format::Json).unwrap();
        assert_eq!(j, d);
    }

    #[test]
    fn minimal_hand_written_ron() {
        let text = r#"(
            tracks: { "a": (source: Bank("Music/A.wav"), loop_start_secs: 4.0) },
            cues: { "c": (stems: [(track: "a")]) },
            states: { "s": (cue: "c") },
            initial_state: "s",
            ducking: (voice_depth: 0.3),
        )"#;
        let d = MusicDef::parse(text, Format::Ron).unwrap();
        d.validate().unwrap();
        assert_eq!(d.tracks["a"].loop_start_secs, Some(4.0));
        assert_eq!(d.ducking.as_ref().unwrap().voice_depth, 0.3);
        assert_eq!(d.cues["c"].stems[0].gain, 1.0);
    }

    #[test]
    fn validation_reports_dangling_references() {
        let mut d = sample();
        d.states.insert("x".into(), StateDef::cue("nope"));
        d.zones.insert(9, "ghost".into());
        let e = d.validate().unwrap_err();
        assert_eq!(e.len(), 2, "{e:?}");
    }

    #[test]
    fn merge_overrides_and_prepends_transitions() {
        let mut base = sample();
        base.transitions.push(TransitionDef { rule: TransitionRule::crossfade(1.0), ..Default::default() });
        let mut over = MusicDef::default();
        over.tracks.insert("a".into(), TrackDef::file("Music/Mod/a.ogg"));
        over.transitions.push(TransitionDef { to: vec!["fight".into()], rule: TransitionRule::crossfade(0.25), ..Default::default() });
        over.states.insert("fight".into(), StateDef::cue("fight"));
        base.merge(over);
        assert_eq!(base.tracks["a"].source, SourceRef::File("Music/Mod/a.ogg".into()));
        assert_eq!(base.rule_for("calm", "fight").fade_in.secs, 0.25);
        assert_eq!(base.rule_for("fight", "calm").fade_in.secs, 1.0);
    }

    #[test]
    fn layer_window() {
        let l = LayerDef { fade_in: (0.2, 0.4), fade_out: Some((0.8, 1.0)), curve: Curve::Linear };
        assert_eq!(l.gain_at(0.0), 0.0);
        assert!((l.gain_at(0.3) - 0.5).abs() < 1e-6);
        assert_eq!(l.gain_at(0.6), 1.0);
        assert!((l.gain_at(0.9) - 0.5).abs() < 1e-6);
        assert_eq!(l.gain_at(1.0), 0.0);
    }
}
