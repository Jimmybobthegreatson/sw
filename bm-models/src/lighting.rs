//! Decoded lighting data of a scene: lights, environment (sun / ambient) volumes, zones, fog and the backdrop sun.
//!
//! Everything here is plain data derived from the shipped files (formats in `bm_formats::light` and `docs/formats.md`):
//!
//! * **Lights** (`ZOMNILIGHT`, `ZSPOTLIGHT`, `ZSPOTLIGHTSQUARE`, omni clones): record in a PRM chunk (colour, intensity, linear
//!   range start/end, spot cone), direction = node +Z. **H** for the field layout, **M** for the exact falloff curve.
//! * **Environments** (`ZENVIRONMENT`): directional light + ambient colours of a room or of the outdoor area, with the
//!   `*_env_character*` twins used for characters. The sun of an outdoor level is the environment that lists static shadow
//!   meshes. **M** (field meaning inferred from values / names; the sun direction agrees with the backdrop sun sprite).
//! * **Zones**: `InsideBound*` boxes; the environment of a point is the one under the location (room) of the smallest box that
//!   contains it, else the one next to the location group (outdoors). **M**.
//! * **Fog**: `CameraFogBegin` / `CameraFogFull` are fractions of `CameraFar`, the colour is `CameraBackgroundColor`
//!   (`0xRRGGBB`). **M** (begin/full semantics fit every level, colour byte order is a guess).

use std::collections::HashMap;

use bm_formats::light::{self, Cone, LightRecord};
use bm_formats::prp::{DefValue, Value};

use crate::math::Mat4;
use crate::scene::SceneNodeInfo;
use bm_formats::prm::Prm;
use bm_formats::scene::SceneGraph;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightShape {
    Omni,
    Spot,
    SpotSquare,
}

/// A point or spot light.
#[derive(Clone, Debug)]
pub struct SceneLight {
    pub node: usize,
    pub name: String,
    pub shape: LightShape,
    /// World position (cm).
    pub position: [f32; 3],
    /// World direction of the cone axis (unit; omni lights carry it too but do not use it).
    pub direction: [f32; 3],
    /// Display space RGB 0..1 (not multiplied by `intensity`).
    pub color: [f32; 3],
    pub intensity: f32,
    pub range_start: f32,
    pub range_end: f32,
    pub cone: Option<Cone>,
    /// Square spots: extra aspect / scale value.
    pub aspect: Option<f32>,
    /// Texture id projected by window projector spots.
    pub projection_texture: u32,
    pub color2: Option<[f32; 3]>,
    pub flags: u16,
    /// PRM chunk of the record.
    pub chunk: u32,
}

impl SceneLight {
    /// Colour times intensity.
    pub fn radiance(&self) -> [f32; 3] {
        [self.color[0] * self.intensity, self.color[1] * self.intensity, self.color[2] * self.intensity]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvKind {
    /// Outdoor sun: directional light + ambient, with static shadow meshes.
    Sun,
    /// Directional / hemispheric ambient of a room or character volume.
    Ambient,
}

/// A `ZENVIRONMENT` volume.
#[derive(Clone, Debug)]
pub struct SceneEnv {
    pub node: usize,
    pub name: String,
    /// Parent node (the location / room group).
    pub parent: Option<usize>,
    pub kind: EnvKind,
    /// `*_env_character*` / crowd variants are used for characters, the others for level geometry.
    pub for_characters: bool,
    /// World direction the light travels (node +Z), unit.
    pub direction: [f32; 3],
    /// Main colour (display space 0..1, **not** multiplied by intensity).
    pub color: [f32; 3],
    pub intensity: f32,
    /// Secondary colour (`word7`): the sky half of the hemisphere for sun environments.
    pub sky: Option<[f32; 3]>,
    /// Ambient (ground half) colour.
    pub ambient: [f32; 3],
    pub shape: u16,
    pub flags: u16,
    /// The four boolean properties after the container lists.
    pub bools: [bool; 4],
    /// Number of static shadow meshes attached.
    pub shadow_mesh_count: usize,
    pub chunk: u32,
}

impl SceneEnv {
    /// Directional light colour (colour * intensity); zero for pure ambient volumes.
    pub fn directional(&self) -> [f32; 3] {
        if self.is_directional() {
            [self.color[0] * self.intensity, self.color[1] * self.intensity, self.color[2] * self.intensity]
        } else {
            [0.0; 3]
        }
    }

    /// True when the volume has a directional component (colour differs from the ambient colour).
    pub fn is_directional(&self) -> bool {
        self.kind == EnvKind::Sun || self.color != self.ambient
    }

    /// Hemisphere ambient: (sky, ground) display-space colours.
    pub fn hemisphere(&self) -> ([f32; 3], [f32; 3]) {
        match (self.kind, self.sky) {
            (EnvKind::Sun, Some(s)) => (s, self.ambient),
            (EnvKind::Sun, None) => (self.ambient, self.ambient),
            // interiors: the colour is the "top" light, the ambient colour the bottom
            (EnvKind::Ambient, _) => ([self.color[0] * self.intensity.min(1.5), self.color[1] * self.intensity.min(1.5), self.color[2] * self.intensity.min(1.5)], self.ambient),
        }
    }
}

/// `InsideBound*` box.
#[derive(Clone, Debug)]
pub struct Zone {
    pub node: usize,
    /// The location / room group the bound belongs to.
    pub location: Option<usize>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub inverse: Mat4,
}

impl Zone {
    pub fn volume(&self) -> f32 {
        (0..3).map(|k| (self.bounds_max[k] - self.bounds_min[k]).max(0.0)).product()
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        let l = self.inverse.transform_point(p);
        (0..3).all(|k| l[k] >= self.bounds_min[k] && l[k] <= self.bounds_max[k])
    }
}

/// Fog and camera settings from the scene definitions.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneFog {
    /// `CameraFar` (cm).
    pub camera_far: f32,
    /// Fog start / full distance in cm (`CameraFogBegin/Full * CameraFar`, start clamped at 0).
    pub start: f32,
    pub full: f32,
    /// Fog / background colour, display space 0..1.
    pub color: [f32; 3],
    pub fov_y_deg: f32,
}

impl Default for SceneFog {
    fn default() -> Self {
        SceneFog { camera_far: 50_000.0, start: 10_000.0, full: 50_000.0, color: [0.6, 0.7, 0.8], fov_y_deg: 67.4 }
    }
}

/// The environments that apply at a point.
#[derive(Clone, Debug, Default)]
pub struct EnvSelection {
    /// Index into [`Lighting::zones`] of the smallest zone containing the point.
    pub zone: Option<usize>,
    /// Index into [`Lighting::environments`] used for level geometry.
    pub level_env: Option<usize>,
    /// Index of the character twin.
    pub character_env: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Lighting {
    pub lights: Vec<SceneLight>,
    pub environments: Vec<SceneEnv>,
    pub zones: Vec<Zone>,
    pub fog: SceneFog,
    /// Unit vector towards the backdrop sun sprite (`Sun_01` below the backdrop group; the backdrop is centred on the camera).
    pub sun_sprite_dir: Option<[f32; 3]>,
    /// scene node -> index into `lights`.
    pub light_of_node: HashMap<usize, usize>,
    children_by_node: HashMap<usize, Vec<usize>>,
}

fn rgb_f(c: [u8; 3]) -> [f32; 3] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-9 { [0.0, -1.0, 0.0] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

fn chunk_of(node: &bm_formats::scene::SceneNode) -> Option<u32> {
    match node.props.get(4)? {
        Value::I32(c) if *c > 0 => Some(*c as u32),
        _ => None,
    }
}

impl Lighting {
    pub(crate) fn build(nodes: &[SceneNodeInfo], graph: &SceneGraph, prm: &Prm) -> Lighting {
        let mut l = Lighting::default();
        for (i, n) in nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                l.children_by_node.entry(p).or_default().push(i);
            }
            let g = &graph.nodes[i];
            let t = n.type_id;
            if light::is_light_type(t) || t == light::TYPE_ENVIRONMENT {
                let Some(chunk) = chunk_of(g) else { continue };
                let Some(bytes) = prm.chunk(chunk as usize) else { continue };
                let env = t == light::TYPE_ENVIRONMENT;
                let Ok(rec) = LightRecord::parse(bytes, env) else { continue };
                let pos = n.world.translation();
                let dir = normalize(n.world.transform_vector([0.0, 0.0, 1.0]));
                if env {
                    let lname = n.name.to_ascii_lowercase();
                    let shadow_mesh_count = match g.props.get(7) {
                        Some(Value::Container(c)) => c.len(),
                        _ => 0,
                    };
                    let b = |k: usize| matches!(g.props.get(8 + k), Some(Value::Bool(true)));
                    let for_characters = lname.contains("character") || lname.contains("crowd");
                    let sunny = shadow_mesh_count > 0 || lname.contains("sun") || lname.contains("direct");
                    let color = rgb_f(rec.color);
                    let ambient = rec.ambient.map(rgb_f).unwrap_or(color);
                    l.environments.push(SceneEnv {
                        node: i,
                        name: n.name.clone(),
                        parent: n.parent,
                        kind: if sunny { EnvKind::Sun } else { EnvKind::Ambient },
                        for_characters,
                        direction: dir,
                        color,
                        intensity: rec.intensity,
                        sky: rec.color2.map(rgb_f),
                        ambient,
                        shape: rec.shape,
                        flags: rec.flags,
                        bools: [b(0), b(1), b(2), b(3)],
                        shadow_mesh_count,
                        chunk,
                    });
                } else {
                    let shape = match (t, rec.cone.is_some(), rec.aspect.is_some()) {
                        (_, true, true) => LightShape::SpotSquare,
                        (_, true, false) => LightShape::Spot,
                        _ => LightShape::Omni,
                    };
                    l.light_of_node.insert(i, l.lights.len());
                    l.lights.push(SceneLight {
                        node: i,
                        name: n.name.clone(),
                        shape,
                        position: pos,
                        direction: dir,
                        color: rgb_f(rec.color),
                        intensity: rec.intensity,
                        range_start: rec.range_start,
                        range_end: rec.range_end,
                        cone: rec.cone,
                        aspect: rec.aspect,
                        projection_texture: rec.projection_texture,
                        color2: rec.color2.map(rgb_f),
                        flags: rec.flags,
                        chunk,
                    });
                }
            } else if n.name.starts_with("InsideBound") {
                // zone box: PRM model header bounding box in node space
                let Some(m) = n.prim_id.and_then(|p| prm.model(p as usize)) else { continue };
                l.zones.push(Zone { node: i, location: n.parent, bounds_min: m.bbox_min, bounds_max: m.bbox_max, inverse: n.world.inverse_affine() });
            } else if n.name.starts_with("Sun_") && l.sun_sprite_dir.is_none() {
                // backdrop sun sprite: the backdrop is authored around the origin and follows the camera
                let under_backdrop = {
                    let mut cur = n.parent;
                    let mut found = false;
                    while let Some(p) = cur {
                        if nodes[p].name.to_ascii_lowercase().contains("backdrop") {
                            found = true;
                            break;
                        }
                        cur = nodes[p].parent;
                    }
                    found
                };
                if under_backdrop && n.prim_id.is_some() {
                    // the sprite quad's centre is the mean of its bbox corners in world space
                    let c = n.world.translation();
                    let mid = prm_center(prm, n);
                    let p = mid.map(|m| n.world.transform_point(m)).unwrap_or(c);
                    if p[1] > 0.0 {
                        l.sun_sprite_dir = Some(normalize(p));
                    }
                }
            }
        }
        l.fog = fog_from_definitions(graph);
        l
    }

    /// Light by scene node.
    pub fn light_for_node(&self, node: usize) -> Option<&SceneLight> {
        self.light_of_node.get(&node).map(|&i| &self.lights[i])
    }

    /// Smallest `InsideBound*` zone containing `p`.
    pub fn zone_at(&self, p: [f32; 3]) -> Option<usize> {
        self.zones.iter().enumerate().filter(|(_, z)| z.contains(p)).min_by(|a, b| a.1.volume().total_cmp(&b.1.volume())).map(|(i, _)| i)
    }

    fn env_nodes_under(&self, root: usize, out: &mut Vec<usize>, nodes_env: &HashMap<usize, usize>) {
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if let Some(&e) = nodes_env.get(&n) {
                out.push(e);
            }
            if let Some(c) = self.children_by_node.get(&n) {
                stack.extend(c.iter().copied());
            }
        }
    }

    /// Environments that apply at world position `p`; `parents` gives the parent of every node (`nodes[i].parent`).
    pub fn environments_at(&self, p: [f32; 3], parents: &dyn Fn(usize) -> Option<usize>) -> EnvSelection {
        let mut sel = EnvSelection { zone: self.zone_at(p), ..Default::default() };
        let env_of_node: HashMap<usize, usize> = self.environments.iter().enumerate().map(|(i, e)| (e.node, i)).collect();
        let pick = |cands: &[usize], sel: &mut EnvSelection, envs: &[SceneEnv]| {
            // prefer the sun / directional one for level geometry
            let mut level: Vec<usize> = cands.iter().copied().filter(|&i| !envs[i].for_characters).collect();
            level.sort_by_key(|&i| (!(envs[i].kind == EnvKind::Sun), i));
            let mut chars: Vec<usize> = cands.iter().copied().filter(|&i| envs[i].for_characters).collect();
            chars.sort_unstable();
            if sel.level_env.is_none() {
                sel.level_env = level.first().copied();
            }
            if sel.character_env.is_none() {
                sel.character_env = chars.first().copied();
            }
        };
        if let Some(zi) = sel.zone {
            let mut cur = self.zones[zi].location;
            let mut first = true;
            while let Some(a) = cur {
                let mut cands = Vec::new();
                if first {
                    self.env_nodes_under(a, &mut cands, &env_of_node);
                } else if let Some(ch) = self.children_by_node.get(&a) {
                    cands.extend(ch.iter().filter_map(|c| env_of_node.get(c).copied()));
                }
                pick(&cands, &mut sel, &self.environments);
                if sel.level_env.is_some() && (sel.character_env.is_some() || !first) {
                    break;
                }
                first = false;
                cur = parents(a);
            }
        }
        if sel.level_env.is_none() {
            // outdoors / unknown: the sun environment of the level
            sel.level_env = self.environments.iter().position(|e| e.kind == EnvKind::Sun && !e.for_characters);
        }
        if sel.character_env.is_none() {
            sel.character_env = self.environments.iter().position(|e| e.for_characters && e.parent == sel.level_env.and_then(|i| self.environments[i].parent));
        }
        sel
    }
}

fn prm_center(prm: &Prm, n: &SceneNodeInfo) -> Option<[f32; 3]> {
    let m = prm.model(n.prim_id? as usize)?;
    Some([(m.bbox_min[0] + m.bbox_max[0]) * 0.5, (m.bbox_min[1] + m.bbox_max[1]) * 0.5, (m.bbox_min[2] + m.bbox_max[2]) * 0.5])
}

fn fog_from_definitions(graph: &SceneGraph) -> SceneFog {
    let mut fog = SceneFog::default();
    let f = |name: &str| -> Option<f32> {
        match &graph.definitions.iter().find(|d| d.name == name)?.value {
            DefValue::Floats(v) => v.first().copied(),
            _ => None,
        }
    };
    if let Some(far) = f("CameraFar") {
        fog.camera_far = far;
    }
    if let Some(fov) = f("CameraFOV") {
        fog.fov_y_deg = fov;
    }
    let begin = f("CameraFogBegin").unwrap_or(0.2);
    let full = f("CameraFogFull").unwrap_or(1.0);
    fog.start = (begin * fog.camera_far).max(0.0);
    fog.full = (full * fog.camera_far).max(fog.start + 1.0);
    if let Some(d) = graph.definitions.iter().find(|d| d.name == "CameraBackgroundColor") {
        if let DefValue::Ints(v) = &d.value {
            if let Some(&c) = v.first() {
                let c = c as u32;
                fog.color = rgb_f([(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            }
        }
    }
    fog
}
