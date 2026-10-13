//! wgpu back end for [`DrawList`]: a UI pass that is recorded *after* the 3D pass of `bm-render`.
//!
//! * [`UiRenderer::render`] records one render pass over an existing colour view (`LoadOp::Load`), so the 3D scene
//!   (or a menu background) stays underneath.
//! * The WGSL module is `src/shaders/ui.wgsl`; a loose `Shaders/WGSL/ui.wgsl` in the VFS overrides it
//!   ([`UI_SHADER_PATH`]) and [`UiRenderer::poll_hot_reload`] re-reads it (broken edits keep the old shader).
//! * [`UiOffscreen`] renders (optionally a 3D scene first) into a texture and reads it back - used for screenshots
//!   and tests.

use std::collections::HashMap;
use std::sync::Arc;

use bm_render::{Camera, DepthTexture, FrameParams, RenderTarget, Renderer, SceneGpu};
use bm_vfs::Vfs;
use wgpu::util::DeviceExt;

use crate::draw::{Blend, DrawList, Vertex};
use crate::texture::{TextureId, TextureStore};

pub const UI_SHADER_PATH: &str = "Shaders/WGSL/ui.wgsl";
pub const BUILTIN_SHADER: &str = include_str!("shaders/ui.wgsl");

struct GpuTexture {
    bind_group: wgpu::BindGroup,
    version: u32,
}

pub struct UiRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    screen_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    source: String,
    pipelines: HashMap<Blend, wgpu::RenderPipeline>,
    textures: HashMap<TextureId, GpuTexture>,
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
    vfs: Option<Arc<Vfs>>,
    /// Last shader compile error from a hot reload (`None` when the active shader is healthy).
    pub shader_error: Option<String>,
    /// Draw calls issued by the last `render`.
    pub last_draw_calls: usize,
}

fn read_override(vfs: &Vfs) -> Option<String> {
    let bytes = vfs.open(UI_SHADER_PATH).ok()?;
    String::from_utf8(bytes).ok()
}

impl UiRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat, vfs: Option<Arc<Vfs>>) -> UiRenderer {
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bm-ui screen layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bm-ui texture layout"),
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
            label: Some("bm-ui pipeline layout"),
            bind_group_layouts: &[Some(&screen_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let sampler = |f: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("bm-ui sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: f,
                min_filter: f,
                ..Default::default()
            })
        };
        let linear = sampler(wgpu::FilterMode::Linear);
        let nearest = sampler(wgpu::FilterMode::Nearest);
        let mut source = BUILTIN_SHADER.to_string();
        if let Some(v) = &vfs {
            if let Some(s) = read_override(v) {
                source = s;
            }
        }
        let module = match bm_render::shaders::compile(&device, UI_SHADER_PATH, &source) {
            Ok(m) => m,
            Err(e) => {
                log::error!("ui shader override rejected: {e}");
                source = BUILTIN_SHADER.to_string();
                bm_render::shaders::compile(&device, UI_SHADER_PATH, &source).expect("builtin ui shader must compile")
            }
        };
        UiRenderer {
            device,
            queue,
            format,
            screen_layout,
            tex_layout,
            pipeline_layout,
            module,
            source,
            pipelines: HashMap::new(),
            textures: HashMap::new(),
            linear,
            nearest,
            vfs,
            shader_error: None,
            last_draw_calls: 0,
        }
    }

    /// Creates the renderer for the same device/queue/format as a `bm_render::Renderer`.
    pub fn for_renderer(r: &Renderer, format: wgpu::TextureFormat, vfs: Option<Arc<Vfs>>) -> UiRenderer {
        UiRenderer::new(r.device().clone(), r.queue().clone(), format, vfs)
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Re-reads `Shaders/WGSL/ui.wgsl` from the VFS; returns true if the pipelines were rebuilt.
    pub fn poll_hot_reload(&mut self) -> bool {
        let Some(vfs) = self.vfs.clone() else { return false };
        let new_src = read_override(&vfs).unwrap_or_else(|| BUILTIN_SHADER.to_string());
        if new_src == self.source {
            return false;
        }
        match bm_render::shaders::compile(&self.device, UI_SHADER_PATH, &new_src) {
            Ok(m) => {
                self.module = m;
                self.source = new_src;
                self.pipelines.clear();
                self.shader_error = None;
                log::info!("ui shader reloaded");
                true
            }
            Err(e) => {
                log::error!("ui shader reload failed: {e}");
                self.source = new_src; // do not re-report every poll
                self.shader_error = Some(e);
                false
            }
        }
    }

    fn pipeline(&mut self, blend: Blend) -> wgpu::RenderPipeline {
        if let Some(p) = self.pipelines.get(&blend) {
            return p.clone();
        }
        let constants = [("SRGB_TARGET", self.format.is_srgb() as u32 as f64)];
        let blend_state = match blend {
            Blend::Alpha => wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            },
            Blend::Additive => wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            },
        };
        let attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 2 },
        ];
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let build = |module: &wgpu::ShaderModule| {
            self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("bm-ui pipeline"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                    targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: Some(blend_state), write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let mut p = build(&self.module);
        if let Some(e) = pollster::block_on(scope.pop()) {
            log::error!("ui pipeline failed ({e}); falling back to the builtin shader");
            self.shader_error = Some(e.to_string());
            if let Ok(m) = bm_render::shaders::compile(&self.device, UI_SHADER_PATH, BUILTIN_SHADER) {
                self.module = m;
                self.source = BUILTIN_SHADER.to_string();
            }
            p = build(&self.module);
        }
        self.pipelines.insert(blend, p.clone());
        p
    }

    fn sync_texture(&mut self, id: TextureId, store: &TextureStore) {
        let ver = store.version(id);
        if self.textures.get(&id).is_some_and(|t| t.version == ver) {
            return;
        }
        let Some(img) = store.get(id) else { return };
        let tex = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some(&img.label),
                size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &img.rgba,
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bm-ui texture bg"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(if img.nearest { &self.nearest } else { &self.linear }) },
            ],
        });
        self.textures.insert(id, GpuTexture { bind_group, version: ver });
    }

    /// Number of textures currently resident on the GPU.
    pub fn resident_textures(&self) -> usize {
        self.textures.len()
    }

    /// Records the UI pass over `target` (`LoadOp::Load`: whatever is there stays). `size` is the target size in
    /// pixels; vertex positions in `list` are in the same pixel space.
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, size: (u32, u32), list: &DrawList, store: &TextureStore) {
        self.last_draw_calls = 0;
        if list.is_empty() {
            return;
        }
        for c in &list.cmds {
            self.sync_texture(c.texture, store);
        }
        let pipes: Vec<(Blend, wgpu::RenderPipeline)> = [Blend::Alpha, Blend::Additive].into_iter().map(|b| (b, self.pipeline(b))).collect();
        let screen = [size.0 as f32, size.1 as f32, 0.0, 0.0];
        let screen_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm-ui screen"),
            contents: bytemuck::cast_slice(&screen),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let screen_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bm-ui screen bg"),
            layout: &self.screen_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: screen_buf.as_entire_binding() }],
        });
        let vbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm-ui vertices"),
            contents: bytemuck::cast_slice(&list.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bm-ui indices"),
            contents: bytemuck::cast_slice(&list.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bm-ui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        pass.set_bind_group(0, &screen_bg, &[]);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
        let mut cur_blend: Option<Blend> = None;
        for c in &list.cmds {
            let Some(t) = self.textures.get(&c.texture).or_else(|| self.textures.get(&TextureId::WHITE)) else { continue };
            if cur_blend != Some(c.blend) {
                let p = &pipes.iter().find(|(b, _)| *b == c.blend).unwrap().1;
                pass.set_pipeline(p);
                cur_blend = Some(c.blend);
            }
            match c.clip {
                Some([x, y, w, h]) => {
                    let x = x.min(size.0);
                    let y = y.min(size.1);
                    let w = w.min(size.0 - x);
                    let h = h.min(size.1 - y);
                    if w == 0 || h == 0 {
                        continue;
                    }
                    pass.set_scissor_rect(x, y, w, h);
                }
                None => pass.set_scissor_rect(0, 0, size.0, size.1),
            }
            pass.set_bind_group(1, &t.bind_group, &[]);
            pass.draw_indexed(c.indices.clone(), 0, 0..1);
            self.last_draw_calls += 1;
        }
    }
}

/// Optional 3D scene drawn by `bm-render` underneath the UI.
pub struct Scene3d<'a> {
    pub renderer: &'a mut Renderer,
    pub scene: Option<&'a SceneGpu>,
    pub camera: &'a Camera,
    pub params: &'a FrameParams,
}

/// Offscreen colour target for tests, screenshots and the examples.
pub struct UiOffscreen {
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    color: wgpu::Texture,
    view: wgpu::TextureView,
    depth: DepthTexture,
}

impl UiOffscreen {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> UiOffscreen {
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bm-ui offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = color.create_view(&wgpu::TextureViewDescriptor::default());
        UiOffscreen { width, height, format, color, view, depth: DepthTexture::new(device, width, height) }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Clears to `clear` (or renders `scene3d` first), draws the UI on top, reads the result back as RGBA8.
    pub fn render_rgba(&self, ui: &mut UiRenderer, scene3d: Option<Scene3d<'_>>, clear: [f64; 4], list: &DrawList, store: &TextureStore) -> Vec<u8> {
        let device = ui.device.clone();
        let queue = ui.queue.clone();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bm-ui offscreen") });
        match scene3d {
            Some(s) => {
                let target = RenderTarget { color: &self.view, depth: &self.depth.view, width: self.width, height: self.height };
                s.renderer.render(&mut encoder, &target, s.scene, s.camera, s.params);
            }
            None => {
                let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("bm-ui clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
        }
        ui.render(&mut encoder, &self.view, (self.width, self.height), list, store);
        let bpr = (self.width * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bm-ui readback"),
            size: (bpr * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(self.height) },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().expect("map callback").expect("map failed");
        let data = slice.get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height as usize {
            out.extend_from_slice(&data[y * bpr as usize..y * bpr as usize + self.width as usize * 4]);
        }
        drop(data);
        buf.unmap();
        if matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
            for p in out.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
        }
        out
    }
}
