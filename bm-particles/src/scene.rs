//! Decoding of the game's particle nodes into [`EmitterDesc`]s. See `docs/particles.md` for the field
//! table and the confidence of every mapping.
//!
//! Three node types are involved:
//!
//! * `0x2000e5` **ZParticleTemplate**: behaviour of one particle kind (36 properties) and the sprite
//!   (`props[4]` = PRM chunk `{0x20000, texture id, 0, 7, blend word}`).
//! * `0x2000ff` **ZParticleEmitter**: placement + spawn schedule (32 properties); `props[31]` names the
//!   template (looked up by node name, usually in the same scene).
//! * `0x2000e4` **ZParticleController**: empty node (one per scene).

use std::collections::HashMap;
use std::sync::Arc;

use bm_formats::prp::Value;
use bm_models::material::Image;
use bm_models::math::Mat4;
use bm_models::Scene;

use crate::curve::{ColorCurve, Curve, Range};
use crate::desc::{BillboardMode, EmitShape, EmitterDesc, ParticleBlend, SimSpace, SpawnDesc, SpriteDesc, SpriteSource};

pub const TYPE_PARTICLE_TEMPLATE: u32 = 0x2000e5;
pub const TYPE_PARTICLE_EMITTER: u32 = 0x2000ff;
pub const TYPE_PARTICLE_CONTROLLER: u32 = 0x2000e4;
/// Weapon clip/shell effect trigger (`ZHM3ClipParticleControl`); not an emitter, ignored here.
pub const TYPE_CLIP_PARTICLE_CONTROL: u32 = 0x10044e;

fn f(props: &[Value], i: usize) -> f32 {
    match props.get(i) {
        Some(Value::F32(v)) => *v,
        Some(Value::I32(v)) => *v as f32,
        Some(Value::I16(v)) => *v as f32,
        _ => 0.0,
    }
}
fn b(props: &[Value], i: usize) -> bool {
    matches!(props.get(i), Some(Value::Bool(true)))
}
fn v3(props: &[Value], i: usize) -> [f32; 3] {
    match props.get(i) {
        Some(Value::Array(a)) if a.len() >= 3 => {
            let g = |k: usize| if let Value::F32(x) = a[k] { x } else { 0.0 };
            [g(0), g(1), g(2)]
        }
        _ => [0.0; 3],
    }
}
fn s(props: &[Value], i: usize) -> String {
    match props.get(i) {
        Some(Value::Str(x)) | Some(Value::Token(x)) => x.clone(),
        _ => String::new(),
    }
}

/// Raw, undecoded-meaning values of a `ZParticleTemplate` (property index in the name).
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateRaw {
    pub name: String,
    /// `.TEX` id of the sprite (PRM sprite chunk word 1).
    pub texture_id: u32,
    /// PRM sprite chunk word 4 (`0x4c001` alpha, `0x4c002` additive, `0x4c000` other).
    pub blend_word: u32,
    pub p6_accel: [f32; 3],
    pub p7_jitter: f32,
    pub p8_life: f32,
    pub p9_life_var: f32,
    pub p10_size: f32,
    pub p11_size_var: f32,
    pub p12_growth: f32,
    pub p13_unknown: f32,
    pub p14_spin: f32,
    pub p15_unknown: f32,
    pub p16_unknown: f32,
    pub p17_random_rotation: bool,
    pub p21_stretch: f32,
    pub p24_unknown: f32,
    pub p25_unknown: f32,
    pub p26_unknown: i32,
    /// 8 RGBA keys, 0..255, evenly spaced over the life.
    pub colors: Vec<[f32; 4]>,
}

impl TemplateRaw {
    /// Decodes a template node; `sprite_chunk` are the 32 byte PRM words of `props[4]`.
    pub fn decode(name: &str, props: &[Value], sprite_chunk: &[u32]) -> Option<TemplateRaw> {
        if props.len() < 36 || sprite_chunk.len() < 5 {
            return None;
        }
        let colors = match &props[35] {
            Value::Array(a) => a
                .chunks_exact(4)
                .map(|c| {
                    let g = |k: usize| if let Value::F32(x) = c[k] { x } else { 0.0 };
                    [g(0), g(1), g(2), g(3)]
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(TemplateRaw {
            name: name.to_string(),
            texture_id: sprite_chunk[1],
            blend_word: sprite_chunk[4],
            p6_accel: v3(props, 6),
            p7_jitter: f(props, 7),
            p8_life: f(props, 8),
            p9_life_var: f(props, 9),
            p10_size: f(props, 10),
            p11_size_var: f(props, 11),
            p12_growth: f(props, 12),
            p13_unknown: f(props, 13),
            p14_spin: f(props, 14),
            p15_unknown: f(props, 15),
            p16_unknown: f(props, 16),
            p17_random_rotation: b(props, 17),
            p21_stretch: f(props, 21),
            p24_unknown: f(props, 24),
            p25_unknown: f(props, 25),
            p26_unknown: if let Some(Value::I32(v)) = props.get(26) { *v } else { 0 },
            colors,
        })
    }
}

/// Raw values of a `ZParticleEmitter` node.
#[derive(Clone, Debug, PartialEq)]
pub struct EmitterRaw {
    /// `props[6]`: spawn box half extents (cm).
    pub box_half: [f32; 3],
    /// `props[7]` (int16): particles per batch.
    pub count: i32,
    /// `props[9]`: seconds between batches.
    pub interval: f32,
    /// `props[10]`: emission window in seconds.
    pub duration: f32,
    /// `props[12]`: cone spread, degrees.
    pub spread_deg: f32,
    /// `props[13]`: initial velocity in emitter space, cm/s.
    pub velocity: [f32; 3],
    /// `props[14]`: speed variation 0..1.
    pub speed_var: f32,
    /// `props[25]`: activation message (`"Activate"`).
    pub message: String,
    /// `props[26..=30]`: `IsGlobal`, `bHide`, `bLighting`, `bAutoStart`, `bDeactivate` (names from the community type registry).
    pub is_global: bool,
    pub hidden: bool,
    pub lighting: bool,
    pub auto_start: bool,
    pub deactivate: bool,
    /// `props[31]`: template node name.
    pub template: String,
}

impl EmitterRaw {
    pub fn decode(props: &[Value]) -> Option<EmitterRaw> {
        if props.len() < 32 {
            return None;
        }
        Some(EmitterRaw {
            box_half: v3(props, 6),
            count: f(props, 7) as i32,
            interval: f(props, 9),
            duration: f(props, 10),
            spread_deg: f(props, 12),
            velocity: v3(props, 13),
            speed_var: f(props, 14),
            message: s(props, 25),
            is_global: b(props, 26),
            hidden: b(props, 27),
            lighting: b(props, 28),
            auto_start: b(props, 29),
            deactivate: b(props, 30),
            template: s(props, 31),
        })
    }
}

/// Builds the plain-data description from a template and (optionally) the emitter that uses it.
/// Without an emitter the template alone becomes a continuous preview effect (a few particles per life, at rest).
pub fn build_desc(t: &TemplateRaw, e: Option<&EmitterRaw>, sprite_name: Option<&str>) -> EmitterDesc {
    let life = t.p8_life.max(0.01);
    let var = t.p9_life_var.clamp(0.0, 1.0);
    let size = t.p10_size.max(0.0);
    let svar = t.p11_size_var.clamp(0.0, 1.0);
    let blend = if t.blend_word == 0x4c002 { ParticleBlend::Additive } else { ParticleBlend::Alpha };
    let colors: Vec<[f32; 4]> = t.colors.iter().map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, c[3] / 255.0]).collect();
    let spin = t.p14_spin;
    let mut d = EmitterDesc {
        name: t.name.clone(),
        sprite: SpriteDesc {
            source: match sprite_name {
                Some(n) => SpriteSource::Texture(n.to_string()),
                None => SpriteSource::default(),
            },
            ..Default::default()
        },
        blend,
        space: SimSpace::World,
        billboard: if t.p21_stretch > 0.0 { BillboardMode::Stretched } else { BillboardMode::Camera },
        stretch: t.p21_stretch,
        lifetime: Range(life * (1.0 - 0.5 * var), life * (1.0 + 0.5 * var)),
        size: Range(size * (1.0 - 0.5 * svar), size),
        size_growth: t.p12_growth,
        acceleration: t.p6_accel,
        velocity_jitter: t.p7_jitter,
        rotation_deg: if t.p17_random_rotation { Range(0.0, 360.0) } else { Range(0.0, 0.0) },
        angular_velocity: Range(spin.min(spin * 0.5), spin.max(spin * 0.5)),
        color: if colors.is_empty() { ColorCurve::default() } else { ColorCurve::evenly_spaced(&colors) },
        size_over_life: Curve::constant(1.0),
        ..Default::default()
    };
    match e {
        None => {
            d.spawn = SpawnDesc { rate: (8.0 / life).clamp(4.0, 60.0), looping: true, ..Default::default() };
            d.speed = Range(0.0, 0.0);
            d.max_particles = 256;
        }
        Some(e) => {
            let h = e.box_half;
            d.shape = if h.iter().any(|v| v.abs() > 1e-3) { EmitShape::Box { half_extents: h } } else { EmitShape::Point };
            let speed = (e.velocity[0] * e.velocity[0] + e.velocity[1] * e.velocity[1] + e.velocity[2] * e.velocity[2]).sqrt();
            if speed > 1e-3 {
                d.direction = [e.velocity[0] / speed, e.velocity[1] / speed, e.velocity[2] / speed];
            }
            let sv = e.speed_var.clamp(0.0, 1.0);
            d.speed = Range(speed * (1.0 - 0.5 * sv), speed);
            d.spread_deg = e.spread_deg.clamp(0.0, 180.0);
            d.space = if e.is_global { SimSpace::World } else { SimSpace::Local };
            let count = e.count.max(1) as u32;
            d.spawn = SpawnDesc {
                rate: 0.0,
                batch_count: count,
                batch_interval: e.interval.max(0.0),
                duration: e.duration.max(0.0),
                looping: e.auto_start,
                ..Default::default()
            };
            let per_life = if e.interval > 1e-4 { (d.lifetime.1 / e.interval).ceil() as u32 * count } else { count };
            d.max_particles = (per_life + count).clamp(count, 2000);
        }
    }
    d
}

/// One `ZParticleEmitter` placed in a scene.
#[derive(Clone, Debug)]
pub struct PlacedEmitter {
    /// Scene graph node index.
    pub node: usize,
    pub name: String,
    pub path: String,
    /// Emitter space -> world, native space.
    pub world: Mat4,
    pub raw: EmitterRaw,
    /// `None` while the template is not known (defined in another scene, see [`SceneParticles::merge_templates`]).
    pub desc: Option<Arc<EmitterDesc>>,
    /// Heuristic: the emitter lives in a template library (`CloneGroup_*`, `Controllers`, `Effects`
    /// groups cloned by scripts at runtime, usually at the origin) rather than at a real level position.
    pub is_template: bool,
}

impl PlacedEmitter {
    /// Level emitters that run from the start: `bAutoStart` set, `bDeactivate` clear and not a template.
    pub fn is_ambient(&self) -> bool {
        self.raw.auto_start && !self.raw.deactivate && !self.raw.hidden && !self.is_template
    }
}

fn looks_like_template(path: &str) -> bool {
    path.contains("CloneGroup") || path.contains("Controllers") || path.contains("/Effects") || path.contains("Effects/")
}

/// All particle data of a scene.
#[derive(Default)]
pub struct SceneParticles {
    pub templates: HashMap<String, TemplateRaw>,
    /// Sprite texture name by template name.
    pub sprite_names: HashMap<String, String>,
    pub emitters: Vec<PlacedEmitter>,
    /// Decoded sprite images by [`SpriteSource::key`].
    pub images: HashMap<String, Arc<Image>>,
}

impl SceneParticles {
    pub fn from_scene(scene: &Scene) -> SceneParticles {
        let mut out = SceneParticles::default();
        for n in &scene.graph.nodes {
            if n.type_id != TYPE_PARTICLE_TEMPLATE {
                continue;
            }
            let Some(Value::I32(chunk)) = n.props.get(4) else { continue };
            let Some(words) = scene.prm.chunk_words(*chunk as usize) else { continue };
            let Some(t) = TemplateRaw::decode(&n.name, &n.props, &words) else { continue };
            if let Some(img) = scene.materials.image(t.texture_id) {
                let src = SpriteSource::Texture(img.name.clone());
                out.sprite_names.insert(t.name.clone(), img.name.clone());
                out.images.insert(src.key(), img);
            }
            // later definitions with the same name win (scene order), like a name lookup would
            out.templates.insert(t.name.clone(), t);
        }
        for n in &scene.graph.nodes {
            if n.type_id != TYPE_PARTICLE_EMITTER {
                continue;
            }
            let Some(raw) = EmitterRaw::decode(&n.props) else { continue };
            let world = scene.nodes[n.index].world;
            let path = scene.path(n.index);
            let is_template = looks_like_template(&path);
            let mut pe = PlacedEmitter { node: n.index, name: n.name.clone(), path, world, raw, desc: None, is_template };
            out.resolve(&mut pe);
            out.emitters.push(pe);
        }
        out
    }

    fn resolve(&self, pe: &mut PlacedEmitter) {
        if let Some(t) = self.templates.get(&pe.raw.template) {
            let sprite = self.sprite_names.get(&t.name).map(|x| x.as_str());
            let mut d = build_desc(t, Some(&pe.raw), sprite);
            d.name = format!("{} ({})", pe.name, t.name);
            pe.desc = Some(Arc::new(d));
        }
    }

    /// Copies templates (and their images) this scene lacks from `other` and resolves the emitters that
    /// were waiting for them. Returns the number of emitters resolved.
    pub fn merge_templates(&mut self, other: &SceneParticles) -> usize {
        for (k, t) in &other.templates {
            if !self.templates.contains_key(k) {
                self.templates.insert(k.clone(), t.clone());
                if let Some(n) = other.sprite_names.get(k) {
                    self.sprite_names.insert(k.clone(), n.clone());
                }
            }
        }
        for (k, im) in &other.images {
            self.images.entry(k.clone()).or_insert_with(|| im.clone());
        }
        let mut emitters = std::mem::take(&mut self.emitters);
        let mut n = 0;
        for pe in &mut emitters {
            if pe.desc.is_none() {
                self.resolve(pe);
                n += pe.desc.is_some() as usize;
            }
        }
        self.emitters = emitters;
        n
    }

    /// Description of a template on its own (single burst, no emitter) for quick previews.
    pub fn template_desc(&self, name: &str) -> Option<EmitterDesc> {
        let t = self.templates.get(name)?;
        Some(build_desc(t, None, self.sprite_names.get(name).map(|x| x.as_str())))
    }

    /// Emitters that start by themselves.
    pub fn ambient(&self) -> impl Iterator<Item = &PlacedEmitter> {
        self.emitters.iter().filter(|e| e.is_ambient() && e.desc.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template_props() -> Vec<Value> {
        let mut p = vec![Value::Token("BOUNDING_Static".into()), Value::Array(vec![]), Value::Array(vec![]), Value::Bool(false), Value::I32(9346), Value::Bool(false)];
        let fl = |x: f32| Value::F32(x);
        p.push(Value::Array(vec![fl(0.0), fl(-800.0), fl(0.0)])); // 6
        for x in [1.0, 1.2, 0.5, 15.0, 0.5, -3.0, 0.0, 50.0, 1.0, 0.0] {
            p.push(fl(x)); // 7..=16
        }
        p.push(Value::Bool(true)); // 17
        p.push(Value::Bool(false));
        p.push(Value::Bool(false));
        p.push(fl(0.0)); // 20
        p.push(fl(0.02)); // 21
        p.push(Value::Array(vec![fl(0.0); 3])); // 22
        p.push(fl(0.0));
        p.push(fl(0.0));
        p.push(fl(1.0));
        p.push(Value::I32(1)); // 26
        p.push(Value::Bool(false));
        p.push(Value::Bool(false));
        p.push(Value::Bool(false));
        p.push(Value::Str(String::new())); // 30
        p.push(Value::I32(9346));
        p.push(Value::Str(String::new()));
        p.push(Value::Str(String::new()));
        p.push(fl(1.0)); // 34
        let mut c = Vec::new();
        for i in 0..8 {
            c.extend([fl(255.0), fl(128.0), fl(0.0), fl(255.0 - 36.0 * i as f32)]);
        }
        p.push(Value::Array(c)); // 35
        p
    }

    #[test]
    fn template_decodes_and_builds_desc() {
        let t = TemplateRaw::decode("T", &template_props(), &[0x20000, 467, 0, 7, 0x4c002]).unwrap();
        assert_eq!(t.texture_id, 467);
        assert_eq!(t.colors.len(), 8);
        let d = build_desc(&t, None, Some("Effects/Glow/Glow_10_FX"));
        assert_eq!(d.blend, ParticleBlend::Additive);
        assert_eq!(d.acceleration, [0.0, -800.0, 0.0]);
        assert_eq!(d.billboard, BillboardMode::Stretched);
        assert!((d.lifetime.0 - 1.2 * 0.75).abs() < 1e-5 && (d.lifetime.1 - 1.2 * 1.25).abs() < 1e-5);
        assert_eq!(d.color.keys.len(), 8);
        assert!((d.color.keys[0].1[0] - 1.0).abs() < 1e-6 && (d.color.keys[7].0 - 1.0).abs() < 1e-6);
        assert_eq!(d.sprite.source, SpriteSource::Texture("Effects/Glow/Glow_10_FX".into()));
    }

    #[test]
    fn emitter_decodes() {
        let fl = |x: f32| Value::F32(x);
        let mut p = vec![Value::Token("BOUNDING_Static".into()), Value::Array(vec![]), Value::Array(vec![]), Value::Bool(false), Value::I32(0), Value::Bool(false)];
        p.push(Value::Array(vec![fl(5.0), fl(0.0), fl(5.0)])); // 6
        p.push(Value::I16(12)); // 7
        p.push(Value::Array(vec![fl(0.0); 3])); // 8
        p.extend([fl(0.0067), fl(0.3), fl(0.0), fl(30.0)]); // 9..=12
        p.push(Value::Array(vec![fl(0.0), fl(200.0), fl(0.0)])); // 13
        p.extend([fl(0.5), fl(0.0), fl(0.0)]); // 14..=16
        p.push(Value::Str(String::new())); // 17
        p.extend([Value::I32(-1), Value::I32(-1), Value::Bool(false), fl(1.0), Value::I32(-1), Value::Bool(false), Value::Bool(false)]); // 18..=24
        p.push(Value::Str("Activate".into())); // 25
        p.extend([Value::Bool(true), Value::Bool(false), Value::Bool(false), Value::Bool(true), Value::Bool(false)]); // 26..=30
        p.push(Value::Str("Smoke".into())); // 31
        let e = EmitterRaw::decode(&p).unwrap();
        assert_eq!((e.count, e.template.as_str(), e.auto_start, e.is_global), (12, "Smoke", true, true));
        let t = TemplateRaw::decode("Smoke", &template_props(), &[0x20000, 1, 0, 7, 0x4c001]).unwrap();
        let d = build_desc(&t, Some(&e), None);
        assert_eq!(d.spawn.batch_count, 12);
        assert!((d.spawn.duration - 0.3).abs() < 1e-6 && d.spawn.looping);
        assert_eq!(d.shape, EmitShape::Box { half_extents: [5.0, 0.0, 5.0] });
        assert_eq!(d.direction, [0.0, 1.0, 0.0]);
        assert!((d.speed.1 - 200.0).abs() < 1e-3 && (d.speed.0 - 150.0).abs() < 1e-3);
        assert_eq!(d.blend, ParticleBlend::Alpha);
    }
}
