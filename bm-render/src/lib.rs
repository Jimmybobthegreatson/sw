//! `bm-render` - wgpu renderer for the Glacier-1 (Hitman: Blood Money) runtime.
//!
//! Window agnostic: give it a `wgpu::Device`/`Queue` and the colour format of your target.
//!
//! ```no_run
//! use bm_render::*;
//! # fn demo(device: wgpu::Device, queue: wgpu::Queue, scene: &bm_models::Scene, view: &wgpu::TextureView, depth: &wgpu::TextureView) {
//! let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Bgra8UnormSrgb);
//! let items = scene.draw_list(&[bm_models::NodeKind::Static, bm_models::NodeKind::Actor]);
//! let gpu = SceneGpu::upload(&mut renderer, scene, &items, &UploadOptions::default());
//! let mut encoder = renderer.device().create_command_encoder(&Default::default());
//! let target = RenderTarget { color: view, depth, width: 1280, height: 720 };
//! renderer.render(&mut encoder, &target, Some(&gpu), &Camera::default(), &FrameParams::default());
//! renderer.queue().submit([encoder.finish()]);
//! # }
//! ```
//!
//! See `docs/render.md` for conventions, the shader override / hot reload mechanism and the hooks.

pub mod camera;
pub mod context;
pub mod gpu_scene;
pub mod hooks;
pub mod offscreen;
pub mod params;
pub mod renderer;
pub mod shaders;

pub use camera::{Camera, FlyController, Frustum};
pub use context::GpuContext;
pub use gpu_scene::{FrameStats, SceneGpu, UploadOptions, UploadStats};
pub use hooks::{ForcedBlend, HookContext, HookStage, MaterialInfo, MaterialOverride, RenderHooks};
pub use offscreen::OffscreenTarget;
pub use params::{lights_from_scene, scene_lights, FrameParams, PointLight, ShadowParams};
pub use renderer::{DepthTexture, RenderTarget, Renderer, DEPTH_FORMAT, SCENE_FORMAT};
