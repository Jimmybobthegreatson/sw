//! GPU + real-file tests. They skip (pass) without a wgpu adapter / without the game files.
#![cfg(feature = "render")]

use std::sync::Arc;

use bm_models::math::Mat4;
use bm_particles::render::{ParticleRenderer, BUILTIN_SOURCE};
use bm_particles::*;
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

fn camera() -> Camera {
    let mut c = Camera { position: [0.0, 60.0, -300.0], pitch: 0.0, ..Camera::default() };
    c.look_at([0.0, 60.0, 0.0]);
    c
}

fn burst_system() -> ParticleSystem {
    let mut s = ParticleSystem::new(9, 1000);
    let d = EmitterDesc {
        blend: ParticleBlend::Additive,
        spawn: SpawnDesc { rate: 0.0, bursts: vec![Burst { time: 0.0, count: 20 }], looping: false, ..Default::default() },
        lifetime: Range(3.0, 3.0),
        size: Range(60.0, 60.0),
        shape: EmitShape::Sphere { radius: 30.0 },
        ..Default::default()
    };
    s.add_emitter(d, Mat4::from_translation([0.0, 60.0, 0.0]));
    s.update(0.1);
    s
}

fn render(r: &mut Renderer, t: &OffscreenTarget) -> Vec<u8> {
    t.render_rgba(r, None, &camera(), &FrameParams::default())
}

fn diff(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

#[test]
fn particles_draw_into_the_pass() {
    let Some(c) = ctx() else { return };
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), fmt);
    let (hook, handle) = ParticleRenderer::new(None);
    r.set_hooks(Box::new(hook));
    let t = OffscreenTarget::new(&c.device, 96, 96, fmt);
    let empty = render(&mut r, &t);
    assert_eq!(handle.last_frame(), (0, 0));
    handle.submit(&burst_system(), &camera());
    let with = render(&mut r, &t);
    assert_eq!(handle.last_frame().0, 20);
    assert!(diff(&empty, &with) > 200, "particles must change the picture");
    handle.set_enabled(false);
    let off = render(&mut r, &t);
    assert_eq!(diff(&empty, &off), 0);
}

#[test]
fn shader_override_and_broken_edit() {
    let Some(c) = ctx() else { return };
    let dir = std::env::temp_dir().join(format!("bm_particles_shader_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Shaders/WGSL")).unwrap();
    std::fs::write(dir.join("Shaders/WGSL/particles.wgsl"), BUILTIN_SOURCE).unwrap();
    let vfs = Arc::new(bm_vfs::Vfs::new(&dir).unwrap());
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), fmt);
    let (hook, handle) = ParticleRenderer::new(Some(vfs));
    r.set_hooks(Box::new(hook));
    let t = OffscreenTarget::new(&c.device, 96, 96, fmt);
    handle.submit(&burst_system(), &camera());
    let base = render(&mut r, &t);

    let patched = BUILTIN_SOURCE.replace("return vec4<f32>(encode(texel.rgb * tint), a);", "return vec4<f32>(0.0, 1.0, 0.0, a);");
    assert_ne!(patched, BUILTIN_SOURCE);
    std::fs::write(dir.join("Shaders/WGSL/particles.wgsl"), &patched).unwrap();
    assert!(handle.poll_hot_reload());
    let green = render(&mut r, &t);
    assert!(diff(&base, &green) > 200);
    let (mut g, mut rd) = (0u64, 0u64);
    for p in green.chunks_exact(4) {
        g += p[1] as u64;
        rd += p[0] as u64;
    }
    assert!(g > rd, "override shader must dominate green ({g} vs {rd})");

    std::fs::write(dir.join("Shaders/WGSL/particles.wgsl"), "not wgsl at all").unwrap();
    assert!(handle.poll_hot_reload());
    let still = render(&mut r, &t);
    assert!(handle.shader_error().is_some());
    assert_eq!(diff(&green, &still), 0, "previous shader stays active");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn real_scene_templates_decode() {
    let Ok(vfs) = bm_vfs::Vfs::from_env() else { return };
    if vfs.stat("Scenes/M01/M01_main.GMS").is_none() {
        return;
    }
    let scene = bm_models::Scene::load(&vfs, "Scenes/M01/M01_main").unwrap();
    let sp = SceneParticles::from_scene(&scene);
    assert!(sp.templates.len() >= 90, "{}", sp.templates.len());
    assert!(sp.emitters.len() > 200);
    // every template got its sprite image
    let miss: Vec<_> = sp.templates.keys().filter(|k| !sp.sprite_names.contains_key(*k)).collect();
    assert!(miss.len() <= 3, "templates without sprite: {miss:?}");
    let fire = sp.template_desc("Fire_FireWood_01").unwrap();
    assert_eq!(fire.blend, ParticleBlend::Additive);
    assert!((fire.lifetime.mid() - 0.8).abs() < 0.05);
    assert_eq!(fire.color.keys.len(), 8);
    let smoke = sp.template_desc("Smoke_FireWood_01").unwrap();
    assert_eq!(smoke.blend, ParticleBlend::Alpha);
    assert!(smoke.acceleration[1] > 0.0);
    // all level-scene emitters whose template lives in the scene are resolved
    let resolved = sp.emitters.iter().filter(|e| e.desc.is_some()).count();
    assert!(resolved * 100 > sp.emitters.len() * 90, "{resolved}/{}", sp.emitters.len());
    // the simulation runs on decoded data
    let mut sys = ParticleSystem::new(1, 5000);
    for e in sp.emitters.iter().filter(|e| e.desc.is_some()).take(40) {
        sys.add_emitter(e.desc.clone().unwrap(), e.world);
    }
    for _ in 0..60 {
        sys.update(1.0 / 30.0);
    }
    assert!(sys.alive_count() > 0);
}
