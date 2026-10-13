//! Building a [`World`] from a loaded scene: collision, actors, doors, pickups, lights, sound / particle
//! emitters and the transform hierarchy.

use crate::ai::{AiComp, Role};
use crate::areas::{SecurityZone, TriggerKind, TriggerVolume};
use crate::collision::{CollisionWorld, StaticMesh, Surface, Tri};
use crate::controller::Body;
use crate::data::{GameData, MissionDef};
use crate::math::{Vec3, character_yaw, mat_pos};
use crate::world::*;
use bm_formats::gameplay::{self, DoorDef, DoorKind, DoorState, PathIndex, PathMode, VolumeKind};
use bm_formats::loader::LoadError;
use bm_formats::prp::Value;
use bm_models::material::BlendMode;
use bm_models::mesh::VertexLayout;
use bm_models::{Mat4, NodeKind, Scene};
use bm_particles::SceneParticles;
use bm_vfs::Vfs;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum GameError {
    #[error("loading scene: {0}")]
    Load(#[from] LoadError),
    #[error("{0}")]
    Other(String),
}

#[derive(Clone, Debug)]
pub struct LoadOptions {
    pub seed: u64,
    /// Spawn the scene's NPCs (`ZHM3Actor` nodes).
    pub spawn_npcs: bool,
    /// Override the player start position (default: the scene's `ZHitman3` node).
    pub player_start: Option<Vec3>,
    /// Use this mission instead of the embedded / VFS one (`Some(None)` = free roam).
    pub mission: Option<Option<MissionDef>>,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions { seed: 1, spawn_npcs: true, player_start: None, mission: None }
    }
}

/// A loaded level: the world plus the scene data renderers need.
pub struct Level {
    pub world: World,
    pub scene: Arc<Scene>,
    pub particles: SceneParticles,
    pub stats: LevelStats,
}

#[derive(Clone, Debug, Default)]
pub struct LevelStats {
    pub collision_nodes: usize,
    pub collision_triangles: usize,
    pub doors: usize,
    pub npcs: usize,
    pub pickups: usize,
    pub lights: usize,
    pub sound_emitters: usize,
    pub particle_emitters: usize,
    pub markers: usize,
    pub npcs_with_routes: usize,
    /// Door leaves driven by the decoded `SingleDoor` / `MultiDoor` controllers.
    pub door_leaves: usize,
    pub rooms: usize,
    pub triggers: usize,
    pub exits: usize,
    /// NPCs whose weapon / route / help points come from their decoded script / `PathFollower`.
    pub npcs_with_params: usize,
}

const TYPE_HERO: u32 = 0x2003e8;
const TYPE_ACTOR: u32 = 0x2003e9;
const TYPE_SNDOBJ: u32 = 0x200012;
const TYPE_BOX: u32 = 0x2000e1;
const TYPE_ZLIST: u32 = 0x800001a;

fn is_excluded_path(path: &str) -> bool {
    path.contains("CloneGroup")
        || path.contains("mainsceneincludes")
        || path.contains("AllLevels/")
        || path.contains("Controllers!")
        // in-game map screen quads (`M01__MAP_*`) live in the scene but are UI, not world
        || path.contains("_MAP.zip")
}

fn material_excluded(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.starts_with("_glacier/") {
        // only the collision proxies are solid
        return !(n.contains("worldcoli") || n.ends_with("/collision"));
    }
    n.contains("watercollision") || n.starts_with("screens/") || n.starts_with("sprites")
}

fn foliage(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["leaf", "leaves", "foliage", "palm", "bush", "grass", "plant", "flower", "vine", "fern", "branch", "scatter"].iter().any(|k| n.contains(k))
}

/// Collects solid triangles of a model (LOD0, rigid, lit meshes) in model space.
fn model_tris(scene: &Scene, model: &bm_models::Model, out: &mut Vec<Tri>) {
    for mesh in &model.meshes {
        if !mesh.is_lod0() || mesh.is_skinned() || mesh.layout == VertexLayout::PosColor10 {
            continue;
        }
        for sub in &mesh.submeshes {
            let desc = scene.materials.desc(sub.material);
            let (name, surface) = match &desc {
                Some(d) => (d.name.as_str(), Surface::from_material_name(&d.name)),
                None => ("", Surface::Default),
            };
            if let Some(d) = &desc {
                if d.is_shadow_only() || matches!(d.render_state.blend, BlendMode::Add | BlendMode::AddBeforeTrans | BlendMode::AddOnOpaque | BlendMode::TransAddOnOpaque) {
                    continue;
                }
                if d.render_state.alpha_test && foliage(&d.name) {
                    continue;
                }
            }
            if material_excluded(name) {
                continue;
            }
            let idx = mesh.submesh_indices(sub);
            for t in idx.chunks_exact(3) {
                let p = |i: u32| Vec3::from_array(mesh.vertices.positions[i as usize]);
                out.push(Tri::new(p(t[0]), p(t[1]), p(t[2]), surface));
            }
        }
    }
}

/// One decoded door: its definition and the model nodes that move (one for single doors, the leaves for double /
/// sliding doors).
pub struct DoorSpec {
    pub def: DoorDef,
    pub leaves: Vec<usize>,
}

/// Resolves the leaves of every `SingleDoor` / `MultiDoor` controller.
pub fn door_specs(scene: &Scene) -> Vec<DoorSpec> {
    let mut out = Vec::new();
    for def in gameplay::doors(&scene.graph) {
        let n = &scene.nodes[def.node];
        if is_excluded_path(&scene.path(def.node)) {
            continue;
        }
        let leaves: Vec<usize> = if n.prim_id.is_some() {
            vec![def.node]
        } else {
            n.children.iter().copied().filter(|&c| scene.nodes[c].prim_id.is_some() && scene.nodes[c].kind == NodeKind::Static).collect()
        };
        if !leaves.is_empty() {
            out.push(DoorSpec { def, leaves });
        }
    }
    out
}

/// Builds the static collision mesh + the door leaves (local-space triangles with their node index).
pub fn build_collision(scene: &Scene, door_leaves: &HashSet<usize>) -> (Vec<Tri>, Vec<(usize, Vec<Tri>)>, usize) {
    let mut tris = Vec::new();
    let mut doors = Vec::new();
    let mut nodes = 0;
    for n in &scene.nodes {
        if n.kind != NodeKind::Static || n.prim_id.is_none() {
            continue;
        }
        let path = scene.path(n.index);
        if is_excluded_path(&path) {
            continue;
        }
        let Some(model) = scene.node_model(n.index) else { continue };
        if door_leaves.contains(&n.index) {
            let mut local = Vec::new();
            model_tris(scene, &model, &mut local);
            if !local.is_empty() {
                doors.push((n.index, local));
            }
            continue;
        }
        let before = tris.len();
        let mut local = Vec::new();
        model_tris(scene, &model, &mut local);
        for t in local {
            tris.push(t.transformed(&n.world));
        }
        if tris.len() > before {
            nodes += 1;
        }
    }
    (tris, doors, nodes)
}

/// Motion of one door leaf, derived from its model bounds (see `docs/formats.md`, "doors").
fn leaf_motion(scene: &Scene, def: &DoorDef, leaf: usize, group: usize) -> DoorMotion {
    let n = &scene.nodes[leaf];
    let bb = n.prim_id.and_then(|p| scene.prm.model(p as usize)).map(|m| (m.bbox_min, m.bbox_max));
    let Some((lo, hi)) = bb else { return DoorMotion::Swing { ext: -1.0, max: std::f32::consts::FRAC_PI_2 * 0.95 } };
    if def.sliding {
        let center = n.world.transform_point([(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5]);
        let g = mat_pos(&scene.nodes[group].world);
        let mut d = (Vec3::from_array(center) - g).flat();
        if d.len() < 1.0 {
            // single sliding leaf: along its own wider horizontal axis
            d = Vec3::from_array(n.world.transform_vector([0.0, 1.0, 0.0])).flat();
        }
        let dir = d.norm_or(Vec3::from_array([1.0, 0.0, 0.0]));
        let corners = [lo[0], hi[0]].into_iter().flat_map(|x| [lo[1], hi[1]].into_iter().flat_map(move |y| [lo[2], hi[2]].into_iter().map(move |z| [x, y, z])));
        let (mut mn, mut mx) = (f32::MAX, f32::MIN);
        for c in corners {
            let w = Vec3::from_array(n.world.transform_point(c));
            let t = w.dot(dir);
            mn = mn.min(t);
            mx = mx.max(t);
        }
        return DoorMotion::Slide { dir, dist: ((mx - mn) * 0.92).clamp(30.0, 170.0) };
    }
    // swing: hinge at the node origin; the leaf extends along local -X (single doors, x = -95..5) or +X (mirrored)
    let (cx, ex, ez) = ((lo[0] + hi[0]) * 0.5, hi[0] - lo[0], hi[2] - lo[2]);
    let ext = if ex >= ez && cx > 0.0 { 1.0 } else { -1.0 };
    DoorMotion::Swing { ext, max: std::f32::consts::FRAC_PI_2 * 0.95 }
}

fn last_segment(name: &str) -> &str {
    name.rsplit('!').next().unwrap_or(name)
}

/// Resolves `ZLIST` / marker nodes into a patrol route for the actor group `group_name`.
fn find_route(scene: &Scene, by_name: &HashMap<&str, usize>, group_name: &str) -> Vec<Vec3> {
    let folder = group_name.rfind('!').map(|i| &group_name[..=i]).unwrap_or("");
    let prefixes = [format!("{group_name}_stuff!"), format!("{group_name}_related!"), folder.to_string()];
    let lower_has = |s: &str, k: &str| s.to_ascii_lowercase().contains(k);
    for prefix in prefixes.iter().filter(|p| !p.is_empty()) {
        for n in &scene.graph.nodes {
            if n.type_id != TYPE_ZLIST || !n.name.starts_with(prefix.as_str()) {
                continue;
            }
            let leaf = last_segment(&n.name);
            if !(lower_has(leaf, "list") || lower_has(leaf, "wp") || lower_has(leaf, "waypoint")) {
                continue;
            }
            let Some(Value::Container(items)) = n.props.get(5) else { continue };
            let mut pts = Vec::new();
            for it in items {
                if let Value::Str(s) = it {
                    let name = s.rsplit('\\').next().unwrap_or(s);
                    if let Some(&idx) = by_name.get(name) {
                        pts.push(mat_pos(&scene.nodes[idx].world));
                    }
                }
            }
            if pts.len() >= 2 {
                return pts;
            }
        }
    }
    // fallback: marker nodes `<group>_stuff!<NN>...` in name order
    let stuff = format!("{group_name}_stuff!");
    let mut marks: Vec<(&str, Vec3)> = scene
        .graph
        .nodes
        .iter()
        .filter(|n| n.type_id != TYPE_ZLIST && n.name.starts_with(&stuff) && n.name.as_bytes().get(stuff.len()).is_some_and(|c| c.is_ascii_digit()))
        .map(|n| (n.name.as_str(), mat_pos(&scene.nodes[n.index].world)))
        .collect();
    marks.sort_by(|a, b| a.0.cmp(b.0));
    if marks.len() >= 2 { marks.into_iter().map(|m| m.1).collect() } else { Vec::new() }
}

/// Waypoint positions of `ZLIST` nodes (`ROOT\...` references), in list order.
fn route_from_lists(scene: &Scene, pidx: &PathIndex, lists: &[String]) -> Vec<Vec3> {
    let mut pts = Vec::new();
    for l in lists {
        let Some(zi) = pidx.resolve(l) else { continue };
        for it in gameplay::zlist_items(&scene.graph.nodes[zi]) {
            if let Some(ni) = pidx.resolve(&it) {
                pts.push(mat_pos(&scene.nodes[ni].world));
            }
        }
    }
    pts
}

impl World {
    /// Spawns an NPC with a body, health, weapon (by role) and an AI brain from [`World::brain_factory`].
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_npc(&mut self, name: &str, role: Role, pos: Vec3, yaw: f32, route: Vec<Vec3>, node: Option<usize>, prim: Option<u32>) -> EntityId {
        let id = self.alloc_id();
        self.spawn_npc_as(id, name, role, pos, yaw, route, node, prim);
        id
    }

    /// Like [`World::spawn_npc`] with a pre-allocated id (script spawns reserve ids ahead of time).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_npc_as(&mut self, id: EntityId, name: &str, role: Role, pos: Vec3, yaw: f32, route: Vec<Vec3>, node: Option<usize>, prim: Option<u32>) {
        self.spawn_with_id(id, name, Kind::Npc, role.as_str(), Transform::from_pose(pos, yaw));
        if let Some(i) = self.infos.get_mut(id) {
            i.node = node;
        }
        self.renders.insert(id, RenderComp { node, prim_id: prim, visible: true });
        let hp = self.data.npc_health.get(role.as_str()).copied().unwrap_or(50.0);
        self.healths.insert(id, Health { hp, max: hp, last_attacker: None, hit_flash: 0.0 });
        let weapon = self.data.npc_weapons.get(role.as_str()).cloned();
        let ammo = weapon.as_ref().and_then(|w| self.data.weapon(w)).map(|w| w.mag_size).unwrap_or(0);
        self.characters.insert(id, Character { role, weapon, ammo, fire_cd: 0.0, walk_speed: 150.0, run_speed: 380.0, outfit: 1 });
        self.bodies.insert(id, Body::new(pos));
        self.anims.insert(id, AnimComp::default());
        let brain = (self.brain_factory)(role);
        self.ais.insert(id, AiComp::new(brain, pos, yaw, route));
    }

    pub fn spawn_door(&mut self, name: &str, closed: Mat4, collider: Option<u32>, node: Option<usize>, prim: Option<u32>) -> EntityId {
        let id = self.spawn(name, Kind::Door, "door", Transform::from_matrix(closed));
        if let Some(i) = self.infos.get_mut(id) {
            i.node = node;
        }
        self.renders.insert(id, RenderComp { node, prim_id: prim, visible: true });
        let mut d = DoorComp::swing(closed, collider);
        d.group = id.0;
        self.doors.insert(id, d);
        id
    }

    pub fn spawn_pickup(&mut self, name: &str, weapon: &str, m: Mat4, node: Option<usize>) -> EntityId {
        let id = self.spawn(name, Kind::Pickup, "pickup", Transform::from_matrix(m));
        if let Some(i) = self.infos.get_mut(id) {
            i.node = node;
        }
        self.renders.insert(id, RenderComp { node, prim_id: None, visible: true });
        self.pickups.insert(id, PickupComp { weapon: weapon.to_string(), taken: false, item: String::new() });
        id
    }
}

/// Loads `base` (e.g. `Scenes/M01/M01_main`) into a ready-to-run level.
pub fn load_level(vfs: &Vfs, base: &str, opts: &LoadOptions) -> Result<Level, GameError> {
    let t0 = std::time::Instant::now();
    let scene = Arc::new(Scene::load(vfs, base)?);
    let t1 = t0.elapsed();
    let specs = door_specs(&scene);
    let leaf_set: HashSet<usize> = specs.iter().flat_map(|d| d.leaves.iter().copied()).collect();
    let (tris, door_tris, collision_nodes) = build_collision(&scene, &leaf_set);
    let t2 = t0.elapsed();
    let data = GameData::load(Some(vfs));
    let mut collision = CollisionWorld::from_static(Arc::new(StaticMesh::new(tris)));
    log::debug!("load_level: scene parse {:.2?}, collision gather (model decode) {:.2?}, BVH {:.2?}", t1, t2 - t1, t0.elapsed() - t2);
    let mut world = World::new(CollisionWorld::default(), data, opts.seed);
    let mut stats = LevelStats { collision_nodes, collision_triangles: collision.triangle_count(), ..Default::default() };

    // doors need the dynamic collider before the world takes ownership
    let mut leaf_colliders: HashMap<usize, u32> = HashMap::new();
    for (node, local) in door_tris {
        let m = scene.nodes[node].world;
        let h = collision.add_dynamic(local, &m);
        leaf_colliders.insert(node, h);
    }
    let (lo, hi) = collision.bounds().unwrap_or((Vec3::ZERO, Vec3::ZERO));
    world.collision = collision;
    world.meta = LevelMeta { scene: base.to_string(), bounds_lo: lo, bounds_hi: hi, kill_y: lo.y - 4000.0, collision_triangles: stats.collision_triangles };

    let by_name: HashMap<&str, usize> = {
        let mut m = HashMap::new();
        for n in &scene.nodes {
            m.entry(n.name.as_str()).or_insert(n.index);
        }
        m
    };
    // entity per scene node (for hierarchy)
    let mut node_entity: HashMap<usize, EntityId> = HashMap::new();

    // ---- player ----
    let hero = scene.nodes.iter().find(|n| n.type_id == TYPE_HERO);
    let (hpos, hyaw, hnode, hprim) = match hero {
        Some(h) => (mat_pos(&h.world), character_yaw(&h.world), Some(h.index), h.prim_id),
        None => (Vec3::ZERO, 0.0, None, None),
    };
    let start = opts.player_start.unwrap_or(hpos);
    let pid = world.spawn_player(start, hyaw, hnode, hprim);
    world.stealth = crate::stealth::Stealth::load(vfs);
    world.stealth.set_defs(gameplay::disguises(&scene.graph));
    if let Some(m) = hprim.and_then(|p| scene.model(p)) {
        world.player_outfits = m.outfit_masks();
        world.stealth.base_outfit = m.resolve_outfit(1);
    }
    if let Some(n) = hnode {
        node_entity.insert(n, pid);
    }

    // ---- scene entities ----
    let gp_actors: HashMap<usize, gameplay::ActorDef> = gameplay::actors(&scene.graph).into_iter().map(|a| (a.node, a)).collect();
    let pidx = PathIndex::new(&scene.graph);
    for n in &scene.nodes {
        let path_excl = is_excluded_path(&scene.path(n.index));
        match (n.kind, n.type_id) {
            (NodeKind::Actor, TYPE_ACTOR) if opts.spawn_npcs => {
                let group = n.parent.map(|p| &scene.nodes[p]);
                let name = group.map(|g| g.name.clone()).unwrap_or_else(|| n.name.clone());
                let def = gp_actors.get(&n.index);
                let script = def.and_then(|d| d.script.as_deref()).unwrap_or("");
                let role = if script.starts_with("alllevels/guard") || name.contains("Guards!") {
                    Role::Guard
                } else if name.contains("Special_characters") {
                    Role::Target
                } else {
                    Role::Civilian
                };
                // patrol route: the decoded PathFollower lists, else the name based search
                let mut route = def.and_then(|d| d.path.as_ref()).map(|p| route_from_lists(&scene, &pidx, &p.lists)).unwrap_or_default();
                let from_data = route.len() >= 2;
                if !from_data {
                    route = find_route(&scene, &by_name, &name);
                }
                if !route.is_empty() {
                    stats.npcs_with_routes += 1;
                }
                let id = world.spawn_npc(&name, role, mat_pos(&n.world), character_yaw(&n.world), route, Some(n.index), n.prim_id);
                if let Some(c) = world.characters.get_mut(id) {
                    c.outfit = scene.actor_outfit(n.index).unwrap_or(1);
                }
                if let Some(d) = def {
                    let mut with_params = from_data;
                    if let Some(w) = d.weapon.as_deref().and_then(|w| world.data.weapon_for_item(w)) {
                        let ammo = world.data.weapon(&w).map(|w| w.mag_size).unwrap_or(0);
                        if let Some(c) = world.characters.get_mut(id) {
                            c.weapon = Some(w);
                            c.ammo = ammo;
                        }
                        with_params = true;
                    }
                    let help = d.help_list.as_deref().map(|h| route_from_lists(&scene, &pidx, &[h.to_string()])).unwrap_or_default();
                    if let Some(ai) = world.ais.get_mut(id) {
                        ai.route_random = d.path.as_ref().is_some_and(|p| p.mode == PathMode::Random);
                        if !help.is_empty() {
                            with_params = true;
                        }
                        ai.help_points = help;
                    }
                    if with_params {
                        stats.npcs_with_params += 1;
                    }
                }
                node_entity.insert(n.index, id);
                stats.npcs += 1;
            }
            (NodeKind::Light, t) => {
                let kind = if t == 0x800024 { LightKind::Omni } else { LightKind::Spot };
                let id = world.spawn(&n.name, Kind::Light, "light", Transform::from_matrix(n.world));
                let comp = match scene.lighting().light_for_node(n.index) {
                    Some(l) => LightComp::from_scene(l),
                    None => LightComp { kind, ..LightComp::omni_default() },
                };
                world.lights.insert(id, comp);
                if let Some(i) = world.infos.get_mut(id) {
                    i.node = Some(n.index);
                }
                node_entity.insert(n.index, id);
                stats.lights += 1;
            }
            (_, TYPE_SNDOBJ) if !path_excl => {
                let id = world.spawn(&n.name, Kind::Sound, "sound", Transform::from_matrix(n.world));
                world.sounds.insert(id, SoundEmitter { name: n.name.clone(), radius: 3000.0 });
                if let Some(i) = world.infos.get_mut(id) {
                    i.node = Some(n.index);
                }
                node_entity.insert(n.index, id);
                stats.sound_emitters += 1;
            }
            (_, TYPE_BOX) if !path_excl => {
                let id = world.spawn(&n.name, Kind::Marker, "marker", Transform::from_matrix(n.world));
                if let Some(i) = world.infos.get_mut(id) {
                    i.node = Some(n.index);
                }
                node_entity.insert(n.index, id);
                stats.markers += 1;
            }
            _ => {}
        }
    }

    // ---- doors (decoded SingleDoor / MultiDoor controllers) ----
    for spec in &specs {
        let mut group = EntityId(0);
        for (k, &leaf) in spec.leaves.iter().enumerate() {
            let n = &scene.nodes[leaf];
            let name = match (spec.leaves.len(), n.parent) {
                (1, _) | (_, None) => n.name.clone(),
                (_, Some(p)) => format!("{}/{}", scene.nodes[p].name, n.name),
            };
            let collider = leaf_colliders.get(&leaf).copied();
            let id = world.spawn_door(&name, n.world, collider, Some(leaf), n.prim_id);
            if k == 0 {
                group = id;
            }
            let motion = leaf_motion(&scene, &spec.def, leaf, spec.def.node);
            let mut open_pose = None;
            if let Some(d) = world.doors.get_mut(id) {
                d.group = group.0;
                d.motion = motion;
                d.lock_difficulty = spec.def.lock_difficulty.clone();
                match spec.def.initial {
                    DoorState::Locked => d.locked = true,
                    DoorState::Open => {
                        d.open = true;
                        d.angle = match motion {
                            DoorMotion::Swing { ext, max } => ext * max,
                            DoorMotion::Slide { dist, .. } => dist,
                        };
                        d.target = d.angle;
                        open_pose = Some((d.current_matrix(), d.collider));
                    }
                    DoorState::Closed => {}
                }
            }
            if let Some((m, c)) = open_pose {
                if let Some(c) = c {
                    world.collision.set_dynamic_transform(c, &m);
                }
                world.set_world(id, m);
            }
            node_entity.insert(leaf, id);
            stats.door_leaves += 1;
        }
        stats.doors += 1;
        let _ = DoorKind::Single;
    }

    // ---- pickups (every `Action` / `AT_PICKUP` node) ----
    for p in gameplay::pickups(&scene.graph) {
        if is_excluded_path(&scene.path(p.node)) {
            continue;
        }
        let n = &scene.nodes[p.node];
        let weapon = world.data.weapon_for_item(p.item_id()).unwrap_or_default();
        let id = world.spawn_pickup(&n.name, &weapon, n.world, Some(p.node));
        if let Some(pc) = world.pickups.get_mut(id) {
            pc.item = p.item_id().to_string();
        }
        node_entity.insert(p.node, id);
        stats.pickups += 1;
    }

    // ---- rooms (.RMI) ----
    let rmi = vfs.open(&format!("{base}.RMI")).ok().and_then(|b| bm_formats::roommap::RoomMap::parse(&b).map_err(|e| log::warn!("{base}.RMI: {e}")).ok());
    let sgp = vfs.open(&format!("{base}.SGP")).ok().and_then(|b| bm_formats::sgp::Sgp::parse(&b).map_err(|e| log::warn!("{base}.SGP: {e}")).ok());
    let rooms = crate::rooms::RoomIndex::build(&scene, rmi).with_portals(sgp.as_ref());
    stats.rooms = rooms.rooms.len();

    // ---- trigger volumes + security zones ----
    let exit_nodes: HashSet<usize> = gameplay::exit_areas(&scene.graph).into_iter().map(|v| v.node).collect();
    for v in gameplay::volumes(&scene.graph) {
        let kind = match v.kind {
            VolumeKind::Action { .. } if exit_nodes.contains(&v.node) => TriggerKind::Exit,
            VolumeKind::Action { event, target, .. } => TriggerKind::Action { event, target: target.rsplit('\\').next().unwrap_or("").to_string() },
            VolumeKind::CheckInside { enter_msg, leave_msg, target, .. } => TriggerKind::Check { enter: enter_msg, leave: leave_msg, target: target.rsplit('\\').next().unwrap_or("").to_string() },
            VolumeKind::Security { zone, .. } => TriggerKind::Security { zone },
            _ => continue,
        };
        let n = &scene.nodes[v.node];
        world.areas.triggers.push(TriggerVolume::new(v.node, &n.name, kind, v.size, n.world));
    }
    world.areas.inside = vec![false; world.areas.triggers.len()];
    // the recording notice of the cameras (`FUN_005bc0e0`)
    world.stealth.tape_notice = scene.loc.as_ref().and_then(|l| l.get("AllLevels/Info/CaughtOnSuveillanceTape")).unwrap_or_default();
    // ---- security cameras: GUESSED view, see `crate::stealth::SecurityCamera` ----
    for n in &scene.nodes {
        let Some(c) = scene.graph.nodes[n.index].controllers.iter().find(|c| c.name == "SecurityCamera") else { continue };
        let f = |i: usize| match c.props.get(i) {
            Some(bm_formats::prp::Value::F32(v)) => Some(*v),
            _ => None,
        };
        let (Some(half), Some(range)) = (f(4), f(5)) else { continue };
        let p0 = n.world.transform_point([0.0, 0.0, 0.0]);
        let p1 = n.world.transform_point([0.0, 0.0, 1.0]);
        let dir = crate::math::v3(p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]).norm();
        world.stealth.cameras.push(crate::stealth::SecurityCamera { pos: mat_pos(&n.world), dir, range: range * 100.0, half_angle_deg: half.clamp(1.0, 89.0), recording_until: f64::NEG_INFINITY });
    }
    stats.triggers = world.areas.triggers.len();
    stats.exits = world.areas.exits().count();
    for (zone, paths) in gameplay::zone_rooms(&scene.graph) {
        world.areas.zones.push(SecurityZone { name: zone, paths: paths.iter().map(|p| gameplay::normalize_ref(p)).collect() });
    }
    world.areas.room_zone = rooms
        .rooms
        .iter()
        .map(|r| {
            let rp = scene.path(r.node).to_ascii_lowercase();
            world
                .areas
                .zones
                .iter()
                .enumerate()
                .flat_map(|(zi, z)| z.paths.iter().filter(|zp| rp == **zp || rp.starts_with(&format!("{zp}/"))).map(move |zp| (zp.len(), zi)))
                .max()
                .map(|(_, zi)| zi)
        })
        .collect();
    world.rooms = Arc::new(rooms);

    let t3 = t0.elapsed();
    // ---- ambient particle emitters ----
    let particles = SceneParticles::from_scene(&scene);
    let t4 = t0.elapsed();
    for (i, e) in particles.emitters.iter().enumerate() {
        if !e.is_ambient() {
            continue;
        }
        let id = world.spawn(&e.name, Kind::Particles, "particles", Transform::from_matrix(e.world));
        world.emitters.insert(id, ParticleEmit { scene_emitter: Some(i), template: e.raw.template.clone() });
        if let Some(info) = world.infos.get_mut(id) {
            info.node = Some(e.node);
        }
        node_entity.insert(e.node, id);
        stats.particle_emitters += 1;
    }

    // ---- hierarchy: nearest ancestor that has an entity ----
    let mut links = Vec::new();
    for (&node, &id) in &node_entity {
        let mut p = scene.nodes[node].parent;
        while let Some(pi) = p {
            if let Some(&pe) = node_entity.get(&pi) {
                links.push((id, pe));
                break;
            }
            p = scene.nodes[pi].parent;
        }
    }
    links.sort_by_key(|(c, _)| c.0);
    for (c, p) in links {
        world.set_parent(c, p);
    }

    // ---- mission: mod override > objectives decoded from the level data > embedded RON ----
    let mission = match &opts.mission {
        Some(m) => m.clone(),
        None => MissionDef::vfs_override(Some(vfs), base)
            .or_else(|| crate::objectives::mission_from_scene(&scene, &world, base))
            .or_else(|| MissionDef::for_level(Some(vfs), base)),
    };
    if let Some(m) = mission {
        world.start_mission(m);
    }

    // Some actors start below / inside the floor (sleepers, seated poses authored for animations): lift them
    // onto the first walkable surface above when there is no floor within reach below.
    for id in world.bodies.ids() {
        let Some(p) = world.pos(id) else { continue };
        let cw = &world.collision;
        let near_floor = cw.ground_below(p + crate::math::v3(0.0, 40.0, 0.0), 80.0, 0.65, false).is_some();
        if near_floor {
            continue;
        }
        if let Some(up) = cw.raycast(p, crate::math::v3(0.0, 1.0, 0.0), 450.0, false) {
            if let Some(g) = cw.ground_below(up.point + crate::math::v3(0.0, 30.0, 0.0), 120.0, 0.65, false) {
                let fixed = g.point;
                if let Some(b) = world.bodies.get_mut(id) {
                    b.pos = fixed;
                    b.last_safe = fixed;
                }
                let yaw = world.transforms.get(id).map(|t| t.yaw).unwrap_or(0.0);
                world.set_pose(id, fixed, yaw);
            }
        }
    }

    let t5 = t0.elapsed();
    // settle everyone on the floor (a second of simulation without input)
    // (no path searches meanwhile: the cold navigation grid made this the slowest part of a level load)
    let idle = crate::player::PlayerInput::default();
    let budget = std::mem::replace(&mut world.path_budget, 0);
    for _ in 0..30 {
        world.step(&idle, crate::sim::TICK_DT);
    }
    world.path_budget = budget;
    world.outbox.clear();
    world.fx.clear();
    world.requests.clear();
    world.time = 0.0;
    world.tick = 0;
    for (_, p) in world.players.iter_mut() {
        p.cam_pitch = -0.15;
    }
    log::debug!("load_level: entities {:.2?}, particles {:.2?}, mission+lift {:.2?}, settle {:.2?}", t3 - t2, t4 - t3, t5 - t4, t0.elapsed() - t5);
    Ok(Level { world, scene, particles, stats })
}
