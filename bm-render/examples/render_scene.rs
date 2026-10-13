//! `cargo run --release -p bm-render --example render_scene -- [scene] [options]`
//!
//! Offscreen (default): renders one PNG.  `--window`: opens a free-fly window (WASD/QE, shift = fast,
//! hold right mouse button to look, 1/2/3 = lit/albedo/normals debug view, F5 = reload shaders).
//!
//! Options
//!   scene            e.g. Scenes/M01/M01_main (default)
//!   --out FILE       PNG path (default render.png)
//!   --size WxH       default 1280x720
//!   --cam x,y,z,yaw,pitch   explicit camera (degrees)
//!   --node NAME      frame the first scene node whose name contains NAME (case-insensitive)
//!   --dist D         camera distance for --node (cm)
//!   --yaw DEG        view yaw for --node (default 200)
//!   --radius R       only upload items within R cm of the focus point (faster for big levels)
//!   --lod all        keep all LODs
//!   --debug N        0 lit, 1 albedo, 2 normals
//!   --no-lights      ignore light nodes
//!   --list NAME      print nodes matching NAME and exit
use std::path::PathBuf;
use std::sync::Arc;

use bm_models::scene::{NodeKind, Scene};
use bm_render::*;

struct Args {
    scene: String,
    out: PathBuf,
    size: (u32, u32),
    cam: Option<[f32; 5]>,
    node: Option<String>,
    dist: Option<f32>,
    yaw: f32,
    radius: Option<f32>,
    lod_all: bool,
    debug: u32,
    lights: bool,
    window: bool,
    list: Option<String>,
    no_skinned: bool,
    kinds: Vec<NodeKind>,
    only: Option<String>,
}

fn parse() -> Args {
    let mut a = Args {
        scene: "Scenes/M01/M01_main".into(),
        out: "render.png".into(),
        size: (1280, 720),
        cam: None,
        node: None,
        dist: None,
        yaw: 200.0,
        radius: None,
        lod_all: false,
        debug: 0,
        lights: true,
        window: false,
        list: None,
        no_skinned: false,
        kinds: vec![NodeKind::Static, NodeKind::Actor, NodeKind::ClothBundle, NodeKind::Scatter],
        only: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(s) = it.next() {
        match s.as_str() {
            "--out" => a.out = it.next().unwrap().into(),
            "--size" => {
                let v = it.next().unwrap();
                let (w, h) = v.split_once('x').unwrap();
                a.size = (w.parse().unwrap(), h.parse().unwrap());
            }
            "--cam" => {
                let v: Vec<f32> = it.next().unwrap().split(',').map(|x| x.parse().unwrap()).collect();
                a.cam = Some([v[0], v[1], v[2], v[3], v[4]]);
            }
            "--node" => a.node = it.next(),
            "--dist" => a.dist = it.next().map(|x| x.parse().unwrap()),
            "--yaw" => a.yaw = it.next().unwrap().parse().unwrap(),
            "--radius" => a.radius = it.next().map(|x| x.parse().unwrap()),
            "--lod" => a.lod_all = it.next().as_deref() == Some("all"),
            "--debug" => a.debug = it.next().unwrap().parse().unwrap(),
            "--no-lights" => a.lights = false,
            "--window" => a.window = true,
            "--list" => a.list = it.next(),
            "--no-skinned" => a.no_skinned = true,
            "--only" => a.only = it.next(),
            "--kinds" => {
                a.kinds = it.next().unwrap().split(',').map(|k| match k {
                    "static" => NodeKind::Static,
                    "actor" => NodeKind::Actor,
                    "cloth" => NodeKind::ClothBundle,
                    "scatter" => NodeKind::Scatter,
                    "link" => NodeKind::Link,
                    "bound" => NodeKind::Bound,
                    _ => panic!("kind"),
                }).collect()
            }
            _ => a.scene = s,
        }
    }
    a
}

struct Prepared {
    renderer: Renderer,
    gpu: SceneGpu,
    camera: Camera,
    params: FrameParams,
}

fn prepare(args: &Args, device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat, vfs: Arc<bm_vfs::Vfs>) -> Prepared {
    let t0 = std::time::Instant::now();
    let scene = Scene::load(&vfs, &args.scene).expect("scene");
    println!("scene loaded in {:.1}s ({} nodes)", t0.elapsed().as_secs_f32(), scene.nodes.len());

    // camera / focus
    let mut camera = Camera::default();
    camera.near = 8.0;
    let mut focus: Option<[f32; 3]> = None;
    if let Some(c) = args.cam {
        camera.position = [c[0], c[1], c[2]];
        camera.yaw = c[3].to_radians();
        camera.pitch = c[4].to_radians();
        focus = Some(camera.position);
    } else if let Some(name) = &args.node {
        let lname = name.to_lowercase();
        let n = scene
            .nodes
            .iter()
            .find(|n| n.name.to_lowercase().contains(&lname))
            .expect("node not found");
        let mut bb = bm_models::Aabb::EMPTY;
        for i in scene.subtree(n.index) {
            if scene.nodes[i].kind.is_visible_geometry() {
                if let Some(b) = scene.world_bbox(i) {
                    bb = bb.union(&b);
                }
            }
        }
        let (center, extent) = if bb.is_empty() {
            (n.world.translation(), 400.0)
        } else {
            (bb.center(), bb.size().iter().cloned().fold(0.0, f32::max))
        };
        let dist = args.dist.unwrap_or((extent * 1.2).max(600.0));
        let yaw = args.yaw.to_radians();
        let dir = [yaw.sin() * 0.8, 0.45, yaw.cos() * 0.8];
        camera.position = camera::add(center, camera::scale(camera::normalize(dir), -dist));
        camera.position[1] = camera.position[1].max(center[1] + dist * 0.3);
        camera.look_at(center);
        println!("framing node '{}' at ({:.0},{:.0},{:.0}) extent {:.0}", n.name, center[0], center[1], center[2], extent);
        focus = Some(center);
    }

    let mut items = scene.draw_list(&args.kinds);
    if let (Some(r), Some(f)) = (args.radius, focus) {
        items.retain(|i| {
            let c = i.world.translation();
            camera::length(camera::sub(c, f)) < r
        });
    }
    if let Some(o) = &args.only {
        let o = o.to_lowercase();
        items.retain(|i| scene.path(i.node).to_lowercase().contains(&o));
    }
    println!("{} draw items", items.len());

    let mut renderer = Renderer::with_vfs(device, queue, format, Some(vfs));
    let opts = UploadOptions { lod0_only: !args.lod_all, include_skinned: !args.no_skinned, ..Default::default() };
    let t1 = std::time::Instant::now();
    let gpu = SceneGpu::upload(&mut renderer, &scene, &items, &opts);
    println!("upload {:.1}s: {:?}", t1.elapsed().as_secs_f32(), gpu.stats);

    let mut params = FrameParams::default();
    params.debug_mode = args.debug;
    if args.lights {
        params.lights = lights_from_scene(&scene, 900.0, [1.0, 0.85, 0.6]);
    }
    if camera.position == [0.0, 170.0, 0.0] && args.cam.is_none() && args.node.is_none() {
        // whole scene: look at the centre of the first big bbox
        let mut b = bm_models::Aabb::EMPTY;
        for i in &items {
            b = b.union(&i.model.bbox.transformed(&i.world));
        }
        let c = b.center();
        camera.position = [c[0], b.max[1] + 5000.0, c[2] - b.size()[2] * 0.5];
        camera.look_at(c);
    }
    Prepared { renderer, gpu, camera, params }
}

fn main() {
    env_logger::init();
    let args = parse();
    let vfs = Arc::new(bm_vfs::Vfs::from_env().expect("game dir (set BM_GAME_DIR)"));
    if let Some(name) = &args.list {
        let scene = Scene::load(&vfs, &args.scene).expect("scene");
        let l = name.to_lowercase();
        for n in scene.nodes.iter().filter(|n| n.name.to_lowercase().contains(&l)).take(60) {
            let w = n.world.translation();
            println!("[{}] {} {:?} ({:.0},{:.0},{:.0}) path={}", n.index, n.name, n.kind, w[0], w[1], w[2], scene.path(n.index));
        }
        return;
    }
    if args.window {
        run_window(args, vfs);
    } else {
        run_offscreen(args, vfs);
    }
}

fn run_offscreen(args: Args, vfs: Arc<bm_vfs::Vfs>) {
    let ctx = GpuContext::new_headless().expect("gpu");
    let info = ctx.adapter_info();
    println!("adapter: {} ({:?}, {:?}){}", info.name, info.backend, info.device_type, if ctx.is_fallback { " [FALLBACK/software]" } else { "" });
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut p = prepare(&args, ctx.device.clone(), ctx.queue.clone(), format, vfs);
    let target = OffscreenTarget::new(&ctx.device, args.size.0, args.size.1, format);
    let t = std::time::Instant::now();
    target.render_png(&mut p.renderer, Some(&p.gpu), &p.camera, &p.params, &args.out).expect("png");
    println!("rendered {} in {:.2}s: {:?}", args.out.display(), t.elapsed().as_secs_f32(), p.renderer.last_frame);
}

// ---------------------------------------------------------------------------------------------- window
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

struct App {
    args: Args,
    vfs: Arc<bm_vfs::Vfs>,
    state: Option<WinState>,
}

struct WinState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: DepthTexture,
    p: Prepared,
    fly: FlyController,
    looking: bool,
    last: std::time::Instant,
    last_poll: std::time::Instant,
    frames: u32,
    fps_t: std::time::Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            el.create_window(Window::default_attributes().with_title("bm-render").with_inner_size(winit::dpi::LogicalSize::new(1280, 720)))
                .unwrap(),
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("adapter");
        println!("adapter: {}", adapter.get_info().name);
        let (device, queue) = GpuContext::request_device(&adapter).unwrap();
        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).unwrap();
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| f.is_srgb()) {
            config.format = *f;
        }
        surface.configure(&device, &config);
        let p = prepare(&self.args, device.clone(), queue, config.format, self.vfs.clone());
        let depth = DepthTexture::new(&device, config.width, config.height);
        self.state = Some(WinState {
            window,
            surface,
            config,
            depth,
            p,
            fly: FlyController::default(),
            looking: false,
            last: std::time::Instant::now(),
            last_poll: std::time::Instant::now(),
            frames: 0,
            fps_t: std::time::Instant::now(),
        });
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, ev: DeviceEvent) {
        if let (Some(s), DeviceEvent::MouseMotion { delta }) = (self.state.as_mut(), ev) {
            if s.looking {
                FlyController::look(&mut s.p.camera, delta.0 as f32, delta.1 as f32);
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, ev: WindowEvent) {
        let Some(s) = self.state.as_mut() else { return };
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(sz) if sz.width > 0 && sz.height > 0 => {
                s.config.width = sz.width;
                s.config.height = sz.height;
                s.surface.configure(s.p.renderer.device(), &s.config);
                s.depth = DepthTexture::new(s.p.renderer.device(), sz.width, sz.height);
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => s.looking = state == ElementState::Pressed,
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(k) = event.physical_key {
                    match k {
                        KeyCode::KeyW => s.fly.forward = down,
                        KeyCode::KeyS => s.fly.back = down,
                        KeyCode::KeyA => s.fly.left = down,
                        KeyCode::KeyD => s.fly.right = down,
                        KeyCode::KeyE | KeyCode::Space => s.fly.up = down,
                        KeyCode::KeyQ | KeyCode::ControlLeft => s.fly.down = down,
                        KeyCode::ShiftLeft => s.fly.fast = down,
                        KeyCode::Digit1 if down => s.p.params.debug_mode = 0,
                        KeyCode::Digit2 if down => s.p.params.debug_mode = 1,
                        KeyCode::Digit3 if down => s.p.params.debug_mode = 2,
                        KeyCode::F5 if down => {
                            s.p.renderer.poll_hot_reload();
                        }
                        KeyCode::Escape => el.exit(),
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let dt = (now - s.last).as_secs_f32();
                s.last = now;
                s.fly.update(&mut s.p.camera, dt);
                s.p.params.time += dt;
                if now - s.last_poll > std::time::Duration::from_secs(1) {
                    s.last_poll = now;
                    s.p.renderer.poll_hot_reload();
                }
                match s.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                        let view = frame.texture.create_view(&Default::default());
                        let mut enc = s.p.renderer.device().create_command_encoder(&Default::default());
                        let target = RenderTarget { color: &view, depth: &s.depth.view, width: s.config.width, height: s.config.height };
                        s.p.renderer.render(&mut enc, &target, Some(&s.p.gpu), &s.p.camera, &s.p.params);
                        s.p.renderer.queue().submit([enc.finish()]);
                        s.window.pre_present_notify();
                        s.p.renderer.queue().present(frame);
                    }
                    _ => s.surface.configure(s.p.renderer.device(), &s.config),
                }
                s.frames += 1;
                if s.fps_t.elapsed().as_secs_f32() > 1.0 {
                    let f = s.p.renderer.last_frame;
                    s.window.set_title(&format!(
                        "bm-render  {} fps  draws {} tris {} inst {}/{}",
                        s.frames, f.draw_calls, f.triangles, f.instances_drawn, f.instances_total
                    ));
                    s.frames = 0;
                    s.fps_t = std::time::Instant::now();
                }
                s.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn run_window(args: Args, vfs: Arc<bm_vfs::Vfs>) {
    let el = EventLoop::new().unwrap();
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App { args, vfs, state: None };
    el.run_app(&mut app).unwrap();
}
