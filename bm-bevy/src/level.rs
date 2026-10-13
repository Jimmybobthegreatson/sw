//! A playable level in Bevy: the [`Game`] simulation, the scene spawned as entities, and the per-frame glue that copies
//! [`RenderState`] (entity transforms, skinning poses, hidden nodes, lights, camera) into the ECS.

use crate::convert::{self, Assets3d, S};
use bevy::light::{CascadeShadowConfigBuilder, GlobalAmbientLight};
use bevy::prelude::*;
use bm_game::{Game, RenderState};
use bm_models::mesh::VertexLayout;
use bm_models::scene::{DrawItem, NodeKind, Scene};
use bm_models::{Mat4 as BmMat4, Model};
use bm_render::FrameParams;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Node kinds drawn (same set as the wgpu renderer).
pub const DRAW_KINDS: &[NodeKind] = &[NodeKind::Static, NodeKind::Scatter, NodeKind::Actor, NodeKind::ClothBundle];
const MAX_POINT_LIGHTS: usize = 16;
const MAX_SPOT_LIGHTS: usize = 8;

#[derive(Component)]
pub struct MainCamera;
#[derive(Component)]
pub struct Sun;
#[derive(Component)]
pub struct PointSlot;
#[derive(Component)]
pub struct SpotSlot;

/// Scene nodes that exist for the native game logic and are never visible in the shipped game.
fn is_gameplay_helper(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("herocutscenes") || p.contains("/coliplanes/") || (p.contains("herosetup") && !p.ends_with("/hero/ground"))
}

struct SkinMesh {
    /// The mesh entity, and the outfit value it belongs to (the player model keeps every outfit so disguises can switch).
    entity: Entity,
    outfit: u8,
    handle: Handle<Mesh>,
    src: Arc<Model>,
    mesh_index: usize,
}

/// The untextured sky dome: its vertex colours are blended towards the fog colour at and below the horizon every frame.
struct Dome {
    handle: Handle<Mesh>,
    /// Native-space vertex positions and authored colours.
    pos: Vec<[f32; 3]>,
    colors: Vec<[u8; 4]>,
    /// Dark grey vertex colours (M01): the reference footage shows a pale cream sky there, so the dome takes the fog colour.
    overcast: bool,
}

struct Item {
    entity: Entity,
    dome: Vec<Dome>,
    /// Sky sphere / cloud planes / mountains: kept centred on the camera and unlit.
    backdrop: bool,
    /// Backdrops are drawn without depth in the original (in scene order, dome first); here each one is shrunk about the
    /// camera by a slightly different factor (same on-screen size) so that depth testing gives the same order.
    backdrop_scale: f32,
    /// Sky sphere, cloud planes and the sun follow the camera; the `Landscape_*` mountain / jungle pieces of the backdrop
    /// group are placed in world space and stay where they are.
    centered: bool,
    base: BmMat4,
    skin: Vec<SkinMesh>,
    /// Outfit value shown now (player only).
    shown_outfit: u8,
}

pub struct Level {
    pub game: Game,
    pub scene: Arc<Scene>,
    pub params: FrameParams,
    items: Vec<Item>,
    node_items: HashMap<usize, usize>,
    subtree_items: HashMap<usize, Vec<usize>>,
    slot_item: HashMap<String, usize>,
    hidden: HashSet<usize>,
    light_target: Option<FrameParams>,
    light_pos: [f32; 3],
    last_time: f64,
    pub rig: Rig,
    /// Camera heading in degrees (0 = +Z), for the HUD compass.
    pub heading: f32,
    /// The floating item of the item selector: root entity and the scale that fits it to ~9 cm.
    preview: HashMap<String, (Entity, f32)>,
    /// Show the floating item this frame.
    pub preview_on: bool,
    /// Weapon name whose model floats (the selected item).
    pub preview_name: String,
    /// Models of the clothes the player can wear: (donor node, outfit) -> index into `items`, spawned on first use.
    disguise_items: HashMap<(usize, u8), usize>,
    /// The disguise item shown last frame.
    disguise_shown: Option<usize>,
}

/// The entities that outlive a level: the 3D camera, the sun and the light pools.
#[derive(Clone)]
pub struct Rig {
    pub camera: Entity,
    pub sun: Entity,
    pub points: Vec<Entity>,
    pub spots: Vec<Entity>,
    /// Gradient sky sphere (zenith -> horizon colours of the zone), kept centred on the camera behind everything.
    pub sky: Entity,
    pub sky_mesh: Handle<Mesh>,
}

/// Render layer of the item selector's floating 3D item.
pub const PREVIEW_LAYER: usize = 2;

impl Rig {
    pub fn spawn(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>) -> Rig {
        let camera = commands
            .spawn((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection { near: 0.08, far: 20000.0, ..default() }),
                Transform::from_xyz(0.0, 1.0, 0.0),
                Msaa::Sample4,
                bevy::core_pipeline::tonemapping::Tonemapping::pick(std::env::var("BM_TONEMAP").ok().as_deref()),
                DistanceFog { color: Color::linear_rgb(0.5, 0.55, 0.6), falloff: FogFalloff::Linear { start: 1.0e6, end: 2.0e6 }, ..default() },
                MainCamera,
            ))
            .id();
        let sun = commands
            .spawn((
                DirectionalLight { shadow_maps_enabled: true, illuminance: 0.0, ..default() },
                CascadeShadowConfigBuilder { num_cascades: 4, maximum_distance: 120.0, first_cascade_far_bound: 6.0, overlap_proportion: 0.2, ..default() }.build(),
                Transform::default().looking_to(Vec3::NEG_Y, Vec3::Z),
                Sun,
            ))
            .id();
        let points = (0..MAX_POINT_LIGHTS)
            .map(|_| commands.spawn((PointLight { intensity: 0.0, shadow_maps_enabled: false, ..default() }, Transform::default(), Visibility::Hidden, PointSlot)).id())
            .collect();
        let spots = (0..MAX_SPOT_LIGHTS)
            .map(|_| commands.spawn((SpotLight { intensity: 0.0, shadow_maps_enabled: false, ..default() }, Transform::default(), Visibility::Hidden, SpotSlot)).id())
            .collect();
        let mut sphere = Sphere::new(6000.0).mesh().uv(32, 16);
        let n = sphere.count_vertices();
        sphere.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.5f32, 0.6, 0.8, 1.0]; n]);
        let sky_mesh = meshes.add(sphere);
        let sky = commands
            .spawn((
                Mesh3d(sky_mesh.clone()),
                MeshMaterial3d(mats.add(StandardMaterial { unlit: true, double_sided: true, cull_mode: None, fog_enabled: false, ..default() })),
                Transform::default(),
                Visibility::Hidden,
                bevy::camera::visibility::NoFrustumCulling,
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .id();
        // the floating item of the item selector: its own camera and light on a separate render layer, drawn over everything
        commands.spawn((
            Camera3d::default(),
            Camera { order: 2, clear_color: ClearColorConfig::None, ..default() },
            Projection::Perspective(PerspectiveProjection { fov: 45f32.to_radians(), near: 0.01, far: 10.0, ..default() }),
            Transform::default(),
            Msaa::Sample4,
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            bevy::camera::visibility::RenderLayers::layer(PREVIEW_LAYER),
        ));
        commands.spawn((
            DirectionalLight { illuminance: 2500.0, shadow_maps_enabled: false, ..default() },
            Transform::default().looking_to(Vec3::new(0.4, -0.6, -1.0), Vec3::Y),
            bevy::camera::visibility::RenderLayers::layer(PREVIEW_LAYER),
        ));
        Rig { camera, sun, points, spots, sky, sky_mesh }
    }
}

/// Spawns the scene of a loaded simulation as entities.
pub fn spawn(game: Game, rig: &Rig, commands: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>) -> Result<Level, String> {
    let mut game = game;
    let scene = game.scene.clone().ok_or("game has no scene")?;
    let t = Instant::now();
    let mut draw = scene.draw_list(DRAW_KINDS);
    let before = draw.len();
    draw.retain(|d| !is_gameplay_helper(&scene.path(d.node)));
    log::info!("{} helper / cutscene-setup nodes left out of the draw list", before - draw.len());

    let mut assets = Assets3d::new();
    let mut items: Vec<Item> = Vec::with_capacity(draw.len() + 16);
    let mut node_items: HashMap<usize, usize> = HashMap::new();
    let mut static_meshes: HashMap<(usize, usize, u8), Handle<Mesh>> = HashMap::new();
    for d in &draw {
        let want = scene.actor_outfit(d.node).unwrap_or(1);
        let item = spawn_item(commands, meshes, images, mats, &mut assets, &scene, d, want, None, None, &mut static_meshes, true, None);
        node_items.insert(d.node, items.len());
        items.push(item);
    }
    if std::env::var("BM_DEBUG_BACKDROP").is_ok() {
        for (it, d) in items.iter().zip(&draw) {
            if it.backdrop {
                let bb = d.model.computed_bbox();
                let w = d.world.translation();
                log::info!("backdrop {} meshes {} tris {} model bbox {:?}..{:?} world pos {:?} det {:.3}", scene.path(d.node).rsplit('/').next().unwrap_or(""), d.model.meshes.len(), d.model.triangle_count(), bb.min, bb.max, w, d.world.det3());
            }
        }
    }
    if std::env::var("BM_DEBUG_OSD").is_ok() {
        for (i, n) in scene.nodes.iter().enumerate() {
            let path = scene.path(i);
            if path.to_ascii_lowercase().contains("/osd.zip/") {
                let w = n.world.translation();
                log::info!("osd {} kind {:?} pos {:?} model {}", path.split("osd.zip/").last().unwrap_or(""), n.kind, w, scene.node_model(i).map(|m| format!("{} meshes {} tris bbox {:?}..{:?}", m.meshes.len(), m.triangle_count(), m.computed_bbox().min, m.computed_bbox().max)).unwrap_or_default());
            }
        }
    }
    let mut rank = 0u32;
    for (it, d) in items.iter_mut().zip(&draw) {
        if it.backdrop {
            let name = scene.nodes[d.node].name.to_ascii_lowercase();
            it.centered = !name.starts_with("landscape");
            if !it.centered {
                continue;
            }
            it.backdrop_scale = if name.starts_with("skysphere") && !name.contains("bloom") {
                1.0
            } else {
                rank += 1;
                (0.995 - 0.0004 * rank as f32).max(0.5)
            };
        }
    }
    // weapon models (held weapons, pickups): one extra hidden item per slot, positioned every frame from `RenderState::props`
    let mut slot_item: HashMap<String, usize> = HashMap::new();
    for s in game.prop_slots() {
        let Some(model) = scene.node_model(s.node) else { continue };
        let parts = (!s.parts.is_empty()).then(|| s.parts.clone());
        let mut remap: Vec<(u32, u32)> = scene.materials.stub_remap(&s.model, model.meshes.iter().map(|m| m.material())).into_iter().collect();
        remap.sort_unstable();
        let remap = (!remap.is_empty()).then_some(remap);
        let d = DrawItem { node: s.node, model, world: BmMat4::IDENTITY };
        let item = spawn_item(commands, meshes, images, mats, &mut assets, &scene, &d, 1, parts.as_ref(), remap.as_ref(), &mut HashMap::new(), false, None);
        commands.entity(item.entity).insert(Visibility::Hidden);
        slot_item.insert(s.key, items.len());
        items.push(item);
    }
    log::info!("scene spawned: {} items in {:.2}s", items.len(), t.elapsed().as_secs_f32());

    // floating items of the item selector: one hidden model per weapon / item the game knows (and the binoculars as fallback)
    let mut previews: HashMap<String, (Entity, f32)> = HashMap::new();
    let mut wanted: Vec<(String, String)> = game.world.data.weapons.values().filter(|w| !w.model.is_empty()).map(|w| (w.name.clone(), w.model.clone())).collect();
    wanted.push((String::new(), "Equip_Binoculars_01".to_string()));
    for (wname, model_name) in wanted {
        let suffix = format!("/{}/ground01", model_name.to_ascii_lowercase());
        let found = (0..scene.nodes.len()).find(|&i| {
            let p = scene.path(i).to_ascii_lowercase();
            p.ends_with(&suffix) && !p.contains("_pickup") && scene.node_model(i).is_some()
        });
        let Some(i) = found else { continue };
        let Some(model) = scene.node_model(i) else { continue };
        let bb = model.computed_bbox();
        let size = (0..3).map(|k| bb.max[k] - bb.min[k]).fold(0.0f32, f32::max).max(1.0);
        let d = DrawItem { node: i, model, world: BmMat4::IDENTITY };
        // the model's header-only stub materials stand for the real ones in front of them (like the weapon models)
        let mut remap: Vec<(u32, u32)> = scene.materials.stub_remap(&model_name, d.model.meshes.iter().map(|m| m.material())).into_iter().collect();
        remap.sort_unstable();
        let remap = (!remap.is_empty()).then_some(remap);
        let item = spawn_item(commands, meshes, images, mats, &mut assets, &scene, &d, 1, None, remap.as_ref(), &mut HashMap::new(), false, Some(PREVIEW_LAYER));
        commands.entity(item.entity).insert(Visibility::Hidden);
        previews.insert(wname, (item.entity, 14.0 / size));
    }
    let preview = previews;
    log::info!("item previews: {}", preview.len());
    let mut params = FrameParams::default();
    let start = game.world.player_pos().map(|p| [p.x, p.y + 100.0, p.z]).unwrap_or([0.0, 100.0, 0.0]);
    params.apply_scene_at(&scene, start);
    log::info!("environment: sun {:?} dir {:?} ambient {:?}/{:?} fog {:?} {}..{} sky {:?}/{:?}", params.sun_color, params.sun_dir, params.ambient_sky, params.ambient_ground, params.fog_color, params.fog_start, params.fog_end, params.sky_top, params.sky_horizon);
    commands.entity(rig.camera).insert(Transform::from_translation(convert::vec3(start)));

    Ok(Level {
        game,
        scene,
        params,
        items,
        node_items,
        subtree_items: HashMap::new(),
        slot_item,
        hidden: HashSet::new(),
        light_target: None,
        light_pos: [f32::MAX; 3],
        last_time: 0.0,
        rig: rig.clone(),
        heading: 0.0,
        preview,
        preview_on: false,
        preview_name: String::new(),
        disguise_items: HashMap::new(),
        disguise_shown: None,
    })
}

impl Level {
    /// Removes the scene's entities and darkens the shared rig.
    pub fn despawn(self, commands: &mut Commands) {
        for it in &self.items {
            commands.entity(it.entity).despawn();
        }
        commands.entity(self.rig.sun).insert(Visibility::Hidden);
        commands.entity(self.rig.sky).insert(Visibility::Hidden);
        for &e in self.rig.points.iter().chain(&self.rig.spots) {
            commands.entity(e).insert(Visibility::Hidden);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_item(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    mats: &mut Assets<StandardMaterial>,
    assets: &mut Assets3d,
    scene: &Scene,
    d: &DrawItem,
    want_outfit: u8,
    parts: Option<&Vec<u32>>,
    remap: Option<&Vec<(u32, u32)>>,
    shared: &mut HashMap<(usize, usize, u8), Handle<Mesh>>,
    share_static: bool,
    layer: Option<usize>,
) -> Item {
    let model: &Arc<Model> = &d.model;
    let outfit = model.resolve_outfit(want_outfit);
    let name = scene.nodes[d.node].name.to_ascii_lowercase();
    let hidden_coli = name.starts_with("worldcoli");
    let root = commands.spawn((Transform::from_matrix(Mat4::IDENTITY), Visibility::Inherited)).id();
    let backdrop = bm_render::gpu_scene::is_sky_node(scene, d.node);
    if let Some(l) = layer {
        commands.entity(root).insert(bevy::camera::visibility::RenderLayers::layer(l));
    }
    let mut item = Item { entity: root, dome: Vec::new(), backdrop, backdrop_scale: 1.0, centered: false, base: d.world, skin: Vec::new(), shown_outfit: outfit };
    // the hero keeps every outfit as a hidden variant: disguises switch between them
    let all_outfits = scene.nodes[d.node].type_id == 0x2003e8;
    commands.entity(root).insert(convert::transform(&d.world));
    if hidden_coli {
        return item;
    }
    for (mi, mesh) in model.meshes.iter().enumerate() {
        let skip = mesh.layout == VertexLayout::PosColor10
            || mesh.indices.is_empty()
            || mesh.vertices.is_empty()
            || !mesh.is_lod0()
            || parts.is_some_and(|p| mesh.part != 0 && !p.contains(&mesh.part))
            || (mesh.is_skinned() && !all_outfits && !mesh.in_outfit(outfit));
        if skip {
            continue;
        }
        let mid = mesh.material();
        let mid = remap.and_then(|r| r.iter().find(|p| p.0 == mid)).map_or(mid, |p| p.1);
        // The Landscape_* pieces (cliff walls, rock faces, jungle slopes) reference header-only stub instances; the engine uses
        // the instance in front of the stub for them. Everywhere else a stub is a blank overlay and stays undrawn.
        let mid = if remap.is_none() && name.starts_with("landscape") { scene.materials.stub_remap("", [mid]).get(&mid).copied().unwrap_or(mid) } else { mid };
        let Some(mut material) = assets.material(scene, images, mats, mid, mesh.is_skinned()) else { continue };
        if backdrop {
            if let Some(m) = mats.get(&material) {
                let mut m = m.clone();
                m.unlit = true;
                m.fog_enabled = false;
                material = mats.add(m);
            }
        }
        let skinned = mesh.is_skinned();
        // the untextured sky dome carries its colour in the vertices and is drawn with the game's sky gain
        let gain = if backdrop && scene.materials.desc(mid).is_some_and(|d| d.diffuse_texture().is_none_or(|t| t == 0)) { 1.6 } else { 1.0 };
        let is_dome = gain > 1.0;
        let handle = if skinned || !share_static || is_dome {
            meshes.add(convert::build_mesh(mesh, skinned || is_dome, gain))
        } else {
            let key = (Arc::as_ptr(model) as usize, mi, 0u8);
            shared.entry(key).or_insert_with(|| meshes.add(convert::build_mesh(mesh, false, gain))).clone()
        };
        let mut child = commands.spawn((Mesh3d(handle.clone()), MeshMaterial3d(material), Transform::default(), Visibility::Inherited));
        if backdrop {
            // sky domes and distant layers must not shade the level
            child.insert((bevy::light::NotShadowCaster, bevy::light::NotShadowReceiver));
        }
        if is_dome {
            if std::env::var("BM_DEBUG_BACKDROP").is_ok() {
                let v = &mesh.vertices;
                let mut rows: Vec<(f32, [u8; 4])> = v.positions.iter().zip(&v.colors).map(|(p, c)| (p[1] / (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1.0), *c)).collect();
                rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                for i in (0..rows.len()).step_by((rows.len() / 12).max(1)) {
                    log::info!("dome elevation {:.2} color {:?}", rows[i].0, rows[i].1);
                }
            }
            let n = mesh.vertices.colors.len().max(1) as f32;
            let avg = mesh.vertices.colors.iter().map(|c| (c[0] as f32 + c[1] as f32 + c[2] as f32) / 765.0).sum::<f32>() / n;
            item.dome.push(Dome { handle: handle.clone(), pos: mesh.vertices.positions.clone(), colors: mesh.vertices.colors.clone(), overcast: avg < 0.45 });
        }
        let child_id = child.id();
        if skinned {
            // the pose moves vertices outside the bind-pose bounds
            child.insert(bevy::camera::visibility::NoFrustumCulling);
            if all_outfits && !mesh.in_outfit(outfit) {
                child.insert(Visibility::Hidden);
            }
            item.skin.push(SkinMesh { entity: child_id, outfit: mesh.variant_mask(), handle, src: model.clone(), mesh_index: mi });
        }
        if let Some(l) = layer {
            child.insert(bevy::camera::visibility::RenderLayers::layer(l));
        }
        let child = child.id();
        commands.entity(root).add_child(child);
    }
    item
}

impl Level {
    fn items_of(&mut self, node: usize) -> Vec<usize> {
        if let Some(v) = self.subtree_items.get(&node) {
            return v.clone();
        }
        let v: Vec<usize> = self.scene.subtree(node).into_iter().filter_map(|n| self.node_items.get(&n).copied()).collect();
        self.subtree_items.insert(node, v.clone());
        v
    }

    /// Spawns (once) the model the player wears when disguised, as named by the render state.
    pub fn ensure_disguise(&mut self, rs: &RenderState, commands: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>) {
        let Some(key) = rs.entities.iter().find(|e| e.kind == bm_game::Kind::Player).and_then(|e| e.disguise) else { return };
        if self.disguise_items.contains_key(&key) {
            return;
        }
        let Some(model) = self.scene.node_model(key.0) else { return };
        let d = DrawItem { node: key.0, model, world: self.scene.nodes[key.0].world };
        let mut assets = Assets3d::new();
        let item = spawn_item(commands, meshes, images, mats, &mut assets, &self.scene, &d, key.1, None, None, &mut HashMap::new(), false, None);
        commands.entity(item.entity).insert(Visibility::Hidden);
        self.disguise_items.insert(key, self.items.len());
        self.items.push(item);
    }

    /// Picks the sun / ambient / fog of the zone around `focus` and moves the frame parameters towards it (`dt` = 0 snaps).
    fn update_lighting(&mut self, focus: [f32; 3], dt: f32) {
        let moved = (0..3).map(|k| (focus[k] - self.light_pos[k]).abs()).fold(0.0f32, f32::max);
        if self.light_target.is_none() || moved > 25.0 {
            let mut t = self.params.clone();
            t.apply_scene_at(&self.scene, focus);
            self.light_target = Some(t);
            self.light_pos = focus;
        }
        let Some(t) = &self.light_target else { return };
        let k = if dt <= 0.0 { 1.0 } else { 1.0 - (-dt * 5.0).exp() };
        let mix3 = |a: &mut [f32; 3], b: [f32; 3]| {
            for i in 0..3 {
                a[i] += (b[i] - a[i]) * k;
            }
        };
        let p = &mut self.params;
        mix3(&mut p.sun_color, t.sun_color);
        mix3(&mut p.ambient_sky, t.ambient_sky);
        mix3(&mut p.ambient_ground, t.ambient_ground);
        mix3(&mut p.fog_color, t.fog_color);
        mix3(&mut p.sky_top, t.sky_top);
        mix3(&mut p.sky_horizon, t.sky_horizon);
        p.fog_start += (t.fog_start - p.fog_start) * k;
        p.fog_end += (t.fog_end - p.fog_end) * k;
        if t.sun_color.iter().any(|&c| c > 0.02) {
            mix3(&mut p.sun_dir, t.sun_dir);
        }
    }
}

/// Everything the per-frame system writes to.
#[derive(bevy::ecs::system::SystemParam)]
pub struct Targets<'w, 's> {
    pub(crate) xforms: Query<'w, 's, (&'static mut Transform, &'static mut Visibility), Without<crate::ui_render::UiQuad>>,
    projections: Query<'w, 's, &'static mut Projection, With<MainCamera>>,
    fog: Query<'w, 's, &'static mut DistanceFog, With<MainCamera>>,
    sun: Query<'w, 's, &'static mut DirectionalLight, With<Sun>>,
    points: Query<'w, 's, &'static mut PointLight>,
    spots: Query<'w, 's, &'static mut SpotLight>,
    ambient: ResMut<'w, GlobalAmbientLight>,
    clear: ResMut<'w, ClearColor>,
    pub(crate) meshes: ResMut<'w, Assets<Mesh>>,
}

/// Game colours are display-space (sRGB) values; Bevy lights and fogs in linear space.
fn lin(c: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| if v <= 0.04045 { v.max(0.0) / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    [f(c[0]), f(c[1]), f(c[2])]
}

fn lum(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Copies the simulation's presentation state into the ECS.
pub fn sync(level: &mut Level, rs: &RenderState, t: &mut Targets) {
    // moving / posed scene objects
    for e in &rs.entities {
        let Some(node) = e.node else { continue };
        let mut items = level.items_of(node);
        if items.is_empty() {
            continue;
        }
        let base = level.scene.nodes[node].world;
        let delta = e.world.mul(&base.inverse_affine());
        // somebody else's clothes: the donor's model is drawn instead of the hero's
        let worn = e.disguise.and_then(|k| level.disguise_items.get(&k).copied());
        if e.kind == bm_game::Kind::Player {
            if let Some(prev) = level.disguise_shown.filter(|p| Some(*p) != worn) {
                if let Ok((_, mut v)) = t.xforms.get_mut(level.items[prev].entity) {
                    *v = Visibility::Hidden;
                }
            }
            level.disguise_shown = worn;
            if let Some(w) = worn {
                for &i in &items {
                    if let Ok((_, mut v)) = t.xforms.get_mut(level.items[i].entity) {
                        *v = Visibility::Hidden;
                    }
                }
                items = vec![w];
            }
        }
        for &i in &items {
            let it = &level.items[i];
            if let Ok((mut tr, mut vis)) = t.xforms.get_mut(it.entity) {
                *tr = if worn == Some(i) { convert::transform(&e.world) } else { convert::transform(&delta.mul(&it.base)) };
                *vis = if e.visible { Visibility::Inherited } else { Visibility::Hidden };
            }
            if e.kind == bm_game::Kind::Player && worn.is_none() && level.items[i].shown_outfit != e.outfit {
                level.items[i].shown_outfit = e.outfit;
                for s in &level.items[i].skin {
                    if let Ok((_, mut v)) = t.xforms.get_mut(s.entity) {
                        *v = if s.outfit == 0 || s.outfit == e.outfit { Visibility::Inherited } else { Visibility::Hidden };
                    }
                }
            }
            if let Some(sk) = &e.skinning {
                let shown = level.items[i].shown_outfit;
                let all = e.kind == bm_game::Kind::Player && worn.is_none();
                for s in &level.items[i].skin {
                    if all && s.outfit != 0 && s.outfit != shown {
                        continue;
                    }
                    if let Some(mut mesh) = t.meshes.get_mut(&s.handle) {
                        convert::skin_into(&mut *mesh, &s.src.meshes[s.mesh_index], sk);
                    }
                }
            }
        }
    }
    // picked-up items and friends
    let now: HashSet<usize> = rs.hidden_nodes.iter().filter_map(|n| level.node_items.get(n).copied()).collect();
    for &i in level.hidden.difference(&now) {
        if let Ok((_, mut v)) = t.xforms.get_mut(level.items[i].entity) {
            *v = Visibility::Inherited;
        }
    }
    for &i in now.difference(&level.hidden) {
        if let Ok((_, mut v)) = t.xforms.get_mut(level.items[i].entity) {
            *v = Visibility::Hidden;
        }
    }
    level.hidden = now;

    // weapon models: the ones named this frame are shown at their matrix, the rest are hidden
    let drawn: HashMap<&str, &BmMat4> = rs.props.iter().map(|p| (p.key.as_str(), &p.world)).collect();
    for (key, &i) in &level.slot_item {
        if let Ok((mut tr, mut v)) = t.xforms.get_mut(level.items[i].entity) {
            match drawn.get(key.as_str()) {
                Some(w) => {
                    *tr = convert::transform(w);
                    *v = Visibility::Inherited;
                }
                None => *v = Visibility::Hidden,
            }
        }
    }

    // camera
    if let Some(c) = &rs.camera {
        if let Ok((mut tr, _)) = t.xforms.get_mut(level.rig.camera) {
            let fwd = convert::dir3(c.forward.to_array());
            level.heading = c.forward.x.atan2(c.forward.z).to_degrees().rem_euclid(360.0);
            *tr = Transform::from_translation(convert::vec3(c.pos.to_array())).looking_to(fwd, Vec3::Y);
        }
        if let Ok(mut p) = t.projections.single_mut() {
            *p = Projection::Perspective(PerspectiveProjection {
                fov: c.fov_deg.to_radians(),
                near: (c.near.max(5.0)) * S,
                far: 20000.0,
                ..default()
            });
        }
    }

    // floating item of the item selector: upper left of the screen, turning slowly
    let shown = if level.preview.contains_key(&level.preview_name) { level.preview_name.clone() } else { String::new() };
    for (name, (e, k)) in &level.preview {
        if let Ok((mut tr, mut v)) = t.xforms.get_mut(*e) {
            let on = level.preview_on && *name == shown;
            *v = if on { Visibility::Inherited } else { Visibility::Hidden };
            if on {
                let spin = Quat::from_rotation_y(rs.time as f32 * 0.9) * Quat::from_rotation_x(-0.35);
                *tr = Transform { translation: Vec3::new(-0.194, 0.087, -0.6), rotation: spin, scale: Vec3::splat(*k) };
            }
        }
    }
    // backdrops stay centred on the camera
    if let Some(c) = &rs.camera {
        let cp = convert::vec3(c.pos.to_array());
        for it in level.items.iter().filter(|i| i.backdrop && i.centered) {
            if let Ok((mut tr, _)) = t.xforms.get_mut(it.entity) {
                let mut x = convert::transform(&it.base);
                x.translation.x += cp.x;
                x.translation.z += cp.z;
                x.translation = cp + (x.translation - cp) * it.backdrop_scale;
                x.scale *= it.backdrop_scale;
                *tr = x;
            }
        }
    }

    // environment
    let dt = (rs.time - level.last_time).clamp(0.0, 0.25) as f32;
    level.last_time = rs.time;
    let cam_pos = rs.camera.as_ref().map(|c| c.pos.to_array()).unwrap_or([0.0; 3]);
    let focus = level.game.world.player_pos().map(|p| [p.x, p.y + 100.0, p.z]).unwrap_or(cam_pos);
    level.update_lighting(focus, dt);
    let p = &level.params;
    let (sun_c, amb_c, fog_c, hor_c, top_c) = (lin(p.sun_color), lin(p.ambient_sky), lin(p.fog_color), lin(p.sky_horizon), lin(p.sky_top));
    if let Ok(mut l) = t.sun.single_mut() {
        let sun_l = lum(sun_c);
        l.illuminance = env_f32("BM_SUN", 4_500.0) * sun_l;
        let m = sun_l.max(1e-4);
        l.color = Color::linear_rgb(sun_c[0] / m, sun_c[1] / m, sun_c[2] / m);
        l.shadow_maps_enabled = sun_l > 0.02;
        if let Ok((mut tr, mut v)) = t.xforms.get_mut(level.rig.sun) {
            *v = Visibility::Inherited;
            *tr = Transform::default().looking_to(-convert::dir3(p.sun_dir), Vec3::Y);
        }
    }
    // The game multiplies albedo by ambient in display space; in linear space that is albedo * decode(ambient). Bevy scales
    // ambient radiance by the exposure (1 / 997 at the default EV100 9.7), so the brightness is the inverse of that.
    let gnd_c = lin(p.ambient_ground);
    let amb_mix = [amb_c[0] * 0.6 + gnd_c[0] * 0.4, amb_c[1] * 0.6 + gnd_c[1] * 0.4, amb_c[2] * 0.6 + gnd_c[2] * 0.4];
    t.ambient.color = Color::linear_rgb(amb_mix[0], amb_mix[1], amb_mix[2]);
    t.ambient.brightness = 997.0 * env_f32("BM_AMB", 1.0);
    t.clear.0 = Color::linear_rgb(hor_c[0], hor_c[1], hor_c[2]);
    if let Ok(mut f) = t.fog.single_mut() {
        f.color = Color::linear_rgb(fog_c[0], fog_c[1], fog_c[2]);
        f.falloff = if p.fog_end > p.fog_start {
            FogFalloff::Linear { start: p.fog_start * S, end: p.fog_end * S }
        } else {
            FogFalloff::Linear { start: 1.0e6, end: 2.0e6 }
        };
    }
    // gradient sky behind the backdrop layers: horizon colour at and below the horizon, zenith colour overhead
    if let Some(mut m) = t.meshes.get_mut(&level.rig.sky_mesh) {
        if let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) = m.attribute(Mesh::ATTRIBUTE_POSITION).cloned() {
            let cols: Vec<[f32; 4]> = pos
                .iter()
                .map(|v| {
                    let e = (v[1] / 6000.0).clamp(-1.0, 1.0);
                    let k = (e / 0.65).clamp(0.0, 1.0);
                    let k = k * k * (3.0 - 2.0 * k);
                    [hor_c[0] + (top_c[0] - hor_c[0]) * k, hor_c[1] + (top_c[1] - hor_c[1]) * k, hor_c[2] + (top_c[2] - hor_c[2]) * k, 1.0]
                })
                .collect();
            m.insert_attribute(Mesh::ATTRIBUTE_COLOR, cols);
        }
    }
    if let (Some(c), Ok((mut tr, mut v))) = (&rs.camera, t.xforms.get_mut(level.rig.sky)) {
        *tr = Transform::from_translation(convert::vec3(c.pos.to_array()));
        *v = Visibility::Inherited;
    }

    // sky dome: blend into the fog colour towards and below the horizon (the original's CameraBackgroundColor haze)
    for it in level.items.iter().filter(|i| !i.dome.is_empty()) {
        for d in &it.dome {
            if d.colors.len() != d.pos.len() {
                continue;
            }
            let cols: Vec<[f32; 4]> = d
                .pos
                .iter()
                .zip(&d.colors)
                .map(|(v, c)| {
                    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-3);
                    let e = v[1] / len;
                    let x = (e / 0.22).clamp(0.0, 1.0);
                    let haze = (1.0 - x * x * (3.0 - 2.0 * x)) * 0.9;
                    let base = if d.overcast {
                        // a touch of the authored gradient on top of the cream haze colour
                        let g = 0.92 + 0.08 * (c[0] as f32 / 255.0);
                        [fog_c[0] * g, fog_c[1] * g, fog_c[2] * g]
                    } else {
                        [convert::srgb_to_linear(c[0], 1.6), convert::srgb_to_linear(c[1], 1.6), convert::srgb_to_linear(c[2], 1.6)]
                    };
                    [base[0] + (fog_c[0] - base[0]) * haze, base[1] + (fog_c[1] - base[1]) * haze, base[2] + (fog_c[2] - base[2]) * haze, c[3] as f32 / 255.0]
                })
                .collect();
            if let Some(mut m) = t.meshes.get_mut(&d.handle) {
                m.insert_attribute(Mesh::ATTRIBUTE_COLOR, cols);
            }
        }
    }

    // dynamic lights: fixed pools of entities, unused slots are hidden
    let (points, spots): (Vec<&bm_game::state::RenderLight>, Vec<&bm_game::state::RenderLight>) = rs.lights.iter().partition(|l| !(l.spot && l.cone.is_some()));
    let lumens = |l: &bm_game::state::RenderLight, range: f32| {
        lum([l.color[0] * l.intensity, l.color[1] * l.intensity, l.color[2] * l.intensity]).max(0.0) * 4.0 * std::f32::consts::PI * (range * 0.5).powi(2) * env_f32("BM_LIGHT", 1500.0)
    };
    let tint = |l: &bm_game::state::RenderLight| {
        let m = l.color[0].max(l.color[1]).max(l.color[2]).max(1e-4);
        Color::linear_rgb(l.color[0] / m, l.color[1] / m, l.color[2] / m)
    };
    for (i, &e) in level.rig.points.iter().enumerate() {
        let l = points.get(i);
        if let Ok((mut tr, mut v)) = t.xforms.get_mut(e) {
            *v = if l.is_some() { Visibility::Inherited } else { Visibility::Hidden };
            if let Some(l) = l {
                *tr = Transform::from_translation(convert::vec3(l.position));
            }
        }
        if let (Some(l), Ok(mut pl)) = (l, t.points.get_mut(e)) {
            let range = l.range.max(1.0) * S;
            pl.intensity = lumens(l, range);
            pl.range = range;
            pl.radius = 0.05;
            pl.color = tint(l);
        }
    }
    for (i, &e) in level.rig.spots.iter().enumerate() {
        let l = spots.get(i);
        if let Ok((mut tr, mut v)) = t.xforms.get_mut(e) {
            *v = if l.is_some() { Visibility::Inherited } else { Visibility::Hidden };
            if let Some(l) = l {
                *tr = Transform::from_translation(convert::vec3(l.position)).looking_to(convert::dir3(l.direction), Vec3::Y);
            }
        }
        if let (Some(l), Ok(mut sl)) = (l, t.spots.get_mut(e)) {
            let range = l.range.max(1.0) * S;
            let (outer, inner) = l.cone.unwrap_or((0.5, 0.8));
            sl.intensity = lumens(l, range);
            sl.range = range;
            sl.outer_angle = outer.clamp(-1.0, 1.0).acos().min(std::f32::consts::FRAC_PI_2 - 0.01);
            sl.inner_angle = inner.clamp(-1.0, 1.0).acos().min(sl.outer_angle);
            sl.color = tint(l);
        }
    }
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

trait TonemapPick {
    fn pick(name: Option<&str>) -> Self;
}

impl TonemapPick for bevy::core_pipeline::tonemapping::Tonemapping {
    fn pick(name: Option<&str>) -> Self {
        use bevy::core_pipeline::tonemapping::Tonemapping::*;
        match name {
            Some("tony") => TonyMcMapface,
            Some("agx") => AgX,
            Some("reinhard") => ReinhardLuminance,
            _ => None,
        }
    }
}
