//! `bm view <scene>`: free-fly viewer of a whole scene (no simulation).

use crate::config::{Config, Mounted, resolve_scene};
use crate::gfx::Gfx;
use bm_models::scene::{NodeKind, Scene};
use bm_render::{Camera, FlyController, FrameParams, SceneGpu, UploadOptions, scene_lights};
use bm_ui::{DrawList, TextureStore};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowId;

pub struct ViewOpts {
    pub scene: String,
    pub cam: Option<[f32; 5]>,
    pub exit_after: Option<f32>,
}

struct Viewer {
    opts: ViewOpts,
    cfg: Config,
    mounted: Mounted,
    gfx: Option<Gfx>,
    scene: Option<Arc<Scene>>,
    gpu: Option<SceneGpu>,
    camera: Camera,
    params: FrameParams,
    fly: FlyController,
    looking: bool,
    store: TextureStore,
    last: Instant,
    started: Instant,
    last_poll: Instant,
    frames: u32,
    fps_t: Instant,
    title: String,
    light_pos: [f32; 3],
}

pub fn run(opts: ViewOpts, cfg: Config, mounted: Mounted) -> Result<(), String> {
    let el = EventLoop::new().map_err(|e| e.to_string())?;
    el.set_control_flow(ControlFlow::Poll);
    let title = opts.scene.clone();
    let mut v = Viewer {
        opts,
        cfg,
        mounted,
        gfx: None,
        scene: None,
        gpu: None,
        camera: Camera { near: 8.0, ..Default::default() },
        params: FrameParams::default(),
        fly: FlyController::default(),
        looking: false,
        store: TextureStore::default(),
        last: Instant::now(),
        started: Instant::now(),
        last_poll: Instant::now(),
        frames: 0,
        fps_t: Instant::now(),
        title,
        light_pos: [f32::MAX; 3],
    };
    el.run_app(&mut v).map_err(|e| e.to_string())
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = crate::app::default_attrs(&self.cfg, &Default::default()).with_title("bm view");
        let mut gfx = match Gfx::new(el, attrs, self.mounted.vfs.clone(), self.cfg.video.vsync) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("error: {e}");
                el.exit();
                return;
            }
        };
        let base = resolve_scene(&self.mounted.vfs, &self.opts.scene);
        let t = Instant::now();
        let scene = match Scene::load(&self.mounted.vfs, &base) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                eprintln!("error: {base}: {e}");
                el.exit();
                return;
            }
        };
        let items = scene.draw_list(&[NodeKind::Static, NodeKind::Actor, NodeKind::ClothBundle, NodeKind::Scatter]);
        let gpu = SceneGpu::upload(&mut gfx.renderer, &scene, &items, &UploadOptions::default());
        println!("{base}: {} nodes, {} draw items, uploaded in {:.1}s {:?}", scene.nodes.len(), items.len(), t.elapsed().as_secs_f32(), gpu.stats);
        self.params.lights = scene_lights(&scene);
        // camera: explicit, else the player start node, else above the scene's bounding box
        if let Some(c) = self.opts.cam {
            self.camera.position = [c[0], c[1], c[2]];
            self.camera.yaw = c[3].to_radians();
            self.camera.pitch = c[4].to_radians();
        } else if let Some(h) = scene.nodes.iter().find(|n| n.type_id == 0x2003e8) {
            let p = h.world.translation();
            self.camera.position = [p[0], p[1] + 170.0, p[2] - 250.0];
        } else {
            let mut b = bm_models::Aabb::EMPTY;
            for i in &items {
                b = b.union(&i.model.bbox.transformed(&i.world));
            }
            let c = b.center();
            self.camera.position = [c[0], b.max[1] + 3000.0, c[2] - 3000.0];
            self.camera.look_at(c);
        }
        self.params.apply_scene_at(&scene, self.camera.position);
        self.light_pos = self.camera.position;
        self.scene = Some(scene);
        self.gpu = Some(gpu);
        self.gfx = Some(gfx);
        self.started = Instant::now();
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, ev: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = ev {
            if self.looking {
                FlyController::look(&mut self.camera, delta.0 as f32, delta.1 as f32);
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, ev: WindowEvent) {
        let Some(g) = self.gfx.as_mut() else { return };
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(sz) => g.resize(sz.width, sz.height),
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => {
                self.looking = state == ElementState::Pressed;
                g.window.set_cursor_visible(!self.looking);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(k) = event.physical_key {
                    match k {
                        KeyCode::KeyW => self.fly.forward = down,
                        KeyCode::KeyS => self.fly.back = down,
                        KeyCode::KeyA => self.fly.left = down,
                        KeyCode::KeyD => self.fly.right = down,
                        KeyCode::KeyE | KeyCode::Space => self.fly.up = down,
                        KeyCode::KeyQ | KeyCode::ControlLeft => self.fly.down = down,
                        KeyCode::ShiftLeft => self.fly.fast = down,
                        KeyCode::Digit1 if down => self.params.debug_mode = 0,
                        KeyCode::Digit2 if down => self.params.debug_mode = 1,
                        KeyCode::Digit3 if down => self.params.debug_mode = 2,
                        KeyCode::Digit4 if down => self.params.debug_mode = 3,
                        KeyCode::F6 if down => self.params.fxaa = !self.params.fxaa,
                        KeyCode::F7 if down => self.params.shadows.enabled = !self.params.shadows.enabled,
                        KeyCode::F5 if down => {
                            g.renderer.poll_hot_reload();
                        }
                        KeyCode::F11 if down => g.toggle_fullscreen(),
                        KeyCode::Escape if down => el.exit(),
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last).as_secs_f32().min(0.1);
                self.last = now;
                self.fly.update(&mut self.camera, dt);
                self.params.time += dt;
                if let Some(sc) = &self.scene {
                    let moved = (0..3).map(|k| (self.camera.position[k] - self.light_pos[k]).abs()).fold(0.0f32, f32::max);
                    if moved > 50.0 {
                        // the zone (room / outdoors) changes the sun, ambient and fog
                        self.params.apply_scene_at(sc, self.camera.position);
                        self.light_pos = self.camera.position;
                    }
                }
                if now - self.last_poll > Duration::from_secs(1) {
                    self.last_poll = now;
                    g.renderer.poll_hot_reload();
                }
                g.render(self.gpu.as_ref(), &self.camera, &self.params, &DrawList::new(), &self.store, false);
                self.frames += 1;
                if self.fps_t.elapsed().as_secs_f32() > 1.0 {
                    let f = g.renderer.last_frame;
                    g.window.set_title(&format!("bm view {} - {} fps, {} draws, {} tris", self.title, self.frames, f.draw_calls, f.triangles));
                    self.frames = 0;
                    self.fps_t = Instant::now();
                }
                if self.opts.exit_after.is_some_and(|s| self.started.elapsed().as_secs_f32() >= s) {
                    el.exit();
                }
                g.window.request_redraw();
            }
            _ => {}
        }
    }
}
