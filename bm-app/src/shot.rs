//! `bm screenshot`: headless offscreen render of a level after N simulated ticks (CI / verification), with optional
//! per-stage timing of the frame loop (`--profile`).

use crate::config::{Config, GameArgs, mount, resolve_scene};
use crate::hud::{HudBridge, draw_overlay};
use crate::level::{LevelView, load_game};
use bm_game::PlayerInput;
use bm_particles::render::ParticleRenderer;
use bm_render::{GpuContext, Renderer, UploadOptions};
use bm_ui::{DrawList, Hud, HudState, Scene3d, UiOffscreen, UiRenderer, UiSystem};
use std::path::PathBuf;
use std::time::Instant;

pub struct ShotArgs {
    pub level: String,
    pub out: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Simulation ticks (1/60 s) before the capture.
    pub ticks: u32,
    /// Hold "forward" for this many ticks (a value >= `ticks` keeps walking through the capture frames).
    pub walk_ticks: u32,
    /// Hold aim (draws the weapon) the whole time.
    pub aim: bool,
    /// Draw the weapon at tick 0.
    pub draw: bool,
    /// Extra yaw turn applied at tick 0, degrees.
    pub turn_deg: f32,
    /// Extra camera pitch applied at tick 0, degrees (positive = up).
    pub pitch_deg: f32,
    /// Free camera `x,y,z,yaw_deg,pitch_deg` (overrides the game camera).
    pub cam: Option<[f32; 5]>,
    /// Console commands (`;` separated, e.g. `tp 4900 100 -2600;door open HO_DoorDA_01`) run before the simulation.
    pub exec: Option<String>,
    pub no_hud: bool,
    pub debug: bool,
    /// Render this many frames (the last one is saved) and print per-stage timings.
    pub frames: u32,
    pub profile: bool,
    /// Camera in front of the n-th character whose name contains this text.
    pub focus: Option<String>,
    pub set: Option<String>,
    pub face: Option<String>,
    pub focus_dist: f32,
    pub focus_nth: usize,
    pub focus_height: f32,
    /// Overrides of the video quality switches for this capture.
    pub no_shadows: bool,
    pub no_fxaa: bool,
    pub shadow_res: Option<u32>,
    pub no_reflections: bool,
}

fn avg(v: &[f32]) -> f32 {
    v.iter().sum::<f32>() / v.len().max(1) as f32
}

pub fn run(args: &GameArgs, cfg: &Config, a: ShotArgs) -> Result<(), String> {
    let m = mount(args, cfg)?;
    for n in &m.notes {
        log::info!("{n}");
    }
    let scene = resolve_scene(&m.vfs, &a.level);
    let t0 = Instant::now();
    let ctx = GpuContext::new_headless()?;
    let info = ctx.adapter_info();
    println!("adapter: {} ({:?}, {:?}){}", info.name, info.backend, info.device_type, if ctx.is_fallback { " [software fallback]" } else { "" });
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::with_vfs(ctx.device.clone(), ctx.queue.clone(), format, Some(m.vfs.clone()));
    let (hook, particles) = ParticleRenderer::new(Some(m.vfs.clone()));
    renderer.set_hooks(Box::new(hook));
    let mut ur = UiRenderer::for_renderer(&renderer, format, Some(m.vfs.clone()));

    let tl = Instant::now();
    let game = load_game(&m, cfg, &scene, None)?;
    println!("game loaded in {:.2}s ({} NPCs, {} collision triangles)", tl.elapsed().as_secs_f32(), game.stats.npcs, game.stats.collision_triangles);
    let tu = Instant::now();
    let mut video = cfg.video.clone();
    video.shadows &= !a.no_shadows;
    video.fxaa &= !a.no_fxaa;
    video.reflections &= !a.no_reflections;
    if let Some(r) = a.shadow_res {
        video.shadow_resolution = r;
    }
    let mut lv = LevelView::new(&mut renderer, &particles, game, &UploadOptions::default(), &video)?;
    println!("scene uploaded in {:.2}s", tu.elapsed().as_secs_f32());
    let mut ui = UiSystem::new(m.vfs.clone());
    if let Err(e) = ui.load_scene_ui(&scene) {
        println!("warning: UI resources: {e}");
    }

    if let Some(f) = &a.face {
        lv.game.face_debug = if f == "none" {
            vec![("Neutral".to_string(), 0.0)]
        } else {
            f.split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.parse().unwrap_or(1.0))).collect()
        };
    }
    if let Some(cmds) = &a.exec {
        for c in cmds.split(';').map(str::trim).filter(|c| !c.is_empty()) {
            println!("> {c}
{}", lv.game.run_command(c));
        }
    }
    // simulate
    let ts = Instant::now();
    let fwd = PlayerInput { move_y: 1.0, aim: a.aim, ..Default::default() };
    let idle = PlayerInput { aim: a.aim, ..Default::default() };
    for i in 0..a.ticks {
        let mut inp = if i < a.walk_ticks { fwd.clone() } else { idle.clone() };
        if i == 0 {
            inp.look_dx = a.turn_deg.to_radians();
            inp.look_dy = a.pitch_deg.to_radians();
            inp.draw = a.draw;
        }
        lv.game.tick(&inp);
        if i % 4 == 0 {
            lv.game.present(1.0 / 15.0);
        }
    }
    println!("simulated {} ticks in {:.2}s (player {:?})", a.ticks, ts.elapsed().as_secs_f32(), lv.game.world.player_pos());

    let mut cam = a.cam;
    if let Some(f) = &a.focus {
        let w = &lv.game.world;
        let mut hits: Vec<(String, bm_game::math::Vec3, f32)> = w
            .infos
            .iter()
            .filter(|(_, i)| i.name.to_lowercase().contains(&f.to_lowercase()) && matches!(i.kind, bm_game::world::Kind::Npc | bm_game::world::Kind::Player))
            .filter_map(|(id, i)| w.transforms.get(id).map(|t| (i.name.clone(), t.pos, t.yaw)))
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0));
        match hits.get(a.focus_nth) {
            Some((name, pos, yaw)) => {
                let fwd = bm_game::math::forward(*yaw);
                let eye = [pos.x + fwd.x * a.focus_dist, pos.y + a.focus_height, pos.z + fwd.z * a.focus_dist];
                let sgn = if a.focus_dist < 0.0 { -1.0f32 } else { 1.0 };
                let cyaw = (-fwd.x * sgn).atan2(-fwd.z * sgn).to_degrees();
                println!("focus on '{name}' at ({:.0},{:.0},{:.0}) yaw {:.0} ({} matches)", pos.x, pos.y, pos.z, yaw.to_degrees(), hits.len());
                cam = Some([eye[0], eye[1], eye[2], cyaw, -4.0]);
            }
            None => println!("focus: no character matches '{f}'"),
        }
    }
    let off = UiOffscreen::new(&ctx.device, a.width, a.height, format);
    let screen = [a.width as f32, a.height as f32];
    let mut hud = HudState::new();
    let mut bridge = HudBridge::default();
    let (mut t_sim, mut t_sync, mut t_ui, mut t_render) = (vec![], vec![], vec![], vec![]);
    let mut last = Vec::new();
    for _ in 0..a.frames.max(1) {
        let t = Instant::now();
        lv.game.frame(1.0 / 60.0, if a.walk_ticks >= a.ticks { &fwd } else { &idle });
        t_sim.push(t.elapsed().as_secs_f32() * 1000.0);

        let t = Instant::now();
        let rs = lv.game.render_state();
        lv.sync(&rs);
        if let Some(c) = cam {
            lv.camera.position = [c[0], c[1], c[2]];
            lv.camera.yaw = c[3].to_radians();
            lv.camera.pitch = c[4].to_radians();
            // free camera: light the zone the camera is in
            lv.update_lighting(lv.camera.position, 0.0);
        }
        if let Some(kv) = &a.set {
            let p = &mut lv.params;
            for it in kv.split(',') {
                let Some((k, v)) = it.split_once('=') else { continue };
                let v: f32 = v.parse().unwrap_or(0.0);
                match k {
                    "fog_start" => p.fog_start = v,
                    "fog_end" => p.fog_end = v,
                    "sky_gain" => p.sky_gain = v,
                    "exposure" => p.exposure = v,
                    "vcs" => p.vertex_color_strength = v,
                    "amb" => {
                        for i in 0..3 {
                            p.ambient_sky[i] *= v;
                            p.ambient_ground[i] *= v;
                        }
                    }
                    "sun" => p.sun_color.iter_mut().for_each(|c| *c *= v),
                    "debug" => p.debug_mode = v as u32,
                    "reflections" => p.reflections = v,
                    _ => {}
                }
            }
        }
        t_sync.push(t.elapsed().as_secs_f32() * 1000.0);

        let t = Instant::now();
        let mut dl = DrawList::new();
        if !a.no_hud {
            let heading = lv.camera.yaw.to_degrees().rem_euclid(360.0);
            bridge.update(&mut hud, lv.game.ui_state(), heading, 1.0 / 60.0);
            Hud::draw(&ui.hud_layout, &hud, &ui.ctx, &mut dl, screen);
            let dbg = a.debug.then(|| format!("{} | pos {:?} | room {} | zone {}", info.name, lv.game.world.player_pos().map(|p| (p.x.round(), p.y.round(), p.z.round())), lv.game.world.player_room_name().unwrap_or("-"), lv.game.world.player_zone_name().unwrap_or("-")));
            draw_overlay(&ui.ctx, &mut dl, screen, lv.game.ui_state(), dbg.as_deref());
        }
        t_ui.push(t.elapsed().as_secs_f32() * 1000.0);

        let t = Instant::now();
        last = off.render_rgba(&mut ur, Some(Scene3d { renderer: &mut renderer, scene: Some(&lv.gpu), camera: &lv.camera, params: &lv.params }), [0.0; 4], &dl, &ui.ctx.textures);
        t_render.push(t.elapsed().as_secs_f32() * 1000.0);
    }
    bm_render::offscreen::write_png(&a.out, a.width, a.height, &last).map_err(|e| format!("{}: {e}", a.out.display()))?;
    let fs = renderer.last_frame;
    println!("wrote {} ({}x{}) | draws {} tris {} instances {}/{}", a.out.display(), a.width, a.height, fs.draw_calls, fs.triangles, fs.instances_drawn, fs.instances_total);
    if a.profile || a.frames > 1 {
        let skip = if t_sim.len() > 3 { 2 } else { 0 };
        println!(
            "frame timing over {} frames (ms): sim {:.2} | sync {:.2} | ui {:.2} | render+gpu+readback {:.2}",
            t_sim.len() - skip,
            avg(&t_sim[skip..]),
            avg(&t_sync[skip..]),
            avg(&t_ui[skip..]),
            avg(&t_render[skip..])
        );
    }
    println!("total {:.2}s", t0.elapsed().as_secs_f32());
    Ok(())
}
