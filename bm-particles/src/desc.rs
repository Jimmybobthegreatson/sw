//! Plain-data emitter definition. Everything is `serde` serialisable so modders can author emitters in
//! RON or JSON (every field has a default, so a file may list only what it changes), and the scene
//! decoder (`crate::scene`) produces exactly the same type from the game's `ZParticleTemplate` /
//! `ZParticleEmitter` nodes.
//!
//! Units: centimetres, seconds, degrees (the engine's native units). Space: left-handed, Y-up.

use serde::{Deserialize, Serialize};

use crate::curve::{ColorCurve, Curve, Range};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParticleBlend {
    /// Straight alpha blending (smoke, steam, debris).
    #[default]
    Alpha,
    /// Additive (`src_alpha * src + dst`): flashes, sparks, glows, fire.
    Additive,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimSpace {
    /// Particles keep their world position after being born (trails stay behind a moving emitter).
    #[default]
    World,
    /// Particles are simulated in the emitter's space and follow it.
    Local,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BillboardMode {
    /// Quad faces the camera and rotates around the view axis.
    #[default]
    Camera,
    /// Camera-facing quad stretched along the screen-projected velocity (length += speed * `stretch`).
    Stretched,
    /// Quad axis follows the 3D velocity; width stays camera-facing (rain, tracers, blood strings).
    VelocityAligned,
}

/// Where particles are born, in emitter space.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum EmitShape {
    #[default]
    Point,
    /// Uniform in a box with the given half extents.
    Box { half_extents: [f32; 3] },
    /// Uniform in a ball.
    Sphere { radius: f32 },
    /// On a horizontal (XZ) disc, uniform area.
    Disc { radius: f32 },
}

/// Texture used by the quads.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpriteSource {
    /// Procedural, no assets needed.
    Builtin(BuiltinSprite),
    /// Image file in the VFS (PNG), e.g. `Textures/Particles/spark.png` shipped by a mod.
    Vfs(String),
    /// A scene texture by its `.TEX` name, e.g. `Effects/Smoke/Smoke_01_FX_H3` (supplied by the scene
    /// loader through `ParticleHandle::register_image`).
    Texture(String),
}

impl SpriteSource {
    /// Key under which the renderer stores the image.
    pub fn key(&self) -> String {
        match self {
            SpriteSource::Builtin(b) => format!("builtin:{b:?}"),
            SpriteSource::Vfs(p) => format!("vfs:{p}"),
            SpriteSource::Texture(n) => format!("tex:{n}"),
        }
    }
}

impl Default for SpriteSource {
    fn default() -> Self {
        SpriteSource::Builtin(BuiltinSprite::SoftCircle)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuiltinSprite {
    /// Radial falloff (glows, flashes, generic puffs).
    SoftCircle,
    /// Noisy puff (smoke, steam).
    Puff,
    /// Bright core with a long falloff (sparks; use with stretch).
    Spark,
    /// Hard-edged disc with a soft rim (droplets).
    Droplet,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameMode {
    /// Always frame 0.
    #[default]
    Fixed,
    /// Random frame chosen at birth.
    Random,
    /// Plays through the sheet during the particle's life.
    OverLife,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpriteDesc {
    pub source: SpriteSource,
    /// Flipbook grid (1x1 = single image).
    pub columns: u32,
    pub rows: u32,
    /// Frames used from the grid (row-major, 0 = all `columns * rows`).
    pub frames: u32,
    pub frame_mode: FrameMode,
}

impl Default for SpriteDesc {
    fn default() -> Self {
        SpriteDesc { source: SpriteSource::default(), columns: 1, rows: 1, frames: 0, frame_mode: FrameMode::Fixed }
    }
}

impl SpriteDesc {
    pub fn frame_count(&self) -> u32 {
        let n = self.columns.max(1) * self.rows.max(1);
        if self.frames == 0 { n } else { self.frames.min(n) }
    }

    /// UV rect `[u0, v0, u1, v1]` (top-left origin) of a frame.
    pub fn uv_rect(&self, frame: u32) -> [f32; 4] {
        let (c, r) = (self.columns.max(1), self.rows.max(1));
        let f = frame % (c * r);
        let (x, y) = (f % c, f / c);
        [x as f32 / c as f32, y as f32 / r as f32, (x + 1) as f32 / c as f32, (y + 1) as f32 / r as f32]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Burst {
    /// Seconds after the emitter starts.
    pub time: f32,
    pub count: u32,
}

/// When particles are born.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpawnDesc {
    /// Continuous emission, particles per second.
    pub rate: f32,
    /// One-off bursts.
    pub bursts: Vec<Burst>,
    /// The game's batch scheme: `batch_count` particles at t = 0 and then every `batch_interval`
    /// seconds while `t < duration` (0 interval = single batch).
    pub batch_count: u32,
    pub batch_interval: f32,
    /// Emission window in seconds (0 = unlimited for `rate`, only t = 0 for batches).
    pub duration: f32,
    /// Restart after the window ended and all bursts fired (ambient emitters).
    pub looping: bool,
    pub start_delay: f32,
}

impl Default for SpawnDesc {
    fn default() -> Self {
        SpawnDesc { rate: 20.0, bursts: Vec::new(), batch_count: 0, batch_interval: 0.0, duration: 0.0, looping: true, start_delay: 0.0 }
    }
}

/// Complete particle emitter description.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmitterDesc {
    pub name: String,
    pub sprite: SpriteDesc,
    pub blend: ParticleBlend,
    pub space: SimSpace,
    pub billboard: BillboardMode,
    /// Length added per unit speed in `Stretched` mode (seconds).
    pub stretch: f32,
    /// Hard cap of simultaneously alive particles of this emitter.
    pub max_particles: u32,
    pub spawn: SpawnDesc,
    pub shape: EmitShape,
    /// Seconds.
    pub lifetime: Range,
    /// Initial speed along `direction`, cm/s.
    pub speed: Range,
    /// Emission axis in emitter space (normalised on use).
    pub direction: [f32; 3],
    /// Cone half angle around `direction`, degrees (180 = any direction).
    pub spread_deg: f32,
    /// Extra random velocity of at most this magnitude in any direction, cm/s.
    pub velocity_jitter: f32,
    /// Constant acceleration in *world* space, cm/s^2 (gravity is `[0, -981, 0]`).
    pub acceleration: [f32; 3],
    /// Linear drag, 1/s (`v *= exp(-drag dt)`).
    pub drag: f32,
    /// Start size (width, cm).
    pub size: Range,
    /// Height / width ratio.
    pub aspect: f32,
    /// Multiplies the size over normalised life.
    pub size_over_life: Curve,
    /// Added to the size every second (cm/s, may be negative); the result is clamped at 0.
    pub size_growth: f32,
    pub rotation_deg: Range,
    /// Degrees per second.
    pub angular_velocity: Range,
    pub color: ColorCurve,
    /// Multiplied with `color` (rgb) and its alpha.
    pub tint: [f32; 4],
    pub opacity: f32,
    /// Fade the quad out when closer than this to the camera, cm (0 = off).
    pub soft_distance: f32,
}

impl Default for EmitterDesc {
    fn default() -> Self {
        EmitterDesc {
            name: String::new(),
            sprite: SpriteDesc::default(),
            blend: ParticleBlend::Alpha,
            space: SimSpace::World,
            billboard: BillboardMode::Camera,
            stretch: 0.0,
            max_particles: 512,
            spawn: SpawnDesc::default(),
            shape: EmitShape::Point,
            lifetime: Range(1.0, 1.0),
            speed: Range(0.0, 0.0),
            direction: [0.0, 1.0, 0.0],
            spread_deg: 0.0,
            velocity_jitter: 0.0,
            acceleration: [0.0; 3],
            drag: 0.0,
            size: Range(10.0, 10.0),
            aspect: 1.0,
            size_over_life: Curve::constant(1.0),
            size_growth: 0.0,
            rotation_deg: Range(0.0, 0.0),
            angular_velocity: Range(0.0, 0.0),
            color: ColorCurve::constant([1.0; 4]),
            tint: [1.0; 4],
            opacity: 1.0,
            soft_distance: 0.0,
        }
    }
}

impl EmitterDesc {
    pub fn from_ron(s: &str) -> Result<EmitterDesc, ron::error::SpannedError> {
        ron::from_str(s)
    }
    pub fn from_json(s: &str) -> Result<EmitterDesc, serde_json::Error> {
        serde_json::from_str(s)
    }
    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).expect("serialisable")
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("serialisable")
    }

    /// Authored example: rising grey smoke (no game assets needed).
    pub fn preset_smoke() -> EmitterDesc {
        EmitterDesc {
            name: "smoke".into(),
            sprite: SpriteDesc { source: SpriteSource::Builtin(BuiltinSprite::Puff), ..Default::default() },
            blend: ParticleBlend::Alpha,
            max_particles: 400,
            spawn: SpawnDesc { rate: 30.0, ..Default::default() },
            shape: EmitShape::Disc { radius: 8.0 },
            lifetime: Range(3.0, 4.5),
            speed: Range(30.0, 55.0),
            spread_deg: 15.0,
            acceleration: [12.0, 20.0, 0.0],
            drag: 0.4,
            size: Range(30.0, 45.0),
            size_growth: 35.0,
            rotation_deg: Range(0.0, 360.0),
            angular_velocity: Range(-25.0, 25.0),
            color: ColorCurve::evenly_spaced(&[
                [0.25, 0.25, 0.25, 0.0],
                [0.3, 0.3, 0.3, 0.55],
                [0.4, 0.4, 0.4, 0.35],
                [0.5, 0.5, 0.5, 0.0],
            ]),
            ..Default::default()
        }
    }

    /// Authored example: a burst of additive stretched sparks.
    pub fn preset_sparks() -> EmitterDesc {
        EmitterDesc {
            name: "sparks".into(),
            sprite: SpriteDesc { source: SpriteSource::Builtin(BuiltinSprite::Spark), ..Default::default() },
            blend: ParticleBlend::Additive,
            billboard: BillboardMode::Stretched,
            stretch: 0.03,
            max_particles: 300,
            spawn: SpawnDesc { rate: 0.0, bursts: vec![Burst { time: 0.0, count: 60 }], looping: false, ..Default::default() },
            lifetime: Range(0.6, 1.4),
            speed: Range(250.0, 600.0),
            spread_deg: 55.0,
            acceleration: [0.0, -981.0, 0.0],
            drag: 0.3,
            size: Range(3.0, 5.0),
            color: ColorCurve::evenly_spaced(&[[1.0, 0.95, 0.7, 1.0], [1.0, 0.6, 0.15, 1.0], [0.8, 0.2, 0.05, 0.0]]),
            ..Default::default()
        }
    }

    /// Authored example: velocity-aligned blood droplets.
    pub fn preset_blood() -> EmitterDesc {
        EmitterDesc {
            name: "blood".into(),
            sprite: SpriteDesc { source: SpriteSource::Builtin(BuiltinSprite::Droplet), ..Default::default() },
            blend: ParticleBlend::Alpha,
            billboard: BillboardMode::VelocityAligned,
            max_particles: 200,
            spawn: SpawnDesc { rate: 0.0, bursts: vec![Burst { time: 0.0, count: 40 }], looping: false, ..Default::default() },
            lifetime: Range(0.7, 1.3),
            speed: Range(150.0, 450.0),
            spread_deg: 30.0,
            acceleration: [0.0, -800.0, 0.0],
            size: Range(3.0, 7.0),
            aspect: 2.5,
            color: ColorCurve::evenly_spaced(&[[0.55, 0.02, 0.02, 1.0], [0.4, 0.0, 0.0, 1.0], [0.3, 0.0, 0.0, 0.0]]),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ron_roundtrip_and_partial() {
        let d = EmitterDesc::preset_sparks();
        let s = d.to_ron();
        assert_eq!(EmitterDesc::from_ron(&s).unwrap(), d);
        let j = d.to_json();
        assert_eq!(EmitterDesc::from_json(&j).unwrap(), d);
        // partial files take defaults
        let p = EmitterDesc::from_ron("(name: \"x\", blend: Additive, lifetime: (2.0, 3.0))").unwrap();
        assert_eq!(p.blend, ParticleBlend::Additive);
        assert_eq!(p.lifetime, Range(2.0, 3.0));
        assert_eq!(p.max_particles, 512);
    }

    #[test]
    fn uv_rect_grid() {
        let s = SpriteDesc { columns: 2, rows: 2, ..Default::default() };
        assert_eq!(s.uv_rect(3), [0.5, 0.5, 1.0, 1.0]);
        assert_eq!(s.uv_rect(1), [0.5, 0.0, 1.0, 0.5]);
        assert_eq!(s.frame_count(), 4);
    }
}
