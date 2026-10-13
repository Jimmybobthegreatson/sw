//! [`bm_script::ScriptHost`] for the game.
//!
//! `ScriptRuntime` calls the host re-entrantly from inside Lua with `&self` only, while the simulation owns
//! the [`World`] mutably. The host therefore works on a **snapshot** (refreshed by [`GameHost::refresh`] before
//! scripts run, once per tick): reads are answered from the snapshot, writes are queued as [`HostCommand`]s that
//! the game applies at a safe point right after the script call (see `Game::apply_host_commands`).
//! Entities created by scripts get their ids immediately from the world's shared id allocator and appear in the
//! snapshot as soon as they are requested.

use crate::collision::CollisionWorld;
use crate::math::Vec3 as GV;
use crate::world::{EntityId, World};
use bm_script::{EntityInfo, EntityQuery, HostError, HostResult, LogLevel, ParticleParams, RayHit, ScriptHost, SoundParams, SpawnRequest, Vec3};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A write requested by a script.
#[derive(Clone, Debug, PartialEq)]
pub enum HostCommand {
    Spawn { id: EntityId, req: SpawnRequest },
    Despawn(EntityId),
    SetPosition(EntityId, Vec3),
    SetRotation(EntityId, Vec3),
    SetModel(EntityId, String),
    SetComponent(EntityId, String, Value),
    Damage { id: EntityId, amount: f32, source: Option<EntityId> },
    Kill { id: EntityId, source: Option<EntityId> },
    PlaySound { handle: u64, name: String, params: SoundParams },
    StopSound(u64),
    MusicCue(String),
    MusicState(String),
    MusicEvent(String),
    Particles { effect: String, pos: Vec3, params: ParticleParams },
    ShowScreen(String),
    HideScreen(String),
    HudVar(String, Value),
    Message(String, f32),
    Console(String),
    ConsoleCommand(String),
}

#[derive(Default)]
struct Shared {
    level: Option<String>,
    time: f64,
    player: Option<EntityId>,
    entities: BTreeMap<u64, EntityInfo>,
    components: BTreeMap<(u64, String), Value>,
    collision: Option<CollisionWorld>,
    commands: Vec<HostCommand>,
    next_handle: u64,
    logs: Vec<(String, LogLevel, String)>,
    console_output: Vec<String>,
}

pub struct GameHost {
    s: RefCell<Shared>,
    ids: Arc<AtomicU64>,
}

const SNAPSHOT_COMPONENTS: [&str; 6] = ["health", "ai", "inventory", "door", "light", "stealth"];

impl GameHost {
    pub fn new(ids: Arc<AtomicU64>) -> GameHost {
        let h = GameHost { s: RefCell::new(Shared::default()), ids };
        h.s.borrow_mut().next_handle = 1;
        h
    }

    /// Copies the state scripts can read. Cheap enough to run every tick (a few hundred entities).
    pub fn refresh(&self, world: &World) {
        let mut s = self.s.borrow_mut();
        s.level = Some(world.meta.scene.clone());
        s.time = world.time;
        s.player = world.player;
        s.entities.clear();
        s.components.clear();
        for (id, _) in world.infos.iter() {
            if let Some(info) = world.info_json(id) {
                s.entities.insert(id.0, info);
            }
            // only characters / interactables carry readable components
            if world.healths.contains(id) || world.doors.contains(id) || world.lights.contains(id) {
                for c in SNAPSHOT_COMPONENTS {
                    if let Some(v) = world.component_json(id, c) {
                        s.components.insert((id.0, c.to_string()), v);
                    }
                }
            }
        }
        s.collision = Some(world.collision.clone());
    }

    pub fn drain_commands(&self) -> Vec<HostCommand> {
        std::mem::take(&mut self.s.borrow_mut().commands)
    }

    pub fn has_commands(&self) -> bool {
        !self.s.borrow().commands.is_empty()
    }

    /// Lines scripts printed to the console since the last call.
    pub fn take_console(&self) -> Vec<String> {
        std::mem::take(&mut self.s.borrow_mut().console_output)
    }

    /// Script log lines since the last call.
    pub fn take_logs(&self) -> Vec<(String, LogLevel, String)> {
        std::mem::take(&mut self.s.borrow_mut().logs)
    }

    fn push(&self, c: HostCommand) {
        self.s.borrow_mut().commands.push(c);
    }

    fn exists(&self, id: EntityId) -> bool {
        self.s.borrow().entities.contains_key(&id.0)
    }
}

impl ScriptHost for GameHost {
    fn level_name(&self) -> Option<String> {
        self.s.borrow().level.clone()
    }

    fn game_time(&self) -> f64 {
        self.s.borrow().time
    }

    fn player(&self) -> Option<EntityId> {
        self.s.borrow().player
    }

    fn raycast(&self, from: Vec3, to: Vec3) -> HostResult<Option<RayHit>> {
        let s = self.s.borrow();
        let Some(cw) = &s.collision else { return Ok(None) };
        let (a, b): (GV, GV) = (from.into(), to.into());
        let d = b - a;
        let len = d.len();
        if len < 1e-4 {
            return Ok(None);
        }
        let dir = d * (1.0 / len);
        let mut best = cw.raycast(a, dir, len, true).map(|h| (h.t, h.point, h.normal, None::<EntityId>));
        // character capsules
        for (id, info) in s.entities.iter() {
            if !info.alive || !(info.kind == "npc" || info.kind == "player") {
                continue;
            }
            let base: GV = info.position.into();
            if let Some(t) = crate::collision::ray_vs_capsule(a, dir, base, 175.0, 30.0) {
                if t <= len && best.as_ref().is_none_or(|b| t < b.0) {
                    let p = a + dir * t;
                    best = Some((t, p, (p - GV { y: p.y, ..base }).norm_or(-dir), Some(EntityId(*id))));
                }
            }
        }
        Ok(best.map(|(t, p, n, e)| RayHit { position: p.into(), normal: n.into(), distance: t, entity: e }))
    }

    fn entity_info(&self, id: EntityId) -> Option<EntityInfo> {
        self.s.borrow().entities.get(&id.0).cloned()
    }

    fn find_entity(&self, name: &str) -> Option<EntityId> {
        let s = self.s.borrow();
        s.entities.iter().find(|(_, e)| e.name.eq_ignore_ascii_case(name)).map(|(id, _)| EntityId(*id))
    }

    fn query_entities(&self, q: &EntityQuery) -> Vec<EntityInfo> {
        let s = self.s.borrow();
        let needle = q.name_contains.as_ref().map(|n| n.to_ascii_lowercase());
        let mut out: Vec<EntityInfo> = s
            .entities
            .values()
            .filter(|e| q.kind.as_ref().is_none_or(|k| &e.kind == k))
            .filter(|e| !q.alive_only || e.alive)
            .filter(|e| needle.as_ref().is_none_or(|n| e.name.to_ascii_lowercase().contains(n)))
            .filter(|e| match (q.near, q.radius) {
                (Some(c), Some(r)) => e.position.dist(c) <= r,
                (Some(_), None) | (None, _) => true,
            })
            .cloned()
            .collect();
        if let Some(c) = q.near {
            out.sort_by(|a, b| a.position.dist(c).total_cmp(&b.position.dist(c)).then(a.id.cmp(&b.id)));
        }
        if let Some(l) = q.limit {
            out.truncate(l);
        }
        out
    }

    fn spawn_entity(&self, req: &SpawnRequest) -> HostResult<EntityId> {
        let t = req.template.as_str();
        let known = t.starts_with("npc_") || t.starts_with("pickup") || matches!(t, "marker" | "prop" | "light");
        if !known {
            return Err(HostError::new(format!("unknown template '{t}' (npc_guard, npc_civilian, npc_target, pickup:<weapon>, marker, prop, light)")));
        }
        let id = EntityId(self.ids.fetch_add(1, Ordering::Relaxed));
        let kind = if t.starts_with("npc_") {
            "npc"
        } else if t.starts_with("pickup") {
            "pickup"
        } else if t == "light" {
            "light"
        } else if t == "marker" {
            "marker"
        } else {
            "prop"
        };
        let name = req.name.clone().unwrap_or_else(|| format!("{t}#{}", id.0));
        self.s.borrow_mut().entities.insert(
            id.0,
            EntityInfo { id, name, kind: kind.to_string(), position: req.position, rotation: req.rotation, model: req.model.clone(), alive: true },
        );
        self.push(HostCommand::Spawn { id, req: req.clone() });
        Ok(id)
    }

    fn despawn_entity(&self, id: EntityId) -> HostResult<()> {
        if self.s.borrow_mut().entities.remove(&id.0).is_none() {
            return Err(HostError::new("no such entity"));
        }
        self.push(HostCommand::Despawn(id));
        Ok(())
    }

    fn set_entity_position(&self, id: EntityId, pos: Vec3) -> HostResult<()> {
        let mut s = self.s.borrow_mut();
        let e = s.entities.get_mut(&id.0).ok_or_else(|| HostError::new("no such entity"))?;
        e.position = pos;
        s.commands.push(HostCommand::SetPosition(id, pos));
        Ok(())
    }

    fn set_entity_rotation(&self, id: EntityId, euler_deg: Vec3) -> HostResult<()> {
        let mut s = self.s.borrow_mut();
        let e = s.entities.get_mut(&id.0).ok_or_else(|| HostError::new("no such entity"))?;
        e.rotation = euler_deg;
        s.commands.push(HostCommand::SetRotation(id, euler_deg));
        Ok(())
    }

    fn set_entity_model(&self, id: EntityId, model: &str) -> HostResult<()> {
        let mut s = self.s.borrow_mut();
        let e = s.entities.get_mut(&id.0).ok_or_else(|| HostError::new("no such entity"))?;
        e.model = Some(model.to_string());
        s.commands.push(HostCommand::SetModel(id, model.to_string()));
        Ok(())
    }

    fn get_component(&self, id: EntityId, component: &str) -> HostResult<Option<Value>> {
        if !self.exists(id) {
            return Err(HostError::new("no such entity"));
        }
        Ok(self.s.borrow().components.get(&(id.0, component.to_string())).cloned())
    }

    fn set_component(&self, id: EntityId, component: &str, value: Value) -> HostResult<()> {
        if !self.exists(id) {
            return Err(HostError::new("no such entity"));
        }
        let mut s = self.s.borrow_mut();
        s.components.insert((id.0, component.to_string()), value.clone());
        s.commands.push(HostCommand::SetComponent(id, component.to_string(), value));
        Ok(())
    }

    fn damage_entity(&self, id: EntityId, amount: f32, source: Option<EntityId>) -> HostResult<()> {
        if !self.exists(id) {
            return Err(HostError::new("no such entity"));
        }
        self.push(HostCommand::Damage { id, amount, source });
        Ok(())
    }

    fn kill_entity(&self, id: EntityId, source: Option<EntityId>) -> HostResult<()> {
        let mut s = self.s.borrow_mut();
        let e = s.entities.get_mut(&id.0).ok_or_else(|| HostError::new("no such entity"))?;
        e.alive = false;
        s.commands.push(HostCommand::Kill { id, source });
        Ok(())
    }

    fn play_sound(&self, name: &str, params: &SoundParams) -> HostResult<u64> {
        let mut s = self.s.borrow_mut();
        let handle = s.next_handle;
        s.next_handle += 1;
        s.commands.push(HostCommand::PlaySound { handle, name: name.to_string(), params: params.clone() });
        Ok(handle)
    }

    fn stop_sound(&self, handle: u64) -> HostResult<()> {
        self.push(HostCommand::StopSound(handle));
        Ok(())
    }

    fn play_music_cue(&self, cue: &str) -> HostResult<()> {
        self.push(HostCommand::MusicCue(cue.to_string()));
        Ok(())
    }

    fn set_music_state(&self, state: &str) -> HostResult<()> {
        self.push(HostCommand::MusicState(state.to_string()));
        Ok(())
    }

    fn music_event(&self, name: &str) -> HostResult<()> {
        self.push(HostCommand::MusicEvent(name.to_string()));
        Ok(())
    }

    fn spawn_particles(&self, effect: &str, pos: Vec3, params: &ParticleParams) -> HostResult<u64> {
        let mut s = self.s.borrow_mut();
        let handle = s.next_handle;
        s.next_handle += 1;
        s.commands.push(HostCommand::Particles { effect: effect.to_string(), pos, params: params.clone() });
        Ok(handle)
    }

    fn show_screen(&self, name: &str) -> HostResult<()> {
        self.push(HostCommand::ShowScreen(name.to_string()));
        Ok(())
    }

    fn hide_screen(&self, name: &str) -> HostResult<()> {
        self.push(HostCommand::HideScreen(name.to_string()));
        Ok(())
    }

    fn set_hud_var(&self, name: &str, value: Value) -> HostResult<()> {
        self.push(HostCommand::HudVar(name.to_string(), value));
        Ok(())
    }

    fn show_message(&self, text: &str, seconds: f32) -> HostResult<()> {
        self.push(HostCommand::Message(text.to_string(), seconds));
        Ok(())
    }

    fn console_print(&self, text: &str) {
        let mut s = self.s.borrow_mut();
        s.console_output.push(text.to_string());
        s.commands.push(HostCommand::Console(text.to_string()));
    }

    fn run_console_command(&self, line: &str) -> HostResult<String> {
        // built-in commands are answered by the game after the script call; echo for now
        self.push(HostCommand::ConsoleCommand(line.to_string()));
        Ok(format!("queued: {line}"))
    }

    fn log(&self, mod_id: &str, level: LogLevel, message: &str) {
        self.s.borrow_mut().logs.push((mod_id.to_string(), level, message.to_string()));
        match level {
            LogLevel::Debug => log::debug!("[{mod_id}] {message}"),
            LogLevel::Info => log::info!("[{mod_id}] {message}"),
            LogLevel::Warn => log::warn!("[{mod_id}] {message}"),
            LogLevel::Error => log::error!("[{mod_id}] {message}"),
        }
    }
}
