//! Save games: player, mission, NPC and door / pickup state serialised with serde (JSON, versioned) into the user
//! directory (`$BM_USER_DIR` or `%APPDATA%/bm-rs`, sub folder `saves`).
//!
//! A save does not contain level data: it is applied on top of a freshly loaded level (`Game::load` of
//! [`SaveGame::level`], then [`Game::load_state`]). Entities are matched by scene node, so entities created by scripts
//! at run time are not part of a save. Transient AI memory (current path, wait timers, last sighting) is not saved: NPCs
//! resume from their saved position and alert state and re-plan.
//!
//! The format is versioned ([`SAVE_VERSION`]); newer files are refused, older ones are read with `#[serde(default)]`
//! filling the fields they lack.

use crate::ai::AlertState;
use crate::anim::AnimState;
use crate::game::Game;
use crate::math::{Vec3, v3};
use crate::mission::{MissionStatus, ObjectiveState};
use crate::player::InvItem;
use crate::world::Kind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Current save format version.
pub const SAVE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("save file: {0}")]
    Io(#[from] std::io::Error),
    #[error("save data: {0}")]
    Format(#[from] serde_json::Error),
    #[error("save version {found} is newer than this build understands ({SAVE_VERSION})")]
    TooNew { found: u32 },
    #[error("the save is for level {saved}, the loaded level is {loaded}")]
    WrongLevel { saved: String, loaded: String },
    #[error("no such slot '{0}'")]
    NoSlot(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemSave {
    pub id: String,
    pub mag: u32,
    pub reserve: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerSave {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub hp: f32,
    pub max_hp: f32,
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub drawn: bool,
    pub crouching: bool,
    pub current: usize,
    pub items: Vec<ItemSave>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NpcSave {
    /// Scene node of the actor (the key a save is matched on).
    pub node: Option<usize>,
    pub name: String,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub hp: f32,
    pub alive: bool,
    /// `idle` / `suspicious` / `searching` / `alerted` / `fleeing` / `dead`.
    pub state: String,
    pub suspicion: f32,
    pub route_idx: usize,
    pub ammo: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DoorSave {
    pub node: Option<usize>,
    pub open: bool,
    pub angle: f32,
    pub target: f32,
    pub locked: bool,
    pub opened_by_npc: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PickupSave {
    pub node: Option<usize>,
    pub taken: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MissionSave {
    /// `none` / `active` / `complete` / `failed`.
    pub status: String,
    /// Completion flags in objective order.
    pub done: Vec<bool>,
    pub picked: Vec<String>,
    pub elapsed: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveGame {
    pub version: u32,
    /// Scene base the save belongs to (`Scenes/M01/M01_main`).
    pub level: String,
    /// Free text shown in menus ("Hacienda - Room 101").
    pub label: String,
    /// Wall clock seconds since the Unix epoch when the file was written (metadata only).
    pub saved_at: u64,
    pub time: f64,
    pub tick: u64,
    pub rng: u64,
    pub room: Option<String>,
    pub player: PlayerSave,
    pub npcs: Vec<NpcSave>,
    pub doors: Vec<DoorSave>,
    pub pickups: Vec<PickupSave>,
    pub mission: MissionSave,
}

fn status_str(s: MissionStatus) -> &'static str {
    match s {
        MissionStatus::None => "none",
        MissionStatus::Active => "active",
        MissionStatus::Complete => "complete",
        MissionStatus::Failed => "failed",
    }
}

fn parse_status(s: &str) -> MissionStatus {
    match s {
        "active" => MissionStatus::Active,
        "complete" => MissionStatus::Complete,
        "failed" => MissionStatus::Failed,
        _ => MissionStatus::None,
    }
}

impl SaveGame {
    pub fn to_json(&self) -> Result<String, SaveError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parses a save and checks its version.
    pub fn from_json(text: &str) -> Result<SaveGame, SaveError> {
        let s: SaveGame = serde_json::from_str(text)?;
        if s.version > SAVE_VERSION {
            return Err(SaveError::TooNew { found: s.version });
        }
        Ok(s)
    }

    pub fn write(&self, path: &Path) -> Result<(), SaveError> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        // write to a temp file first so a crash never leaves a half written save
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_json()?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<SaveGame, SaveError> {
        SaveGame::from_json(&std::fs::read_to_string(path)?)
    }
}

/// Folder of the save slots.
pub fn save_dir() -> PathBuf {
    bm_script::package::user_dir().join("saves")
}

/// File of a named slot (`<save dir>/<slot>.json`; the name is reduced to safe characters).
pub fn slot_path(slot: &str) -> PathBuf {
    let clean: String = slot.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    save_dir().join(format!("{}.json", if clean.is_empty() { "quick" } else { &clean }))
}

/// Slots found in the save folder, newest first, with their labels.
pub fn list_slots() -> Vec<(String, SaveGame)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(save_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Ok(s) = SaveGame::read(&p) {
                v.push((p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string(), s));
            }
        }
    }
    v.sort_by(|a, b| b.1.saved_at.cmp(&a.1.saved_at).then(a.0.cmp(&b.0)));
    v
}

/// What [`Game::load_state`] restored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadReport {
    pub npcs: usize,
    pub npcs_missing: usize,
    pub doors: usize,
    pub pickups: usize,
}

impl Game {
    /// Snapshot of the current state.
    pub fn save_state(&self, label: &str) -> SaveGame {
        let w = &self.world;
        let mut s = SaveGame {
            version: SAVE_VERSION,
            level: w.meta.scene.clone(),
            label: label.to_string(),
            saved_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            time: w.time,
            tick: w.tick,
            rng: w.rng.0,
            room: w.player_room_name().map(str::to_string),
            ..Default::default()
        };
        if let Some(pid) = w.player {
            let pos = w.pos(pid).unwrap_or(Vec3::ZERO);
            let yaw = w.transforms.get(pid).map(|t| t.yaw).unwrap_or(0.0);
            let (hp, max_hp) = w.healths.get(pid).map(|h| (h.hp, h.max)).unwrap_or((0.0, 0.0));
            let pl = w.players.get(pid);
            s.player = PlayerSave {
                pos: pos.to_array(),
                yaw,
                hp,
                max_hp,
                cam_yaw: pl.map(|p| p.cam_yaw).unwrap_or(0.0),
                cam_pitch: pl.map(|p| p.cam_pitch).unwrap_or(0.0),
                drawn: pl.is_some_and(|p| p.drawn),
                crouching: pl.is_some_and(|p| p.crouching),
                current: pl.map(|p| p.current).unwrap_or(0),
                items: pl.map(|p| p.items.iter().map(|i| ItemSave { id: i.id.clone(), mag: i.mag, reserve: i.reserve }).collect()).unwrap_or_default(),
            };
        }
        for (id, ai) in w.ais.iter() {
            let Some(info) = w.infos.get(id) else { continue };
            if info.node.is_none() {
                continue;
            }
            s.npcs.push(NpcSave {
                node: info.node,
                name: info.name.clone(),
                pos: w.pos(id).unwrap_or(Vec3::ZERO).to_array(),
                yaw: w.transforms.get(id).map(|t| t.yaw).unwrap_or(0.0),
                hp: w.healths.get(id).map(|h| h.hp).unwrap_or(0.0),
                alive: info.alive,
                state: ai.state.as_str().to_string(),
                suspicion: ai.suspicion,
                route_idx: ai.route_idx,
                ammo: w.characters.get(id).map(|c| c.ammo).unwrap_or(0),
            });
        }
        for (id, d) in w.doors.iter() {
            s.doors.push(DoorSave { node: w.infos.get(id).and_then(|i| i.node), open: d.open, angle: d.angle, target: d.target, locked: d.locked, opened_by_npc: d.opened_by_npc });
        }
        for (id, p) in w.pickups.iter() {
            if let Some(node) = w.infos.get(id).and_then(|i| i.node) {
                s.pickups.push(PickupSave { node: Some(node), taken: p.taken });
            }
        }
        s.mission = MissionSave {
            status: status_str(w.mission.status).to_string(),
            done: w.mission.objectives.iter().map(|o| o.done).collect(),
            picked: w.mission.picked.clone(),
            elapsed: w.mission.elapsed,
        };
        s
    }

    /// Applies a save to the loaded level. The level must be the one the save was made in.
    pub fn load_state(&mut self, s: &SaveGame) -> Result<LoadReport, SaveError> {
        if s.version > SAVE_VERSION {
            return Err(SaveError::TooNew { found: s.version });
        }
        if s.level != self.world.meta.scene {
            return Err(SaveError::WrongLevel { saved: s.level.clone(), loaded: self.world.meta.scene.clone() });
        }
        let mut rep = LoadReport::default();
        let w = &mut self.world;
        w.time = s.time;
        w.tick = s.tick;
        w.rng.0 = s.rng;

        // player
        if let Some(pid) = w.player {
            let p = Vec3::from_array(s.player.pos);
            if let Some(b) = w.bodies.get_mut(pid) {
                b.pos = p;
                b.last_safe = p;
                b.vel_y = 0.0;
            }
            w.set_pose(pid, p, s.player.yaw);
            if let Some(h) = w.healths.get_mut(pid) {
                h.max = if s.player.max_hp > 0.0 { s.player.max_hp } else { h.max };
                h.hp = s.player.hp.clamp(0.0, h.max);
            }
            if let Some(info) = w.infos.get_mut(pid) {
                info.alive = s.player.hp > 0.0;
            }
            if let Some(pl) = w.players.get_mut(pid) {
                pl.cam_yaw = s.player.cam_yaw;
                pl.cam_pitch = s.player.cam_pitch;
                pl.drawn = s.player.drawn;
                pl.crouching = s.player.crouching;
                pl.items = s.player.items.iter().filter(|i| w.data.weapons.contains_key(&i.id)).map(|i| InvItem { id: i.id.clone(), mag: i.mag, reserve: i.reserve }).collect();
                pl.current = s.player.current.min(pl.items.len().saturating_sub(1));
            }
        }

        // NPCs by node
        let by_node: std::collections::HashMap<usize, crate::world::EntityId> =
            w.infos.iter().filter(|(_, i)| i.kind == Kind::Npc).filter_map(|(id, i)| i.node.map(|n| (n, id))).collect();
        for n in &s.npcs {
            let Some(&id) = n.node.and_then(|node| by_node.get(&node)) else {
                rep.npcs_missing += 1;
                continue;
            };
            let p = Vec3::from_array(n.pos);
            if let Some(b) = w.bodies.get_mut(id) {
                b.pos = p;
                b.last_safe = p;
                b.vel_y = 0.0;
            }
            w.set_pose(id, p, n.yaw);
            if let Some(h) = w.healths.get_mut(id) {
                h.hp = n.hp.clamp(0.0, h.max.max(n.hp));
            }
            if let Some(c) = w.characters.get_mut(id) {
                c.ammo = n.ammo;
            }
            if let Some(i) = w.infos.get_mut(id) {
                i.alive = n.alive;
            }
            if let Some(ai) = w.ais.get_mut(id) {
                ai.suspicion = n.suspicion;
                ai.route_idx = if ai.route.is_empty() { 0 } else { n.route_idx % ai.route.len() };
                ai.path.clear();
                ai.goal = None;
                ai.arrived = false;
                ai.state = match n.state.as_str() {
                    "suspicious" => AlertState::Suspicious,
                    "searching" => AlertState::Searching,
                    "alerted" => AlertState::Alerted,
                    "fleeing" => AlertState::Fleeing,
                    "dead" => AlertState::Dead,
                    _ => AlertState::Idle,
                };
                if !n.alive {
                    ai.die();
                }
            }
            if let Some(a) = w.anims.get_mut(id) {
                a.state = if n.alive { AnimState::Idle } else { AnimState::Dead };
                a.dead_blend = if n.alive { 0.0 } else { 1.0 };
            }
            rep.npcs += 1;
        }

        // doors
        let door_by_node: std::collections::HashMap<usize, crate::world::EntityId> = w.doors.iter().filter_map(|(id, _)| w.infos.get(id).and_then(|i| i.node).map(|n| (n, id))).collect();
        for d in &s.doors {
            let Some(&id) = d.node.and_then(|n| door_by_node.get(&n)) else { continue };
            let (m, collider) = match w.doors.get_mut(id) {
                Some(c) => {
                    c.open = d.open;
                    c.angle = d.angle;
                    c.target = d.target;
                    c.locked = d.locked;
                    c.opened_by_npc = d.opened_by_npc;
                    c.idle = 0.0;
                    (c.current_matrix(), c.collider)
                }
                None => continue,
            };
            if let Some(c) = collider {
                w.collision.set_dynamic_transform(c, &m);
            }
            w.set_world(id, m);
            rep.doors += 1;
        }

        // pickups
        let pick_by_node: std::collections::HashMap<usize, crate::world::EntityId> = w.pickups.iter().filter_map(|(id, _)| w.infos.get(id).and_then(|i| i.node).map(|n| (n, id))).collect();
        for p in &s.pickups {
            let Some(&id) = p.node.and_then(|n| pick_by_node.get(&n)) else { continue };
            if let Some(c) = w.pickups.get_mut(id) {
                c.taken = p.taken;
            }
            if let Some(i) = w.infos.get_mut(id) {
                i.alive = !p.taken;
            }
            if let Some(r) = w.renders.get_mut(id) {
                r.visible = !p.taken;
            }
            rep.pickups += 1;
        }

        // mission
        w.mission.status = parse_status(&s.mission.status);
        for (o, done) in w.mission.objectives.iter_mut().zip(&s.mission.done) {
            let _: &mut ObjectiveState = o;
            o.done = *done;
        }
        w.mission.picked = s.mission.picked.clone();
        w.mission.elapsed = s.mission.elapsed;

        // queues belong to the old state; the area tracker re-announces the player's room
        w.requests.clear();
        w.outbox.clear();
        w.fx.clear();
        w.noises.clear();
        w.areas.player_room = None;
        w.areas.player_zone = None;
        w.areas.inside = vec![false; w.areas.triggers.len()];
        self.ui.messages.clear();
        let _ = v3(0.0, 0.0, 0.0);
        let level = s.level.clone();
        let ev = bm_script::Event { name: "on_game_loaded".into(), payload: serde_json::json!({ "level": level, "label": s.label }) };
        self.log_event(ev);
        Ok(rep)
    }

    /// Writes the current state into slot `slot` of the save folder. Returns the file path.
    pub fn save_slot(&self, slot: &str, label: &str) -> Result<PathBuf, SaveError> {
        let p = slot_path(slot);
        self.save_state(label).write(&p)?;
        Ok(p)
    }

    /// Loads slot `slot` into the running level (it must belong to this level).
    pub fn load_slot(&mut self, slot: &str) -> Result<LoadReport, SaveError> {
        let p = slot_path(slot);
        if !p.exists() {
            return Err(SaveError::NoSlot(slot.to_string()));
        }
        let s = SaveGame::read(&p)?;
        self.load_state(&s)
    }

    /// Default label of a save: level title and room.
    pub fn save_label(&self) -> String {
        let title = self.world.mission.title();
        let room = self.world.player_room_name().unwrap_or("");
        match (title.is_empty(), room.is_empty()) {
            (false, false) => format!("{title} - {room}"),
            (false, true) => title.to_string(),
            (true, false) => room.to_string(),
            _ => self.world.meta.scene.clone(),
        }
    }
}
