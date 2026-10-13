//! Host-facing API: the [`ScriptHost`] trait that `bm-game` implements, and the plain data types
//! that cross the Rust/Lua boundary.
//!
//! All methods take `&self` (hosts use interior mutability) so that the runtime can call the host
//! re-entrantly from inside a Lua callback without borrow conflicts. Every method has a default
//! implementation that reports "unsupported", so a host can implement only a subset.
//!
//! **Re-entrancy rule for hosts**: do not synchronously call `ScriptRuntime::dispatch` from inside a
//! `ScriptHost` method; queue the event and dispatch it at a safe point of the frame instead.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostError(pub String);

impl HostError {
    pub fn unsupported(what: &str) -> Self {
        HostError(format!("not supported by this host: {what}"))
    }
    pub fn new(m: impl Into<String>) -> Self {
        HostError(m.into())
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for HostError {}

pub type HostResult<T> = Result<T, HostError>;

/// Entity handle. Passed to Lua as a plain number (exact up to 2^53).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityId(pub u64);

/// Position/direction in native engine space (left-handed, Y up, centimetres).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn dist(self, o: Vec3) -> f32 {
        let (dx, dy, dz) = (self.x - o.x, self.y - o.y, self.z - o.z);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityInfo {
    pub id: EntityId,
    pub name: String,
    /// Game-defined kind, e.g. `"npc"`, `"prop"`, `"player"`, `"weapon"`.
    pub kind: String,
    pub position: Vec3,
    /// Euler rotation in degrees.
    pub rotation: Vec3,
    pub model: Option<String>,
    pub alive: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntityQuery {
    pub kind: Option<String>,
    /// Substring match on the entity name (case-insensitive).
    pub name_contains: Option<String>,
    pub near: Option<Vec3>,
    pub radius: Option<f32>,
    pub alive_only: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpawnRequest {
    /// Template / archetype name known to the game (e.g. a prop or NPC template).
    pub template: String,
    pub name: Option<String>,
    pub position: Vec3,
    pub rotation: Vec3,
    pub model: Option<String>,
    /// Initial component values.
    pub components: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RayHit {
    pub position: Vec3,
    pub normal: Vec3,
    pub distance: f32,
    pub entity: Option<EntityId>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SoundParams {
    pub position: Option<Vec3>,
    /// Attach to (and follow) this entity.
    pub entity: Option<EntityId>,
    pub volume: Option<f32>,
    pub pitch: Option<f32>,
    pub looping: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParticleParams {
    pub entity: Option<EntityId>,
    pub scale: Option<f32>,
    pub lifetime: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// What the game must provide to scripts. See `docs/modding.md` for the semantics of each call.
#[allow(unused_variables)]
pub trait ScriptHost {
    // ---- world ----
    fn level_name(&self) -> Option<String> {
        None
    }
    /// Seconds of game time since level start.
    fn game_time(&self) -> f64 {
        0.0
    }
    fn player(&self) -> Option<EntityId> {
        None
    }
    fn raycast(&self, from: Vec3, to: Vec3) -> HostResult<Option<RayHit>> {
        Err(HostError::unsupported("world.raycast"))
    }

    // ---- entities ----
    fn entity_info(&self, id: EntityId) -> Option<EntityInfo> {
        None
    }
    fn find_entity(&self, name: &str) -> Option<EntityId> {
        None
    }
    fn query_entities(&self, q: &EntityQuery) -> Vec<EntityInfo> {
        Vec::new()
    }
    fn spawn_entity(&self, req: &SpawnRequest) -> HostResult<EntityId> {
        Err(HostError::unsupported("entity.spawn"))
    }
    fn despawn_entity(&self, id: EntityId) -> HostResult<()> {
        Err(HostError::unsupported("entity.despawn"))
    }
    fn set_entity_position(&self, id: EntityId, pos: Vec3) -> HostResult<()> {
        Err(HostError::unsupported("entity.set_position"))
    }
    fn set_entity_rotation(&self, id: EntityId, euler_deg: Vec3) -> HostResult<()> {
        Err(HostError::unsupported("entity.set_rotation"))
    }
    /// Swap the visual model (VFS path or model name).
    fn set_entity_model(&self, id: EntityId, model: &str) -> HostResult<()> {
        Err(HostError::unsupported("entity.set_model"))
    }
    fn get_component(&self, id: EntityId, component: &str) -> HostResult<Option<Value>> {
        Err(HostError::unsupported("entity.get_component"))
    }
    fn set_component(&self, id: EntityId, component: &str, value: Value) -> HostResult<()> {
        Err(HostError::unsupported("entity.set_component"))
    }
    fn damage_entity(&self, id: EntityId, amount: f32, source: Option<EntityId>) -> HostResult<()> {
        Err(HostError::unsupported("entity.damage"))
    }
    fn kill_entity(&self, id: EntityId, source: Option<EntityId>) -> HostResult<()> {
        Err(HostError::unsupported("entity.kill"))
    }

    // ---- audio ----
    /// Play a sound by bank name / `#sound` id / mod file; returns a handle for `stop_sound`.
    fn play_sound(&self, name: &str, params: &SoundParams) -> HostResult<u64> {
        Err(HostError::unsupported("audio.play_sound"))
    }
    fn stop_sound(&self, handle: u64) -> HostResult<()> {
        Err(HostError::unsupported("audio.stop_sound"))
    }
    fn play_music_cue(&self, cue: &str) -> HostResult<()> {
        Err(HostError::unsupported("audio.play_music"))
    }
    fn set_music_state(&self, state: &str) -> HostResult<()> {
        Err(HostError::unsupported("audio.set_music_state"))
    }
    fn music_event(&self, name: &str) -> HostResult<()> {
        Err(HostError::unsupported("audio.music_event"))
    }

    // ---- fx ----
    fn spawn_particles(&self, effect: &str, pos: Vec3, params: &ParticleParams) -> HostResult<u64> {
        Err(HostError::unsupported("fx.spawn_particles"))
    }

    // ---- ui ----
    fn show_screen(&self, name: &str) -> HostResult<()> {
        Err(HostError::unsupported("ui.show_screen"))
    }
    fn hide_screen(&self, name: &str) -> HostResult<()> {
        Err(HostError::unsupported("ui.hide_screen"))
    }
    fn set_hud_var(&self, name: &str, value: Value) -> HostResult<()> {
        Err(HostError::unsupported("ui.set_hud_var"))
    }
    /// Show a transient message/subtitle line.
    fn show_message(&self, text: &str, seconds: f32) -> HostResult<()> {
        Err(HostError::unsupported("ui.message"))
    }

    // ---- console / misc ----
    /// Print a line to the in-game console.
    fn console_print(&self, text: &str) {}
    /// Run a built-in (non-mod) console command; used by `console.exec`.
    fn run_console_command(&self, line: &str) -> HostResult<String> {
        Err(HostError::unsupported("console.exec"))
    }
    fn log(&self, mod_id: &str, level: LogLevel, message: &str) {
        match level {
            LogLevel::Debug => log::debug!("[{mod_id}] {message}"),
            LogLevel::Info => log::info!("[{mod_id}] {message}"),
            LogLevel::Warn => log::warn!("[{mod_id}] {message}"),
            LogLevel::Error => log::error!("[{mod_id}] {message}"),
        }
    }
}
