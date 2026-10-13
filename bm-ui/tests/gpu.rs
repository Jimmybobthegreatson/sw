//! GPU tests (skip when no adapter is available): pixels, blending, scissor, shader override + hot reload.

use std::fs;
use std::sync::Arc;

use bm_render::GpuContext;
use bm_ui::*;

fn gpu() -> Option<GpuContext> {
    GpuContext::new_headless().ok()
}

fn px(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn near(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= tol)
}

#[test]
fn rect_alpha_additive_and_scissor() {
    let Some(g) = gpu() else { return eprintln!("no gpu: skipping") };
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut ur = UiRenderer::new(g.device.clone(), g.queue.clone(), fmt, None);
    let off = UiOffscreen::new(&g.device, 64, 64, fmt);
    let store = TextureStore::new();
    let mut dl = DrawList::new();
    dl.rect(Rect::new(0.0, 0.0, 32.0, 32.0), Color([255, 0, 0, 255]));
    dl.rect(Rect::new(32.0, 0.0, 32.0, 32.0), Color([255, 255, 255, 128])); // 50% white over the clear colour
    dl.rect_blend(Rect::new(0.0, 32.0, 32.0, 32.0), Color([0, 0, 255, 255]), Blend::Additive);
    dl.push_clip(Rect::new(32.0, 32.0, 16.0, 32.0));
    dl.rect(Rect::new(32.0, 32.0, 32.0, 32.0), Color([0, 255, 0, 255]));
    dl.pop_clip();
    let out = off.render_rgba(&mut ur, None, [0.0, 0.0, 0.0, 1.0], &dl, &store);
    // sRGB target: authored sRGB colours must come back unchanged
    assert!(near(px(&out, 64, 16, 16), [255, 0, 0, 255], 2), "{:?}", px(&out, 64, 16, 16));
    // 50 % white over black, blended in linear light then encoded: ~188 (not 128)
    let mid = px(&out, 64, 48, 16);
    assert!(mid[0] > 150 && mid[0] < 215 && mid[0] == mid[1], "{mid:?}");
    assert!(near(px(&out, 64, 16, 48), [0, 0, 255, 255], 2));
    assert!(near(px(&out, 64, 40, 48), [0, 255, 0, 255], 2), "clip interior");
    assert!(near(px(&out, 64, 56, 48), [0, 0, 0, 255], 2), "clipped away");
    assert!(ur.last_draw_calls >= 3);
}

#[test]
fn textured_quad_and_texture_replace() {
    let Some(g) = gpu() else { return eprintln!("no gpu: skipping") };
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut ur = UiRenderer::new(g.device.clone(), g.queue.clone(), fmt, None);
    let off = UiOffscreen::new(&g.device, 32, 32, fmt);
    let mut store = TextureStore::new();
    let mut img = Image::solid(2, 2, [0, 0, 0, 255], "t");
    img.rgba[0..4].copy_from_slice(&[255, 255, 0, 255]); // top-left texel yellow
    img.nearest = true;
    let id = store.add(img);
    let mut dl = DrawList::new();
    dl.quad(id, Rect::new(0.0, 0.0, 32.0, 32.0), [0.0, 0.0, 1.0, 1.0], Color::WHITE, Blend::Alpha);
    let out = off.render_rgba(&mut ur, None, [0.0; 4], &dl, &store);
    assert!(near(px(&out, 32, 4, 4), [255, 255, 0, 255], 2));
    assert!(near(px(&out, 32, 24, 24), [0, 0, 0, 255], 2));
    // replacing the image bumps the version and re-uploads
    store.replace(id, Image::solid(2, 2, [0, 255, 255, 255], "t"));
    let out = off.render_rgba(&mut ur, None, [0.0; 4], &dl, &store);
    assert!(near(px(&out, 32, 4, 4), [0, 255, 255, 255], 2));
}

#[test]
fn shader_override_hot_reload_and_broken_edit_recovery() {
    let Some(g) = gpu() else { return eprintln!("no gpu: skipping") };
    let root = std::env::temp_dir().join(format!("bm-ui-shader-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("Shaders/WGSL")).unwrap();
    // start with the unmodified builtin (new files need a restart, edits are picked up)
    fs::write(root.join("Shaders/WGSL/ui.wgsl"), bm_ui::gpu::BUILTIN_SHADER).unwrap();
    let vfs = Arc::new(bm_vfs::Vfs::new(&root).unwrap());
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut ur = UiRenderer::new(g.device.clone(), g.queue.clone(), fmt, Some(vfs));
    let off = UiOffscreen::new(&g.device, 16, 16, fmt);
    let store = TextureStore::new();
    let mut dl = DrawList::new();
    dl.rect(Rect::new(0.0, 0.0, 16.0, 16.0), Color([200, 100, 50, 255]));
    let base = off.render_rgba(&mut ur, None, [0.0, 0.0, 0.0, 1.0], &dl, &store);
    assert!(near(px(&base, 16, 8, 8), [200, 100, 50, 255], 2));

    // modded shader: swap red/blue
    let modded = bm_ui::gpu::BUILTIN_SHADER
        .replace("return vec4<f32>(rgb * tint, t.a * i.color.a);", "let c = rgb * tint; return vec4<f32>(c.b, c.g, c.r, t.a * i.color.a);");
    assert_ne!(modded, bm_ui::gpu::BUILTIN_SHADER);
    fs::write(root.join("Shaders/WGSL/ui.wgsl"), &modded).unwrap();
    assert!(ur.poll_hot_reload());
    let out = off.render_rgba(&mut ur, None, [0.0, 0.0, 0.0, 1.0], &dl, &store);
    assert!(near(px(&out, 16, 8, 8), [50, 100, 200, 255], 2), "{:?}", px(&out, 16, 8, 8));

    // broken edit: previous shader stays active, error is reported
    fs::write(root.join("Shaders/WGSL/ui.wgsl"), "this is not wgsl").unwrap();
    assert!(!ur.poll_hot_reload());
    assert!(ur.shader_error.is_some());
    let out = off.render_rgba(&mut ur, None, [0.0, 0.0, 0.0, 1.0], &dl, &store);
    assert!(near(px(&out, 16, 8, 8), [50, 100, 200, 255], 2));

    // fix it again
    fs::write(root.join("Shaders/WGSL/ui.wgsl"), bm_ui::gpu::BUILTIN_SHADER).unwrap();
    assert!(ur.poll_hot_reload());
    assert!(ur.shader_error.is_none());
    let out = off.render_rgba(&mut ur, None, [0.0, 0.0, 0.0, 1.0], &dl, &store);
    assert!(near(px(&out, 16, 8, 8), [200, 100, 50, 255], 2));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn ui_pass_after_3d_pass_keeps_the_scene_underneath() {
    use bm_render::{Camera, FrameParams, Renderer};
    let Some(g) = gpu() else { return eprintln!("no gpu: skipping") };
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(g.device.clone(), g.queue.clone(), fmt);
    let mut ur = UiRenderer::for_renderer(&r, fmt, None);
    let off = UiOffscreen::new(&g.device, 64, 64, fmt);
    let store = TextureStore::new();
    let cam = Camera::default();
    let params = FrameParams::default();
    // empty UI list: the sky gradient must be visible
    let empty = DrawList::new();
    let sky = off.render_rgba(&mut ur, Some(Scene3d { renderer: &mut r, scene: None, camera: &cam, params: &params }), [0.0; 4], &empty, &store);
    assert!(sky.chunks(4).any(|p| p[0] > 10 || p[1] > 10 || p[2] > 10), "sky pass produced only black");
    let mut dl = DrawList::new();
    dl.rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color([255, 0, 255, 255]));
    let out = off.render_rgba(&mut ur, Some(Scene3d { renderer: &mut r, scene: None, camera: &cam, params: &params }), [0.0; 4], &dl, &store);
    assert!(near(px(&out, 64, 3, 3), [255, 0, 255, 255], 2));
    assert_eq!(px(&out, 64, 40, 40), px(&sky, 64, 40, 40), "UI must not disturb pixels it does not cover");
}
