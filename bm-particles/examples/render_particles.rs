//! Offscreen particle showcase.
//!
//! `cargo run --release -p bm-particles --example render_particles -- [options]`
//!
//!   --scene S        scene path (default Scenes/M01/M01_main); `--no-scene` renders only the effects (no game files needed)
//!   --cam x,y,z,yaw,pitch   camera (degrees); default frames the focus point
//!   --focus x,y,z    focus point for the camera / authored test emitters / radius (default: first ambient emitter)
//!   --dist D         camera distance from the focus point (default 500)
//!   --radius R       geometry + emitter radius around the focus (default 3000)
//!   --fx NAME        also play the level emitter whose template name contains NAME at the focus point (repeatable)
//!   --no-geometry    load the scene for its particle data only (sky + ground backdrop)
//!   --spread DX     spacing of the --fx effects along X (default 120)
//!   --tests          authored smoke / sparks / blood test emitters in a row at the focus point
//!   --ambient        start the level's auto-start emitters inside the radius
//!   --time T         simulated seconds before the frame is rendered (default 3)
//!   --size WxH       default 1280x720
//!   --out FILE       default particles.png
//!   --list           print the scene's templates / emitters and exit
use std::path::PathBuf;
use std::sync::Arc;

use bm_models::math::Mat4;
use bm_models::scene::{NodeKind, Scene};
use bm_particles::render::ParticleRenderer;
use bm_particles::{EmitterDesc, ParticleSystem, SceneParticles};
use bm_render::*;

fn parse_floats(s: &str) -> Vec<f32> {
    s.split(',').map(|x| x.trim().parse().unwrap()).collect()
}

fn main() {
    env_logger::init();
    let mut scene_path = "Scenes/M01/M01_main".to_string();
    let mut no_scene = false;
    let mut cam: Option<Vec<f32>> = None;
    let mut focus: Option<[f32; 3]> = None;
    let mut dist = 500.0f32;
    let mut radius = 3000.0f32;
    let mut fx: Vec<String> = Vec::new();
    let mut tests = false;
    let mut no_geometry = false;
    let mut spread = 120.0f32;
    let mut ambient = false;
    let mut time = 3.0f32;
    let mut size = (1280u32, 720u32);
    let mut out = PathBuf::from("particles.png");
    let mut list = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--scene" => scene_path = it.next().unwrap(),
            "--no-scene" => no_scene = true,
            "--cam" => cam = Some(parse_floats(&it.next().unwrap())),
            "--focus" => {
                let v = parse_floats(&it.next().unwrap());
                focus = Some([v[0], v[1], v[2]]);
            }
            "--dist" => dist = it.next().unwrap().parse().unwrap(),
            "--radius" => radius = it.next().unwrap().parse().unwrap(),
            "--fx" => fx.push(it.next().unwrap()),
            "--tests" => tests = true,
            "--no-geometry" => no_geometry = true,
            "--spread" => spread = it.next().unwrap().parse().unwrap(),
            "--ambient" => ambient = true,
            "--time" => time = it.next().unwrap().parse().unwrap(),
            "--size" => {
                let v = it.next().unwrap();
                let (w, h) = v.split_once('x').unwrap();
                size = (w.parse().unwrap(), h.parse().unwrap());
            }
            "--out" => out = it.next().unwrap().into(),
            "--list" => list = true,
            _ => panic!("unknown option {a}"),
        }
    }

    let vfs = if no_scene { None } else { Some(Arc::new(bm_vfs::Vfs::from_env().expect("game dir (BM_GAME_DIR)"))) };
    let scene = vfs.as_ref().map(|v| Scene::load(v, &scene_path).expect("scene"));
    let sp = scene.as_ref().map(SceneParticles::from_scene);

    if list {
        let sp = sp.expect("--list needs a scene");
        let mut names: Vec<_> = sp.templates.keys().collect();
        names.sort();
        println!("{} templates: {names:?}", names.len());
        for e in &sp.emitters {
            let w = e.world.translation();
            println!(
                "emitter {:30} tpl {:28} ({:.0},{:.0},{:.0}) auto={} resolved={} path={}",
                e.name, e.raw.template, w[0], w[1], w[2], e.is_ambient(), e.desc.is_some(), e.path
            );
        }
        return;
    }

    // focus point
    let focus = focus.unwrap_or_else(|| {
        sp.as_ref()
            .and_then(|s| s.ambient().next().map(|e| e.world.translation()))
            .unwrap_or([0.0, 100.0, 0.0])
    });

    // GPU
    let ctx = GpuContext::new_headless().expect("gpu");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::with_vfs(ctx.device.clone(), ctx.queue.clone(), format, vfs.clone());
    let (hook, handle) = ParticleRenderer::new(vfs.clone());
    renderer.set_hooks(Box::new(hook));

    // geometry
    let gpu = scene.as_ref().filter(|_| !no_geometry).map(|scene| {
        let mut items = scene.draw_list(&[NodeKind::Static, NodeKind::Actor, NodeKind::ClothBundle, NodeKind::Scatter]);
        items.retain(|i| {
            let c = i.world.translation();
            let d = [c[0] - focus[0], c[1] - focus[1], c[2] - focus[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < radius
        });
        println!("{} geometry items in radius", items.len());
        SceneGpu::upload(&mut renderer, scene, &items, &UploadOptions::default())
    });

    // simulation
    let mut system = ParticleSystem::new(1234, 20000);
    let mut spawned = 0;
    if let Some(sp) = &sp {
        handle.register_images(&sp.images);
        if ambient {
            for e in sp.ambient() {
                let w = e.world.translation();
                let d = [w[0] - focus[0], w[1] - focus[1], w[2] - focus[2]];
                if (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < radius {
                    system.add_emitter(e.desc.clone().unwrap(), e.world);
                    spawned += 1;
                }
            }
        }
        let n = fx.len() as f32;
        for (i, name) in fx.iter().enumerate() {
            let l = name.to_lowercase();
            let at = [focus[0] + (i as f32 - (n - 1.0) * 0.5) * spread, focus[1], focus[2]];
            let found = sp.emitters.iter().find(|e| e.raw.template.to_lowercase().contains(&l) && e.desc.is_some());
            let desc = match found {
                Some(e) => e.desc.clone().unwrap(),
                None => match sp.templates.keys().find(|k| k.to_lowercase().contains(&l)).and_then(|k| sp.template_desc(k)) {
                    Some(mut d) => {
                        d.spawn.looping = true;
                        Arc::new(d)
                    }
                    None => {
                        eprintln!("no emitter or template matching {name}");
                        continue;
                    }
                },
            };
            system.add_emitter(desc, Mat4::from_translation(at));
            spawned += 1;
        }
    }
    if tests {
        let offsets = [-160.0f32, 0.0, 160.0];
        for (d, dx) in [EmitterDesc::preset_smoke(), EmitterDesc::preset_sparks(), EmitterDesc::preset_blood()].into_iter().zip(offsets) {
            let mut d = d;
            // looping so the frame always has particles
            d.spawn.looping = true;
            if d.spawn.bursts.len() == 1 {
                d.spawn.duration = 1.5;
            }
            system.add_emitter(d, Mat4::from_translation([focus[0] + dx, focus[1], focus[2]]));
            spawned += 1;
        }
    }
    println!("{spawned} emitters, simulating {time}s");
    let steps = (time * 30.0) as u32;
    for _ in 0..steps {
        system.update(1.0 / 30.0);
    }
    println!("{} live particles", system.alive_count());

    // camera
    let mut camera = Camera::default();
    camera.near = 8.0;
    match cam {
        Some(c) => {
            camera.position = [c[0], c[1], c[2]];
            camera.yaw = c[3].to_radians();
            camera.pitch = c[4].to_radians();
        }
        None => {
            camera.position = [focus[0], focus[1] + dist * 0.25, focus[2] - dist];
            camera.look_at([focus[0], focus[1] + dist * 0.2, focus[2]]);
        }
    }
    let mut params = FrameParams::default();
    if let Some(scene) = &scene {
        params.lights = lights_from_scene(scene, 900.0, [1.0, 0.85, 0.6]);
    }
    handle.submit(&system, &camera);
    let target = OffscreenTarget::new(&ctx.device, size.0, size.1, format);
    target.render_png(&mut renderer, gpu.as_ref(), &camera, &params, &out).expect("png");
    println!("wrote {} ({:?} instances/batches)", out.display(), handle.last_frame());
}
