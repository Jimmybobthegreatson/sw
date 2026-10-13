//! Audio glue: turns [`FxEvent`]s into `bm-sound` voices (footsteps by surface, weapon shots, impacts, pain
//! grunts), positions the listener at the camera, runs `ZSNDOBJ` ambient emitters near the listener and drives
//! the dynamic music (`bm-music`) from the alert state of the NPCs around the player.

use crate::ai::AlertState;
use crate::collision::Surface;
use crate::math::{Rng, Vec3};
use crate::player::Camera;
use crate::world::{FxEvent, World};
use bm_models::Scene;
use bm_music::{MissionEvent, MusicOptions, MusicSystem};
use bm_sound::{AudioEngine, Listener, PlayOptions, ReverbParams, SoundLibrary, SoundPlayer, SoundScene, VoiceHandle};
use bm_vfs::Vfs;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel {
    #[default]
    Calm,
    Suspicious,
    Alert,
    Combat,
}

#[derive(Clone, Debug, Default)]
pub struct AudioStats {
    pub played: u64,
    pub failed: u64,
    pub active_emitters: usize,
}

struct EmitterInfo {
    node: usize,
    path: String,
    pos: Vec3,
}

pub struct AudioDirector {
    pub player: SoundPlayer,
    pub music: Option<MusicSystem>,
    pub stats: AudioStats,
    lib: Arc<SoundLibrary>,
    emitters: Vec<EmitterInfo>,
    active: HashMap<usize, VoiceHandle>,
    failed_emitters: HashSet<usize>,
    scripted: HashMap<u64, VoiceHandle>,
    footsteps: HashMap<&'static str, Vec<String>>,
    impacts: HashMap<&'static str, Vec<String>>,
    pain: Vec<String>,
    rng: Rng,
    level: AlertLevel,
    started_music: bool,
    /// `MusicLocationDef` zones: `(zone id, lower-case room names)`.
    zones: Vec<(i32, Vec<String>)>,
    /// Room the reverb / music zone currently follow (`None` outside every room).
    pub room: Option<String>,
    last_room: Option<Option<usize>>,
    /// Name of the room effect applied to the reverb (diagnostics).
    pub reverb_room: Option<String>,
    /// Music zone last requested from the music system.
    pub music_zone: Option<i32>,
}

impl AudioDirector {
    /// Opens the level's sound bank, `.SND` emitters and (optionally) the music system on `engine`.
    pub fn open(vfs: Arc<Vfs>, scene: &Scene, base: &str, mut engine: Box<dyn AudioEngine>, with_music: bool) -> Result<AudioDirector, String> {
        let lib = Arc::new(SoundLibrary::open_level(vfs.clone(), base).map_err(|e| e.to_string())?);
        let emitters = match lib.snd(0) {
            Some(db) => SoundScene::from_scene(scene, db)
                .emitters()
                .filter_map(|n| n.sound.as_ref().map(|s| EmitterInfo { node: n.node, path: s.path.clone(), pos: Vec3::from_array(n.position) }))
                .collect(),
            None => Vec::new(),
        };
        let mut music = None;
        let mut zones: Vec<(i32, Vec<String>)> = Vec::new();
        if with_music {
            match bm_music::load_level_def(vfs, lib.clone(), base).and_then(|set| {
                zones = set.level.zones.iter().map(|z| (z.id, z.rooms.iter().map(|r| r.to_ascii_lowercase()).collect())).collect();
                set.system(MusicOptions::for_engine(&*engine))
            }) {
                Ok(mut m) => {
                    if let Err(e) = m.attach(&mut *engine) {
                        log::warn!("music attach failed: {e}");
                    } else {
                        music = Some(m);
                    }
                }
                Err(e) => log::warn!("music unavailable: {e}"),
            }
        }
        let pick = |dir: &str| -> Vec<String> { lib.search(dir).into_iter().map(|i| i.name).collect() };
        let mut footsteps = HashMap::new();
        for s in [Surface::Stone, Surface::Carpet, Surface::Gravel] {
            let d = s.footstep_dir();
            footsteps.entry(d).or_insert_with(|| pick(&format!("Character/Footsteps/{d}/")));
        }
        let mut impacts = HashMap::new();
        for d in ["Stone", "Metal", "Snow", "Glass", "Wood"] {
            impacts.insert(d, pick(&format!("Impacts_Bullet/{d}/")));
        }
        let pain = pick("Character/Expressions/Pain/");
        let player = SoundPlayer::new(lib.clone(), engine);
        Ok(AudioDirector {
            player,
            music,
            stats: AudioStats::default(),
            lib,
            emitters,
            active: HashMap::new(),
            failed_emitters: HashSet::new(),
            scripted: HashMap::new(),
            footsteps,
            impacts,
            pain,
            rng: Rng::new(0xA0D10),
            level: AlertLevel::Calm,
            started_music: false,
            zones,
            room: None,
            last_room: None,
            reverb_room: None,
            music_zone: None,
        })
    }

    pub fn library(&self) -> &Arc<SoundLibrary> {
        &self.lib
    }

    pub fn alert_level(&self) -> AlertLevel {
        self.level
    }

    fn pick_from(&mut self, names: &[String]) -> Option<String> {
        if names.is_empty() {
            None
        } else {
            Some(names[self.rng.below(names.len())].clone())
        }
    }

    fn play(&mut self, name: &str, pos: Option<Vec3>, volume: f32, pitch: f32, looping: bool, range: (f32, f32)) -> Option<VoiceHandle> {
        let o = PlayOptions {
            position: pos.map(|p| p.to_array()),
            volume: Some(volume),
            pitch: Some(pitch),
            looping,
            min_max_m: Some(range),
            ..Default::default()
        };
        match self.player.play_name(name, &o) {
            Ok(h) => {
                self.stats.played += 1;
                h
            }
            Err(e) => {
                self.stats.failed += 1;
                log::debug!("sound '{name}': {e}");
                None
            }
        }
    }

    /// Alert level the NPCs near the player imply.
    pub fn compute_alert(world: &World) -> AlertLevel {
        let Some(pp) = world.player_pos() else { return AlertLevel::Calm };
        let mut lvl = AlertLevel::Calm;
        for (id, ai) in world.ais.iter() {
            if ai.dead {
                continue;
            }
            let near = world.pos(id).is_some_and(|p| p.dist(pp) < 4500.0);
            let l = match ai.state {
                AlertState::Alerted if ai.sees_player => AlertLevel::Combat,
                AlertState::Alerted | AlertState::Fleeing => {
                    if near { AlertLevel::Alert } else { AlertLevel::Suspicious }
                }
                AlertState::Suspicious | AlertState::Searching => {
                    if near { AlertLevel::Suspicious } else { AlertLevel::Calm }
                }
                _ => AlertLevel::Calm,
            };
            lvl = lvl.max(l);
        }
        lvl
    }

    pub fn mission_event(&mut self, e: MissionEvent) {
        if let Some(m) = &self.music {
            m.mission_event(e);
        }
    }

    /// Per frame: listener, music state, emitters and the frame's presentation events.
    pub fn update(&mut self, world: &World, cam: Option<&Camera>, events: &[FxEvent]) {
        if let Some(c) = cam {
            self.player.engine.set_listener(Listener { position: c.pos.to_array(), forward: c.forward.to_array(), up: c.up.to_array() });
        }
        // music
        if !self.started_music {
            self.started_music = true;
            self.mission_event(MissionEvent::MissionStart);
        }
        let lvl = Self::compute_alert(world);
        if lvl != self.level {
            self.level = lvl;
            self.mission_event(match lvl {
                AlertLevel::Calm => MissionEvent::Calm,
                AlertLevel::Suspicious => MissionEvent::Suspicious,
                AlertLevel::Alert => MissionEvent::Alert,
                AlertLevel::Combat => MissionEvent::Combat,
            });
        }
        self.follow_room(world);
        // ambient emitters
        if let Some(c) = cam {
            if world.tick % 15 == 0 {
                self.update_emitters(c.pos);
            }
        }
        for ev in events {
            self.handle(world, ev);
        }
    }

    /// Music zone id (`MusicLocationDef`) a room belongs to.
    pub fn zone_for_room(&self, room: &str) -> Option<i32> {
        let n = room.to_ascii_lowercase();
        self.zones.iter().find(|(_, rooms)| rooms.iter().any(|r| *r == n)).map(|(id, _)| *id)
    }

    /// Reverb preset of the listener's room (`SndDb::room_effects` are named after the rooms) and the music zone
    /// (`MusicLocationDef` room lists) follow the room the player stands in.
    fn follow_room(&mut self, world: &World) {
        let cur = world.areas.player_room;
        if self.last_room == Some(cur) {
            return;
        }
        self.last_room = Some(cur);
        let name = world.player_room_name().map(str::to_string);
        let lname = name.as_ref().map(|n| n.to_ascii_lowercase());
        // reverb
        let effect = lname.as_ref().and_then(|n| {
            self.lib.snd(0).and_then(|db| db.room_effects.iter().find(|e| e.name.rsplit('/').next().is_some_and(|last| last.eq_ignore_ascii_case(n))).map(ReverbParams::from_eax))
        });
        self.reverb_room = effect.as_ref().and(name.clone());
        self.player.engine.set_reverb(effect.unwrap_or_else(ReverbParams::off));
        // music zone
        if let (Some(m), Some(n)) = (&self.music, &lname) {
            if let Some((id, _)) = self.zones.iter().find(|(_, rooms)| rooms.iter().any(|r| r == n)) {
                if self.music_zone != Some(*id) {
                    self.music_zone = Some(*id);
                    m.set_zone(*id);
                }
            }
        } else if let Some(n) = &lname {
            if let Some((id, _)) = self.zones.iter().find(|(_, rooms)| rooms.iter().any(|r| r == n)) {
                self.music_zone = Some(*id);
            }
        }
        self.room = name;
    }

    fn update_emitters(&mut self, listener: Vec3) {
        // stop far voices
        let far: Vec<usize> = self
            .active
            .keys()
            .copied()
            .filter(|n| self.emitters.iter().find(|e| e.node == *n).is_none_or(|e| e.pos.dist(listener) > 3500.0))
            .collect();
        for n in far {
            if let Some(h) = self.active.remove(&n) {
                self.player.engine.stop(h, 0.2);
            }
        }
        self.active.retain(|_, h| self.player.engine.is_playing(*h));
        if self.active.len() >= 6 {
            self.stats.active_emitters = self.active.len();
            return;
        }
        let mut near: Vec<(f32, usize)> = self
            .emitters
            .iter()
            .enumerate()
            .filter(|(_, e)| !self.active.contains_key(&e.node) && !self.failed_emitters.contains(&e.node))
            .map(|(i, e)| (e.pos.dist(listener), i))
            .filter(|(d, _)| *d < 2500.0)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, i) in near.into_iter().take(6 - self.active.len()) {
            let (node, path, pos) = {
                let e = &self.emitters[i];
                (e.node, e.path.clone(), e.pos)
            };
            let o = PlayOptions { position: Some(pos.to_array()), looping: true, min_max_m: Some((4.0, 30.0)), ..Default::default() };
            match self.player.play_object(0, &path, &o) {
                Ok(Some(h)) => {
                    self.active.insert(node, h);
                    self.stats.played += 1;
                }
                _ => {
                    self.failed_emitters.insert(node);
                    self.stats.failed += 1;
                }
            }
        }
        self.stats.active_emitters = self.active.len();
    }

    fn handle(&mut self, world: &World, ev: &FxEvent) {
        match ev {
            FxEvent::Footstep { pos, surface, run, who } => {
                let dir = surface.footstep_dir();
                let names = self.footsteps.get(dir).cloned().unwrap_or_default();
                if let Some(n) = self.pick_from(&names) {
                    let vol = if Some(*who) == world.player { 0.9 } else { 0.55 } * if *run { 1.0 } else { 0.7 };
                    let pitch = 0.95 + self.rng.f32() * 0.1;
                    self.play(&n, Some(*pos), vol, pitch, false, (2.0, 25.0));
                }
            }
            FxEvent::Shot { pos, weapon, .. } => {
                if let Some(def) = world.data.weapon(weapon) {
                    if !def.fire_sound.is_empty() {
                        let s = def.fire_sound.clone();
                        self.play(&s, Some(*pos), 1.0, 1.0, false, (10.0, 120.0));
                    }
                }
            }
            FxEvent::Reload { pos, weapon } => {
                if let Some(def) = world.data.weapon(weapon) {
                    if !def.reload_sound.is_empty() {
                        let s = def.reload_sound.clone();
                        self.play(&s, Some(*pos), 0.8, 1.0, false, (5.0, 40.0));
                    }
                }
            }
            FxEvent::Impact { pos, surface, .. } => {
                let names = self.impacts.get(surface.impact_dir()).cloned().unwrap_or_default();
                if let Some(n) = self.pick_from(&names) {
                    self.play(&n, Some(*pos), 0.9, 1.0, false, (4.0, 50.0));
                }
            }
            FxEvent::Death { pos, .. } => {
                let names = self.pain.clone();
                if let Some(n) = self.pick_from(&names) {
                    self.play(&n, Some(*pos), 1.0, 1.0, false, (5.0, 40.0));
                }
            }
            FxEvent::Sound { name, pos, entity, volume, pitch, looping, handle } => {
                let p = pos.or_else(|| entity.and_then(|e| world.pos(e)));
                if let Some(h) = self.play(name, p, *volume, *pitch, *looping, (5.0, 60.0)) {
                    self.scripted.insert(*handle, h);
                }
            }
            FxEvent::StopSound { handle } => {
                if let Some(h) = self.scripted.remove(handle) {
                    self.player.engine.stop(h, 0.05);
                }
            }
            _ => {}
        }
    }
}
