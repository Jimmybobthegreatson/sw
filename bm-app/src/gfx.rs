//! Window + wgpu surface + the 3D renderer, UI renderer and particle hook that draw into it.

use bm_particles::render::{ParticleHandle, ParticleRenderer};
use bm_render::{Camera, DepthTexture, FrameParams, GpuContext, RenderTarget, Renderer, SceneGpu};
use bm_ui::{DrawList, TextureStore, UiRenderer};
use bm_vfs::Vfs;
use std::sync::Arc;
use std::time::Instant;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window};

pub struct Gfx {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub depth: DepthTexture,
    pub renderer: Renderer,
    pub ui: UiRenderer,
    pub particles: ParticleHandle,
    pub adapter_name: String,
    pub backend: String,
    #[allow(dead_code)]
    present_modes: Vec<wgpu::PresentMode>,
}

/// Wall-clock timings of one presented frame, in milliseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimes {
    /// Recording the 3D + UI passes.
    pub record_ms: f32,
    /// Acquire + submit + present (includes vsync waits).
    pub present_ms: f32,
    /// With `wait_gpu`: time until the GPU finished the frame.
    pub gpu_ms: f32,
}

impl Gfx {
    pub fn new(el: &ActiveEventLoop, attrs: winit::window::WindowAttributes, vfs: Arc<Vfs>, vsync: bool) -> Result<Gfx, String> {
        let window = Arc::new(el.create_window(attrs).map_err(|e| format!("window: {e}"))?);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone()).map_err(|e| format!("surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("no suitable GPU adapter: {e}"))?;
        let info = adapter.get_info();
        log::info!("adapter: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
        let (device, queue) = GpuContext::request_device(&adapter)?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by the adapter")?;
        if let Some(f) = caps.formats.iter().find(|f| f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = pick_present_mode(&caps.present_modes, vsync);
        surface.configure(&device, &config);
        let mut renderer = Renderer::with_vfs(device.clone(), queue.clone(), config.format, Some(vfs.clone()));
        let (hook, particles) = ParticleRenderer::new(Some(vfs.clone()));
        renderer.set_hooks(Box::new(hook));
        let ui = UiRenderer::for_renderer(&renderer, config.format, Some(vfs));
        let depth = DepthTexture::new(&device, config.width, config.height);
        Ok(Gfx {
            window,
            surface,
            config,
            depth,
            renderer,
            ui,
            particles,
            adapter_name: info.name,
            backend: format!("{:?}", info.backend),
            present_modes: caps.present_modes,
        })
    }

    #[allow(dead_code)]
    pub fn set_vsync(&mut self, vsync: bool) {
        self.config.present_mode = pick_present_mode(&self.present_modes, vsync);
        self.surface.configure(self.renderer.device(), &self.config);
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(self.renderer.device(), &self.config);
        self.depth = DepthTexture::new(self.renderer.device(), w, h);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn toggle_fullscreen(&self) {
        let on = self.window.fullscreen().is_some();
        self.window.set_fullscreen(if on { None } else { Some(Fullscreen::Borderless(None)) });
    }

    /// Renders the 3D scene (or just the sky when `scene` is None) and the UI list into the swapchain and presents.
    pub fn render(
        &mut self,
        scene: Option<&SceneGpu>,
        camera: &Camera,
        params: &FrameParams,
        ui_list: &DrawList,
        store: &TextureStore,
        wait_gpu: bool,
    ) -> FrameTimes {
        let mut t = FrameTimes::default();
        let t0 = Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(self.renderer.device(), &self.config);
                return t;
            }
            _ => return t,
        };
        let acquire = t0.elapsed();
        let t1 = Instant::now();
        let view = frame.texture.create_view(&Default::default());
        let device = self.renderer.device().clone();
        let queue = self.renderer.queue().clone();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bm frame") });
        let target = RenderTarget { color: &view, depth: &self.depth.view, width: self.config.width, height: self.config.height };
        self.renderer.render(&mut enc, &target, scene, camera, params);
        self.ui.render(&mut enc, &view, (self.config.width, self.config.height), ui_list, store);
        t.record_ms = t1.elapsed().as_secs_f32() * 1000.0;
        let t2 = Instant::now();
        queue.submit([enc.finish()]);
        if wait_gpu {
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            t.gpu_ms = t2.elapsed().as_secs_f32() * 1000.0;
        }
        self.window.pre_present_notify();
        queue.present(frame);
        t.present_ms = (acquire + t2.elapsed()).as_secs_f32() * 1000.0;
        t
    }
}

fn pick_present_mode(modes: &[wgpu::PresentMode], vsync: bool) -> wgpu::PresentMode {
    if vsync {
        return wgpu::PresentMode::Fifo;
    }
    for m in [wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox] {
        if modes.contains(&m) {
            return m;
        }
    }
    wgpu::PresentMode::Fifo
}
