//! A playable level on the GPU: the [`Game`] simulation plus the uploaded scene, and the per-frame glue that copies
//! [`RenderState`] (entity transforms, skinning poses, hidden nodes, lights, particles) into `bm-render`.

use crate::config::{Audio, Config, Mounted};
use bm_game::{Game, LoadOptions, RenderState};
use bm_models::scene::{NodeKind, Scene};
use bm_models::Mat4;
use bm_particles::render::ParticleHandle;
use bm_render::{FrameParams, PointLight, Renderer, SceneGpu, UploadOptions};
use bm_script::{RuntimeConfig, ScriptRuntime};
use bm_sound::mixer::Bus;
use bm_sound::{AudioEngine, MixerEngine};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Node kinds drawn by the renderer.
pub const DRAW_KINDS: &[NodeKind] = &[NodeKind::Static, NodeKind::Scatter, NodeKind::Actor, NodeKind::ClothBundle];

/// Sound output: the shared mixer and (when available) the cpal stream pulling from it.
pub struct AudioOut {
    pub engine: MixerEngine,
    /// A real output device is open (false = silent headless mixer).
    #[allow(dead_code)]
    pub device: bool,
    #[cfg(feature = "audio")]
    _stream: Option<bm_sound::device::DeviceOutput>,
}

impl AudioOut {
    /// Opens the default output device; falls back to a silent headless mixer (and says so in the log).
    pub fn open(cfg: &Audio) -> AudioOut {
        #[cfg(feature = "audio")]
        if cfg.enabled {
            match bm_sound::device::DeviceOutput::default_rate() {
                Ok(rate) => {
                    let engine = MixerEngine::headless(rate);
                    match bm_sound::device::DeviceOutput::start(engine.mixer()) {
                        Ok(out) => {
                            log::info!("audio device open: {} Hz, {} ch", out.sample_rate, out.channels);
                            let mut a = AudioOut { engine, device: true, _stream: Some(out) };
                            a.apply(cfg);
                            return a;
                        }
                        Err(e) => log::warn!("audio device failed ({e}); running silent"),
                    }
                }
                Err(e) => log::warn!("no audio device ({e}); running silent"),
            }
        }
        let mut a = AudioOut {
            engine: MixerEngine::headless(48000),
            device: false,
            #[cfg(feature = "audio")]
            _stream: None,
        };
        a.apply(cfg);
        a
    }

    pub fn engine_stop_all(&mut self) {
        self.engine.stop_all(None);
    }

    pub fn apply(&mut self, cfg: &Audio) {
        let e = &mut self.engine;
        e.set_master_volume(cfg.master);
        e.set_bus_volume(Bus::Sfx, cfg.sfx);
        e.set_bus_volume(Bus::Voice, cfg.voice);
        e.set_bus_volume(Bus::Ambient, cfg.ambient);
        e.set_bus_volume(Bus::Music, cfg.music_volume);
    }
}

/// Everything that can go wrong while loading a level, as one printable message.
pub fn load_game(m: &Mounted, cfg: &Config, scene: &str, audio: Option<&AudioOut>) -> Result<Game, String> {
    let t = Instant::now();
    let opts = LoadOptions { seed: cfg.game.seed, ..Default::default() };
    let mut game = Game::load(m.vfs.clone(), scene, &opts).map_err(|e| format!("loading {scene}: {e}"))?;
    log::info!("level {scene} simulated-loaded in {:.2}s ({} NPCs)", t.elapsed().as_secs_f32(), game.stats.npcs);
    if let Some(a) = audio {
        let mut engine = a.engine.clone();
        engine.stop_all(None);
        if let Err(e) = game.enable_audio(Box::new(engine), cfg.audio.music) {
            log::warn!("audio disabled for this level: {e}");
        }
    }
    attach_mods(&mut game, m);
    Ok(game)
}

/// Builds the script runtime on the game's host, loads the resolved mods and applies data patches.
pub fn attach_mods(game: &mut Game, m: &Mounted) {
    if m.set.order.is_empty() {
        return;
    }
    let mut rt = ScriptRuntime::new(game.host(), RuntimeConfig { save_dir: Some(m.dirs.save_dir.clone()), ..Default::default() });
    rt.data_mut().set_base("weapons", game.world.data.weapons_json());
    let (data, reports) = rt.load_mods(&m.set);
    for e in &data.errors {
        log::warn!("mod data {}: {}: {}", e.mod_id, e.file, e.message);
    }
    for r in &reports {
        if !r.ok {
            log::warn!("mod script load failed: {r:?}");
        }
    }
    if let Some(w) = rt.data().get("weapons").cloned() {
        game.world.data.apply_weapons_json(&w);
    }
    log::info!("{} mod(s) active", m.set.order.len());
    game.attach_scripts(rt);
}

/// Scene nodes that exist for the native game logic and are never visible in the shipped game: the hero's cutscene
/// setups (a posed duplicate of Agent 47 lying in his own blood pool, camera targets ...) and collision proxies.
fn is_gameplay_helper(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("herocutscenes") || p.contains("/coliplanes/") || (p.contains("herosetup") && !p.ends_with("/hero/ground"))
}

pub struct LevelView {
    pub game: Game,
    pub scene: Arc<Scene>,
    pub gpu: SceneGpu,
    pub params: FrameParams,
    pub camera: bm_render::Camera,
    #[allow(dead_code)]
    pub upload_secs: f32,
    node_item: HashMap<usize, usize>,
    slot_item: HashMap<String, usize>,
    item_base: Vec<Mat4>,
    subtree_items: HashMap<usize, Vec<usize>>,
    hidden: HashSet<usize>,
    particles: ParticleHandle,
    fov_scale: f32,
    logged: bool,
    /// Target lighting (sun / ambient / fog of the zone the player is in); `params` follows it smoothly.
    light_target: Option<FrameParams>,
    last_time: f64,
    /// Position the zone lookup used the last time (re-evaluated when it moved by more than a few cm).
    light_pos: [f32; 3],
}

impl LevelView {
    /// Uploads the game's scene (all static, actor and cloth nodes) and registers the particle sprites.
    pub fn new(renderer: &mut Renderer, particles: &ParticleHandle, mut game: Game, opts: &UploadOptions, video: &crate::config::Video) -> Result<LevelView, String> {
        let scene = game.scene.clone().ok_or("game has no scene")?;
        let t = Instant::now();
        let mut items = scene.draw_list(DRAW_KINDS);
        let before = items.len();
        items.retain(|d| !is_gameplay_helper(&scene.path(d.node)));
        log::info!("{} helper / cutscene-setup nodes left out of the draw list", before - items.len());
        let node_item: HashMap<usize, usize> = items.iter().enumerate().map(|(i, d)| (d.node, i)).collect();
        let mut item_base: Vec<Mat4> = items.iter().map(|d| d.world).collect();
        // weapon models (held weapons, pickups): one extra item per slot, positioned every frame from `RenderState::props`
        let mut slot_item: HashMap<String, usize> = HashMap::new();
        let mut opts = opts.clone();
        for s in game.prop_slots() {
            if let Some(model) = scene.node_model(s.node) {
                if !s.parts.is_empty() {
                    opts.part_filter_by_node.insert(s.node, s.parts);
                }
                let mut remap: Vec<(u32, u32)> = scene.materials.stub_remap(&s.model, model.meshes.iter().map(|m| m.material())).into_iter().collect();
                remap.sort_unstable();
                if !remap.is_empty() {
                    opts.material_remap_by_node.insert(s.node, remap);
                }
                slot_item.insert(s.key, items.len());
                item_base.push(Mat4::IDENTITY);
                items.push(bm_models::scene::DrawItem { node: s.node, model, world: Mat4::IDENTITY });
            }
        }
        let mut gpu = SceneGpu::upload(renderer, &scene, &items, &opts);
        for &i in slot_item.values() {
            gpu.set_visible(i, false);
        }
        let upload_secs = t.elapsed().as_secs_f32();
        log::info!("scene uploaded: {} items in {:.2}s {:?}", items.len(), upload_secs, gpu.stats);
        if log::log_enabled!(log::Level::Debug) {
            if let Some(pp) = game.world.player_pos() {
                for d in &items {
                    let t = d.world.translation();
                    let dist = ((t[0] - pp.x).powi(2) + (t[2] - pp.z).powi(2)).sqrt();
                    if dist < 120.0 && (t[1] - pp.y).abs() < 300.0 {
                        log::debug!("item near start: node {} {} kind {:?} skinned {}", d.node, scene.path(d.node), scene.nodes[d.node].kind, d.model.skeleton.is_some());
                    }
                }
            }
        }
        if let Some(fx) = &game.fx {
            particles.register_images(&fx.templates.images);
        }
        let mut params = FrameParams::default();
        video.apply(&mut params);
        // lighting of the player's start position right away (fog, sun, ambient from the scene data)
        let start = game.world.player_pos().map(|p| [p.x, p.y + 100.0, p.z]).unwrap_or([0.0, 100.0, 0.0]);
        params.apply_scene_at(&scene, start);
        Ok(LevelView {
            game,
            scene,
            gpu,
            params,
            camera: bm_render::Camera { near: 8.0, ..Default::default() },
            upload_secs,
            node_item,
            slot_item,
            item_base,
            subtree_items: HashMap::new(),
            hidden: HashSet::new(),
            particles: particles.clone(),
            fov_scale: 1.0,
            logged: false,
            light_target: None,
            last_time: 0.0,
            light_pos: [f32::MAX; 3],
        })
    }

    /// Picks the sun / ambient / fog of the zone around `focus` and moves the frame parameters towards it
    /// (`dt` = 0 snaps). Lights, shadows and post settings in `params` are left alone.
    pub fn update_lighting(&mut self, focus: [f32; 3], dt: f32) {
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

    fn items_of(&mut self, node: usize) -> Vec<usize> {
        if let Some(v) = self.subtree_items.get(&node) {
            return v.clone();
        }
        let v: Vec<usize> = self.scene.subtree(node).into_iter().filter_map(|n| self.node_item.get(&n).copied()).collect();
        self.subtree_items.insert(node, v.clone());
        v
    }

    /// Copies the simulation's presentation state into the GPU scene, camera and lights.
    pub fn sync(&mut self, rs: &RenderState) {
        for e in &rs.entities {
            let Some(node) = e.node else { continue };
            let items = self.items_of(node);
            if items.is_empty() {
                continue;
            }
            let base = self.scene.nodes[node].world;
            let delta = e.world.mul(&base.inverse_affine());
            if !self.logged {
                let d = delta.translation();
                log::debug!(
                    "entity {} ({:?}) node {} '{}' items {} delta ({:.0},{:.0},{:.0}) skinned {}",
                    e.name, e.kind, node, self.scene.nodes[node].name, items.len(), d[0], d[1], d[2], e.skinning.is_some()
                );
            }
            for &i in &items {
                self.gpu.set_world(i, delta.mul(&self.item_base[i]));
                self.gpu.set_visible(i, e.visible);
                if let Some(sk) = &e.skinning {
                    let ok = self.gpu.set_skinning(i, sk);
                    if !self.logged && ok {
                        let dev = sk.iter().map(|m| m.0.iter().flatten().zip(Mat4::IDENTITY.0.iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max)).fold(0.0f32, f32::max);
                        log::debug!("  skinning uploaded: {} bones, max deviation from identity {:.3}", sk.len(), dev);
                    }
                }
            }
        }
        // picked-up items and friends
        let mut now: HashSet<usize> = HashSet::new();
        for &n in &rs.hidden_nodes {
            if let Some(&i) = self.node_item.get(&n) {
                now.insert(i);
            }
        }
        for &i in self.hidden.difference(&now) {
            self.gpu.set_visible(i, true);
        }
        for &i in now.difference(&self.hidden) {
            self.gpu.set_visible(i, false);
        }
        self.hidden = now;

        // weapon models: the ones named this frame are shown at their matrix, the rest are hidden
        if !self.logged {
            for p in &rs.props {
                let t = p.world.translation();
                log::debug!("prop {} slot {:?} at ({:.0},{:.0},{:.0})", p.key, self.slot_item.get(&p.key), t[0], t[1], t[2]);
            }
            log::debug!("{} prop slots, {} drawn", self.slot_item.len(), rs.props.len());
        }
        let drawn: HashMap<&str, &Mat4> = rs.props.iter().map(|p| (p.key.as_str(), &p.world)).collect();
        for (key, &i) in &self.slot_item {
            match drawn.get(key.as_str()) {
                Some(w) => {
                    self.gpu.set_world(i, **w);
                    self.gpu.set_visible(i, true);
                }
                None => self.gpu.set_visible(i, false),
            }
        }

        if let Some(c) = &rs.camera {
            self.camera.position = c.pos.to_array();
            let f = c.forward.to_array();
            self.camera.yaw = f[0].atan2(f[2]);
            self.camera.pitch = f[1].clamp(-1.0, 1.0).asin();
            self.camera.fov_y = (c.fov_deg * self.fov_scale).to_radians();
            self.camera.near = c.near.max(5.0);
        }
        self.params.lights = rs
            .lights
            .iter()
            .map(|l| PointLight {
                position: l.position,
                radius: l.range.max(1.0),
                color: [l.color[0] * l.intensity, l.color[1] * l.intensity, l.color[2] * l.intensity],
                range_start: l.range_start.min(l.range),
                direction: l.direction,
                cone: if l.spot { l.cone } else { None },
            })
            .collect();
        self.params.time = rs.time as f32;
        let dt = (rs.time - self.last_time).clamp(0.0, 0.25) as f32;
        self.last_time = rs.time;
        let focus = self.game.world.player_pos().map(|p| [p.x, p.y + 100.0, p.z]).unwrap_or(self.camera.position);
        self.update_lighting(focus, dt);
        self.particles.submit_list(rs.particles.clone());
        self.logged = true;
    }
}
