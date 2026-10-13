//! Builds a playable [`MusicDef`] from a level's original music table.
//!
//! The real cue logic of Blood Money lives in native code (`ZMusicController`, per-level
//! `g_MusicControllerMxx__Event__*` handlers, see `docs/music.md`) and is not data. What *is* data and
//! verified is the 64-slot track table and the `MusicLocationDef` zones. This module turns them into
//! states a game can drive: a per-level playlist per mood (explore / suspense / alert / combat / ...),
//! diegetic zone tracks (party music, cello, ballroom ...), stingers, mission-event hooks and
//! transitions. Everything it produces is ordinary data; mods can replace any part by merging their
//! own definition files (`Music/<level>/*.ron`).

use crate::curve::{Curve, FadeDef};
use crate::def::*;
use crate::error::Result;
use crate::level::{LevelMusic, SlotTrack, ZoneKind};
use crate::modding::{self, track_key};
use crate::source::GameResolver;
use crate::system::{MissionEvent, MusicOptions, MusicSystem};
use bm_sound::SoundLibrary;
use bm_vfs::Vfs;
use std::sync::Arc;

/// Inferred purpose of a slot of the 64-entry music table (from the track names, the engine event
/// names in `HitmanBloodMoney.exe` and measured loudness / tail behaviour; confidence M).
pub fn slot_role(slot: u32) -> &'static str {
    match slot {
        1 | 2 | 4 | 7 | 8 | 10 | 28 | 45 | 46 => "ambient / exploration bed",
        3 | 53 => "action outro (Action3_End)",
        54..=58 => "action / combat",
        5 | 47 => "club music (Club Heaven)",
        6 => "licensed: M11 Slasher",
        9 => "apocalypse (M11 event)",
        11 | 12 => "hunter / pursuit (intro + body)",
        13 | 14 => "scary zone (intro + body)",
        16..=18 => "short scary stinger/loop",
        19 => "Tosca (short)",
        20 | 40 | 59 => "47 main theme / menu bounce",
        21 | 22 => "Tosca long (playback + male vocal stems)",
        23 | 60 | 61 => "secret invasion suspense (+ end)",
        24 => "licensed: M10 salsa",
        25..=27 => "party music (M06 salsa / blues / rock, stereo)",
        37..=39 => "party music, radio mix (11 kHz mono, positional)",
        29 | 30 => "suspense (fast / slow)",
        31 | 32 => "sniper loop (slow / fast)",
        33..=35 => "harp (end / loop / mixed)",
        36 => "ballroom (M09)",
        41..=43 => "Tomorrow Never Dies (mix / vocal / music: karaoke)",
        44 => "funeral (M13)",
        48 => "death sting",
        49 | 50 => "choir sting (long / short)",
        51 => "cello (M01, positional)",
        52 => "Ave Maria",
        62 => "credits",
        _ => "unknown",
    }
}

/// Result of [`load_level_def`].
pub struct LevelMusicSet {
    pub level: LevelMusic,
    pub def: MusicDef,
    pub resolver: Arc<GameResolver>,
    /// Definition files merged on top of the generated defaults (mods).
    pub merged_files: Vec<String>,
    /// Problems with mod files that were skipped.
    pub warnings: Vec<String>,
}

impl LevelMusicSet {
    /// Creates the playable system for this level.
    pub fn system(&self, opts: MusicOptions) -> Result<MusicSystem> {
        MusicSystem::new(self.def.clone(), self.resolver.clone(), opts)
    }
}

/// Reads the level's music table + zones, generates the default definition and merges any mod
/// definition files (`Music/_global/*.ron|json`, `Music/<level>/*.ron|json`).
pub fn load_level_def(vfs: Arc<Vfs>, lib: Arc<SoundLibrary>, scene_base: &str) -> Result<LevelMusicSet> {
    let level = LevelMusic::load(&vfs, &lib, scene_base, true)?;
    let base = if level.def_name.is_some() { default_level_def(&level) } else { default_bank_def(&lib, &level.level) };
    let (def, merged, warnings) = merge_mod_definitions(&vfs, &level.level, base);
    let resolver = Arc::new(GameResolver::new(vfs, lib, &level.level, level.slot_files()));
    Ok(LevelMusicSet { level, def, resolver, merged_files: merged, warnings })
}

/// Merges the mod definition files of `level` over `def` in order (`Music/_global/*`, then
/// `Music/<level>/*`; RON before JSON, alphabetical). A file that fails to parse or whose merged
/// result does not validate is skipped and reported. Returns `(definition, merged files, warnings)`.
pub fn merge_mod_definitions(vfs: &Vfs, level: &str, mut def: MusicDef) -> (MusicDef, Vec<String>, Vec<String>) {
    let mut merged = Vec::new();
    let mut warnings = Vec::new();
    for f in modding::definition_files(vfs, level) {
        let Some(fmt) = Format::from_path(&f) else { continue };
        let text = match vfs.open(&f).map(|b| String::from_utf8_lossy(&b).into_owned()) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("{f}: {e}"));
                continue;
            }
        };
        match MusicDef::parse(&text, fmt) {
            Ok(over) => {
                let mut candidate = def.clone();
                candidate.merge(over);
                match candidate.validate() {
                    Ok(()) => {
                        def = candidate;
                        merged.push(f);
                    }
                    Err(e) => warnings.push(format!("{f}: {}", e.join("; "))),
                }
            }
            Err(e) => warnings.push(format!("{f}: {e}")),
        }
    }
    (def, merged, warnings)
}


/// Fallback for scenes without a music table (front end, intro / post-mission screens): every
/// `Music/*` entry of the loaded banks becomes a track, a single-track cue and a state named after the
/// file (`47_main_theme`, `13_schubert_ave_maria`...). The "main theme" loops; the rest play once.
pub fn default_bank_def(lib: &SoundLibrary, level: &str) -> MusicDef {
    let mut d = MusicDef { name: format!("{level} (bank)"), ..Default::default() };
    d.states.insert("silent".into(), StateDef::default());
    let mut first: Option<String> = None;
    let mut names: Vec<String> = lib.iter().filter(|i| i.streamed && i.name.to_ascii_lowercase().starts_with("music/")).map(|i| i.name).collect();
    names.sort();
    names.dedup();
    for n in names {
        let key = track_key(&n);
        let theme = key.contains("main_theme");
        d.tracks.insert(key.clone(), TrackDef { looping: theme, loop_crossfade_secs: if theme { 2.0 } else { 0.0 }, ..TrackDef::bank(&n) });
        d.cues.insert(key.clone(), CueDef::single(&key));
        d.states.insert(key.clone(), StateDef::cue(&key));
        if theme || first.is_none() {
            first = Some(key);
        }
    }
    d.initial_state = first.unwrap_or_else(|| "silent".into());
    d.default_transition = TransitionRule::crossfade(2.0);
    d
}

/// Options for the generated default.
#[derive(Clone, Debug)]
pub struct DefaultsOptions {
    /// Scale track gain by the original `#sound` volume byte relative to 39 (the modal value).
    pub apply_volume_pct: bool,
}

impl Default for DefaultsOptions {
    fn default() -> Self {
        DefaultsOptions { apply_volume_pct: true }
    }
}

/// Slots whose audio is meant to loop (diegetic party / dance music: tail as loud as the head, wrap
/// seam of a normal sample step). Value: loop crossfade seconds (0 = hard, sample accurate wrap).
fn loop_info(slot: u32) -> Option<f32> {
    match slot {
        24 | 25 | 27 | 37 => Some(0.0),
        26 | 36 | 38 | 39 => Some(0.4),
        // "...Loop" named material and the hunter body fade out at the file end: loop with a long crossfade
        11 | 31 | 32 | 34 => Some(2.0),
        // short ambience beds
        28 => Some(2.0),
        51 => Some(1.0),
        _ => None,
    }
}

fn first_present(l: &LevelMusic, slots: &[u32]) -> Vec<u32> {
    slots.iter().copied().filter(|s| l.slot(*s as usize).map(|t| t.file.is_some()).unwrap_or(false)).collect()
}

fn gain_for(t: &SlotTrack, o: &DefaultsOptions) -> f32 {
    if o.apply_volume_pct { (t.volume_pct as f32 / 39.0).clamp(0.1, 2.5) } else { 1.0 }
}

/// Generated definition with default options.
pub fn default_level_def(l: &LevelMusic) -> MusicDef {
    default_level_def_with(l, &DefaultsOptions::default())
}

pub fn default_level_def_with(l: &LevelMusic, o: &DefaultsOptions) -> MusicDef {
    let mut d = MusicDef { name: format!("{} (generated)", l.level), ..Default::default() };
    let key_of = |d: &mut MusicDef, slot: u32| -> Option<String> {
        let t = l.slot(slot as usize)?;
        let file = t.file.as_ref()?;
        let key = track_key(file);
        if !d.tracks.contains_key(&key) {
            let mut td = TrackDef::bank(file);
            td.trim = gain_for(t, o);
            match loop_info(slot) {
                Some(x) => {
                    td.looping = true;
                    td.loop_crossfade_secs = x;
                }
                None => td.looping = false,
            }
            d.tracks.insert(key.clone(), td);
        }
        Some(key)
    };
    let single_cue = |d: &mut MusicDef, slot: u32| -> Option<String> {
        let key = key_of(d, slot)?;
        d.cues.entry(key.clone()).or_insert_with(|| CueDef::single(&key));
        Some(key)
    };

    // ---- states
    d.states.insert("silent".into(), StateDef::default());

    let explore: Vec<String> = first_present(l, &[2, 7, 4, 8, 28, 10]).into_iter().filter_map(|s| single_cue(&mut d, s)).collect();
    if !explore.is_empty() {
        d.states.insert(
            "explore".into(),
            StateDef {
                cue: Some(explore[0].clone()),
                playlist: explore[1..].iter().take(3).cloned().collect(),
                playlist_fade_secs: 4.0,
                playlist_overlap_secs: 6.0,
                follow_zone: true,
                intensity: Some(0.0),
                ..Default::default()
            },
        );
    }

    let suspense: Vec<String> = first_present(l, &[30, 29]).into_iter().filter_map(|s| single_cue(&mut d, s)).collect();
    if !suspense.is_empty() {
        d.states.insert(
            "suspense".into(),
            StateDef {
                cue: Some(suspense[0].clone()),
                playlist: suspense[1..].to_vec(),
                playlist_fade_secs: 3.0,
                playlist_overlap_secs: 4.0,
                zone_locked: true,
                intensity: Some(0.4),
                ..Default::default()
            },
        );
    }

    // scary zone: intro (14) then body (13)
    if let Some(body) = key_of(&mut d, 13) {
        let intro = key_of(&mut d, 14);
        d.cues.insert("scary".into(), CueDef { stems: vec![StemDef { name: "main".into(), track: body, intro, ..Default::default() }], ..Default::default() });
        if let Some(t) = d.tracks.values_mut().find(|t| matches!(&t.source, SourceRef::Bank(n) if l.slot(13).and_then(|s| s.file.as_ref()) == Some(n))) {
            t.looping = true;
            t.loop_crossfade_secs = 3.0;
        }
        d.states.insert("scary".into(), StateDef { cue: Some("scary".into()), zone_locked: true, intensity: Some(0.6), ..Default::default() });
    }

    // alert / hunted: hunter intro (12) + body (11), else secret invasion
    if let Some(body) = key_of(&mut d, 11) {
        let intro = key_of(&mut d, 12);
        d.cues.insert("hunter".into(), CueDef { stems: vec![StemDef { name: "main".into(), track: body, intro, ..Default::default() }], ..Default::default() });
        d.states.insert("alert".into(), StateDef { cue: Some("hunter".into()), zone_locked: true, min_hold_secs: 4.0, intensity: Some(0.8), ..Default::default() });
    } else if let Some(c) = single_cue(&mut d, 23) {
        d.states.insert("alert".into(), StateDef { cue: Some(c), zone_locked: true, min_hold_secs: 4.0, intensity: Some(0.8), ..Default::default() });
    }

    let combat: Vec<String> = first_present(l, &[55, 57, 56, 54]).into_iter().filter_map(|s| single_cue(&mut d, s)).collect();
    if !combat.is_empty() {
        d.states.insert(
            "combat".into(),
            StateDef {
                cue: Some(combat[0].clone()),
                playlist: combat[1..].to_vec(),
                shuffle: true,
                playlist_fade_secs: 1.5,
                playlist_overlap_secs: 3.0,
                zone_locked: true,
                min_hold_secs: 6.0,
                intensity: Some(1.0),
                ..Default::default()
            },
        );
    }

    let sniper: Vec<String> = first_present(l, &[31, 32]).into_iter().filter_map(|s| single_cue(&mut d, s)).collect();
    if !sniper.is_empty() {
        d.states.insert("sniper".into(), StateDef { cue: Some(sniper[0].clone()), playlist: sniper[1..].to_vec(), zone_locked: true, ..Default::default() });
    }

    if let Some(c) = single_cue(&mut d, 20) {
        d.states.insert("complete".into(), StateDef { cue: Some(c), zone_locked: true, ..Default::default() });
    }
    d.states.insert("failed".into(), StateDef { zone_locked: true, ..Default::default() });

    // ---- diegetic / zone states
    for (slot, name) in [(25u32, "party_salsa"), (26, "party_blues"), (27, "party_rock"), (36, "ballroom"), (51, "cello"), (41, "karaoke")] {
        if let Some(c) = single_cue(&mut d, slot) {
            d.states.insert(name.into(), StateDef { cue: Some(c), ..Default::default() });
        }
    }
    // Tosca: playback (21) + male vocal (22) are two synchronised 204.5 s streams
    if let (Some(pb), Some(voc)) = (key_of(&mut d, 21), key_of(&mut d, 22)) {
        d.cues.insert(
            "tosca".into(),
            CueDef {
                stems: vec![
                    StemDef { name: "playback".into(), track: pb, ..Default::default() },
                    StemDef { name: "vocal".into(), track: voc, ..Default::default() },
                ],
                ..Default::default()
            },
        );
        d.states.insert("opera".into(), StateDef { cue: Some("tosca".into()), ..Default::default() });
    }

    // ---- stingers
    for (slot, name, duck) in [(48u32, "death", 0.2f32), (16, "scary_1", 1.0), (17, "scary_2", 1.0), (18, "scary_3", 1.0), (50, "choir_short", 1.0), (49, "choir_long", 1.0), (3, "action_end", 0.6), (60, "invasion", 1.0)] {
        if let Some(k) = key_of(&mut d, slot) {
            d.tracks.get_mut(&k).map(|t| t.looping = false);
            d.stingers.insert(name.into(), StingerDef { track: k, duck_depth: duck, duck_release_secs: 1.5, ..Default::default() });
        }
    }

    // ---- zones
    for z in &l.zones {
        let ln = z.name.to_ascii_lowercase();
        let state = match z.kind {
            ZoneKind::Silent => "silent",
            ZoneKind::Music => {
                if ln.contains("salsa") {
                    "party_salsa"
                } else if ln.contains("blues") {
                    "party_blues"
                } else if ln.contains("partyrock") || ln.contains("party_rock") {
                    "party_rock"
                } else if ln.contains("ballroom") || ln.contains("dancing") {
                    "ballroom"
                } else if ln.contains("cello") {
                    "cello"
                } else {
                    "explore"
                }
            }
        };
        if d.states.contains_key(state) {
            d.zones.insert(z.id, state.to_string());
        } else if d.states.contains_key("explore") {
            d.zones.insert(z.id, "explore".into());
        }
    }

    // ---- transitions (first match wins)
    let fade = |out: f32, inn: f32| TransitionRule { fade_out: FadeDef::new(out, Curve::EqualPower), fade_in: FadeDef::new(inn, Curve::EqualPower), ..Default::default() };
    d.transitions = vec![
        TransitionDef { to: vec!["combat".into()], rule: fade(0.8, 0.8), ..Default::default() },
        TransitionDef { to: vec!["alert".into()], rule: fade(1.5, 1.5), ..Default::default() },
        TransitionDef { to: vec!["failed".into()], rule: TransitionRule { stinger: d.stingers.contains_key("death").then(|| "death".to_string()), ..fade(0.3, 0.1) }, ..Default::default() },
        TransitionDef { to: vec!["silent".into()], rule: TransitionRule { fade_out: FadeDef::new(2.5, Curve::SCurve), fade_in: FadeDef::new(0.1, Curve::Linear), ..Default::default() }, ..Default::default() },
        TransitionDef { from: vec!["combat".into(), "alert".into(), "suspense".into(), "scary".into()], to: vec!["explore".into()], rule: fade(5.0, 4.0), ..Default::default() },
    ];
    d.transitions.retain(|t| t.from.iter().chain(&t.to).all(|s| d.states.contains_key(s)));
    d.default_transition = fade(3.0, 3.0);

    // ---- mission hooks (fallback logic, not original)
    let mut hook = |e: MissionEvent, state: Option<&str>, stinger: Option<&str>, intensity: Option<f32>| {
        let st = state.filter(|s| d.states.contains_key(*s)).map(|s| s.to_string());
        let sg = stinger.filter(|s| d.stingers.contains_key(*s)).map(|s| s.to_string());
        if st.is_some() || sg.is_some() || intensity.is_some() {
            d.events.insert(e.name().into(), EventAction { state: st, stinger: sg, intensity, intensity_delta: None });
        }
    };
    hook(MissionEvent::MissionStart, Some("explore"), None, Some(0.0));
    hook(MissionEvent::Calm, Some("explore"), None, Some(0.0));
    hook(MissionEvent::Suspicious, Some("suspense"), None, Some(0.4));
    hook(MissionEvent::Alert, Some("alert"), None, Some(0.8));
    hook(MissionEvent::Combat, Some("combat"), None, Some(1.0));
    hook(MissionEvent::TargetKilled, None, Some("choir_short"), None);
    hook(MissionEvent::MissionComplete, Some("complete"), None, None);
    hook(MissionEvent::MissionFailed, Some("failed"), None, None);
    hook(MissionEvent::PlayerDead, Some("failed"), None, None);
    if d.states.contains_key("sniper") {
        d.events.insert("sniper".into(), EventAction { state: Some("sniper".into()), ..Default::default() });
    }

    d.initial_state = if d.states.contains_key("explore") { "explore".into() } else { "silent".into() };
    d.intensity = Some(IntensityDef { smooth_secs: 1.5, initial: 0.0 });
    d.ducking = Some(DuckingDef { voice_depth: 0.55, attack_secs: 0.15, release_secs: 1.2 });
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::ZoneInfo;

    fn slot(n: u32, file: &str, pct: u8) -> Option<SlotTrack> {
        Some(SlotTrack { slot: n, sound: file.to_lowercase(), file: Some(file.to_string()), volume_pct: pct, mode: 0, aux: 0, duration: Some(100.0), sample_rate: Some(44100), channels: Some(2), flags: Some(0x80) })
    }

    fn fake_level() -> LevelMusic {
        let mut slots = vec![None; 64];
        slots[2] = slot(2, "Music/Exploration.wav", 8);
        slots[7] = slot(7, "Music/Light_Ambient.wav", 12);
        slots[11] = slot(11, "Music/Hunter_V2.wav", 39);
        slots[12] = slot(12, "Music/Hunter_Intro.wav", 15);
        slots[13] = slot(13, "Music/Scary_Zone3.wav", 39);
        slots[14] = slot(14, "Music/Scary_Zone3_Intro.wav", 39);
        slots[21] = slot(21, "Music/Tosca_Long_Playback.wav", 50);
        slots[22] = slot(22, "Music/Tosca_Long_male.wav", 100);
        slots[25] = slot(25, "Music/Licensed/M06_Salsa_Bomba.wav", 39);
        slots[29] = slot(29, "Music/SuspenseFast.wav", 39);
        slots[30] = slot(30, "Music/SuspenseSlow.wav", 39);
        slots[48] = slot(48, "Music/Death_Sound_01.wav", 39);
        slots[50] = slot(50, "Music/Choir3++.wav", 22);
        slots[55] = slot(55, "Music/Action1.wav", 39);
        slots[57] = slot(57, "Music/Action2.wav", 56);
        slots[20] = slot(20, "Music/47 Main Theme.wav", 39);
        LevelMusic {
            scene_base: "Scenes/M99/M99_main".into(),
            level: "M99".into(),
            def_name: Some("x".into()),
            slots,
            zones: vec![
                ZoneInfo { id: 0, name: "Zone_0__No_Music".into(), rooms: vec![], kind: ZoneKind::Silent },
                ZoneInfo { id: 1, name: "03_PartySalsa".into(), rooms: vec![], kind: ZoneKind::Music },
                ZoneInfo { id: 2, name: "Zone_2__Cellar".into(), rooms: vec![], kind: ZoneKind::Music },
            ],
        }
    }

    #[test]
    fn generated_definition_validates_and_is_playable_data() {
        let d = default_level_def(&fake_level());
        d.validate().unwrap();
        assert_eq!(d.initial_state, "explore");
        assert_eq!(d.states["explore"].cue.as_deref(), Some("exploration"));
        assert_eq!(d.states["explore"].playlist, vec!["light_ambient".to_string()]);
        assert!(d.states["combat"].shuffle);
        assert_eq!(d.zones[&0], "silent");
        assert_eq!(d.zones[&1], "party_salsa");
        assert_eq!(d.zones[&2], "explore");
        // intro + body wiring
        let hunter = &d.cues["hunter"];
        assert_eq!(hunter.stems[0].intro.as_deref(), Some("hunter_intro"));
        assert!(d.tracks["hunter_v2"].looping);
        assert!(!d.tracks["exploration"].looping);
        // stems
        assert_eq!(d.cues["tosca"].stems.len(), 2);
        // volume byte relative to 39
        assert!((d.tracks["exploration"].trim - 8.0 / 39.0).abs() < 1e-6);
        // mission hooks
        assert_eq!(d.events["combat"].state.as_deref(), Some("combat"));
        assert_eq!(d.events["mission_failed"].state.as_deref(), Some("failed"));
        assert_eq!(d.rule_for("explore", "failed").stinger.as_deref(), Some("death"));
        assert_eq!(d.rule_for("combat", "explore").fade_out.secs, 5.0);
        // round trip through RON stays valid
        let back = MusicDef::parse(&d.to_ron(), Format::Ron).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn empty_level_still_yields_a_valid_silent_definition() {
        let l = LevelMusic { scene_base: "Scenes/X/X_main".into(), level: "X".into(), def_name: None, slots: vec![None; 64], zones: vec![] };
        let d = default_level_def(&l);
        d.validate().unwrap();
        assert_eq!(d.initial_state, "silent");
    }

    #[test]
    fn roles_cover_the_table() {
        assert_eq!(slot_role(2), "ambient / exploration bed");
        assert_eq!(slot_role(0), "unknown");
    }

    #[test]
    fn missing_input_error_type_is_usable() {
        let e = crate::error::MusicError::Unknown { kind: "state", name: "x".into() };
        assert!(e.to_string().contains("state"));
    }
}
