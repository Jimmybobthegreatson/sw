//! The renderer core: pipelines, frame uniforms, shadow cascades, the main pass and the present pass. Takes any
//! `wgpu::Device`/`Queue` and a target format; knows nothing about windows.
//!
//! Frame structure: sun shadow cascades (depth only) -> main pass into an internal display-space `Rgba8Unorm` scene colour
//! texture (sky, opaque, transparent, hook draws) -> present pass (FXAA, display space -> target format) into the target.

use std::collections::HashMap;
use std::sync::Arc;

use bm_models::math::Mat4;
use bm_vfs::Vfs;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::camera::{self, Camera};
use crate::gpu_scene::{FrameStats, InstanceRaw, SceneGpu, ShadowBox, Vertex};
use crate::hooks::{HookContext, HookStage, RenderHooks};
use crate::params::FrameParams;
use crate::shaders::{self, ShaderLib};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Format of the internal main-pass target (display space values, see `main.wgsl`).
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const MAX_LIGHTS: usize = 16;
pub const MAX_CASCADES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlendClass {
    Opaque,
    Alpha,
    Add,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CullKind {
    None,
    Back,
}

/// Everything that selects a render pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PipelineKey {
    pub shader: usize,
    pub skinned: bool,
    pub alpha_test: bool,
    pub blend: BlendClass,
    pub depth_write: bool,
    pub cull: CullKind,
    /// Mirrored instance (negative determinant): flips the winding.
    pub flip: bool,
    pub z_bias: i32,
    /// Sky dome layer (drawn first, no depth test/write, centred on the camera).
    pub sky: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ShadowKey {
    shader: usize,
    skinned: bool,
    alpha: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightRaw {
    pos_range: [f32; 4],
    color_inv: [f32; 4],
    dir_cos: [f32; 4],
    spot: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameRaw {
    view_proj: [[f32; 4]; 4],
    cam_pos: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    cam_fwd: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    ambient_sky: [f32; 4],
    ambient_ground: [f32; 4],
    fog_color: [f32; 4],
    fog_params: [f32; 4],
    sky_top: [f32; 4],
    sky_horizon: [f32; 4],
    shadow_vp: [[[f32; 4]; 4]; MAX_CASCADES],
    shadow_splits: [f32; 4],
    shadow_params: [f32; 4],
    shadow_texel: [f32; 4],
    shadow_range: [f32; 4],
    lights: [LightRaw; MAX_LIGHTS],
}

pub(crate) struct Defaults {
    pub white: wgpu::TextureView,
    pub black: wgpu::TextureView,
    pub flat_normal: wgpu::TextureView,
}

/// Colour + depth views a frame is rendered into.
pub struct RenderTarget<'a> {
    pub color: &'a wgpu::TextureView,
    pub depth: &'a wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

/// Depth buffer helper (recreate on resize).
pub struct DepthTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl DepthTexture {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> DepthTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bm depth"),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        DepthTexture { texture, view, width, height }
    }
}

/// The cascaded shadow map: one 2D array texture with `MAX_CASCADES` layers.
struct ShadowMap {
    size: u32,
    #[allow(dead_code)]
    texture: wgpu::Texture,
    array_view: wgpu::TextureView,
    layers: Vec<wgpu::TextureView>,
}

struct SceneColor {
    width: u32,
    height: u32,
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub struct Renderer {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub color_format: wgpu::TextureFormat,
    pub(crate) shaders: ShaderLib,
    pub(crate) frame_layout: wgpu::BindGroupLayout,
    pub(crate) material_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sky_layout: wgpu::PipelineLayout,
    shadow_layout_plain: wgpu::PipelineLayout,
    shadow_layout_alpha: wgpu::PipelineLayout,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    shadow_pipelines: HashMap<ShadowKey, wgpu::RenderPipeline>,
    sky_pipeline: Option<wgpu::RenderPipeline>,
    post_layout: wgpu::BindGroupLayout,
    post_pipelines: HashMap<bool, wgpu::RenderPipeline>,
    post_sampler: wgpu::Sampler,
    pub(crate) defaults: Defaults,
    dummy_bones: wgpu::Buffer,
    dummy_shadow: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    shadow_map: Option<ShadowMap>,
    scene_color: Option<SceneColor>,
    pub(crate) hooks: Option<Box<dyn RenderHooks>>,
    vfs: Option<Arc<Vfs>>,
    /// Statistics of the last `render` call.
    pub last_frame: FrameStats,
    /// Errors of the last hot reload attempt (cleared on success).
    pub shader_errors: Vec<String>,
}

fn tex1(device: &wgpu::Device, queue: &wgpu::Queue, fmt: wgpu::TextureFormat, px: [u8; 4]) -> wgpu::TextureView {
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("bm default texture"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &px,
        )
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// One lit-and-shadowed frame: the orthographic matrix of every cascade, split distances and culling boxes.
struct Cascade {
    vp: Mat4,
    box_: ShadowBox,
    texel: f32,
    range: f32,
    far_depth: f32,
}

fn ortho_cascades(camera: &Camera, aspect: f32, sun_dir: camera::Vec3, count: usize, distance: f32, size: u32) -> Vec<Cascade> {
    // light basis (left-handed): z along the direction the light travels
    let d = camera::normalize([-sun_dir[0], -sun_dir[1], -sun_dir[2]]);
    let helper = if d[1].abs() > 0.95 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let right = camera::normalize(camera::cross(helper, d));
    let up = camera::cross(d, right);
    let view = Mat4([
        [right[0], up[0], d[0], 0.0],
        [right[1], up[1], d[1], 0.0],
        [right[2], up[2], d[2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let near = camera.near.max(5.0);
    let distance = distance.max(near * 4.0);
    let tan_y = (camera.fov_y * 0.5).tan();
    let tan_x = tan_y * aspect;
    let (cf, cr, cu) = (camera.forward(), camera.right(), camera.up());
    // practical split scheme: a blend of uniform and logarithmic
    let split = |i: usize| -> f32 {
        let t = i as f32 / count as f32;
        let uni = near + (distance - near) * t;
        let log = near * (distance / near).powf(t);
        uni * 0.4 + log * 0.6
    };
    let mut out = Vec::new();
    for i in 0..count {
        let (n, f) = (split(i), split(i + 1));
        let mut pts = [[0.0f32; 3]; 8];
        for (k, p) in pts.iter_mut().enumerate() {
            let dd = if k & 4 == 0 { n } else { f };
            let sx = if k & 1 == 0 { -1.0 } else { 1.0 };
            let sy = if k & 2 == 0 { -1.0 } else { 1.0 };
            *p = [
                camera.position[0] + cf[0] * dd + cr[0] * dd * tan_x * sx + cu[0] * dd * tan_y * sy,
                camera.position[1] + cf[1] * dd + cr[1] * dd * tan_x * sx + cu[1] * dd * tan_y * sy,
                camera.position[2] + cf[2] * dd + cr[2] * dd * tan_x * sx + cu[2] * dd * tan_y * sy,
            ];
        }
        // bounding sphere (rotation invariant -> no shimmering)
        let mut c = [0.0f32; 3];
        for p in &pts {
            for k in 0..3 {
                c[k] += p[k] / 8.0;
            }
        }
        let mut r = 0.0f32;
        for p in &pts {
            r = r.max(camera::length(camera::sub(*p, c)));
        }
        r = (r * 16.0).ceil() / 16.0;
        let texel = 2.0 * r / size as f32;
        let mut cl = view.transform_point(c);
        cl[0] = (cl[0] / texel).floor() * texel;
        cl[1] = (cl[1] / texel).floor() * texel;
        // casters may sit far towards the sun (tall buildings, trees)
        let extra = 6000.0;
        let z_min = cl[2] - r - extra;
        let z_max = cl[2] + r + 200.0;
        let range = z_max - z_min;
        let ortho = Mat4([
            [1.0 / r, 0.0, 0.0, 0.0],
            [0.0, 1.0 / r, 0.0, 0.0],
            [0.0, 0.0, 1.0 / range, 0.0],
            [-cl[0] / r, -cl[1] / r, -z_min / range, 1.0],
        ]);
        out.push(Cascade {
            vp: ortho.mul(&view),
            box_: ShadowBox { view, center: [cl[0], cl[1]], half_extent: r, z_min, z_max },
            texel,
            range,
            far_depth: f,
        });
    }
    out
}

impl Renderer {
    /// `color_format` is the format of the target views passed to `render`. Any format works: the present pass converts the
    /// internal display-space image to it (sRGB targets get a linear conversion, other formats the display values).
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, color_format: wgpu::TextureFormat) -> Renderer {
        Renderer::with_vfs(device, queue, color_format, None)
    }

    /// Like [`Renderer::new`] but resolves `Shaders/WGSL/*.wgsl` overrides through the VFS.
    pub fn with_vfs(
        device: wgpu::Device,
        queue: wgpu::Queue,
        color_format: wgpu::TextureFormat,
        vfs: Option<Arc<Vfs>>,
    ) -> Renderer {
        let shaders = ShaderLib::new(&device, vfs.as_ref());
        let vis_all = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bm frame layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: vis_all,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bm material layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(2),
                tex_entry(3),
                tex_entry(4),
                tex_entry(5),
                tex_entry(6),
                tex_entry(7),
                tex_entry(8),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bm pipeline layout"),
            bind_group_layouts: &[Some(&frame_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bm sky layout"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        let shadow_layout_plain = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bm shadow layout"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        let shadow_layout_alpha = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bm shadow alpha layout"),
            bind_group_layouts: &[Some(&frame_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bm present layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(1),
            ],
        });
        let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bm present sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bm shadow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let defaults = Defaults {
            white: tex1(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, [255; 4]),
            black: tex1(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, [0, 0, 0, 255]),
            flat_normal: tex1(&device, &queue, wgpu::TextureFormat::Rgba8Unorm, [128, 128, 255, 255]),
        };
        let dummy_bones = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm dummy bones"),
            contents: bytemuck::cast_slice(&[bm_models::Mat4::IDENTITY.to_cols_array()]),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let dummy_shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bm dummy shadow"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let dummy_shadow = dummy_shadow_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        Renderer {
            device,
            queue,
            color_format,
            shaders,
            frame_layout,
            material_layout,
            pipeline_layout,
            sky_layout,
            shadow_layout_plain,
            shadow_layout_alpha,
            pipelines: HashMap::new(),
            shadow_pipelines: HashMap::new(),
            sky_pipeline: None,
            post_layout,
            post_pipelines: HashMap::new(),
            post_sampler,
            defaults,
            dummy_bones,
            dummy_shadow,
            shadow_sampler,
            shadow_map: None,
            scene_color: None,
            hooks: None,
            vfs,
            last_frame: FrameStats::default(),
            shader_errors: Vec::new(),
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Installs the extension hooks (see [`RenderHooks`]). Must be set before `SceneGpu::upload` for
    /// `material` overrides to take effect.
    pub fn set_hooks(&mut self, hooks: Box<dyn RenderHooks>) {
        self.hooks = Some(hooks);
    }

    pub fn clear_hooks(&mut self) {
        self.hooks = None;
    }

    pub fn attach_vfs(&mut self, vfs: Arc<Vfs>) {
        self.vfs = Some(vfs);
    }

    /// Loads a custom WGSL module through the VFS (builtin shader if it is missing or invalid).
    pub fn ensure_shader(&mut self, path: &str) -> usize {
        self.shaders.ensure(&self.device, self.vfs.as_ref(), path).unwrap_or(shaders::BUILTIN)
    }

    /// Re-reads all shader sources from the VFS (mods can edit `Shaders/WGSL/*.wgsl` while the game runs)
    /// and rebuilds the pipelines when something changed. Cheap enough to call about once a second.
    /// Returns true if pipelines were rebuilt; compile errors are logged and kept in `shader_errors`
    /// while the previous working version stays active.
    pub fn poll_hot_reload(&mut self) -> bool {
        let Some(vfs) = self.vfs.clone() else { return false };
        let (changed, errors) = self.shaders.reload(&self.device, &vfs);
        if !errors.is_empty() {
            for e in &errors {
                log::error!("shader reload failed: {e}");
            }
            self.shader_errors = errors;
        } else if changed {
            self.shader_errors.clear();
        }
        if changed {
            self.pipelines.clear();
            self.shadow_pipelines.clear();
            self.post_pipelines.clear();
            self.sky_pipeline = None;
            log::info!("shaders reloaded");
        }
        changed
    }

    fn vertex_layouts() -> ([wgpu::VertexAttribute; 7], [wgpu::VertexAttribute; 5]) {
        use wgpu::VertexFormat as F;
        let v = [
            wgpu::VertexAttribute { format: F::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: F::Float32x3, offset: 12, shader_location: 1 },
            wgpu::VertexAttribute { format: F::Float32x4, offset: 24, shader_location: 2 },
            wgpu::VertexAttribute { format: F::Float32x2, offset: 40, shader_location: 3 },
            wgpu::VertexAttribute { format: F::Unorm8x4, offset: 48, shader_location: 4 },
            wgpu::VertexAttribute { format: F::Uint16x4, offset: 52, shader_location: 5 },
            wgpu::VertexAttribute { format: F::Float32x4, offset: 60, shader_location: 6 },
        ];
        let i = [
            wgpu::VertexAttribute { format: F::Float32x4, offset: 0, shader_location: 8 },
            wgpu::VertexAttribute { format: F::Float32x4, offset: 16, shader_location: 9 },
            wgpu::VertexAttribute { format: F::Float32x4, offset: 32, shader_location: 10 },
            wgpu::VertexAttribute { format: F::Float32x4, offset: 48, shader_location: 11 },
            wgpu::VertexAttribute { format: F::Uint32, offset: 64, shader_location: 12 },
        ];
        (v, i)
    }

    fn build_pipeline(&self, key: &PipelineKey, shader: usize) -> wgpu::RenderPipeline {
        let module = &self.shaders.entries[shader].module;
        let (va, ia) = Self::vertex_layouts();
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &va,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<InstanceRaw>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &ia,
            }),
        ];
        let blend = match key.blend {
            BlendClass::Opaque => None,
            BlendClass::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
            BlendClass::Add => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
        };
        let constants = [
            ("SKINNED", key.skinned as u32 as f64),
            ("ALPHA_TEST", key.alpha_test as u32 as f64),
            ("SKY_LAYER", key.sky as u32 as f64),
        ];
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bm pipeline"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Seen from the side the vertex normal points to, Glacier triangles run counter-clockwise on screen
                // (`cross(b-a, c-a) . n < 0` in the left-handed data space), so they are front faces for `Ccw`.
                front_face: if key.flip { wgpu::FrontFace::Cw } else { wgpu::FrontFace::Ccw },
                cull_mode: match key.cull {
                    CullKind::None => None,
                    CullKind::Back => Some(wgpu::Face::Back),
                },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(key.depth_write && !key.sky),
                depth_compare: Some(if key.sky { wgpu::CompareFunction::Always } else { wgpu::CompareFunction::GreaterEqual }),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: key.z_bias, slope_scale: 0.0, clamp: 0.0 },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    fn pipeline(&mut self, key: PipelineKey) -> wgpu::RenderPipeline {
        if let Some(p) = self.pipelines.get(&key) {
            return p.clone();
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut p = self.build_pipeline(&key, key.shader);
        if let Some(e) = pollster::block_on(scope.pop()) {
            log::error!("pipeline for shader {} failed ({e}); using the builtin shader", self.shaders.entries[key.shader].path);
            p = self.build_pipeline(&key, shaders::BUILTIN);
        }
        self.pipelines.insert(key, p.clone());
        p
    }

    fn shadow_pipeline(&mut self, key: ShadowKey) -> wgpu::RenderPipeline {
        if let Some(p) = self.shadow_pipelines.get(&key) {
            return p.clone();
        }
        let module = &self.shaders.entries[key.shader].module;
        let (va, ia) = Self::vertex_layouts();
        let buffers = [
            Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &va }),
            Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<InstanceRaw>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &ia }),
        ];
        let constants = [("SKINNED", key.skinned as u32 as f64)];
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bm shadow pipeline"),
            layout: Some(if key.alpha { &self.shadow_layout_alpha } else { &self.shadow_layout_plain }),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_shadow"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                buffers: &buffers,
            },
            fragment: if key.alpha {
                Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_shadow"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                    targets: &[],
                })
            } else {
                None
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 1, slope_scale: 1.5, clamp: 0.0 },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            log::error!("shadow pipeline failed: {e}");
        }
        self.shadow_pipelines.insert(key, p.clone());
        p
    }

    fn sky_pipeline(&mut self) -> wgpu::RenderPipeline {
        if let Some(p) = &self.sky_pipeline {
            return p.clone();
        }
        let module = &self.shaders.entries[shaders::BUILTIN].module;
        let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bm sky"),
            layout: Some(&self.sky_layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.sky_pipeline = Some(p.clone());
        p
    }

    fn post_pipeline(&mut self, fxaa: bool) -> wgpu::RenderPipeline {
        if let Some(p) = self.post_pipelines.get(&fxaa) {
            return p.clone();
        }
        let module = &self.shaders.entries[shaders::POST].module;
        let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bm present pipeline layout"),
            bind_group_layouts: &[Some(&self.post_layout)],
            immediate_size: 0,
        });
        let constants = [("TARGET_SRGB", self.color_format.is_srgb() as u32 as f64), ("FXAA", fxaa as u32 as f64)];
        let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bm present"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_post"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_post"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                targets: &[Some(wgpu::ColorTargetState { format: self.color_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.post_pipelines.insert(fxaa, p.clone());
        p
    }

    fn ensure_scene_color(&mut self, w: u32, h: u32) {
        if self.scene_color.as_ref().is_some_and(|s| s.width == w && s.height == h) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bm scene color"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.scene_color = Some(SceneColor { width: w, height: h, texture, view });
    }

    fn ensure_shadow_map(&mut self, size: u32) {
        if self.shadow_map.as_ref().is_some_and(|s| s.size == size) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bm shadow map"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: MAX_CASCADES as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let layers = (0..MAX_CASCADES as u32)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        self.shadow_map = Some(ShadowMap { size, texture, array_view, layers });
    }

    fn frame_uniform(&self, camera: &Camera, p: &FrameParams, aspect: f32, cascades: &[Cascade], resolution: u32) -> FrameRaw {
        let vp = camera.view_proj(aspect);
        let t = (camera.fov_y * 0.5).tan();
        let v4 = |v: [f32; 3], w: f32| [v[0], v[1], v[2], w];
        let sc = camera::scale;
        let mut lights = [LightRaw::zeroed(); MAX_LIGHTS];
        // the lights whose range reaches closest to the camera
        let mut order: Vec<(f32, usize)> = p
            .lights
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let d = camera::length(camera::sub(l.position, camera.position));
                (d - l.radius, i)
            })
            .collect();
        order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut count = 0;
        for (_, i) in order.into_iter().take(MAX_LIGHTS) {
            let l = &p.lights[i];
            let span = (l.radius - l.range_start).max(1.0);
            let (spot, cos_outer, inv_cos) = match l.cone {
                Some((co, ci)) => (1.0, co, 1.0 / (ci - co).max(1e-3)),
                None => (0.0, 0.0, 0.0),
            };
            lights[count] = LightRaw {
                pos_range: v4(l.position, l.radius),
                color_inv: v4(l.color, 1.0 / span),
                dir_cos: v4(camera::normalize(l.direction), cos_outer),
                spot: [spot, inv_cos, 0.0, 0.0],
            };
            count += 1;
        }
        let mut shadow_vp = [[[0.0f32; 4]; 4]; MAX_CASCADES];
        let mut splits = [1e9f32; 4];
        let mut texel = [1.0f32; 4];
        let mut range = [1.0f32; 4];
        for (i, c) in cascades.iter().enumerate().take(MAX_CASCADES) {
            shadow_vp[i] = c.vp.0;
            splits[i] = c.far_depth;
            texel[i] = c.texel;
            range[i] = c.range;
        }
        splits[3] = cascades.len().min(MAX_CASCADES) as f32;
        let sh = &p.shadows;
        FrameRaw {
            view_proj: vp.0,
            cam_pos: v4(camera.position, p.time),
            cam_right: v4(sc(camera.right(), t * aspect), 0.0),
            cam_up: v4(sc(camera.up(), t), 0.0),
            cam_fwd: v4(camera.forward(), 0.0),
            sun_dir: v4(camera::normalize(p.sun_dir), p.debug_mode as f32),
            sun_color: v4(p.sun_color, p.exposure),
            ambient_sky: v4(p.ambient_sky, p.reflections),
            ambient_ground: v4(p.ambient_ground, 0.0),
            fog_color: v4(p.fog_color, if p.fog_end > p.fog_start { p.fog_end } else { 0.0 }),
            fog_params: [p.fog_start, p.vertex_color_strength, count as f32, 0.0],
            sky_top: v4(p.sky_top, p.sky_gain),
            sky_horizon: v4(p.sky_horizon, 0.0),
            shadow_vp,
            shadow_splits: splits,
            shadow_params: [sh.strength, sh.depth_bias, sh.normal_bias, 1.0 / resolution.max(1) as f32],
            shadow_texel: texel,
            shadow_range: range,
            lights,
        }
    }

    fn frame_bind_group(&self, buf: &wgpu::Buffer, bones: &wgpu::Buffer, shadow: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bm frame bind group"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: bones.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(shadow) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.shadow_sampler) },
            ],
        })
    }

    /// Records a frame into `encoder`: shadow cascades, clear, sky, sorted opaque pass, transparent passes, then the present
    /// pass into `target`. `scene` may be `None` (sky only). One call per submit is recommended (all per-frame buffers are
    /// created inside and are therefore safe to use for several renders per queue submission).
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
        scene: Option<&SceneGpu>,
        camera: &Camera,
        params: &FrameParams,
    ) {
        let mut camera = camera.clone();
        let mut params = params.clone();
        if let Some(h) = self.hooks.as_mut() {
            h.before_frame(&mut camera, &mut params);
        }
        let aspect = target.width.max(1) as f32 / target.height.max(1) as f32;
        self.ensure_scene_color(target.width, target.height);

        // ---- sun shadow cascades ---------------------------------------------------------------------------------
        let sun_on = params.sun_color.iter().cloned().fold(0.0f32, f32::max) > 0.02;
        let shadows_on = params.shadows.enabled && sun_on && scene.is_some() && params.debug_mode != 1;
        let resolution = params.shadows.resolution.clamp(256, 4096);
        let n_cascades = (params.shadows.cascades.clamp(1, MAX_CASCADES as u32)) as usize;
        let cascades = if shadows_on { ortho_cascades(&camera, aspect, params.sun_dir, n_cascades, params.shadows.distance, resolution) } else { Vec::new() };
        if shadows_on {
            self.ensure_shadow_map(resolution);
        }
        let raw = self.frame_uniform(&camera, &params, aspect, &cascades, resolution);
        let frame_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm frame uniform"),
            contents: bytemuck::bytes_of(&raw),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bones: wgpu::Buffer = scene.map(|s| s.bones.clone()).unwrap_or_else(|| self.dummy_bones.clone());
        let mut stats = FrameStats::default();
        if let (true, Some(scene)) = (shadows_on, scene) {
            for (ci, c) in cascades.iter().enumerate() {
                let plan = scene.plan_shadow(&c.box_);
                stats.shadow_cascades += 1;
                // frame uniform of this cascade: the light matrix in `view_proj`; the (unused) shadow input is the dummy texture
                let mut craw = raw;
                craw.view_proj = c.vp.0;
                let cbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("bm shadow frame uniform"),
                    contents: bytemuck::bytes_of(&craw),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let cbg = self.frame_bind_group(&cbuf, &bones, &self.dummy_shadow);
                let ibuf = (!plan.instances.is_empty()).then(|| {
                    self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("bm shadow instances"),
                        contents: bytemuck::cast_slice(&plan.instances),
                        usage: wgpu::BufferUsages::VERTEX,
                    })
                });
                let mut pipes: HashMap<ShadowKey, wgpu::RenderPipeline> = HashMap::new();
                for cmd in &plan.cmds {
                    let key = ShadowKey { shader: shaders::BUILTIN, skinned: cmd.skinned, alpha: cmd.alpha };
                    if !pipes.contains_key(&key) {
                        let p = self.shadow_pipeline(key);
                        pipes.insert(key, p);
                    }
                }
                let layer = &self.shadow_map.as_ref().expect("shadow map").layers[ci];
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("bm shadow pass"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: layer,
                            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    })
                    .forget_lifetime();
                pass.set_bind_group(0, &cbg, &[]);
                if let Some(ibuf) = &ibuf {
                    pass.set_vertex_buffer(1, ibuf.slice(..));
                    let mut cur_key: Option<ShadowKey> = None;
                    let mut cur_mat = usize::MAX;
                    let mut cur_page = usize::MAX;
                    for cmd in &plan.cmds {
                        let key = ShadowKey { shader: shaders::BUILTIN, skinned: cmd.skinned, alpha: cmd.alpha };
                        if cur_key != Some(key) {
                            pass.set_pipeline(&pipes[&key]);
                            cur_key = Some(key);
                            cur_mat = usize::MAX;
                        }
                        if cmd.alpha && cur_mat != cmd.material {
                            pass.set_bind_group(1, &scene.materials[cmd.material].bind_group, &[]);
                            cur_mat = cmd.material;
                        }
                        let d = &scene.lists[cmd.list].draws[cmd.draw];
                        if cur_page != d.page {
                            let page = &scene.pages[d.page];
                            pass.set_vertex_buffer(0, page.vbuf.slice(..));
                            pass.set_index_buffer(page.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                            cur_page = d.page;
                        }
                        pass.draw_indexed(d.first_index..d.first_index + d.index_count, d.base_vertex, cmd.first_instance..cmd.first_instance + cmd.instance_count);
                        stats.shadow_draw_calls += 1;
                    }
                }
                stats.shadow_triangles += plan.triangles;
                drop(pass);
            }
        }

        // ---- main pass --------------------------------------------------------------------------------------------
        let shadow_view = if shadows_on { &self.shadow_map.as_ref().expect("shadow map").array_view } else { &self.dummy_shadow };
        let frame_bg = self.frame_bind_group(&frame_buf, &bones, shadow_view);

        let plan = scene.map(|s| s.plan(&camera.frustum(aspect), camera.position, params.max_draw_distance));
        let mut fstats = plan.as_ref().map(|p| p.stats).unwrap_or_default();
        fstats.lights_used = raw.fog_params[2] as usize;
        fstats.shadow_cascades = stats.shadow_cascades;
        fstats.shadow_draw_calls = stats.shadow_draw_calls;
        fstats.shadow_triangles = stats.shadow_triangles;
        let mut stats = fstats;
        if let (Ok(_), Some(p), Some(sc)) = (std::env::var("BM_DEBUG_DRAWS"), &plan, scene) {
            // debugging aid: material names of this frame's draws (order, draw count, instance count)
            let mut seen: std::collections::BTreeMap<(u8, String), (usize, u32)> = Default::default();
            for c in &p.cmds {
                let e = seen.entry((c.order, sc.materials[c.material].name.clone())).or_default();
                e.0 += 1;
                e.1 += c.instance_count;
            }
            for ((order, name), (n, inst)) in seen {
                eprintln!("draw order {order} '{name}': {n} draws, {inst} instances");
            }
        }
        let instance_buf = plan.as_ref().filter(|p| !p.instances.is_empty()).map(|p| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bm instances"),
                contents: bytemuck::cast_slice(&p.instances),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        // make sure all pipelines exist before the pass starts
        let mut pipes: HashMap<PipelineKey, wgpu::RenderPipeline> = HashMap::new();
        if let Some(p) = &plan {
            for c in &p.cmds {
                if !pipes.contains_key(&c.key) {
                    let pl = self.pipeline(c.key);
                    pipes.insert(c.key, pl);
                }
            }
        }
        let sky = if params.draw_sky { Some(self.sky_pipeline()) } else { None };

        let clear = if params.draw_sky {
            wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }
        } else {
            let c = params.clear_color;
            wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 }
        };
        let scene_view = self.scene_color.as_ref().expect("scene colour").view.clone();
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bm main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &scene_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        pass.set_bind_group(0, &frame_bg, &[]);
        if let Some(sky) = &sky {
            pass.set_pipeline(sky);
            pass.draw(0..3, 0..1);
        }

        let ctx = HookContext {
            device: &self.device,
            queue: &self.queue,
            camera: &camera,
            params: &params,
            frame_bind_group_layout: &self.frame_layout,
            frame_bind_group: &frame_bg,
            color_format: SCENE_FORMAT,
            depth_format: DEPTH_FORMAT,
            aspect,
        };

        if let (Some(plan), Some(scene), Some(ibuf)) = (&plan, scene, &instance_buf) {
            let mut cur_key: Option<PipelineKey> = None;
            let mut cur_mat = usize::MAX;
            let mut cur_page = usize::MAX;
            let mut extra_after_opaque = false;
            pass.set_vertex_buffer(1, ibuf.slice(..));
            for c in &plan.cmds {
                if c.order > 1 && !extra_after_opaque {
                    extra_after_opaque = true;
                    if let Some(h) = self.hooks.as_mut() {
                        h.draw_extra(HookStage::AfterOpaque, &ctx, &mut pass);
                    }
                    // hooks may change state: re-establish everything
                    pass.set_bind_group(0, &frame_bg, &[]);
                    pass.set_vertex_buffer(1, ibuf.slice(..));
                    cur_key = None;
                    cur_mat = usize::MAX;
                    cur_page = usize::MAX;
                }
                if cur_key != Some(c.key) {
                    pass.set_pipeline(&pipes[&c.key]);
                    cur_key = Some(c.key);
                    stats.pipelines_switches += 1;
                }
                if cur_mat != c.material {
                    pass.set_bind_group(1, &scene.materials[c.material].bind_group, &[]);
                    cur_mat = c.material;
                }
                let d = &scene.lists[c.list].draws[c.draw];
                if cur_page != d.page {
                    let page = &scene.pages[d.page];
                    pass.set_vertex_buffer(0, page.vbuf.slice(..));
                    pass.set_index_buffer(page.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    cur_page = d.page;
                }
                pass.draw_indexed(
                    d.first_index..d.first_index + d.index_count,
                    d.base_vertex,
                    c.first_instance..c.first_instance + c.instance_count,
                );
            }
            if !extra_after_opaque {
                if let Some(h) = self.hooks.as_mut() {
                    h.draw_extra(HookStage::AfterOpaque, &ctx, &mut pass);
                }
            }
        } else if let Some(h) = self.hooks.as_mut() {
            h.draw_extra(HookStage::AfterOpaque, &ctx, &mut pass);
        }
        if let Some(h) = self.hooks.as_mut() {
            h.draw_extra(HookStage::AfterTransparent, &ctx, &mut pass);
        }
        drop(pass);

        // ---- present pass ----------------------------------------------------------------------------------------
        let post = self.post_pipeline(params.fxaa);
        let post_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bm present bind group"),
            layout: &self.post_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&self.post_sampler) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&scene_view) },
            ],
        });
        let mut pp = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bm present pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        pp.set_pipeline(&post);
        pp.set_bind_group(0, &post_bg, &[]);
        pp.draw(0..3, 0..1);
        drop(pp);
        self.last_frame = stats;
    }
}
