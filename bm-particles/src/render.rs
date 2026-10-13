//! GPU rendering of particles inside `bm-render`'s main pass (feature `render`).
//!
//! ```ignore
//! let (hook, handle) = bm_particles::render::ParticleRenderer::new(Some(vfs.clone()));
//! renderer.set_hooks(Box::new(hook));
//! handle.register_images(&scene_particles.images);           // game textures by `tex:<name>`
//! // every frame:
//! system.update(dt);
//! handle.submit(&system, &camera);                           // builds + stores the sorted draw list
//! renderer.render(&mut encoder, &target, Some(&gpu), &camera, &params);   // hook draws the particles
//! handle.poll_hot_reload();                                  // ~1 Hz, optional
//! ```
//!
//! Instanced billboards (6 vertices per particle, 68 byte instances), one draw per run of equal
//! sprite + blend after depth sorting. Depth test only (reverse-Z `GreaterEqual`), no depth write.
//! The WGSL (`particles.wgsl`) is overridable through the VFS path [`SHADER_PATH`] and hot reloadable.
//! `EmitterDesc::soft_distance` fades particles near the camera on the CPU; soft fading against scene
//! depth is not implemented (the main pass cannot sample its own depth buffer; it would need a depth
//! pre-pass copy from bm-render).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bm_render::{Camera, HookContext, HookStage, RenderHooks};
use bm_vfs::Vfs;
use wgpu::util::DeviceExt;

use crate::desc::{BuiltinSprite, ParticleBlend};
use crate::sim::{ParticleInstance, ParticleSystem, RenderList, View};
use crate::sprites::{builtin_image, SpriteImage};

/// VFS path that overrides the builtin shader.
pub const SHADER_PATH: &str = "Shaders/WGSL/particles.wgsl";
/// Embedded source (template for modders).
pub const BUILTIN_SOURCE: &str = include_str!("particles.wgsl");

/// Camera basis of a `bm_render::Camera`.
pub fn view_from_camera(c: &Camera) -> View {
    View { position: c.position, right: c.right(), up: c.up(), forward: c.forward() }
}

struct Shared {
    list: RenderList,
    images: HashMap<String, Arc<SpriteImage>>,
    new_images: Vec<String>,
    /// Source waiting to be compiled by the hook.
    pending_shader: Option<String>,
    current_shader: String,
    shader_error: Option<String>,
    vfs: Option<Arc<Vfs>>,
    opacity: f32,
    stage: HookStage,
    enabled: bool,
    drawn_instances: u32,
    drawn_batches: u32,
}

/// Cheap clonable control handle: feed it draw lists and images while the renderer owns the hook.
#[derive(Clone)]
pub struct ParticleHandle {
    shared: Arc<Mutex<Shared>>,
}

impl ParticleHandle {
    /// Builds the sorted draw list of `system` for `camera` and stores it for the next frame.
    pub fn submit(&self, system: &ParticleSystem, camera: &Camera) {
        let mut list = RenderList::default();
        system.collect(&view_from_camera(camera), &mut list);
        self.submit_list(list);
    }

    /// Stores a ready-made list (for custom views).
    pub fn submit_list(&self, list: RenderList) {
        self.shared.lock().unwrap().list = list;
    }

    /// Registers (or replaces) an image under a `SpriteSource::key()` such as `tex:Effects/Smoke/Smoke_01_FX_H3`.
    pub fn register_image(&self, key: &str, image: SpriteImage) {
        let mut s = self.shared.lock().unwrap();
        s.images.insert(key.to_string(), Arc::new(image));
        s.new_images.push(key.to_string());
    }

    /// Registers decoded scene textures (`SceneParticles::images`).
    pub fn register_images(&self, images: &HashMap<String, Arc<bm_models::material::Image>>) {
        for (k, im) in images {
            self.register_image(k, SpriteImage { width: im.width, height: im.height, rgba: im.rgba.clone() });
        }
    }

    /// Global opacity multiplier (default 1).
    pub fn set_opacity(&self, o: f32) {
        self.shared.lock().unwrap().opacity = o;
    }

    pub fn set_enabled(&self, on: bool) {
        self.shared.lock().unwrap().enabled = on;
    }

    /// Which hook stage draws the particles (default `AfterTransparent`).
    pub fn set_stage(&self, stage: HookStage) {
        self.shared.lock().unwrap().stage = stage;
    }

    /// Re-reads `Shaders/WGSL/particles.wgsl` from the VFS; a changed file is compiled at the next draw, a
    /// broken one is reported via [`ParticleHandle::shader_error`] and the working shader stays.
    pub fn poll_hot_reload(&self) -> bool {
        let mut s = self.shared.lock().unwrap();
        let Some(vfs) = s.vfs.clone() else { return false };
        let Ok(bytes) = vfs.open(SHADER_PATH) else { return false };
        let Ok(text) = String::from_utf8(bytes) else { return false };
        if text != s.current_shader && s.pending_shader.as_deref() != Some(text.as_str()) {
            s.pending_shader = Some(text);
            return true;
        }
        false
    }

    pub fn shader_error(&self) -> Option<String> {
        self.shared.lock().unwrap().shader_error.clone()
    }

    /// `(instances, batches)` drawn in the last frame.
    pub fn last_frame(&self) -> (u32, u32) {
        let s = self.shared.lock().unwrap();
        (s.drawn_instances, s.drawn_batches)
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    params: [f32; 4],
}

struct GpuSprite {
    bind_group: wgpu::BindGroup,
}

/// The `RenderHooks` implementation. Create with [`ParticleRenderer::new`] and install the hook with
/// `Renderer::set_hooks`.
pub struct ParticleRenderer {
    shared: Arc<Mutex<Shared>>,
    gpu: Option<Gpu>,
}

struct Gpu {
    module: wgpu::ShaderModule,
    frame_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    frame_buf: wgpu::Buffer,
    frame_bg: wgpu::BindGroup,
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat, ParticleBlend), wgpu::RenderPipeline>,
    sprites: HashMap<String, GpuSprite>,
    fallback: GpuSprite,
    instances: Option<wgpu::Buffer>,
    instance_capacity: usize,
}

impl ParticleRenderer {
    /// Returns the hook and its control handle. With a `Vfs`, `Shaders/WGSL/particles.wgsl` and `vfs:` sprites resolve through it.
    pub fn new(vfs: Option<Arc<Vfs>>) -> (ParticleRenderer, ParticleHandle) {
        let mut source = BUILTIN_SOURCE.to_string();
        let mut pending = None;
        if let Some(v) = &vfs {
            if let Ok(b) = v.open(SHADER_PATH) {
                if let Ok(t) = String::from_utf8(b) {
                    if t != source {
                        pending = Some(t);
                    }
                    source = BUILTIN_SOURCE.to_string();
                }
            }
        }
        let shared = Arc::new(Mutex::new(Shared {
            list: RenderList::default(),
            images: HashMap::new(),
            new_images: Vec::new(),
            pending_shader: pending,
            current_shader: source,
            shader_error: None,
            vfs,
            opacity: 1.0,
            stage: HookStage::AfterTransparent,
            enabled: true,
            drawn_instances: 0,
            drawn_batches: 0,
        }));
        (ParticleRenderer { shared: shared.clone(), gpu: None }, ParticleHandle { shared })
    }
}

fn compile(device: &wgpu::Device, src: &str) -> Result<wgpu::ShaderModule, String> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(SHADER_PATH), source: wgpu::ShaderSource::Wgsl(src.into()) });
    match pollster::block_on(scope.pop()) {
        Some(e) => Err(e.to_string()),
        None => Ok(module),
    }
}

fn is_srgb(f: wgpu::TextureFormat) -> bool {
    f.is_srgb()
}

fn upload_sprite(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, im: &SpriteImage) -> GpuSprite {
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("particle sprite"),
            size: wgpu::Extent3d { width: im.width.max(1), height: im.height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &im.rgba,
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("particle sprite bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    });
    GpuSprite { bind_group }
}

fn builtin_by_key(key: &str) -> Option<BuiltinSprite> {
    let name = key.strip_prefix("builtin:")?;
    [BuiltinSprite::SoftCircle, BuiltinSprite::Puff, BuiltinSprite::Spark, BuiltinSprite::Droplet].into_iter().find(|b| format!("{b:?}") == name)
}

fn decode_png(bytes: &[u8]) -> Option<SpriteImage> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        _ => return None,
    };
    Some(SpriteImage { width: info.width, height: info.height, rgba })
}

impl Gpu {
    fn new(ctx: &HookContext, source: &str) -> Gpu {
        let device = ctx.device;
        let module = compile(device, source).unwrap_or_else(|e| {
            log::error!("particle shader rejected ({e}); using builtin");
            compile(device, BUILTIN_SOURCE).expect("builtin particle shader compiles")
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle frame layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle sprite layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle pipeline layout"),
            bind_group_layouts: &[Some(&frame_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("particle sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle frame bg"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() }],
        });
        let fallback = upload_sprite(device, ctx.queue, &tex_layout, &sampler, &builtin_image(BuiltinSprite::SoftCircle));
        Gpu {
            module,
            frame_layout,
            tex_layout,
            pipeline_layout,
            sampler,
            frame_buf,
            frame_bg,
            pipelines: HashMap::new(),
            sprites: HashMap::new(),
            fallback,
            instances: None,
            instance_capacity: 0,
        }
    }

    fn pipeline(&mut self, ctx: &HookContext, blend: ParticleBlend) -> wgpu::RenderPipeline {
        let key = (ctx.color_format, ctx.depth_format, blend);
        if let Some(p) = self.pipelines.get(&key) {
            return p.clone();
        }
        let attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 24, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 36, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 52, shader_location: 4 },
        ];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ParticleInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &attrs,
        })];
        let blend_state = match blend {
            ParticleBlend::Alpha => wgpu::BlendState::ALPHA_BLENDING,
            ParticleBlend::Additive => wgpu::BlendState {
                color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
                alpha: wgpu::BlendComponent::OVER,
            },
        };
        let constants = [("GAMMA_ENCODE", (!is_srgb(ctx.color_format)) as u32 as f64)];
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let p = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particle pipeline"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                targets: &[Some(wgpu::ColorTargetState { format: ctx.color_format, blend: Some(blend_state), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: ctx.depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            log::error!("particle pipeline failed: {e}");
        }
        self.pipelines.insert(key, p.clone());
        p
    }
}

impl RenderHooks for ParticleRenderer {
    fn draw_extra(&mut self, stage: HookStage, ctx: &HookContext, pass: &mut wgpu::RenderPass<'_>) {
        let mut sh = self.shared.lock().unwrap();
        if !sh.enabled || sh.stage != stage {
            return;
        }
        // (re)create GPU state / apply a shader change
        if self.gpu.is_none() {
            let src = sh.pending_shader.take().map(|s| {
                sh.current_shader = s.clone();
                s
            });
            let src = src.unwrap_or_else(|| sh.current_shader.clone());
            self.gpu = Some(Gpu::new(ctx, &src));
        } else if let Some(src) = sh.pending_shader.take() {
            match compile(ctx.device, &src) {
                Ok(m) => {
                    let g = self.gpu.as_mut().unwrap();
                    g.module = m;
                    g.pipelines.clear();
                    sh.current_shader = src;
                    sh.shader_error = None;
                }
                Err(e) => {
                    log::error!("particle shader edit rejected: {e}");
                    sh.current_shader = src; // do not retry the same broken text
                    sh.shader_error = Some(e);
                }
            }
        }
        let gpu = self.gpu.as_mut().unwrap();
        // drop sprites that were re-registered
        for k in std::mem::take(&mut sh.new_images) {
            gpu.sprites.remove(&k);
        }
        let n = sh.list.instances.len();
        if n == 0 {
            sh.drawn_instances = 0;
            sh.drawn_batches = 0;
            return;
        }
        // textures for every batch
        let keys: Vec<String> = sh.list.batches.iter().map(|b| b.sprite.to_string()).collect();
        for k in &keys {
            if gpu.sprites.contains_key(k) {
                continue;
            }
            let image: Option<SpriteImage> = if let Some(im) = sh.images.get(k) {
                Some((**im).clone())
            } else if let Some(b) = builtin_by_key(k) {
                Some(builtin_image(b))
            } else if let (Some(path), Some(vfs)) = (k.strip_prefix("vfs:"), sh.vfs.as_ref()) {
                vfs.open(path).ok().and_then(|b| decode_png(&b))
            } else {
                None
            };
            let sprite = match image {
                Some(im) => upload_sprite(ctx.device, ctx.queue, &gpu.tex_layout, &gpu.sampler, &im),
                None => {
                    log::warn!("particle sprite {k} not found, using the soft circle");
                    upload_sprite(ctx.device, ctx.queue, &gpu.tex_layout, &gpu.sampler, &builtin_image(BuiltinSprite::SoftCircle))
                }
            };
            gpu.sprites.insert(k.clone(), sprite);
        }
        // uniforms + instances
        let u = FrameUniform { view_proj: ctx.camera.view_proj(ctx.aspect).0, params: [sh.opacity, 2.2, 0.0, 0.0] };
        ctx.queue.write_buffer(&gpu.frame_buf, 0, bytemuck::bytes_of(&u));
        if gpu.instance_capacity < n {
            let cap = n.next_power_of_two().max(1024);
            gpu.instances = Some(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particle instances"),
                size: (cap * std::mem::size_of::<ParticleInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            gpu.instance_capacity = cap;
        }
        let ibuf = gpu.instances.clone().unwrap();
        ctx.queue.write_buffer(&ibuf, 0, bytemuck::cast_slice(&sh.list.instances));
        // draw
        pass.set_bind_group(0, &gpu.frame_bg, &[]);
        pass.set_vertex_buffer(0, ibuf.slice(..));
        let mut current: Option<ParticleBlend> = None;
        let batches = sh.list.batches.clone();
        for b in &batches {
            if current != Some(b.blend) {
                let p = gpu.pipeline(ctx, b.blend);
                pass.set_pipeline(&p);
                current = Some(b.blend);
            }
            let sprite = gpu.sprites.get(&*b.sprite).unwrap_or(&gpu.fallback);
            pass.set_bind_group(1, &sprite.bind_group, &[]);
            pass.draw(0..6, b.first..b.first + b.count);
        }
        sh.drawn_instances = n as u32;
        sh.drawn_batches = batches.len() as u32;
        let _ = &gpu.frame_layout;
    }
}
