//! GPU smoke tests. They skip (pass) when no wgpu adapter exists; software adapters are accepted.
use std::sync::Arc;

use bm_render::*;

fn ctx() -> Option<GpuContext> {
    match GpuContext::new_headless() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn center_pixel(rgba: &[u8], w: u32, h: u32) -> [u8; 4] {
    let i = ((h / 2 * w + w / 2) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn sky_camera() -> Camera {
    Camera { pitch: 0.6, ..Camera::default() }
}

#[test]
fn sky_only_frame_renders() {
    let Some(c) = ctx() else { return };
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), fmt);
    let t = OffscreenTarget::new(&c.device, 64, 64, fmt);
    let px = t.render_rgba(&mut r, None, &sky_camera(), &FrameParams::default());
    let p = center_pixel(&px, 64, 64);
    assert!(p[2] > p[0], "sky should be bluish, got {p:?}");
    assert_eq!(p[3], 255);
}

#[test]
fn shader_override_and_hot_reload() {
    let Some(c) = ctx() else { return };
    let dir = std::env::temp_dir().join(format!("bm_render_hot_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Shaders/WGSL")).unwrap();
    // the VFS indexes loose files when it is created, so the (initially identical) file must exist
    // before; later edits of its content are picked up by `poll_hot_reload`
    std::fs::write(dir.join("Shaders/WGSL/main.wgsl"), bm_render::shaders::BUILTIN_SOURCE).unwrap();
    let vfs = Arc::new(bm_vfs::Vfs::new(&dir).unwrap());
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::with_vfs(c.device.clone(), c.queue.clone(), fmt, Some(vfs));
    let t = OffscreenTarget::new(&c.device, 64, 64, fmt);
    let params = FrameParams { sun_color: [0.0; 3], ..FrameParams::default() };
    let before = center_pixel(&t.render_rgba(&mut r, None, &sky_camera(), &params), 64, 64);
    assert!(before[2] > before[0]);

    // a modder drops a loose override of the builtin shader next to the game files
    let patched = bm_render::shaders::BUILTIN_SOURCE.replace(
        "var col = mix(u_frame.sky_horizon.rgb, u_frame.sky_top.rgb, pow(t, 0.55));",
        "var col = vec3<f32>(1.0, 0.0, 0.0);",
    );
    assert_ne!(patched, bm_render::shaders::BUILTIN_SOURCE);
    std::fs::write(dir.join("Shaders/WGSL/main.wgsl"), &patched).unwrap();
    assert!(r.poll_hot_reload(), "reload must pick up the loose file");
    let after = center_pixel(&t.render_rgba(&mut r, None, &sky_camera(), &params), 64, 64);
    assert!(after[0] > 200 && after[1] < 40 && after[2] < 40, "got {after:?}");

    // a broken edit keeps the previous working shader and reports the error
    std::fs::write(dir.join("Shaders/WGSL/main.wgsl"), "this is not wgsl").unwrap();
    assert!(!r.poll_hot_reload());
    assert!(!r.shader_errors.is_empty());
    let still = center_pixel(&t.render_rgba(&mut r, None, &sky_camera(), &params), 64, 64);
    assert!(still[0] > 200, "previous shader must stay active, got {still:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn real_scene_renders_something() {
    let Some(c) = ctx() else { return };
    let Ok(vfs) = bm_vfs::Vfs::from_env() else { return };
    if vfs.stat("Scenes/M01/M01_main.GMS").is_none() {
        return;
    }
    let scene = bm_models::Scene::load(&vfs, "Scenes/M01/M01_main").unwrap();
    let mut items = scene.draw_list(&[bm_models::NodeKind::Static, bm_models::NodeKind::Actor]);
    items.retain(|i| {
        let t = i.world.translation();
        (t[0] - 2200.0).abs() < 3000.0 && (t[2] + 2300.0).abs() < 3000.0
    });
    assert!(!items.is_empty());
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), fmt);
    let gpu = SceneGpu::upload(&mut r, &scene, &items, &UploadOptions { max_texture_size: 256, ..Default::default() });
    assert!(gpu.stats.triangles > 1000 && gpu.stats.textures > 10);
    let mut cam = Camera { position: [2200.0, 1500.0, -5500.0], ..Camera::default() };
    cam.look_at([2200.0, 400.0, -2300.0]);
    let t = OffscreenTarget::new(&c.device, 256, 144, fmt);
    let px = t.render_rgba(&mut r, Some(&gpu), &cam, &FrameParams::default());
    assert!(r.last_frame.draw_calls > 10);
    // geometry must add variation beyond the smooth sky gradient
    let distinct: std::collections::HashSet<[u8; 3]> = px.chunks_exact(4).map(|p| [p[0] / 8, p[1] / 8, p[2] / 8]).collect();
    assert!(distinct.len() > 60, "only {} distinct colours", distinct.len());
}

struct HideEverything;
impl RenderHooks for HideEverything {
    fn material(&mut self, _: &MaterialInfo) -> MaterialOverride {
        MaterialOverride { skip: true, ..Default::default() }
    }
}

#[test]
fn material_hook_can_hide_geometry() {
    let Some(c) = ctx() else { return };
    let Ok(vfs) = bm_vfs::Vfs::from_env() else { return };
    if vfs.stat("Scenes/M01/M01_main.GMS").is_none() {
        return;
    }
    let scene = bm_models::Scene::load(&vfs, "Scenes/M01/M01_main").unwrap();
    let items: Vec<_> = scene.draw_list(&[bm_models::NodeKind::Static]).into_iter().take(200).collect();
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), fmt);
    r.set_hooks(Box::new(HideEverything));
    let gpu = SceneGpu::upload(&mut r, &scene, &items, &UploadOptions::default());
    assert_eq!(gpu.stats.meshes, 0);
}

fn m01_ground_subset() -> Option<(bm_models::Scene, Vec<bm_models::scene::DrawItem>)> {
    let vfs = bm_vfs::Vfs::from_env().ok()?;
    vfs.stat("Scenes/M01/M01_main.GMS")?;
    let scene = bm_models::Scene::load(&vfs, "Scenes/M01/M01_main").ok()?;
    // the ground slab in front of the Hacienda plus everything within 1500 cm of its middle (palms, grass)
    let mut items = scene.draw_list(&[bm_models::NodeKind::Static]);
    items.retain(|i| {
        let p = scene.path(i.node);
        let t = i.world.translation();
        p.contains("M01_Outside_01b/Ground_03") || ((t[0] + 1100.0).abs() < 1500.0 && (t[2] - 2400.0).abs() < 1500.0 && p.contains("M01_Outside_01b"))
    });
    Some((scene, items))
}

/// Regression: Glacier triangles are counter-clockwise on screen seen from the normal side. With the wrong front face the whole
/// ground (and every roof) was culled and the sky showed through.
#[test]
fn ground_is_visible_from_above() {
    let Some(c) = ctx() else { return };
    let Some((scene, items)) = m01_ground_subset() else { return };
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), fmt);
    let gpu = SceneGpu::upload(&mut r, &scene, &items, &UploadOptions { max_texture_size: 256, ..Default::default() });
    let cam = Camera { position: [-1100.0, 1200.0, 2400.0], pitch: -1.5, ..Camera::default() };
    let t = OffscreenTarget::new(&c.device, 128, 72, fmt);
    let params = FrameParams { draw_sky: false, clear_color: [1.0, 0.0, 1.0, 1.0], shadows: ShadowParams { enabled: false, ..Default::default() }, ..FrameParams::default() };
    let px = t.render_rgba(&mut r, Some(&gpu), &cam, &params);
    let magenta = px.chunks_exact(4).filter(|p| p[0] > 240 && p[1] < 20 && p[2] > 240).count();
    assert!(magenta < px.len() / 4 / 4, "the ground must cover the view, {magenta} of {} pixels are background", px.len() / 4);
}

#[test]
fn sun_shadows_render_and_darken() {
    let Some(c) = ctx() else { return };
    let Some((scene, items)) = m01_ground_subset() else { return };
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), fmt);
    let gpu = SceneGpu::upload(&mut r, &scene, &items, &UploadOptions { max_texture_size: 256, ..Default::default() });
    let mut cam = Camera { position: [-1100.0, 250.0, 1700.0], ..Camera::default() };
    cam.look_at([-1100.0, 0.0, 2500.0]);
    let t = OffscreenTarget::new(&c.device, 192, 108, fmt);
    let mut params = FrameParams::from_scene_at(&scene, [-1100.0, 100.0, 2400.0]);
    assert!(params.sun_color[0] > 0.5, "M01 outdoors has a sun: {:?}", params.sun_color);
    params.shadows = ShadowParams { enabled: false, ..Default::default() };
    let off = t.render_rgba(&mut r, Some(&gpu), &cam, &params);
    assert_eq!(r.last_frame.shadow_cascades, 0);
    params.shadows = ShadowParams { enabled: true, resolution: 1024, ..Default::default() };
    let on = t.render_rgba(&mut r, Some(&gpu), &cam, &params);
    assert!(r.last_frame.shadow_cascades >= 1 && r.last_frame.shadow_draw_calls > 0, "{:?}", r.last_frame);
    let lum = |px: &[u8]| px.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum::<u64>();
    assert!(lum(&on) < lum(&off), "shadows must remove sun light ({} vs {})", lum(&on), lum(&off));
    // no sun, no shadow pass
    params.sun_color = [0.0; 3];
    t.render_rgba(&mut r, Some(&gpu), &cam, &params);
    assert_eq!(r.last_frame.shadow_cascades, 0);
}

#[test]
fn fxaa_and_present_pass_keep_flat_colours() {
    let Some(c) = ctx() else { return };
    for fmt in [wgpu::TextureFormat::Rgba8UnormSrgb, wgpu::TextureFormat::Rgba8Unorm] {
        let mut r = Renderer::new(c.device.clone(), c.queue.clone(), fmt);
        let t = OffscreenTarget::new(&c.device, 32, 32, fmt);
        let mut params = FrameParams { draw_sky: false, clear_color: [0.5, 0.25, 0.75, 1.0], ..FrameParams::default() };
        let a = center_pixel(&t.render_rgba(&mut r, None, &sky_camera(), &params), 32, 32);
        params.fxaa = false;
        let b = center_pixel(&t.render_rgba(&mut r, None, &sky_camera(), &params), 32, 32);
        assert_eq!(a, b, "a flat colour must not change with FXAA ({fmt:?})");
        // the clear colour is a display-space value: both target kinds show it unchanged (+-1 for the sRGB round trip)
        for (got, want) in a.iter().zip([128u8, 64, 191, 255]) {
            assert!((*got as i32 - want as i32).abs() <= 2, "{fmt:?}: got {a:?}");
        }
    }
}
