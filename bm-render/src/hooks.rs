//! Modder / game extension points of the renderer.

use bm_models::MaterialDesc;

use crate::camera::Camera;
use crate::params::FrameParams;

/// What the renderer tells a hook about a material while uploading a scene.
#[derive(Debug)]
pub struct MaterialInfo<'a> {
    pub desc: &'a MaterialDesc,
}

/// Per-material changes returned by [`RenderHooks::material`].
#[derive(Clone, Debug, Default)]
pub struct MaterialOverride {
    /// Do not draw meshes using this material.
    pub skip: bool,
    /// Multiplied with `v4DiffuseColor` (rgba).
    pub color_mul: Option<[f32; 4]>,
    /// Replaces the illumination colour and enables emissive output (rgb).
    pub emissive: Option<[f32; 3]>,
    /// Path (resolved through the VFS, e.g. `Shaders/WGSL/glass.wgsl`) of a WGSL module that replaces the
    /// builtin shader for this material. It must declare the same bindings and the entry points
    /// `vs_main` / `fs_main` as the builtin `main.wgsl` (copy it as a template). Hot reloadable.
    pub shader: Option<String>,
    /// Force a blend class instead of the one derived from the `.MAT` render state.
    pub force_blend: Option<ForcedBlend>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForcedBlend {
    Opaque,
    Alpha,
    Additive,
}

/// Where in the frame [`RenderHooks::draw_extra`] is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookStage {
    /// After the sky and all opaque geometry.
    AfterOpaque,
    /// After everything (sorted transparent geometry included), before the pass ends.
    AfterTransparent,
}

/// Context handed to [`RenderHooks::draw_extra`]. Bind group 0 of the builtin layout (frame uniforms,
/// bone matrices) is already set on the pass; custom pipelines may use the same layout.
pub struct HookContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub camera: &'a Camera,
    pub params: &'a FrameParams,
    pub frame_bind_group_layout: &'a wgpu::BindGroupLayout,
    pub frame_bind_group: &'a wgpu::BindGroup,
    pub color_format: wgpu::TextureFormat,
    pub depth_format: wgpu::TextureFormat,
    pub aspect: f32,
}

/// Extension trait for modders and for bm-game (debug draw, particles, UI-in-world, post tweaks).
/// All methods have no-op defaults. Install with `Renderer::set_hooks`.
///
/// Typical modding uses:
/// * recolour / hide / re-shade materials while a scene is uploaded (`material`),
/// * animate the environment every frame (`before_frame`: sun, fog, exposure, camera shake),
/// * draw extra geometry inside the main pass (`draw_extra`), e.g. particles from `bm-particles`.
pub trait RenderHooks: Send {
    /// Called once per distinct material when a scene is uploaded.
    fn material(&mut self, _info: &MaterialInfo) -> MaterialOverride {
        MaterialOverride::default()
    }

    /// Called at the start of every `Renderer::render`; may edit the camera and frame parameters.
    fn before_frame(&mut self, _camera: &mut Camera, _params: &mut FrameParams) {}

    /// Record extra draw calls into the main render pass.
    fn draw_extra(&mut self, _stage: HookStage, _ctx: &HookContext, _pass: &mut wgpu::RenderPass<'_>) {}
}
