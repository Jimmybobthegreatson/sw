//! In-memory [`ScriptHost`] used by tests and tools. It stores entities, records every mutating call
//! and log line, and implements the whole trait.

use crate::host::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum HostCall {
    Spawn(SpawnRequest),
    Despawn(EntityId),
    SetPosition(EntityId, Vec3),
    SetRotation(EntityId, Vec3),
    SetModel(EntityId, String),
    SetComponent(EntityId, String, Value),
    Damage(EntityId, f32, Option<EntityId>),
    Kill(EntityId, Option<EntityId>),
    PlaySound(String, SoundParams),
    StopSound(u64),
    PlayMusic(String),
    MusicState(String),
    MusicEvent(String),
    Particles(String, Vec3, ParticleParams),
    ShowScreen(String),
    HideScreen(String),
    HudVar(String, Value),
    Message(String, f32),
    ConsolePrint(String),
    ConsoleCommand(String),
}

#[derive(Default)]
struct State {
    level: Option<String>,
    time: f64,
    player: Option<EntityId>,
    next_id: u64,
    next_handle: u64,
    entities: BTreeMap<u64, EntityInfo>,
    components: BTreeMap<(u64, String), Value>,
    calls: Vec<HostCall>,
    logs: Vec<(String, LogLevel, String)>,
}

#[derive(Default)]
pub struct MockHost {
    s: RefCell<State>,
}

impl MockHost {
    pub fn new() -> MockHost {
        let m = MockHost::default();
        m.s.borrow_mut().next_id = 1;
        m.s.borrow_mut().next_handle = 1;
        m
    }
    pub fn set_level(&self, level: &str) {
        self.s.borrow_mut().level = Some(level.to_string());
    }
    pub fn set_time(&self, t: f64) {
        self.s.borrow_mut().time = t;
    }
    pub fn set_player(&self, id: EntityId) {
        self.s.borrow_mut().player = Some(id);
    }
    pub fn add_entity(&self, name: &str, kind: &str, pos: Vec3) -> EntityId {
        let mut s = self.s.borrow_mut();
        let id = EntityId(s.next_id);
        s.next_id += 1;
        s.entities.insert(
            id.0,
            EntityInfo {
                id,
                name: name.to_string(),
                kind: kind.to_string(),
                position: pos,
                rotation: Vec3::ZERO,
                model: None,
                alive: true,
            },
        );
        id
    }
    pub fn entity(&self, id: EntityId) -> Option<EntityInfo> {
        self.s.borrow().entities.get(&id.0).cloned()
    }
    pub fn component(&self, id: EntityId, name: &str) -> Option<Value> {
        self.s.borrow().components.get(&(id.0, name.to_string())).cloned()
    }
    pub fn calls(&self) -> Vec<HostCall> {
        self.s.borrow().calls.clone()
    }
    pub fn clear_calls(&self) {
        self.s.borrow_mut().calls.clear();
    }
    pub fn logs(&self) -> Vec<(String, LogLevel, String)> {
        self.s.borrow().logs.clone()
    }
    pub fn log_text(&self) -> String {
        self.s.borrow().logs.iter().map(|l| l.2.clone()).collect::<Vec<_>>().join("\n")
    }
    fn rec(&self, c: HostCall) {
        self.s.borrow_mut().calls.push(c);
    }
    fn with_entity<R>(&self, id: EntityId, f: impl FnOnce(&mut EntityInfo) -> R) -> HostResult<R> {
        let mut s = self.s.borrow_mut();
        s.entities.get_mut(&id.0).map(f).ok_or_else(|| HostError::new(format!("no such entity {}", id.0)))
    }
}

impl ScriptHost for MockHost {
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
        // Hits the first entity within 50 units of the segment midpoint-to-end line (very rough).
        let s = self.s.borrow();
        for e in s.entities.values() {
            if e.position.dist(to) < 50.0 {
                return Ok(Some(RayHit {
                    position: e.position,
                    normal: Vec3::new(0.0, 1.0, 0.0),
                    distance: from.dist(e.position),
                    entity: Some(e.id),
                }));
            }
        }
        Ok(None)
    }
    fn entity_info(&self, id: EntityId) -> Option<EntityInfo> {
        self.entity(id)
    }
    fn find_entity(&self, name: &str) -> Option<EntityId> {
        self.s.borrow().entities.values().find(|e| e.name.eq_ignore_ascii_case(name)).map(|e| e.id)
    }
    fn query_entities(&self, q: &EntityQuery) -> Vec<EntityInfo> {
        let s = self.s.borrow();
        let mut v: Vec<EntityInfo> = s
            .entities
            .values()
            .filter(|e| q.kind.as_ref().is_none_or(|k| &e.kind == k))
            .filter(|e| q.name_contains.as_ref().is_none_or(|n| e.name.to_lowercase().contains(&n.to_lowercase())))
            .filter(|e| !q.alive_only || e.alive)
            .filter(|e| match (q.near, q.radius) {
                (Some(p), Some(r)) => e.position.dist(p) <= r,
                _ => true,
            })
            .cloned()
            .collect();
        if let Some(n) = q.limit {
            v.truncate(n);
        }
        v
    }
    fn spawn_entity(&self, req: &SpawnRequest) -> HostResult<EntityId> {
        self.rec(HostCall::Spawn(req.clone()));
        let id = self.add_entity(req.name.as_deref().unwrap_or(&req.template), &req.template, req.position);
        let mut s = self.s.borrow_mut();
        if let Some(e) = s.entities.get_mut(&id.0) {
            e.rotation = req.rotation;
            e.model = req.model.clone();
        }
        for (k, v) in &req.components {
            s.components.insert((id.0, k.clone()), v.clone());
        }
        Ok(id)
    }
    fn despawn_entity(&self, id: EntityId) -> HostResult<()> {
        self.rec(HostCall::Despawn(id));
        self.s.borrow_mut().entities.remove(&id.0).map(|_| ()).ok_or_else(|| HostError::new("no such entity"))
    }
    fn set_entity_position(&self, id: EntityId, pos: Vec3) -> HostResult<()> {
        self.rec(HostCall::SetPosition(id, pos));
        self.with_entity(id, |e| e.position = pos)
    }
    fn set_entity_rotation(&self, id: EntityId, r: Vec3) -> HostResult<()> {
        self.rec(HostCall::SetRotation(id, r));
        self.with_entity(id, |e| e.rotation = r)
    }
    fn set_entity_model(&self, id: EntityId, model: &str) -> HostResult<()> {
        self.rec(HostCall::SetModel(id, model.to_string()));
        self.with_entity(id, |e| e.model = Some(model.to_string()))
    }
    fn get_component(&self, id: EntityId, c: &str) -> HostResult<Option<Value>> {
        Ok(self.component(id, c))
    }
    fn set_component(&self, id: EntityId, c: &str, v: Value) -> HostResult<()> {
        self.rec(HostCall::SetComponent(id, c.to_string(), v.clone()));
        self.with_entity(id, |_| ())?;
        self.s.borrow_mut().components.insert((id.0, c.to_string()), v);
        Ok(())
    }
    fn damage_entity(&self, id: EntityId, amount: f32, source: Option<EntityId>) -> HostResult<()> {
        self.rec(HostCall::Damage(id, amount, source));
        self.with_entity(id, |_| ())
    }
    fn kill_entity(&self, id: EntityId, source: Option<EntityId>) -> HostResult<()> {
        self.rec(HostCall::Kill(id, source));
        self.with_entity(id, |e| e.alive = false)
    }
    fn play_sound(&self, name: &str, params: &SoundParams) -> HostResult<u64> {
        self.rec(HostCall::PlaySound(name.to_string(), params.clone()));
        let mut s = self.s.borrow_mut();
        let h = s.next_handle;
        s.next_handle += 1;
        Ok(h)
    }
    fn stop_sound(&self, handle: u64) -> HostResult<()> {
        self.rec(HostCall::StopSound(handle));
        Ok(())
    }
    fn play_music_cue(&self, cue: &str) -> HostResult<()> {
        self.rec(HostCall::PlayMusic(cue.to_string()));
        Ok(())
    }
    fn set_music_state(&self, state: &str) -> HostResult<()> {
        self.rec(HostCall::MusicState(state.to_string()));
        Ok(())
    }
    fn music_event(&self, name: &str) -> HostResult<()> {
        self.rec(HostCall::MusicEvent(name.to_string()));
        Ok(())
    }
    fn spawn_particles(&self, effect: &str, pos: Vec3, params: &ParticleParams) -> HostResult<u64> {
        self.rec(HostCall::Particles(effect.to_string(), pos, params.clone()));
        let mut s = self.s.borrow_mut();
        let h = s.next_handle;
        s.next_handle += 1;
        Ok(h)
    }
    fn show_screen(&self, name: &str) -> HostResult<()> {
        self.rec(HostCall::ShowScreen(name.to_string()));
        Ok(())
    }
    fn hide_screen(&self, name: &str) -> HostResult<()> {
        self.rec(HostCall::HideScreen(name.to_string()));
        Ok(())
    }
    fn set_hud_var(&self, name: &str, value: Value) -> HostResult<()> {
        self.rec(HostCall::HudVar(name.to_string(), value));
        Ok(())
    }
    fn show_message(&self, text: &str, seconds: f32) -> HostResult<()> {
        self.rec(HostCall::Message(text.to_string(), seconds));
        Ok(())
    }
    fn console_print(&self, text: &str) {
        self.rec(HostCall::ConsolePrint(text.to_string()));
    }
    fn run_console_command(&self, line: &str) -> HostResult<String> {
        self.rec(HostCall::ConsoleCommand(line.to_string()));
        Ok(format!("mock ran: {line}"))
    }
    fn log(&self, mod_id: &str, level: LogLevel, message: &str) {
        self.s.borrow_mut().logs.push((mod_id.to_string(), level, message.to_string()));
    }
}
