//! Per-frame parameters: sun, ambient, fog, sky, lights, shadows and post effects.
//!
//! All colours are **display space** (gamma encoded, 0..1) like in the original engine: the shader multiplies them with the
//! (non-sRGB sampled) albedo and the final image is converted to the target format in the present pass.

use bm_models::lighting::{EnvKind, Lighting, SceneEnv, SceneFog, SceneLight};
use bm_models::scene::Scene;

use crate::camera::{normalize, Vec3};

/// A point or spot light (decoded `ZOMNILIGHT` / `ZSPOTLIGHT` / `ZSPOTLIGHTSQUARE`, see `bm_models::lighting`).
#[derive(Clone, Debug)]
pub struct PointLight {
    pub position: Vec3,
    /// Range end in cm: the light has no effect beyond this distance.
    pub radius: f32,
    /// Linear (display space) RGB, pre-multiplied by intensity.
    pub color: [f32; 3],
    /// Range start in cm: full intensity up to here, linear falloff to `radius`.
    pub range_start: f32,
    /// Spot cone axis (unit) - ignored unless `cone` is set.
    pub direction: Vec3,
    /// `(cos outer, cos inner)` of a spot light.
    pub cone: Option<(f32, f32)>,
}

impl PointLight {
    /// A point light with a falloff from 0 to `radius` (the pre-decoding behaviour).
    pub fn new(position: Vec3, radius: f32, color: [f32; 3]) -> PointLight {
        PointLight { position, radius, color, range_start: 0.0, direction: [0.0, -1.0, 0.0], cone: None }
    }

    /// From a decoded scene light.
    pub fn from_scene(l: &SceneLight) -> PointLight {
        PointLight {
            position: l.position,
            radius: l.range_end.max(1.0),
            color: l.radiance(),
            range_start: l.range_start.min(l.range_end),
            direction: l.direction,
            cone: l.cone.map(|c| (c.cos_outer, c.cos_inner)),
        }
    }
}

/// Interior (room environment) ambient tuning, see [`FrameParams::apply_environment`].
pub const INTERIOR_AMBIENT_GAIN: f32 = 1.35;
pub const INTERIOR_AMBIENT_FLOOR: f32 = 0.07;

/// Sun shadow map settings.
#[derive(Clone, Debug)]
pub struct ShadowParams {
    pub enabled: bool,
    /// Texture size of every cascade (square).
    pub resolution: u32,
    /// Number of cascades (1..=3).
    pub cascades: u32,
    /// Farthest shadowed distance from the camera (cm).
    pub distance: f32,
    /// How strongly shadowed areas lose sun light (1 = fully dark sun term).
    pub strength: f32,
    /// Receiver depth bias in light-space cm (scaled with the cascade texel size).
    pub depth_bias: f32,
    /// Receiver offset along the normal in texels.
    pub normal_bias: f32,
}

impl Default for ShadowParams {
    fn default() -> Self {
        ShadowParams { enabled: true, resolution: 2048, cascades: 2, distance: 9000.0, strength: 0.85, depth_bias: 1.5, normal_bias: 1.5 }
    }
}

/// Everything that changes per frame besides the camera.
#[derive(Clone, Debug)]
pub struct FrameParams {
    /// Unit vector pointing TOWARDS the sun.
    pub sun_dir: Vec3,
    /// RGB * intensity of the directional light (zero = no sun, no shadows).
    pub sun_color: [f32; 3],
    pub ambient_sky: [f32; 3],
    pub ambient_ground: [f32; 3],
    pub sky_top: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub fog_color: [f32; 3],
    /// Linear fog: none before `fog_start`, full fog colour at `fog_end` (cm). `fog_end <= fog_start` disables the fog.
    pub fog_start: f32,
    pub fog_end: f32,
    /// Multiplier of the baked vertex colour (ambient occlusion style) 0..1.
    pub vertex_color_strength: f32,
    /// Brightness multiplier of the vertex coloured sky dome / backdrop layer (the domes are texture-less gradients).
    pub sky_gain: f32,
    pub exposure: f32,
    /// 0 = lit, 1 = albedo only, 2 = world normals, 3 = shadow term, 4 = light count.
    pub debug_mode: u32,
    /// Draw the sky gradient (otherwise the clear colour is used).
    pub draw_sky: bool,
    pub clear_color: [f32; 4],
    /// Seconds, available to shaders as `u_frame.cam_pos.w`.
    pub time: f32,
    /// Point lights; the 16 most relevant to the camera are used each frame.
    pub lights: Vec<PointLight>,
    /// Draw only items within this distance of the camera (0 = unlimited).
    pub max_draw_distance: f32,
    pub shadows: ShadowParams,
    /// Strength of environment reflections (`mapEnvironment` / procedural sky), 0 = off.
    pub reflections: f32,
    /// FXAA in the present pass.
    pub fxaa: bool,
}

impl Default for FrameParams {
    fn default() -> Self {
        FrameParams {
            sun_dir: normalize([0.45, 0.75, 0.35]),
            sun_color: [1.0, 0.93, 0.80],
            ambient_sky: [0.45, 0.5, 0.56],
            ambient_ground: [0.30, 0.28, 0.25],
            sky_top: [0.40, 0.58, 0.85],
            sky_horizon: [0.78, 0.86, 0.93],
            fog_color: [0.78, 0.86, 0.93],
            fog_start: 8000.0,
            fog_end: 60000.0,
            vertex_color_strength: 1.0,
            sky_gain: 1.6,
            exposure: 1.0,
            debug_mode: 0,
            draw_sky: true,
            clear_color: [0.1, 0.1, 0.12, 1.0],
            time: 0.0,
            lights: Vec::new(),
            max_draw_distance: 0.0,
            shadows: ShadowParams::default(),
            reflections: 1.0,
            fxaa: true,
        }
    }
}

impl FrameParams {
    /// Fog from the scene definitions (`CameraFogBegin/Full`, `CameraFar`, `CameraBackgroundColor`); also sets the sky horizon colour.
    pub fn apply_fog(&mut self, fog: &SceneFog) {
        self.fog_color = fog.color;
        self.fog_start = fog.start;
        self.fog_end = fog.full;
        self.sky_horizon = fog.color;
    }

    /// Sun / ambient from a decoded environment volume (`Scene::environments_at`). Pure ambient volumes switch the sun off.
    pub fn apply_environment(&mut self, env: &SceneEnv) {
        let (mut sky, mut ground) = env.hemisphere();
        if env.kind == EnvKind::Ambient {
            // Tuning (guess, see docs/render.md): rooms get a mild ambient lift and floor. The engine adds baked light / light
            // maps that are not decoded, so unlit corners come out too black with the raw colours alone.
            for k in 0..3 {
                sky[k] = (sky[k] * INTERIOR_AMBIENT_GAIN).max(INTERIOR_AMBIENT_FLOOR);
                ground[k] = (ground[k] * INTERIOR_AMBIENT_GAIN).max(INTERIOR_AMBIENT_FLOOR * 0.8);
            }
        }
        self.ambient_sky = sky;
        self.ambient_ground = ground;
        if env.kind == EnvKind::Sun {
            self.sun_color = env.directional();
            self.sun_dir = normalize([-env.direction[0], -env.direction[1], -env.direction[2]]);
        } else {
            self.sun_color = [0.0; 3];
        }
    }

    /// Everything decodable from the scene at a camera position: fog, the sun / ambient of the zone and (all) lights.
    pub fn from_scene_at(scene: &Scene, position: Vec3) -> FrameParams {
        let mut p = FrameParams::default();
        p.apply_scene_at(scene, position);
        p
    }

    /// Re-applies the scene's environment at `position` (call when the camera moves into another zone).
    pub fn apply_scene_at(&mut self, scene: &Scene, position: Vec3) {
        let l: &Lighting = scene.lighting();
        self.apply_fog(&l.fog);
        let sel = scene.environments_at(position);
        match sel.level_env.map(|i| &l.environments[i]) {
            Some(env) => self.apply_environment(env),
            None => self.sun_color = [0.0; 3],
        }
        // sky colours derived from the fog colour: a darker, bluer zenith
        self.sky_top = [self.fog_color[0] * 0.55, self.fog_color[1] * 0.7, (self.fog_color[2] * 0.9 + 0.1).min(1.0)];
    }
}

/// Collects the decoded lights (`NodeKind::Light` nodes) of a scene: colour, intensity, range and cone come from the PRM light
/// records (`bm_models::lighting`).
pub fn scene_lights(scene: &Scene) -> Vec<PointLight> {
    scene.lighting().lights.iter().map(PointLight::from_scene).collect()
}

/// Deprecated shape of [`scene_lights`]: the old API assumed that nothing but the position was known and applied one colour and
/// radius to all lights. The arguments are now ignored.
pub fn lights_from_scene(scene: &Scene, _radius: f32, _color: [f32; 3]) -> Vec<PointLight> {
    scene_lights(scene)
}
