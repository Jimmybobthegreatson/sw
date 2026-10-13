//! Scene upload: `Vec<DrawItem>` -> GPU buffers, textures, materials and per-model instance lists.

use std::collections::HashMap;
use std::sync::Arc;

use bm_models::material::{BlendMode, CullMode, MaterialDesc, TileMode};
use bm_models::math::{Aabb, Mat4};
use bm_models::mesh::{Mesh, Model, VertexLayout};
use bm_models::scene::{DrawItem, Scene};
use bm_models::skeleton::{Pose, Skeleton};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::camera::{self, Frustum};
use crate::hooks::{ForcedBlend, MaterialInfo};
use crate::renderer::{BlendClass, CullKind, PipelineKey, Renderer};

/// Interleaved vertex (76 bytes) shared by all layouts.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

/// Per-instance data (80 bytes): model matrix + first bone of the instance's palette.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct InstanceRaw {
    pub model: [[f32; 4]; 4],
    pub bone_base: u32,
    pub _pad: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialRaw {
    diffuse: [f32; 4],
    specular: [f32; 4],
    illum: [f32; 4],
    /// x specular power, y alpha ref, z opacity, w reflection level
    params: [f32; 4],
    /// rgb reflection colour
    refl: [f32; 4],
    /// alpha fade: angle scale, angle offset, distance scale, distance offset
    fade: [f32; 4],
    flags: [u32; 4],
}

pub const F_NORMAL: u32 = 1;
pub const F_SPEC: u32 = 2;
pub const F_ILLUM: u32 = 4;
pub const F_TWO_SIDED: u32 = 8;
pub const F_FOG: u32 = 16;
pub const F_UNLIT: u32 = 32;
/// Opaque window / glass material with a reflection mask: drawn blended, the mask decides the pane opacity.
pub const F_GLASS: u32 = 8192;
pub const F_SPEC_ENABLED: u32 = 64;
pub const F_ALPHA_FADE: u32 = 128;
pub const F_REFLECT_MAP: u32 = 256;
pub const F_REFLECT_SKY: u32 = 512;
pub const F_REFL_MASK: u32 = 1024;
pub const F_REFL_FALLOFF: u32 = 2048;
pub const F_WATER: u32 = 4096;

/// Options of [`SceneGpu::upload`].
#[derive(Clone, Debug)]
pub struct UploadOptions {
    /// Only upload the most detailed LOD (see `docs/models.md`).
    pub lod0_only: bool,
    /// Outfit value (variant mask, see `Mesh::in_outfit`) of skinned character meshes for nodes without an actor
    /// outfit of their own. 1 = the first outfit.
    pub outfit: u8,
    /// Use the outfit value stored in `ZHM3Actor` nodes (`Scene::actor_outfit`) so every NPC wears exactly one outfit.
    pub actor_outfits: bool,
    /// Per scene node overrides of the outfit value (disguises, scripts).
    pub outfit_by_node: HashMap<usize, u8>,
    /// Per scene node part filter for modular models (`Mesh::part`, the Custom_* weapons): only meshes whose part id is in
    /// the list (and meshes without a part id) are uploaded. Nodes without an entry draw every part. Items that share a
    /// node (a weapon in two hands) share one filter; instance lists are keyed by (model, outfit, filter).
    pub part_filter_by_node: HashMap<usize, Vec<u32>>,
    /// Per scene node replacement of material instance ids (`stub id -> real id`, `MaterialLibrary::stub_remap`): the weapon models
    /// reference header-only stub instances that are blank (and skipped) everywhere else. Part of the instance list key like the
    /// part filter.
    pub material_remap_by_node: HashMap<usize, Vec<(u32, u32)>>,
    /// Skip `SHADOW` / `STATICSHADOW` materials.
    pub skip_shadow_materials: bool,
    /// Textures larger than this (in either dimension) are uploaded without their top mip levels.
    pub max_texture_size: u32,
    /// Upload skinned meshes (bind pose until `set_pose` is called).
    pub include_skinned: bool,
    /// Skip helper materials (`_Glacier/Collision`, `AlwaysInShadowMaterial*`; see [`is_helper_material`]).
    pub skip_helper_materials: bool,
    /// Draw `SkySphere*` backdrop items as a camera centred sky layer (otherwise they are skipped).
    pub sky_layer: bool,
}

impl Default for UploadOptions {
    fn default() -> Self {
        UploadOptions { lod0_only: true, outfit: 1, actor_outfits: true, outfit_by_node: HashMap::new(), part_filter_by_node: HashMap::new(), material_remap_by_node: HashMap::new(), skip_shadow_materials: true, max_texture_size: 1024, include_skinned: true, sky_layer: true, skip_helper_materials: true }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UploadStats {
    pub items: usize,
    pub models: usize,
    pub meshes: usize,
    pub skipped_meshes: usize,
    pub triangles: usize,
    pub vertices: usize,
    pub materials: usize,
    pub textures: usize,
    pub missing_textures: usize,
    pub texture_bytes: usize,
    pub skinned_instances: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub instances_total: usize,
    pub instances_drawn: usize,
    pub draw_calls: usize,
    pub triangles: usize,
    pub pipelines_switches: usize,
    pub lights_used: usize,
    /// Sun shadow pass: cascades rendered, draw calls and triangles summed over all cascades.
    pub shadow_cascades: usize,
    pub shadow_draw_calls: usize,
    pub shadow_triangles: usize,
}

pub(crate) struct Page {
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
}

pub(crate) struct MeshDraw {
    pub page: usize,
    pub base_vertex: i32,
    pub first_index: u32,
    pub index_count: u32,
    pub material: usize,
    pub skinned: bool,
}

struct Inst {
    world: Mat4,
    bounds: Aabb,
    bone_base: u32,
    visible: bool,
    flip: bool,
}

pub(crate) struct InstanceList {
    local_bounds: Aabb,
    skinned: bool,
    pub sky: bool,
    instances: Vec<Inst>,
    pub draws: Vec<MeshDraw>,
}

pub(crate) struct MaterialGpu {
    pub bind_group: wgpu::BindGroup,
    pub key: PipelineKey,
    pub order: u8,
    pub name: String,
    /// Rendered into the sun shadow map.
    pub casts_shadow: bool,
    /// The shadow pass needs the diffuse alpha (alpha tested / blended foliage).
    pub shadow_alpha: bool,
    #[allow(dead_code)]
    pub buffer: wgpu::Buffer,
}

struct ItemRef {
    list: usize,
    slot: usize,
    skeleton: Option<Arc<Skeleton>>,
    bone_base: u32,
}

/// One draw call produced by culling.
pub(crate) struct DrawCmd {
    pub order: u8,
    /// Sky layer only: blend order of the material (opaque dome first, then Trans, Add ... in graph order).
    pub sub: u8,
    pub dist: f32,
    pub key: PipelineKey,
    pub material: usize,
    pub page: usize,
    pub draw: usize,
    pub list: usize,
    pub first_instance: u32,
    pub instance_count: u32,
}

/// Light-space culling volume of one shadow cascade.
pub(crate) struct ShadowBox {
    /// World -> light space (x, y spanning the shadow map, z along the light direction).
    pub view: Mat4,
    pub center: [f32; 2],
    pub half_extent: f32,
    /// Light-space z interval that is rendered (casters nearer to the sun than the receivers included).
    pub z_min: f32,
    pub z_max: f32,
}

pub(crate) struct ShadowCmd {
    pub skinned: bool,
    pub alpha: bool,
    pub material: usize,
    pub page: usize,
    pub draw: usize,
    pub list: usize,
    pub first_instance: u32,
    pub instance_count: u32,
}

pub(crate) struct ShadowPlan {
    pub instances: Vec<InstanceRaw>,
    pub cmds: Vec<ShadowCmd>,
    pub triangles: usize,
}

pub(crate) struct FramePlan {
    pub instances: Vec<InstanceRaw>,
    pub cmds: Vec<DrawCmd>,
    pub stats: FrameStats,
}

/// A scene resident on the GPU.
pub struct SceneGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub(crate) pages: Vec<Page>,
    pub(crate) lists: Vec<InstanceList>,
    pub(crate) materials: Vec<MaterialGpu>,
    pub(crate) bones: wgpu::Buffer,
    items: Vec<ItemRef>,
    pub stats: UploadStats,
}

const PAGE_MAX_VERTS: usize = 800_000;
const PAGE_MAX_INDICES: usize = 6_000_000;

struct PageBuilder {
    verts: Vec<Vertex>,
    indices: Vec<u32>,
    pages: Vec<Page>,
}

impl PageBuilder {
    fn flush(&mut self, device: &wgpu::Device) {
        if self.verts.is_empty() {
            return;
        }
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm vertex page"),
            contents: bytemuck::cast_slice(&self.verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm index page"),
            contents: bytemuck::cast_slice(&self.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.pages.push(Page { vbuf, ibuf });
        self.verts.clear();
        self.indices.clear();
    }

    /// Appends a mesh, returns `(page, base_vertex, first_index)`.
    fn push(&mut self, device: &wgpu::Device, verts: Vec<Vertex>, idx: &[u32]) -> (usize, i32, u32) {
        if !self.verts.is_empty()
            && (self.verts.len() + verts.len() > PAGE_MAX_VERTS || self.indices.len() + idx.len() > PAGE_MAX_INDICES)
        {
            self.flush(device);
        }
        let base = self.verts.len() as i32;
        let first = self.indices.len() as u32;
        self.verts.extend(verts);
        self.indices.extend_from_slice(idx);
        (self.pages.len(), base, first)
    }
}

/// Backdrop items: anything below a `*Backdrop*` group (sky sphere, cloud planes, mountains, ocean
/// bloom cards) or named `SkySphere*`. The engine keeps these centred on the camera.
pub fn is_sky_node(scene: &Scene, node: usize) -> bool {
    let mut cur = Some(node);
    while let Some(i) = cur {
        let Some(n) = scene.nodes.get(i) else { return false };
        let l = n.name.to_ascii_lowercase();
        if l.contains("backdrop") || l.starts_with("skysphere") {
            return true;
        }
        cur = n.parent;
    }
    false
}

/// Engine helper materials that are never meant to be seen: `_Glacier/Collision` (green collision
/// proxies), `AlwaysInShadowMaterial*` (shadow-receiver planes) and `Screens/*` (the walkable-area quads of the
/// in-game map screens, which sit inside the level at y = 0).
pub fn is_helper_material(name: &str) -> bool {
    name.starts_with("_Glacier/") || name.starts_with("AlwaysInShadow") || name.starts_with("Screens/")
}

/// Unnamed, untextured placeholder instances (`.MAT` ids 1..5): the engine draws nothing with them.
fn is_blank_material(d: &bm_models::MaterialDesc) -> bool {
    (d.name.is_empty() && d.diffuse_texture().is_none()) || d.name == "Bad"
}

fn build_vertices(m: &Mesh) -> Vec<Vertex> {
    let v = &m.vertices;
    (0..v.positions.len())
        .map(|i| Vertex {
            pos: v.positions[i],
            normal: v.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
            tangent: v.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]),
            uv: v.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
            color: v.colors.get(i).copied().unwrap_or([255; 4]),
            joints: v.joints.get(i).copied().unwrap_or([0; 4]),
            weights: v.weights.get(i).copied().unwrap_or([0.0; 4]),
        })
        .collect()
}

fn box_down(src: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let mut s = 0u32;
                let mut n = 0u32;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                        s += src[(sy * w + sx) * 4 + c] as u32;
                        n += 1;
                    }
                }
                out[(y * nw + x) * 4 + c] = (s / n) as u8;
            }
        }
    }
    (out, nw, nh)
}

/// Decoded mip chain: `(width, height, levels)`.
fn decode_mips(scene: &Scene, id: u32, cap: u32) -> Option<(u32, u32, Vec<Vec<u8>>)> {
    let img = scene.materials.tex().image(id)?;
    let (w0, h0) = (img.width as usize, img.height as usize);
    if w0 == 0 || h0 == 0 {
        return None;
    }
    let mut levels: Vec<(Vec<u8>, usize, usize)> = Vec::new();
    for l in 0..img.mips.len().max(1) {
        let (w, h) = ((w0 >> l).max(1), (h0 >> l).max(1));
        match img.decode_rgba(l) {
            Ok(d) if d.len() == w * h * 4 => levels.push((d, w, h)),
            _ => break,
        }
    }
    if levels.is_empty() {
        return None;
    }
    if levels.len() == 1 {
        // no stored mips: generate a full chain
        loop {
            let (d, w, h) = levels.last().unwrap();
            if *w == 1 && *h == 1 {
                break;
            }
            let n = box_down(d, *w, *h);
            levels.push(n);
        }
    }
    let mut skip = 0;
    while skip + 1 < levels.len() && levels[skip].1.max(levels[skip].2) as u32 > cap {
        skip += 1;
    }
    let (w, h) = (levels[skip].1 as u32, levels[skip].2 as u32);
    Some((w, h, levels.into_iter().skip(skip).map(|l| l.0).collect()))
}

fn tile_to_address(t: TileMode) -> wgpu::AddressMode {
    match t {
        TileMode::Tiled => wgpu::AddressMode::Repeat,
        TileMode::Mirrored => wgpu::AddressMode::MirrorRepeat,
        // `NONE`/empty tile modes occur on materials whose UVs clearly tile (floors, walls): repeating is
        // the safe choice, clamping smears them.
        TileMode::Clamp => wgpu::AddressMode::Repeat,
    }
}

struct Upload<'a> {
    r: &'a mut Renderer,
    scene: &'a Scene,
    opts: &'a UploadOptions,
    textures: HashMap<(u32, bool), Option<wgpu::TextureView>>,
    samplers: HashMap<(u8, u8), wgpu::Sampler>,
    stats: UploadStats,
}

impl Upload<'_> {
    fn texture(&mut self, id: u32, srgb: bool) -> Option<wgpu::TextureView> {
        if let Some(t) = self.textures.get(&(id, srgb)) {
            return t.clone();
        }
        let view = decode_mips(self.scene, id, self.opts.max_texture_size).map(|(w, h, mips)| {
            let format = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
            let data: Vec<u8> = mips.iter().flatten().copied().collect();
            self.stats.texture_bytes += data.len();
            let tex = self.r.device.create_texture_with_data(
                &self.r.queue,
                &wgpu::TextureDescriptor {
                    label: Some("bm texture"),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: mips.len() as u32,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::MipMajor,
                &data,
            );
            tex.create_view(&wgpu::TextureViewDescriptor::default())
        });
        if view.is_some() {
            self.stats.textures += 1;
        } else {
            self.stats.missing_textures += 1;
        }
        self.textures.insert((id, srgb), view.clone());
        view
    }

    /// True when texture `id` is stored in the two-channel `U8V8` format (normal maps).
    fn is_two_channel(&self, id: u32) -> bool {
        self.scene.materials.image(id).is_some_and(|i| i.source_format == "U8V8")
    }

    fn sampler(&mut self, u: TileMode, v: TileMode) -> wgpu::Sampler {
        let key = (u as u8, v as u8);
        if let Some(s) = self.samplers.get(&key) {
            return s.clone();
        }
        let s = self.r.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bm sampler"),
            address_mode_u: tile_to_address(u),
            address_mode_v: tile_to_address(v),
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        self.samplers.insert(key, s.clone());
        s
    }

    /// Builds the GPU material; `None` = do not draw.
    fn material(&mut self, desc: Option<&MaterialDesc>) -> Option<MaterialGpu> {
        let fallback;
        let desc = match desc {
            Some(d) => d,
            None => {
                fallback = MaterialDesc {
                    id: 0,
                    name: "<default>".into(),
                    shader: None,
                    render_state: Default::default(),
                    textures: vec![],
                    colors: Default::default(),
                    floats: Default::default(),
                    flags: Default::default(),
                };
                &fallback
            }
        };
        let ov = match self.r.hooks.as_mut() {
            Some(h) => h.material(&MaterialInfo { desc }),
            None => Default::default(),
        };
        if ov.skip {
            return None;
        }
        let rs = &desc.render_state;
        let blend_mode = &rs.blend;
        let mut class = if blend_mode.is_additive() {
            BlendClass::Add
        } else if blend_mode.is_alpha_blended() {
            BlendClass::Alpha
        } else {
            BlendClass::Opaque
        };
        match ov.force_blend {
            Some(ForcedBlend::Opaque) => class = BlendClass::Opaque,
            Some(ForcedBlend::Alpha) => class = BlendClass::Alpha,
            Some(ForcedBlend::Additive) => class = BlendClass::Add,
            None => {}
        }
        // Window panes: many window materials are `Opaque` in the data (dirty glass baked into the diffuse map) but carry a reflection
        // mask that is white on the panes and black on the frame. Draw them blended with the mask as pane transparency.
        let lname = desc.name.to_ascii_lowercase();
        let glassy = class == BlendClass::Opaque
            && ov.force_blend.is_none()
            && (lname.contains("window") || lname.contains("glass"))
            && !lname.contains("glow")
            && desc.flag("ReflectionEnabled")
            && desc.texture("mapReflectionMask").is_some_and(|t| t != 0)
            && !rs.alpha_test;
        if glassy {
            class = BlendClass::Alpha;
        }
        // draw order: opaque, *_ON_OPAQUE, ADD_BEFORE_TRANS, TRANS, other additive
        let order = match (&class, blend_mode) {
            (BlendClass::Opaque, _) => 1,
            (_, BlendMode::TransOnOpaque | BlendMode::AddOnOpaque | BlendMode::TransAddOnOpaque) => 2,
            (_, BlendMode::AddBeforeTrans) => 3,
            (BlendClass::Alpha, _) => 4,
            _ => 5,
        };

        // textures. Everything is sampled as plain (gamma encoded) Rgba8Unorm: lighting happens in display space, see main.wgsl.
        // A diffuse / specular map stored in the two-channel `U8V8` normal-map format carries no colour (the Dragunov texture set of
        // every level is such a duplicate of its normal map: the decode is a flat cyan): draw the surface in a neutral dark grey.
        let diffuse_id = desc.diffuse_texture();
        let flat_diffuse = diffuse_id.is_some_and(|t| self.is_two_channel(t));
        let diff = diffuse_id.filter(|_| !flat_diffuse).and_then(|t| self.texture(t, false));
        let norm = desc.normal_texture().and_then(|t| self.texture(t, false));
        let spec = desc.specular_mask_texture().filter(|&t| !self.is_two_channel(t)).and_then(|t| self.texture(t, false));
        let illum = desc.illumination_texture().and_then(|t| self.texture(t, false));
        let refl_enabled = desc.flag("ReflectionEnabled");
        let refl_level = desc.floats.get("vReflectionLevel").and_then(|v| v.first().copied()).unwrap_or(1.0);
        let env = if refl_enabled { desc.texture("mapEnvironment").and_then(|t| self.texture(t, false)) } else { None };
        let rmask = if refl_enabled { desc.texture("mapReflectionMask").and_then(|t| self.texture(t, false)) } else { None };
        let rfall = if refl_enabled { desc.texture("mapReflectionFallOff").and_then(|t| self.texture(t, false)) } else { None };

        let mut flags = 0u32;
        if norm.is_some() && desc.flags.get("BumpEnabled") != Some(&false) {
            flags |= F_NORMAL;
        }
        if spec.is_some() {
            flags |= F_SPEC;
        }
        if desc.flag("SpecularEnabled") {
            flags |= F_SPEC_ENABLED;
        }
        if (illum.is_some() && desc.flags.get("IlluminationEnabled") != Some(&false)) || ov.emissive.is_some() {
            flags |= F_ILLUM;
        }
        if rs.cull == CullMode::TwoSided {
            flags |= F_TWO_SIDED;
        }
        if rs.fog_enabled && class != BlendClass::Add {
            flags |= F_FOG;
        }
        if class == BlendClass::Add {
            flags |= F_UNLIT;
        }
        if glassy {
            flags |= F_GLASS;
        }
        // self-lit effect cards (window glows, light shafts drawn with a plain blend): not shaded by scene lights
        if lname.starts_with("effects/") && lname.contains("glow") {
            flags |= F_UNLIT;
        }
        if refl_enabled && refl_level > 0.0 && class != BlendClass::Add {
            flags |= if env.is_some() { F_REFLECT_MAP } else { F_REFLECT_SKY };
            if rmask.is_some() {
                flags |= F_REFL_MASK;
            }
            if rfall.is_some() {
                flags |= F_REFL_FALLOFF;
            }
        }
        let mut fade = [0.0f32; 4];
        if desc.floats.contains_key("gm_vFresnelPow") {
            // water surface (`WaterSurface` material): fresnel blend of a deep water colour and the sky reflection
            let f = |k: &str, d: f32| desc.floats.get(k).and_then(|v| v.first().copied()).unwrap_or(d);
            fade = [f("gm_vFresnelMin", 0.0), f("gm_vFresnelMax", 1.0), f("gm_vFresnelPow", 5.0), f("v4BumpScale", 0.3)];
            flags |= F_WATER;
            flags &= !(F_REFLECT_MAP | F_REFLECT_SKY | F_ALPHA_FADE);
        } else if desc.flag("AlphaFadeEnabled") {
            if let (Some(a), Some(d)) = (desc.floats.get("gm_vAlphaFadeAngleScaleOffset"), desc.floats.get("gm_vAlphaFadeDistScaleOffset")) {
                if a.len() >= 2 && d.len() >= 2 {
                    fade = [a[0], a[1], d[0], d[1]];
                    flags |= F_ALPHA_FADE;
                }
            }
        }

        let mut diffuse = desc.diffuse_color();
        if flat_diffuse {
            for c in &mut diffuse[..3] {
                *c *= 0.22;
            }
        }
        if let Some(m) = ov.color_mul {
            for i in 0..4 {
                diffuse[i] *= m[i];
            }
        }
        let sc = desc.colors.get("v4SpecularColor").copied().unwrap_or([1.0, 1.0, 1.0, 1.0]);
        let level = desc.floats.get("vSpecularLevel").and_then(|v| v.first().copied()).unwrap_or(0.5);
        let power = desc.floats.get("v4SpecularPower").and_then(|v| v.first().copied()).unwrap_or(20.0);
        let ic = match ov.emissive {
            Some(e) => [e[0], e[1], e[2], 1.0],
            None => desc.colors.get("v4IlluminationColor").copied().unwrap_or([1.0; 4]),
        };
        let rc = desc.colors.get("v4ReflectionColor").copied().unwrap_or([1.0; 4]);
        let alpha_ref = if rs.alpha_test { (rs.alpha_ref as f32).max(1.0) } else { 0.0 };
        let opacity = if class == BlendClass::Opaque { 1.0 } else { rs.opacity };
        let raw = MaterialRaw {
            diffuse,
            specular: [sc[0], sc[1], sc[2], level],
            illum: ic,
            params: [power, alpha_ref, opacity, refl_level],
            refl: [rc[0], rc[1], rc[2], 0.0],
            fade,
            flags: [flags, 0, 0, 0],
        };
        let buffer = self.r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm material"),
            contents: bytemuck::bytes_of(&raw),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let (tu, tv) = desc
            .textures
            .iter()
            .find(|t| t.slot == "mapDiffuse")
            .map(|t| (t.tile_u, t.tile_v))
            .unwrap_or((TileMode::Tiled, TileMode::Tiled));
        let sampler = self.sampler(tu, tv);
        let d = &self.r.defaults;
        let (dv, nv, sv, iv) = (
            diff.as_ref().unwrap_or(&d.white),
            norm.as_ref().unwrap_or(&d.flat_normal),
            spec.as_ref().unwrap_or(&d.white),
            illum.as_ref().unwrap_or(&d.black),
        );
        let (ev, mv, fv) = (env.as_ref().unwrap_or(&d.white), rmask.as_ref().unwrap_or(&d.white), rfall.as_ref().unwrap_or(&d.white));
        let bind_group = self.r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bm material bind group"),
            layout: &self.r.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(dv) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(nv) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(sv) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(iv) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(ev) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(mv) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(fv) },
            ],
        });
        let shader = match &ov.shader {
            Some(p) => self.r.ensure_shader(p),
            None => crate::shaders::BUILTIN,
        };
        let key = PipelineKey {
            shader,
            skinned: false,
            alpha_test: rs.alpha_test,
            blend: class,
            depth_write: class == BlendClass::Opaque,
            cull: if rs.cull == CullMode::TwoSided { CullKind::None } else { CullKind::Back },
            flip: false,
            z_bias: rs.z_bias,
            sky: false,
        };
        // sun shadow casting: everything that is (mostly) opaque; blended foliage / fences through an alpha test
        let casts_shadow = class != BlendClass::Add && (class == BlendClass::Opaque || (rs.opacity >= 0.5 && matches!(blend_mode, BlendMode::Trans | BlendMode::TransOnOpaque)));
        let shadow_alpha = rs.alpha_test || class != BlendClass::Opaque;
        Some(MaterialGpu { bind_group, key, order, name: desc.name.clone(), buffer, casts_shadow, shadow_alpha })
    }
}

impl SceneGpu {
    /// Uploads the draw list. Geometry is deduplicated per model; repeated models become instances.
    pub fn upload(renderer: &mut Renderer, scene: &Scene, items: &[DrawItem], opts: &UploadOptions) -> SceneGpu {
        let device = renderer.device.clone();
        let queue = renderer.queue.clone();
        let mut up = Upload {
            r: renderer,
            scene,
            opts,
            textures: HashMap::new(),
            samplers: HashMap::new(),
            stats: UploadStats::default(),
        };
        let mut pb = PageBuilder { verts: Vec::new(), indices: Vec::new(), pages: Vec::new() };
        let mut materials: Vec<MaterialGpu> = Vec::new();
        let mut mat_index: HashMap<u32, Option<usize>> = HashMap::new();
        let mut lists: Vec<InstanceList> = Vec::new();
        let mut list_of_model: HashMap<(*const Model, u8, Option<Vec<u32>>, Option<Vec<(u32, u32)>>), Option<usize>> = HashMap::new();
        let mut item_refs: Vec<ItemRef> = Vec::new();
        let mut bone_mats: Vec<[f32; 16]> = Vec::new();

        for item in items {
            let model = &item.model;
            // world collision proxies (`WorldColi_*`, drawn with plain diffuse colours such as bright green) are never visible
            if opts.skip_helper_materials && scene.nodes[item.node].name.to_ascii_lowercase().starts_with("worldcoli") {
                item_refs.push(ItemRef { list: usize::MAX, slot: 0, skeleton: None, bone_base: 0 });
                continue;
            }
            if !opts.sky_layer && is_sky_node(scene, item.node) {
                item_refs.push(ItemRef { list: usize::MAX, slot: 0, skeleton: None, bone_base: 0 });
                continue;
            }
            // every character wears exactly one outfit: the model holds the meshes of all of them
            let want = opts.outfit_by_node.get(&item.node).copied().or_else(|| if opts.actor_outfits { scene.actor_outfit(item.node) } else { None }).unwrap_or(opts.outfit);
            let outfit = model.resolve_outfit(want);
            let parts = opts.part_filter_by_node.get(&item.node).cloned();
            let remap = opts.material_remap_by_node.get(&item.node).cloned();
            let li = *list_of_model.entry((Arc::as_ptr(model), outfit, parts.clone(), remap.clone())).or_insert_with(|| {
                let mut draws = Vec::new();
                let mut any_skinned = false;
                for mesh in &model.meshes {
                    let skip = mesh.layout == VertexLayout::PosColor10
                        || mesh.indices.is_empty()
                        || mesh.vertices.is_empty()
                        || (opts.lod0_only && !mesh.is_lod0())
                        || parts.as_ref().is_some_and(|p| mesh.part != 0 && !p.contains(&mesh.part))
                        || (mesh.is_skinned() && (!opts.include_skinned || !mesh.in_outfit(outfit)));
                    if skip {
                        up.stats.skipped_meshes += 1;
                        continue;
                    }
                    let mid = mesh.material();
                    let mid = remap.as_ref().and_then(|r| r.iter().find(|p| p.0 == mid)).map_or(mid, |p| p.1);
                    let desc = up.scene.materials.desc(mid);
                    if let Ok(f) = std::env::var("BM_MAT_FILTER") {
                        // debugging aid: draw only materials whose name contains the text, or `#id` (a .MAT instance id); `!` inverts
                        let n = desc.as_ref().map(|d| d.name.as_str()).unwrap_or("");
                        let matches = |x: &str| match x.strip_prefix('#').and_then(|i| i.parse::<u32>().ok()) {
                            Some(id) => mid == id,
                            None => n.contains(x),
                        };
                        let hit = match f.strip_prefix('!') {
                            Some(x) => !matches(x),
                            None => matches(&f),
                        };
                        if !hit {
                            up.stats.skipped_meshes += 1;
                            continue;
                        }
                    }
                    if (opts.skip_shadow_materials && desc.as_ref().is_some_and(|d| d.is_shadow_only()))
                        || (opts.skip_helper_materials && desc.as_ref().is_some_and(|d| is_helper_material(&d.name) || is_blank_material(d)))
                    {
                        up.stats.skipped_meshes += 1;
                        continue;
                    }
                    let mi = *mat_index.entry(mid).or_insert_with(|| {
                        up.material(desc.as_deref()).map(|m| {
                            materials.push(m);
                            materials.len() - 1
                        })
                    });
                    let Some(mi) = mi else {
                        up.stats.skipped_meshes += 1;
                        continue;
                    };
                    let skinned = mesh.is_skinned();
                    any_skinned |= skinned;
                    let (page, base_vertex, first_index) = pb.push(&device, build_vertices(mesh), &mesh.indices);
                    up.stats.meshes += 1;
                    up.stats.triangles += mesh.triangle_count();
                    up.stats.vertices += mesh.vertices.len();
                    draws.push(MeshDraw {
                        page,
                        base_vertex,
                        first_index,
                        index_count: mesh.indices.len() as u32,
                        material: mi,
                        skinned,
                    });
                }
                if draws.is_empty() {
                    return None;
                }
                up.stats.models += 1;
                let mut b = if model.bbox.is_empty() { model.computed_bbox() } else { model.bbox };
                if any_skinned {
                    for k in 0..3 {
                        b.min[k] -= 100.0;
                        b.max[k] += 100.0;
                    }
                }
                lists.push(InstanceList { local_bounds: b, skinned: any_skinned, sky: is_sky_node(scene, item.node), instances: Vec::new(), draws });
                Some(lists.len() - 1)
            });
            let Some(li) = li else {
                // keep item indices stable even when nothing is drawn
                item_refs.push(ItemRef { list: usize::MAX, slot: 0, skeleton: None, bone_base: 0 });
                continue;
            };
            let list = &mut lists[li];
            let mut bone_base = 0u32;
            let mut skeleton = None;
            if list.skinned {
                if let Some(sk) = &model.skeleton {
                    bone_base = bone_mats.len() as u32;
                    for m in sk.skinning_matrices(&Pose::bind(sk)) {
                        bone_mats.push(m.to_cols_array());
                    }
                    skeleton = Some(sk.clone());
                    up.stats.skinned_instances += 1;
                } else {
                    // skinned meshes without a skeleton: identity palette
                    bone_base = bone_mats.len() as u32;
                    for _ in 0..128 {
                        bone_mats.push(Mat4::IDENTITY.to_cols_array());
                    }
                }
            }
            list.instances.push(Inst {
                world: item.world,
                bounds: list.local_bounds.transformed(&item.world),
                bone_base,
                visible: true,
                flip: item.world.det3() < 0.0,
            });
            item_refs.push(ItemRef { list: li, slot: list.instances.len() - 1, skeleton, bone_base });
        }
        pb.flush(&device);
        let mut stats = up.stats.clone();
        stats.items = items.len();
        stats.materials = materials.len();
        if bone_mats.is_empty() {
            bone_mats.push(Mat4::IDENTITY.to_cols_array());
        }
        let bones = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm bones"),
            contents: bytemuck::cast_slice(&bone_mats),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        SceneGpu { device, queue, pages: pb.pages, lists, materials, bones, items: item_refs, stats }
    }

    /// Number of items passed to `upload` (indices of `set_pose` / `set_world` / `set_visible`).
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Updates the model matrix of an item (e.g. a moving prop or character).
    pub fn set_world(&mut self, item: usize, world: Mat4) {
        let Some(r) = self.items.get(item).filter(|r| r.list != usize::MAX) else { return };
        let list = &mut self.lists[r.list];
        let b = list.local_bounds.transformed(&world);
        let inst = &mut list.instances[r.slot];
        inst.world = world;
        inst.bounds = b;
        inst.flip = world.det3() < 0.0;
    }

    pub fn set_visible(&mut self, item: usize, visible: bool) {
        if let Some(r) = self.items.get(item).filter(|r| r.list != usize::MAX) {
            self.lists[r.list].instances[r.slot].visible = visible;
        }
    }

    /// Uploads the skinning matrices of `pose` for a skinned item (`Skeleton::skinning_matrices`).
    /// Returns false when the item has no skeleton.
    pub fn set_pose(&mut self, item: usize, pose: &Pose) -> bool {
        let Some(r) = self.items.get(item) else { return false };
        let Some(sk) = &r.skeleton else { return false };
        let mats: Vec<[f32; 16]> = sk.skinning_matrices(pose).iter().map(|m| m.to_cols_array()).collect();
        self.queue.write_buffer(&self.bones, r.bone_base as u64 * 64, bytemuck::cast_slice(&mats));
        true
    }

    /// Uploads ready-made skinning matrices (e.g. `RenderEntity::skinning` from bm-game) for a skinned item.
    /// Extra matrices are ignored; returns false when the item has no skeleton.
    pub fn set_skinning(&mut self, item: usize, mats: &[Mat4]) -> bool {
        let Some(r) = self.items.get(item) else { return false };
        let Some(sk) = &r.skeleton else { return false };
        let n = mats.len().min(sk.bones.len().max(1));
        let raw: Vec<[f32; 16]> = mats.iter().take(n).map(|m| m.to_cols_array()).collect();
        if raw.is_empty() {
            return false;
        }
        self.queue.write_buffer(&self.bones, r.bone_base as u64 * 64, bytemuck::cast_slice(&raw));
        true
    }

    /// World matrix currently set for an item (None for items that were not uploaded).
    pub fn item_world(&self, item: usize) -> Option<Mat4> {
        let r = self.items.get(item).filter(|r| r.list != usize::MAX)?;
        Some(self.lists[r.list].instances[r.slot].world)
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Culls, batches and sorts; produces instance data and draw commands.
    pub(crate) fn plan(&self, frustum: &Frustum, cam_pos: [f32; 3], max_dist: f32) -> FramePlan {
        let mut instances: Vec<InstanceRaw> = Vec::new();
        let mut cmds: Vec<DrawCmd> = Vec::new();
        let mut stats = FrameStats::default();
        let max_d2 = if max_dist > 0.0 { max_dist * max_dist } else { f32::INFINITY };
        for (li, list) in self.lists.iter().enumerate() {
            stats.instances_total += list.instances.len();
            // visible instances: (instance, dist) split into normal / mirrored
            let mut groups: [Vec<(&Inst, f32)>; 2] = [Vec::new(), Vec::new()];
            for inst in &list.instances {
                if !inst.visible || (!list.sky && !frustum.intersects_aabb(&inst.bounds)) {
                    continue;
                }
                let c = inst.bounds.center();
                let d = camera::sub(c, cam_pos);
                let d2 = camera::dot(d, d);
                if d2 > max_d2 && !list.sky {
                    continue;
                }
                groups[inst.flip as usize].push((inst, d2.sqrt()));
            }
            let mut ranges: [(u32, u32); 2] = [(0, 0); 2];
            let mut sorted_idx: Vec<(u32, f32, bool)> = Vec::new(); // (instance buffer index, dist, flip)
            for (g, members) in groups.iter().enumerate() {
                ranges[g].0 = instances.len() as u32;
                for (inst, dist) in members {
                    sorted_idx.push((instances.len() as u32, *dist, g == 1));
                    let mut model = inst.world.0;
                    if list.sky {
                        // backdrop objects are authored around the origin and follow the camera
                        model[3][0] += cam_pos[0];
                        model[3][1] += cam_pos[1];
                        model[3][2] += cam_pos[2];
                    }
                    instances.push(InstanceRaw { model, bone_base: inst.bone_base, _pad: [0; 3] });
                }
                ranges[g].1 = members.len() as u32;
            }
            if sorted_idx.is_empty() {
                continue;
            }
            stats.instances_drawn += sorted_idx.len();
            for (di, d) in list.draws.iter().enumerate() {
                let m = &self.materials[d.material];
                let mut key = m.key;
                key.skinned = d.skinned;
                key.sky = list.sky;
                if list.sky {
                    // backdrop domes / cloud cards are authored to be seen from either side
                    key.cull = CullKind::None;
                }
                if m.order == 1 || list.sky {
                    for g in 0..2 {
                        if ranges[g].1 == 0 {
                            continue;
                        }
                        let mut k = key;
                        k.flip = g == 1;
                        cmds.push(DrawCmd {
                            order: if list.sky { 0 } else { 1 },
                            sub: if list.sky { m.order } else { 0 },
                            dist: 0.0,
                            key: k,
                            material: d.material,
                            page: d.page,
                            draw: di,
                            list: li,
                            first_instance: ranges[g].0,
                            instance_count: ranges[g].1,
                        });
                    }
                } else {
                    for &(ii, dist, flip) in &sorted_idx {
                        let mut k = key;
                        k.flip = flip;
                        cmds.push(DrawCmd {
                            order: m.order,
                            sub: 0,
                            dist,
                            key: k,
                            material: d.material,
                            page: d.page,
                            draw: di,
                            list: li,
                            first_instance: ii,
                            instance_count: 1,
                        });
                    }
                }
            }
        }
        cmds.sort_by(|a, b| {
            a.order.cmp(&b.order).then_with(|| {
                if a.order == 0 {
                    // backdrop layer: opaque dome first, then by blend class in authoring (graph) order
                    (a.sub, a.list, a.draw, a.key).cmp(&(b.sub, b.list, b.draw, b.key))
                } else if a.order == 1 {
                    (a.key, a.material, a.page, a.list).cmp(&(b.key, b.material, b.page, b.list))
                } else {
                    b.dist.partial_cmp(&a.dist).unwrap_or(std::cmp::Ordering::Equal)
                }
            })
        });
        for c in &cmds {
            let d = &self.lists[c.list].draws[c.draw];
            stats.triangles += d.index_count as usize / 3 * c.instance_count as usize;
        }
        stats.draw_calls = cmds.len();
        FramePlan { instances, cmds, stats }
    }
}

impl SceneGpu {
    /// Collects the shadow casters that touch a cascade box (instances are culled by their world bounds in light space).
    pub(crate) fn plan_shadow(&self, b: &ShadowBox) -> ShadowPlan {
        let mut instances: Vec<InstanceRaw> = Vec::new();
        let mut cmds: Vec<ShadowCmd> = Vec::new();
        let mut triangles = 0usize;
        for (li, list) in self.lists.iter().enumerate() {
            if list.sky {
                continue;
            }
            // does any draw of the list cast?
            if !list.draws.iter().any(|d| self.materials[d.material].casts_shadow) {
                continue;
            }
            let first = instances.len() as u32;
            for inst in &list.instances {
                if !inst.visible {
                    continue;
                }
                let (mn, mx) = (inst.bounds.min, inst.bounds.max);
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for c in 0..8 {
                    let p = [if c & 1 == 0 { mn[0] } else { mx[0] }, if c & 2 == 0 { mn[1] } else { mx[1] }, if c & 4 == 0 { mn[2] } else { mx[2] }];
                    let l = b.view.transform_point(p);
                    for k in 0..3 {
                        lo[k] = lo[k].min(l[k]);
                        hi[k] = hi[k].max(l[k]);
                    }
                }
                if hi[0] < b.center[0] - b.half_extent
                    || lo[0] > b.center[0] + b.half_extent
                    || hi[1] < b.center[1] - b.half_extent
                    || lo[1] > b.center[1] + b.half_extent
                    || hi[2] < b.z_min
                    || lo[2] > b.z_max
                {
                    continue;
                }
                instances.push(InstanceRaw { model: inst.world.0, bone_base: inst.bone_base, _pad: [0; 3] });
            }
            let count = instances.len() as u32 - first;
            if count == 0 {
                continue;
            }
            for (di, d) in list.draws.iter().enumerate() {
                let m = &self.materials[d.material];
                if !m.casts_shadow {
                    continue;
                }
                triangles += d.index_count as usize / 3 * count as usize;
                cmds.push(ShadowCmd {
                    skinned: d.skinned,
                    alpha: m.shadow_alpha,
                    material: d.material,
                    page: d.page,
                    draw: di,
                    list: li,
                    first_instance: first,
                    instance_count: count,
                });
            }
        }
        cmds.sort_by_key(|c| (c.alpha, c.skinned, c.material, c.page, c.list));
        ShadowPlan { instances, cmds, triangles }
    }
}

impl std::fmt::Debug for SceneGpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneGpu").field("stats", &self.stats).finish()
    }
}

#[allow(dead_code)]
fn _material_names(s: &SceneGpu) -> Vec<&str> {
    s.materials.iter().map(|m| m.name.as_str()).collect()
}
