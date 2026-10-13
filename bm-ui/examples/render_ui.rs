//! Renders the real main menu (menu/frontend.xml + the game's own fonts and strings) and a mock HUD offscreen.
//!
//! `cargo run --release -p bm-ui --example render_ui -- [--out DIR] [--size 1280x720] [--page MainMenu] [--sky]`
//!
//! Writes `menu.png`, `options.png` and `hud.png` into DIR (default `target/ui_out`). Needs `BM_GAME_DIR`
//! (or the default install); without a game dir it falls back to nothing and exits with a message.

use std::path::PathBuf;
use std::sync::Arc;

use bm_render::{Camera, FrameParams, GpuContext, Renderer};
use bm_ui::*;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let _ = env_logger::try_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = PathBuf::from(arg(&args, "--out").unwrap_or_else(|| "target/ui_out".into()));
    let (w, h) = arg(&args, "--size")
        .and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1280), b.parse().unwrap_or(720))))
        .unwrap_or((1280u32, 720u32));
    let page = arg(&args, "--page").unwrap_or_else(|| "MainMenu".into());
    let sky = args.iter().any(|a| a == "--sky");
    std::fs::create_dir_all(&out_dir).unwrap();

    let vfs = match bm_vfs::Vfs::from_env() {
        Ok(v) => Arc::new(v),
        Err(e) => {
            eprintln!("no game dir: {e}");
            return;
        }
    };
    let mut ui = UiSystem::new(vfs.clone());
    ui.load_scene_ui("Scenes/HitmanBloodMoney").expect("front-end scene");
    ui.prefetch_sprites(&["HitmanBackground", "HitmanBackgroundFaded", "MouseBackLight"]);
    ui.add_scene_texture_sprite("logo", "Bitmaps/Screens/MainMenu/HMBMLogo_01");
    println!("fonts: {:?}; sprites: {:?}; textures: {}", ui.ctx.fonts.names(), ui.ctx.sprite_names(), ui.ctx.textures.len());

    let gpu = GpuContext::new_headless().expect("gpu");
    println!("adapter: {}", gpu.adapter_info().name);
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::with_vfs(gpu.device.clone(), gpu.queue.clone(), fmt, Some(vfs.clone()));
    let mut ur = UiRenderer::new(gpu.device.clone(), gpu.queue.clone(), fmt, Some(vfs.clone()));
    let target = UiOffscreen::new(&gpu.device, w, h, fmt);
    let screen = [w as f32, h as f32];
    let vp = Viewport::new(screen[0], screen[1], 1024.0, 768.0);

    // ---- main menu
    let doc = ui.load_menu("frontend").expect("menu/frontend.xml");
    println!("frontend.xml: {} windows, {} warnings", doc.windows.len(), doc.warnings.len());
    let mut nav = MenuNav::new(doc.clone(), &page);
    nav.handle(&Nav::Down, &ui.ctx);
    let mut dl = DrawList::new();
    dl.push_transform(vp.transform());
    dl.gradient_v(Rect::new(0.0, 0.0, 1024.0, 768.0), Color([26, 22, 20, 255]), Color([6, 6, 8, 255]));
    if let Some(logo) = ui.ctx.sprite("logo") {
        logo.draw(&mut dl, [512.0 - logo.size[0] * 0.5, 40.0], None, Color::WHITE);
    }
    nav.draw(&ui.ctx, &mut dl);
    // 4:3 canvas on a 16:9 target: tint the pillar boxes
    dl.pop_transform();
    let rgba = target.render_rgba(&mut ur, None, [0.0, 0.0, 0.0, 1.0], &dl, &ui.ctx.textures);
    bm_ui::image_io::write_png(&out_dir.join("menu.png"), w, h, &rgba).unwrap();
    println!("menu.png: {} quads, {} draw calls", dl.quad_count(), ur.last_draw_calls);

    // ---- options page (sliders, toggles, checkboxes)
    for (k, v) in [("Audio_Volume_Sfx", 70.0), ("Audio_Volume_Dialog", 100.0), ("Audio_Volume_Music", 40.0), ("UseEAX", 1.0)] {
        nav.set_option(k, v);
    }
    let mut nav2 = MenuNav::new(doc.clone(), "SoundMenu");
    nav2.options = nav.options.clone();
    nav2.handle(&Nav::Down, &ui.ctx);
    let mut dl = DrawList::new();
    dl.push_transform(vp.transform());
    dl.gradient_v(Rect::new(0.0, 0.0, 1024.0, 768.0), Color([26, 22, 20, 255]), Color([6, 6, 8, 255]));
    nav2.draw(&ui.ctx, &mut dl);
    let rgba = target.render_rgba(&mut ur, None, [0.0, 0.0, 0.0, 1.0], &dl, &ui.ctx.textures);
    bm_ui::image_io::write_png(&out_dir.join("options.png"), w, h, &rgba).unwrap();

    // ---- mock HUD over the (optional) 3D sky pass
    ui.ctx.input_map = InputMap::defaults();
    let mut st = HudState::new();
    st.set("health", 72.0).set("health_max", 100.0).set("notoriety", 6.0).set("notoriety_max", 10.0);
    st.set("suspicion", 0.6).set("suspicion_max", 1.0).set("has_weapon", 1.0).set("show_compass", 1.0).set("heading", 35.0);
    st.set("ammo_clip", 7.0).set("ammo_reserve", 28.0).set_text("weapon_name", "SILVERBALLERS");
    st.prompts.push(Prompt { action: Action::UseAction, text: "%Open door".into() });
    st.say(Some("Diana"), "Agent 47, the target is in the courtyard.", 10.0);
    st.update(1.0);
    let mut dl = DrawList::new();
    Hud::draw(&ui.hud_layout, &st, &ui.ctx, &mut dl, screen);
    let scene3d = if sky {
        let cam = Camera::default();
        let params = FrameParams::default();
        Some((cam, params))
    } else {
        None
    };
    let rgba = match &scene3d {
        Some((cam, params)) => {
            target.render_rgba(&mut ur, Some(Scene3d { renderer: &mut renderer, scene: None, camera: cam, params }), [0.0; 4], &dl, &ui.ctx.textures)
        }
        None => {
            // stand-in backdrop so the HUD is readable
            let mut bg = DrawList::new();
            bg.gradient_v(Rect::new(0.0, 0.0, screen[0], screen[1]), Color([120, 140, 160, 255]), Color([50, 60, 50, 255]));
            bg.append(&dl);
            target.render_rgba(&mut ur, None, [0.0, 0.0, 0.0, 1.0], &bg, &ui.ctx.textures)
        }
    };
    bm_ui::image_io::write_png(&out_dir.join("hud.png"), w, h, &rgba).unwrap();
    println!("wrote {}", out_dir.display());
}
