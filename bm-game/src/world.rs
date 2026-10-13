//! The simulation world: a small struct-of-arrays entity store (one sparse `Vec` per component, indexed by
//! the dense entity id, iteration order = id order, hence deterministic) plus level data and queues.
//!
//! Space: native engine space (left-handed, Y up, cm), see [`crate::math`].

use crate::ai::{AiComp, Role};
use crate::anim::AnimState;
use crate::collision::{CollisionWorld, Hit, Surface, ray_vs_capsule};
use crate::controller::{Body, BodyParams};
use crate::data::{GameData, WeaponKind};
use crate::math::{Rng, Vec3, forward, matrix_yaw, mat_pos, pose_matrix, v3};
use crate::mission::MissionState;
use crate::nav::{NavCache, NavConfig};
use crate::player::PlayerComp;
use bm_models::Mat4;
use bm_script::Event;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use bm_script::EntityId;

// ---------------------------------------------------------------------------------------------- store

/// Sparse component storage indexed by entity id.
#[derive(Clone, Debug)]
pub struct Store<T> {
    v: Vec<Option<T>>,
}

impl<T> Default for Store<T> {
    fn default() -> Self {
        Store { v: Vec::new() }
    }
}

impl<T> Store<T> {
    pub fn insert(&mut self, id: EntityId, c: T) -> Option<T> {
        let i = id.0 as usize;
        if self.v.len() <= i {
            self.v.resize_with(i + 1, || None);
        }
        self.v[i].replace(c)
    }
    pub fn get(&self, id: EntityId) -> Option<&T> {
        self.v.get(id.0 as usize)?.as_ref()
    }
    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut T> {
        self.v.get_mut(id.0 as usize)?.as_mut()
    }
    pub fn remove(&mut self, id: EntityId) -> Option<T> {
        self.v.get_mut(id.0 as usize)?.take()
    }
    pub fn take(&mut self, id: EntityId) -> Option<T> {
        self.remove(id)
    }
    pub fn contains(&self, id: EntityId) -> bool {
        self.get(id).is_some()
    }
    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &T)> {
        self.v.iter().enumerate().filter_map(|(i, c)| c.as_ref().map(|c| (EntityId(i as u64), c)))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EntityId, &mut T)> {
        self.v.iter_mut().enumerate().filter_map(|(i, c)| c.as_mut().map(|c| (EntityId(i as u64), c)))
    }
    pub fn ids(&self) -> Vec<EntityId> {
        self.iter().map(|(i, _)| i).collect()
    }
    pub fn len(&self) -> usize {
        self.v.iter().filter(|c| c.is_some()).count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ----------------------------------------------------------------------------------------- components

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Player,
    Npc,
    Door,
    Pickup,
    Light,
    Sound,
    Particles,
    Marker,
    Prop,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Player => "player",
            Kind::Npc => "npc",
            Kind::Door => "door",
            Kind::Pickup => "pickup",
            Kind::Light => "light",
            Kind::Sound => "sound",
            Kind::Particles => "particles",
            Kind::Marker => "marker",
            Kind::Prop => "prop",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Info {
    pub name: String,
    pub kind: Kind,
    pub alive: bool,
    /// Scene graph node that carries the model / transform (if loaded from a level).
    pub node: Option<usize>,
    pub template: String,
    pub model: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub world: Mat4,
    pub pos: Vec3,
    pub yaw: f32,
}

impl Transform {
    pub fn from_matrix(m: Mat4) -> Transform {
        Transform { world: m, pos: mat_pos(&m), yaw: matrix_yaw(&m) }
    }
    pub fn from_pose(pos: Vec3, yaw: f32) -> Transform {
        Transform { world: pose_matrix(pos, yaw), pos, yaw }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Hierarchy {
    pub parent: Option<EntityId>,
    pub children: Vec<EntityId>,
    /// Transform relative to the parent entity.
    pub local: Option<Mat4>,
}

#[derive(Clone, Debug)]
pub struct RenderComp {
    pub node: Option<usize>,
    pub prim_id: Option<u32>,
    pub visible: bool,
}

#[derive(Clone, Debug)]
pub struct Health {
    pub hp: f32,
    pub max: f32,
    pub last_attacker: Option<EntityId>,
    pub hit_flash: f32,
}

#[derive(Clone, Debug)]
pub struct Character {
    pub role: Role,
    pub weapon: Option<String>,
    pub ammo: u32,
    pub fire_cd: f32,
    pub walk_speed: f32,
    pub run_speed: f32,
    /// Outfit value the actor wears (`ZHM3Actor` property 6); people with the same value are colleagues.
    pub outfit: u8,
}

#[derive(Clone, Debug)]
pub struct AnimComp {
    pub state: AnimState,
    pub time: f32,
    pub phase: f32,
    pub speed: f32,
    pub dead_blend: f32,
}

impl Default for AnimComp {
    fn default() -> Self {
        AnimComp { state: AnimState::Idle, time: 0.0, phase: 0.0, speed: 0.0, dead_blend: 0.0 }
    }
}

/// How a door leaf moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DoorMotion {
    /// Hinged at the node origin, swinging about the local Y axis. `ext` is the side the leaf extends to along its
    /// local X axis (`-1` for the usual single door whose model spans x = -95..5, `+1` for the mirrored leaf of a
    /// double door); `max` is the full opening angle (radians).
    Swing { ext: f32, max: f32 },
    /// Slides along the world direction `dir` (unit) by `dist` cm.
    Slide { dir: Vec3, dist: f32 },
}

#[derive(Clone, Debug)]
pub struct DoorComp {
    pub collider: Option<u32>,
    /// World matrix of the closed door.
    pub closed: Mat4,
    pub open: bool,
    /// Current opening amount: angle in radians for swing doors, distance in cm for sliding doors.
    pub angle: f32,
    pub target: f32,
    pub locked: bool,
    pub motion: DoorMotion,
    /// Entity id of the first leaf of the door this leaf belongs to (leaves of one door open together).
    pub group: u64,
    /// Lock-pick difficulty token of the original data (`EASY`, `IMPOSSIBLE` = not pickable, ...).
    pub lock_difficulty: String,
    /// Seconds without anybody near after which a door opened by an NPC closes again (0 = stays open).
    pub auto_close: f32,
    pub idle: f32,
    pub opened_by_npc: bool,
    /// How many times the door was opened (statistics / tests).
    pub open_count: u32,
}

impl DoorComp {
    pub fn swing(closed: Mat4, collider: Option<u32>) -> DoorComp {
        DoorComp {
            collider,
            closed,
            open: false,
            angle: 0.0,
            target: 0.0,
            locked: false,
            motion: DoorMotion::Swing { ext: -1.0, max: std::f32::consts::FRAC_PI_2 * 0.95 },
            group: 0,
            lock_difficulty: "IMPOSSIBLE".into(),
            auto_close: 0.0,
            idle: 0.0,
            opened_by_npc: false,
            open_count: 0,
        }
    }

    /// World matrix of the leaf at the current opening amount.
    pub fn current_matrix(&self) -> Mat4 {
        match self.motion {
            DoorMotion::Swing { .. } => self.closed.mul(&pose_matrix(Vec3::ZERO, self.angle)),
            DoorMotion::Slide { dir, .. } => {
                let mut m = self.closed;
                m.0[0][3] += dir.x * self.angle;
                m.0[1][3] += dir.y * self.angle;
                m.0[2][3] += dir.z * self.angle;
                m
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct PickupComp {
    pub weapon: String,
    pub taken: bool,
    /// Original item definition name (`CC_Hammer_01`, `Item_Cello_01`), empty for script spawns.
    pub item: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    Omni,
    Spot,
}

/// A light of the scene (decoded from the PRM light records, see `bm_models::lighting`) or spawned by a script.
#[derive(Clone, Debug)]
pub struct LightComp {
    pub kind: LightKind,
    /// Display space RGB (not multiplied by `intensity`).
    pub color: [f32; 3],
    /// Range end (cm): no light beyond.
    pub range: f32,
    /// Range start (cm): full intensity up to here.
    pub range_start: f32,
    pub intensity: f32,
    pub on: bool,
    /// World direction of a spot cone axis.
    pub direction: [f32; 3],
    /// `(cos outer, cos inner)` of spot lights.
    pub cone: Option<(f32, f32)>,
}

impl LightComp {
    /// Warm omni light with the old placeholder values (scripted lights without data).
    pub fn omni_default() -> LightComp {
        LightComp { kind: LightKind::Omni, color: [1.0, 0.85, 0.6], range: 900.0, range_start: 0.0, intensity: 1.0, on: true, direction: [0.0, -1.0, 0.0], cone: None }
    }

    pub fn from_scene(l: &bm_models::lighting::SceneLight) -> LightComp {
        LightComp {
            kind: if l.cone.is_some() { LightKind::Spot } else { LightKind::Omni },
            color: l.color,
            range: l.range_end.max(1.0),
            range_start: l.range_start.min(l.range_end),
            intensity: l.intensity,
            on: true,
            direction: l.direction,
            cone: l.cone.map(|c| (c.cos_outer, c.cos_inner)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SoundEmitter {
    pub name: String,
    pub radius: f32,
}

#[derive(Clone, Debug)]
pub struct ParticleEmit {
    /// Index into `SceneParticles::emitters` of the level, if loaded from a scene.
    pub scene_emitter: Option<usize>,
    pub template: String,
}

// ------------------------------------------------------------------------------------------- queues

/// A cancelable / modifiable gameplay request, resolved at a safe point of the frame (scripts may veto).
#[derive(Clone, Debug)]
pub enum Request {
    Damage {
        victim: EntityId,
        attacker: Option<EntityId>,
        amount: f32,
        weapon: Option<String>,
        zone: Option<String>,
        point: Vec3,
        dir: Vec3,
    },
    Pickup {
        who: EntityId,
        item: EntityId,
    },
    Action {
        actor: EntityId,
        action: String,
        target: Option<EntityId>,
    },
}

impl Request {
    /// The script event that announces this request.
    pub fn to_event(&self, w: &World) -> Event {
        match self {
            Request::Damage { victim, attacker, amount, weapon, zone, .. } => {
                Event::damage(victim.0, attacker.map(|a| a.0), *amount, weapon.as_deref(), zone.as_deref())
            }
            Request::Pickup { who, item } => {
                let name = w.pickups.get(*item).map(|p| p.weapon.clone()).unwrap_or_default();
                Event::pickup(who.0, &name, Some(item.0))
            }
            Request::Action { actor, action, target } => Event::action(actor.0, action, target.map(|t| t.0)),
        }
    }

    /// Applies handler modifications of the event payload (currently the damage amount).
    pub fn apply_payload(&mut self, payload: &Value) {
        if let Request::Damage { amount, .. } = self {
            if let Some(a) = payload.get("amount").and_then(|v| v.as_f64()) {
                *amount = a as f32;
            }
        }
    }
}

/// Presentation events consumed by audio / particles / UI (drained once per frame).
#[derive(Clone, Debug, PartialEq)]
pub enum FxEvent {
    Shot { pos: Vec3, dir: Vec3, weapon: String, shooter: EntityId },
    Impact { pos: Vec3, normal: Vec3, surface: Surface },
    Blood { pos: Vec3, dir: Vec3 },
    Footstep { pos: Vec3, surface: Surface, run: bool, who: EntityId },
    Reload { pos: Vec3, weapon: String },
    Door { pos: Vec3, open: bool },
    Pickup { pos: Vec3 },
    Death { pos: Vec3, who: EntityId },
    Melee { pos: Vec3, weapon: String },
    Message { text: String, seconds: f32 },
    /// Scripted one-shot sound by bank name.
    Sound { name: String, pos: Option<Vec3>, entity: Option<EntityId>, volume: f32, pitch: f32, looping: bool, handle: u64 },
    StopSound { handle: u64 },
    Particles { template: String, pos: Vec3, scale: f32 },
}

/// A sound the NPCs can hear.
#[derive(Clone, Debug)]
pub struct Noise {
    pub pos: Vec3,
    pub radius: f32,
    pub time: f64,
    pub source: EntityId,
}

#[derive(Clone, Debug, Default)]
pub struct LevelMeta {
    /// Scene base, e.g. `Scenes/M01/M01_main`.
    pub scene: String,
    pub bounds_lo: Vec3,
    pub bounds_hi: Vec3,
    /// Entities below this height are considered fallen out of the world and recovered.
    pub kill_y: f32,
    pub collision_triangles: usize,
}

#[derive(Clone, Debug)]
pub struct HitScan {
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
    pub surface: Surface,
    pub entity: Option<EntityId>,
    pub zone: Option<&'static str>,
    pub collider: Option<u32>,
}

// ------------------------------------------------------------------------------------------- world

pub type BrainFactory = Box<dyn Fn(Role) -> Box<dyn crate::ai::AiBrain> + Send>;

pub struct World {
    pub time: f64,
    pub tick: u64,
    pub dt: f32,
    ids: Arc<AtomicU64>,
    pub infos: Store<Info>,
    pub transforms: Store<Transform>,
    pub hier: Store<Hierarchy>,
    pub renders: Store<RenderComp>,
    pub healths: Store<Health>,
    pub characters: Store<Character>,
    pub ais: Store<AiComp>,
    pub bodies: Store<Body>,
    pub anims: Store<AnimComp>,
    pub players: Store<PlayerComp>,
    pub doors: Store<DoorComp>,
    pub pickups: Store<PickupComp>,
    pub lights: Store<LightComp>,
    pub sounds: Store<SoundEmitter>,
    pub emitters: Store<ParticleEmit>,

    pub collision: CollisionWorld,
    pub nav: NavCache,
    pub nav_cfg: NavConfig,
    pub body_params: BodyParams,
    pub data: GameData,
    pub mission: MissionState,
    pub meta: LevelMeta,
    /// Room volumes of the level (from the decoded `.RMI`), shared with the audio / save systems.
    pub rooms: Arc<crate::rooms::RoomIndex>,
    /// Trigger volumes, security zones and the player's current room / zone.
    pub areas: crate::areas::Areas,
    pub rng: Rng,
    pub player: Option<EntityId>,
    /// Notoriety, disguise and difficulty rules (`crate::stealth`).
    pub stealth: crate::stealth::Stealth,
    /// Outfit values the player model has meshes for.
    pub player_outfits: Vec<u8>,

    pub requests: Vec<Request>,
    /// Post-facto script events produced by the simulation (kill, mission, spawn ...).
    pub outbox: Vec<Event>,
    pub fx: Vec<FxEvent>,
    pub noises: Vec<Noise>,
    pub brain_factory: BrainFactory,
    /// NPCs farther than this from the player are frozen unless alerted (cm).
    pub ai_radius: f32,
    /// Maximum number of NPC path searches started per tick (0 = NPCs only stand / fall; used while settling at load).
    pub path_budget: usize,
    names: std::collections::HashMap<String, EntityId>,
}

impl World {
    pub fn new(collision: CollisionWorld, data: GameData, seed: u64) -> World {
        let (lo, hi) = collision.bounds().unwrap_or((Vec3::ZERO, Vec3::ZERO));
        let meta = LevelMeta {
            scene: String::new(),
            bounds_lo: lo,
            bounds_hi: hi,
            kill_y: lo.y - 3000.0,
            collision_triangles: collision.triangle_count(),
        };
        World {
            time: 0.0,
            tick: 0,
            dt: 1.0 / 60.0,
            ids: Arc::new(AtomicU64::new(1)),
            infos: Store::default(),
            transforms: Store::default(),
            hier: Store::default(),
            renders: Store::default(),
            healths: Store::default(),
            characters: Store::default(),
            ais: Store::default(),
            bodies: Store::default(),
            anims: Store::default(),
            players: Store::default(),
            doors: Store::default(),
            pickups: Store::default(),
            lights: Store::default(),
            sounds: Store::default(),
            emitters: Store::default(),
            collision,
            nav: NavCache::default(),
            nav_cfg: NavConfig::default(),
            body_params: BodyParams::default(),
            data,
            mission: MissionState::default(),
            meta,
            rooms: Arc::new(crate::rooms::RoomIndex::default()),
            areas: crate::areas::Areas::default(),
            rng: Rng::new(seed),
            player: None,
            stealth: crate::stealth::Stealth::default(),
            player_outfits: Vec::new(),
            requests: Vec::new(),
            outbox: Vec::new(),
            fx: Vec::new(),
            noises: Vec::new(),
            brain_factory: Box::new(|role| Box::new(crate::ai::SimpleBrain::new(role))),
            ai_radius: 9000.0,
            path_budget: 4,
            names: Default::default(),
        }
    }

    /// Shared id allocator (the script host reserves ids for entities it spawns asynchronously).
    pub fn id_allocator(&self) -> Arc<AtomicU64> {
        self.ids.clone()
    }

    pub fn alloc_id(&mut self) -> EntityId {
        EntityId(self.ids.fetch_add(1, Ordering::Relaxed))
    }

    // ----------------------------------------------------------------------------- creation

    /// Creates an entity with the basic components. Returns its id.
    pub fn spawn(&mut self, name: &str, kind: Kind, template: &str, tr: Transform) -> EntityId {
        let id = self.alloc_id();
        self.spawn_with_id(id, name, kind, template, tr);
        id
    }

    pub fn spawn_with_id(&mut self, id: EntityId, name: &str, kind: Kind, template: &str, tr: Transform) {
        self.infos.insert(id, Info { name: name.to_string(), kind, alive: true, node: None, template: template.to_string(), model: None });
        self.transforms.insert(id, tr);
        self.names.entry(name.to_ascii_lowercase()).or_insert(id);
    }

    pub fn find(&self, name: &str) -> Option<EntityId> {
        self.names.get(&name.to_ascii_lowercase()).copied().filter(|id| self.infos.contains(*id))
    }

    /// Case-insensitive substring search in entity names (id order).
    pub fn find_containing(&self, needle: &str) -> Vec<EntityId> {
        let n = needle.to_ascii_lowercase();
        self.infos.iter().filter(|(_, i)| i.name.to_ascii_lowercase().contains(&n)).map(|(id, _)| id).collect()
    }

    pub fn despawn(&mut self, id: EntityId) -> bool {
        let Some(info) = self.infos.remove(id) else { return false };
        let key = info.name.to_ascii_lowercase();
        if self.names.get(&key) == Some(&id) {
            self.names.remove(&key);
        }
        self.transforms.remove(id);
        if let Some(h) = self.hier.remove(id) {
            if let Some(p) = h.parent.and_then(|p| self.hier.get_mut(p)) {
                p.children.retain(|c| *c != id);
            }
            for c in h.children {
                if let Some(ch) = self.hier.get_mut(c) {
                    ch.parent = None;
                }
            }
        }
        self.renders.remove(id);
        self.healths.remove(id);
        self.characters.remove(id);
        self.ais.remove(id);
        self.bodies.remove(id);
        self.anims.remove(id);
        self.players.remove(id);
        if let Some(d) = self.doors.remove(id) {
            if let Some(c) = d.collider {
                self.collision.set_dynamic_enabled(c, false);
            }
        }
        self.pickups.remove(id);
        self.lights.remove(id);
        self.sounds.remove(id);
        self.emitters.remove(id);
        if self.player == Some(id) {
            self.player = None;
        }
        true
    }

    pub fn set_parent(&mut self, child: EntityId, parent: EntityId) {
        let (Some(pt), Some(ct)) = (self.transforms.get(parent), self.transforms.get(child)) else { return };
        let local = pt.world.inverse_affine().mul(&ct.world);
        self.hier.insert(child, Hierarchy { parent: Some(parent), children: self.hier.get(child).map(|h| h.children.clone()).unwrap_or_default(), local: Some(local) });
        match self.hier.get_mut(parent) {
            Some(h) => h.children.push(child),
            None => {
                self.hier.insert(parent, Hierarchy { parent: None, children: vec![child], local: None });
            }
        }
    }

    /// Moves an entity (and, through the hierarchy, its children).
    pub fn set_world(&mut self, id: EntityId, m: Mat4) {
        if let Some(t) = self.transforms.get_mut(id) {
            *t = Transform::from_matrix(m);
        }
        let children = self.hier.get(id).map(|h| h.children.clone()).unwrap_or_default();
        for c in children {
            if let Some(local) = self.hier.get(c).and_then(|h| h.local) {
                self.set_world(c, m.mul(&local));
            }
        }
    }

    pub fn set_pose(&mut self, id: EntityId, pos: Vec3, yaw: f32) {
        self.set_world(id, pose_matrix(pos, yaw));
    }

    // ----------------------------------------------------------------------------- queries

    pub fn pos(&self, id: EntityId) -> Option<Vec3> {
        self.transforms.get(id).map(|t| t.pos)
    }

    pub fn is_alive(&self, id: EntityId) -> bool {
        self.infos.get(id).is_some_and(|i| i.alive)
    }

    pub fn player_pos(&self) -> Option<Vec3> {
        self.player.and_then(|p| self.pos(p))
    }

    pub fn entity_count(&self) -> usize {
        self.infos.len()
    }

    pub fn count_kind(&self, k: Kind) -> usize {
        self.infos.iter().filter(|(_, i)| i.kind == k).count()
    }

    /// Nearest hit of a ray against world geometry and (optionally) character capsules.
    pub fn hitscan(&self, origin: Vec3, dir: Vec3, range: f32, ignore: Option<EntityId>) -> Option<HitScan> {
        let world: Option<Hit> = self.collision.raycast(origin, dir, range, true);
        let mut best: Option<HitScan> = world.map(|h| HitScan {
            t: h.t,
            point: h.point,
            normal: h.normal,
            surface: h.surface,
            entity: None,
            zone: None,
            collider: h.collider,
        });
        let limit = best.as_ref().map_or(range, |b| b.t);
        let mut best_t = limit;
        for (id, body) in self.bodies.iter() {
            if Some(id) == ignore || !self.is_alive(id) {
                continue;
            }
            let h = body.height(&self.body_params);
            if let Some(t) = ray_vs_capsule(origin, dir, body.pos, h, self.body_params.radius) {
                if t < best_t {
                    best_t = t;
                    let p = origin + dir * t;
                    let frac = (p.y - body.pos.y) / h;
                    let zone = if frac >= 0.84 {
                        "head"
                    } else if frac >= 0.45 {
                        "torso"
                    } else {
                        "legs"
                    };
                    let n = (p - v3(body.pos.x, p.y, body.pos.z)).norm_or(-dir);
                    best = Some(HitScan { t, point: p, normal: n, surface: Surface::Flesh, entity: Some(id), zone: Some(zone), collider: None });
                }
            }
        }
        best
    }

    pub fn line_of_sight(&self, a: Vec3, b: Vec3) -> bool {
        self.collision.line_of_sight(a, b)
    }

    // ------------------------------------------------------------------------- combat

    /// Fires `weapon` from `origin` along `dir`. Queues damage requests / fx / noise; returns whether something was hit.
    pub fn fire_weapon(&mut self, shooter: EntityId, weapon_id: &str, origin: Vec3, dir: Vec3) -> bool {
        let Some(w) = self.data.weapon(weapon_id).cloned() else { return false };
        let mut hit_any = false;
        if matches!(w.kind, WeaponKind::Melee | WeaponKind::Tool) {
            let Some(spos) = self.pos(shooter) else { return false };
            let fwd = dir.flat().norm_or(v3(0.0, 0.0, 1.0));
            let mut best: Option<(f32, EntityId)> = None;
            for (id, _) in self.healths.iter() {
                if id == shooter || !self.is_alive(id) {
                    continue;
                }
                let Some(p) = self.pos(id) else { continue };
                let d = p - spos;
                let dist = d.flat().len();
                if dist > w.range || d.y.abs() > 120.0 {
                    continue;
                }
                if dist > 1.0 && d.flat().norm().dot(fwd) < 0.5 {
                    continue;
                }
                if best.map_or(true, |(bd, _)| dist < bd) {
                    best = Some((dist, id));
                }
            }
            self.fx.push(FxEvent::Melee { pos: spos, weapon: weapon_id.to_string() });
            if w.noise > 0.0 {
                self.noises.push(Noise { pos: spos, radius: w.noise, time: self.time, source: shooter });
            }
            if let Some((_, id)) = best {
                let p = self.pos(id).unwrap_or(spos) + v3(0.0, 120.0, 0.0);
                self.requests.push(Request::Damage {
                    victim: id,
                    attacker: Some(shooter),
                    amount: w.damage,
                    weapon: Some(weapon_id.to_string()),
                    zone: Some("torso".into()),
                    point: p,
                    dir: fwd,
                });
                return true;
            }
            return false;
        }
        self.fx.push(FxEvent::Shot { pos: origin, dir, weapon: weapon_id.to_string(), shooter });
        if w.noise > 0.0 {
            self.noises.push(Noise { pos: origin, radius: w.noise, time: self.time, source: shooter });
        }
        let right = dir.cross(Vec3::UP).norm_or(v3(1.0, 0.0, 0.0));
        let up = right.cross(dir).norm_or(Vec3::UP);
        for _ in 0..w.pellets.max(1) {
            let spread = w.spread_deg.to_radians();
            let (sx, sy) = (self.rng.range(-1.0, 1.0) * spread.tan(), self.rng.range(-1.0, 1.0) * spread.tan());
            let d = (dir + right * sx + up * sy).norm();
            let Some(h) = self.hitscan(origin, d, w.range, Some(shooter)) else { continue };
            hit_any = true;
            match h.entity {
                Some(victim) => {
                    let mult = if h.zone == Some("head") { w.headshot_mult } else if h.zone == Some("legs") { 0.6 } else { 1.0 };
                    self.requests.push(Request::Damage {
                        victim,
                        attacker: Some(shooter),
                        amount: w.damage * mult,
                        weapon: Some(weapon_id.to_string()),
                        zone: h.zone.map(|z| z.to_string()),
                        point: h.point,
                        dir: d,
                    });
                }
                None => self.fx.push(FxEvent::Impact { pos: h.point, normal: h.normal, surface: h.surface }),
            }
        }
        hit_any
    }

    /// Applies damage immediately (after scripts had their say). Returns true if the victim died.
    pub fn apply_damage(&mut self, victim: EntityId, attacker: Option<EntityId>, amount: f32, weapon: Option<&str>, point: Vec3, dir: Vec3) -> bool {
        if !self.is_alive(victim) || amount <= 0.0 {
            return false;
        }
        let Some(h) = self.healths.get_mut(victim) else { return false };
        h.hp = (h.hp - amount).max(0.0);
        h.hit_flash = 0.25;
        h.last_attacker = attacker;
        let dead = h.hp <= 0.0;
        self.fx.push(FxEvent::Blood { pos: point, dir });
        if let Some(a) = self.anims.get_mut(victim) {
            if a.state != AnimState::Dead {
                a.state = AnimState::Hit;
                a.time = 0.0;
            }
        }
        if !dead {
            let apos = attacker.and_then(|a| self.pos(a));
            if let Some(a) = attacker {
                self.register_witnesses(point, a, Some(victim));
            }
            if let Some(ai) = self.ais.get_mut(victim) {
                ai.on_damaged(apos, self.time);
            }
        } else {
            self.kill_now(victim, attacker, weapon);
        }
        dead
    }

    pub fn kill_now(&mut self, victim: EntityId, killer: Option<EntityId>, weapon: Option<&str>) {
        let Some(info) = self.infos.get_mut(victim) else { return };
        if !info.alive {
            return;
        }
        info.alive = false;
        if let Some(h) = self.healths.get_mut(victim) {
            h.hp = 0.0;
        }
        if let Some(a) = self.anims.get_mut(victim) {
            a.state = AnimState::Dead;
            a.time = 0.0;
            a.dead_blend = 0.0;
        }
        if let Some(ai) = self.ais.get_mut(victim) {
            ai.die();
        }
        let pos = self.pos(victim).unwrap_or(Vec3::ZERO);
        self.fx.push(FxEvent::Death { pos, who: victim });
        self.outbox.push(Event::kill(victim.0, killer.map(|k| k.0), weapon));
        // witnesses: nearby NPCs notice the killer, and the notoriety counts them
        if let Some(k) = killer {
            self.register_witnesses(pos, k, Some(victim));
            let kp = self.pos(k);
            let ids = self.ais.ids();
            for id in ids {
                if id == victim {
                    continue;
                }
                let Some(p) = self.pos(id) else { continue };
                if p.dist(pos) < 1800.0 && self.collision.line_of_sight(p + v3(0.0, 150.0, 0.0), pos + v3(0.0, 100.0, 0.0)) {
                    if let Some(ai) = self.ais.get_mut(id) {
                        ai.witness(kp, self.time, k == self.player.unwrap_or(EntityId(0)));
                    }
                }
            }
        }
    }

    /// Resolves queued requests. `hook` is called for each (scripts dispatch the matching event there, may
    /// modify the request and return `false` to cancel it).
    pub fn resolve_requests(&mut self, hook: &mut dyn FnMut(&World, &mut Request) -> bool) {
        for _ in 0..8 {
            if self.requests.is_empty() {
                break;
            }
            let reqs = std::mem::take(&mut self.requests);
            for mut r in reqs {
                if !hook(self, &mut r) {
                    continue;
                }
                match r {
                    Request::Damage { victim, attacker, amount, weapon, point, dir, .. } => {
                        self.apply_damage(victim, attacker, amount, weapon.as_deref(), point, dir);
                    }
                    Request::Pickup { who, item } => self.do_pickup(who, item),
                    Request::Action { actor, action, target } => self.do_action(actor, &action, target),
                }
            }
        }
    }

    fn do_pickup(&mut self, who: EntityId, item: EntityId) {
        let Some(p) = self.pickups.get_mut(item) else { return };
        if p.taken {
            return;
        }
        p.taken = true;
        let weapon = p.weapon.clone();
        let item_name = p.item.clone();
        let pos = self.pos(item).unwrap_or(Vec3::ZERO);
        if let Some(r) = self.renders.get_mut(item) {
            r.visible = false;
        }
        if let Some(i) = self.infos.get_mut(item) {
            i.alive = false;
        }
        if !weapon.is_empty() {
            if let Some(pl) = self.players.get_mut(who) {
                pl.give(&weapon, &self.data);
            }
        }
        self.fx.push(FxEvent::Pickup { pos });
        let name = match self.data.weapon(&weapon) {
            Some(w) => w.name.clone(),
            None => crate::player::pretty_item(if item_name.is_empty() { &weapon } else { &item_name }),
        };
        self.fx.push(FxEvent::Message { text: format!("Picked up: {name}"), seconds: 2.5 });
        if !weapon.is_empty() {
            self.mission.note_pickup(&weapon);
        }
        if !item_name.is_empty() {
            self.mission.note_pickup(&item_name.to_ascii_lowercase());
        }
    }

    fn do_action(&mut self, actor: EntityId, action: &str, target: Option<EntityId>) {
        match (action, target) {
            ("open_door" | "close_door" | "toggle_door", Some(t)) => {
                let from = self.pos(actor).unwrap_or(Vec3::ZERO);
                self.toggle_door(t, from);
            }
            ("take_disguise", Some(t)) if Some(actor) == self.player => {
                self.take_disguise(t);
            }
            ("drop_disguise", _) if Some(actor) == self.player => self.drop_disguise(),
            _ => {}
        }
    }

    // ----------------------------------------------------------------------- script-facing helpers

    pub fn info_json(&self, id: EntityId) -> Option<bm_script::EntityInfo> {
        let i = self.infos.get(id)?;
        let t = self.transforms.get(id);
        Some(bm_script::EntityInfo {
            id,
            name: i.name.clone(),
            kind: i.kind.as_str().to_string(),
            position: t.map(|t| t.pos.into()).unwrap_or_default(),
            rotation: crate::math::rad_to_deg3(t.map(|t| t.yaw).unwrap_or(0.0)).into(),
            model: i.model.clone(),
            alive: i.alive,
        })
    }

    pub fn component_json(&self, id: EntityId, comp: &str) -> Option<Value> {
        match comp {
            "health" => self.healths.get(id).map(|h| json!({ "hp": h.hp, "max": h.max })),
            "ai" => self.ais.get(id).map(|a| a.to_json()),
            "transform" => self.transforms.get(id).map(|t| json!({ "x": t.pos.x, "y": t.pos.y, "z": t.pos.z, "yaw": t.yaw })),
            "inventory" => self.players.get(id).map(|p| p.inventory_json()),
            "stealth" if Some(id) == self.player => Some(self.stealth.to_json()),
            "door" => self.doors.get(id).map(|d| json!({ "open": d.open, "locked": d.locked, "angle": d.angle })),
            "light" => self.lights.get(id).map(|l| json!({ "on": l.on, "intensity": l.intensity, "range": l.range, "color": l.color })),
            _ => None,
        }
    }

    pub fn set_component_json(&mut self, id: EntityId, comp: &str, v: &Value) -> bool {
        match comp {
            "health" => {
                let Some(h) = self.healths.get_mut(id) else { return false };
                if let Some(x) = v.get("hp").and_then(|x| x.as_f64()) {
                    h.hp = (x as f32).clamp(0.0, h.max.max(x as f32));
                }
                if let Some(x) = v.get("max").and_then(|x| x.as_f64()) {
                    h.max = x as f32;
                }
                true
            }
            "stealth" if Some(id) == self.player => {
                if let Some(x) = v.get("notoriety").and_then(|x| x.as_f64()) {
                    let d = x as f32 - self.stealth.notoriety;
                    self.add_notoriety(d, "script");
                }
                if let Some(x) = v.get("difficulty").and_then(|x| x.as_u64()) {
                    self.stealth.difficulty = (x as usize).min(3);
                }
                if v.get("disguised").and_then(|x| x.as_bool()) == Some(false) {
                    self.drop_disguise();
                }
                true
            }
            "door" => {
                let Some(d) = self.doors.get_mut(id) else { return false };
                if let Some(x) = v.get("locked").and_then(|x| x.as_bool()) {
                    d.locked = x;
                }
                true
            }
            "light" => {
                let Some(l) = self.lights.get_mut(id) else { return false };
                if let Some(x) = v.get("on").and_then(|x| x.as_bool()) {
                    l.on = x;
                }
                if let Some(x) = v.get("intensity").and_then(|x| x.as_f64()) {
                    l.intensity = x as f32;
                }
                true
            }
            _ => false,
        }
    }

    pub fn facing(&self, id: EntityId) -> Vec3 {
        forward(self.transforms.get(id).map(|t| t.yaw).unwrap_or(0.0))
    }
}
