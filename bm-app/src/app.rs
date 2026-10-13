//! `bm run`: the windowed game. Main menu (the game's own `frontend.xml`) -> level -> pause menu (`ingame.xml`).

use crate::config::{Config, Mounted, resolve_scene};
use crate::gfx::{FrameTimes, Gfx};
use crate::hud::{HudBridge, draw_overlay};
use crate::input::{LookConfig, Pads, player_input, translate};
use crate::level::{AudioOut, LevelView, load_game};
use bm_render::{Camera, FrameParams, UploadOptions};
use bm_sound::AudioEngine;
use bm_ui::{
    Action, Color, DrawList, Hud, HudState, InputEvent, InputState, MenuEvent, MenuNav, Nav, Rect, ScreenStack, TextOptions, UiSystem, Viewport,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowAttributes, WindowId};

#[derive(Clone, Debug, Default)]
pub struct RunOpts {
    /// Start this level immediately instead of showing the main menu.
    pub level: Option<String>,
    pub fullscreen: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub profile: bool,
    pub no_audio: bool,
    /// Close the window after this many seconds (automated checks).
    pub exit_after: Option<f32>,
    /// Save PNGs of the rendered frame at these times (seconds after start).
    pub shots: Vec<(f32, PathBuf)>,
    /// Synthetic key taps (seconds after start, winit KeyCode name such as `Enter`, `Escape`, `ArrowDown`).
    pub inject: Vec<(f32, String)>,
    /// Scripted input for automated runs: hold "forward" this many seconds after the level started.
    pub walk_secs: f32,
    /// Start straight from this save slot (`bm run --load quick`): the level is taken from the save.
    pub load_slot: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Menu,
    Play,
    Pause,
}

enum Cmd {
    Start,
    Resume,
    /// Quick save into the `quick` slot, then back to the game.
    SaveGame,
    /// Load the `quick` slot (restarting the level first when the save belongs to another one).
    LoadGame,
    QuitToMenu,
    Restart,
    Exit,
}

/// Windows of the game menu XML that start the (native, memory card based) save / load flows.
const SAVE_PAGES: [&str; 5] = ["SaveGameCheckMenu", "SaveConfirmationMenu", "SelectSaveGameMenu", "SaveGameMenu", "EnumerateSaveGamesMenu"];
const LOAD_PAGES: [&str; 4] = ["LoadGameCheckMenu", "SelectLoadGameMenu", "LoadGameConfirmation", "LoadGameMenu"];

#[derive(Default)]
struct Prof {
    frames: u32,
    since: Option<Instant>,
    sim: f32,
    sync: f32,
    ui: f32,
    t: FrameTimes,
}

pub struct App {
    opts: RunOpts,
    cfg: Config,
    cfg_dirty: bool,
    mounted: Mounted,
    gfx: Option<Gfx>,
    audio: AudioOut,
    ui: Option<UiSystem>,
    menu: Option<MenuNav>,
    level: Option<LevelView>,
    level_name: String,
    mode: Mode,
    input: InputState,
    pads: Pads,
    hud: HudState,
    bridge: HudBridge,
    screens: Option<ScreenStack>,
    last: Instant,
    started: Instant,
    last_poll: Instant,
    level_started: Option<Instant>,
    prof: Prof,
    debug: bool,
    fps_shown: f32,
    shots_done: usize,
    injected: usize,
    pending_release: Vec<String>,
    skip_menu_input: bool,
    cursor_px: [f32; 2],
    grabbed: bool,
    /// Save slot to apply as soon as the level it belongs to has finished loading.
    pending_load: Option<String>,
}

pub fn run(opts: RunOpts, cfg: Config, mounted: Mounted) -> Result<(), String> {
    let el = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    el.set_control_flow(ControlFlow::Poll);
    let audio = AudioOut::open(&crate::config::Audio { enabled: cfg.audio.enabled && !opts.no_audio, ..cfg.audio.clone() });
    let mut app = App {
        opts,
        cfg,
        cfg_dirty: false,
        mounted,
        gfx: None,
        audio,
        ui: None,
        menu: None,
        level: None,
        level_name: String::new(),
        mode: Mode::Menu,
        input: InputState::new(),
        pads: Pads::new(),
        hud: HudState::new(),
        bridge: HudBridge::default(),
        screens: None,
        last: Instant::now(),
        started: Instant::now(),
        last_poll: Instant::now(),
        level_started: None,
        prof: Prof::default(),
        debug: false,
        fps_shown: 0.0,
        shots_done: 0,
        injected: 0,
        pending_release: Vec::new(),
        pending_load: None,
        skip_menu_input: false,
        cursor_px: [0.0, 0.0],
        grabbed: false,
    };
    let r = el.run_app(&mut app).map_err(|e| format!("event loop: {e}"));
    app.shutdown();
    r
}

impl App {
    fn shutdown(&mut self) {
        if let Some(l) = &mut self.level {
            if let Some(a) = &l.game.audio {
                let m = self.audio.engine.stats();
                log::info!("audio: {} sounds started, {} failed, {} voices, {} audible, peak {:.2}, {} frames rendered, music {}", a.stats.played, a.stats.failed, m.active_voices, m.audible_voices, m.peak, m.rendered_frames, a.music.is_some());
            }
            if let Some(rt) = l.game.scripts() {
                rt.shutdown();
            }
        }
        if self.cfg_dirty {
            if let Err(e) = self.cfg.save() {
                log::warn!("could not save config: {e}");
            }
        }
    }

    fn window(&self) -> Option<&Window> {
        self.gfx.as_ref().map(|g| &*g.window)
    }

    fn set_grab(&mut self, on: bool) {
        let Some(w) = self.gfx.as_ref().map(|g| g.window.clone()) else { return };
        if on == self.grabbed {
            return;
        }
        if on {
            let r = w.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(e) = r {
                log::debug!("cursor grab: {e}");
            }
            w.set_cursor_visible(false);
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(true);
        }
        self.grabbed = on;
    }

    // ------------------------------------------------------------------------------------------ menus

    fn apply_audio_options(&mut self, options: &std::collections::HashMap<String, f32>) {
        let a = &mut self.cfg.audio;
        let g = |n: &str, d: f32| options.get(n).map(|v| v / 100.0).unwrap_or(d);
        a.sfx = g("Audio_Volume_Sfx", a.sfx);
        a.voice = g("Audio_Volume_Dialog", a.voice);
        a.music_volume = g("Audio_Volume_Music", a.music_volume);
        a.ambient = g("Audio_Volume_Ambience", a.ambient);
        let a = a.clone();
        self.audio.apply(&a);
        self.cfg_dirty = true;
    }

    fn seed_options(&self, nav: &mut MenuNav) {
        let a = &self.cfg.audio;
        nav.set_option("Audio_Volume_Sfx", a.sfx * 100.0);
        nav.set_option("Audio_Volume_Dialog", a.voice * 100.0);
        nav.set_option("Audio_Volume_Music", a.music_volume * 100.0);
        nav.set_option("Audio_Volume_Ambience", a.ambient * 100.0);
    }

    fn open_main_menu(&mut self) {
        self.level = None;
        self.set_grab(false);
        let Some(ui) = self.ui.as_mut() else { return };
        if let Err(e) = ui.load_scene_ui("Scenes/HitmanBloodMoney") {
            log::warn!("front-end scene unavailable: {e}");
        }
        ui.prefetch_sprites(&["HitmanBackground", "HitmanBackgroundFaded", "MouseBackLight", "SaveLoadDialog"]);
        ui.add_scene_texture_sprite("logo", "Bitmaps/Screens/MainMenu/HMBMLogo_01");
        match ui.load_menu("frontend") {
            Some(doc) => {
                let mut nav = MenuNav::new(doc, "MainMenu");
                self.seed_options(&mut nav);
                self.menu = Some(nav);
                self.mode = Mode::Menu;
            }
            None => {
                log::warn!("menu/frontend.xml missing: starting the default level");
                self.menu = None;
                let l = self.cfg.game.level.clone();
                self.start_level(&l);
            }
        }
    }

    fn open_pause_menu(&mut self) {
        let Some(ui) = self.ui.as_mut() else { return };
        match ui.load_menu("ingame") {
            Some(doc) => {
                let mut nav = MenuNav::new(doc, "IngameMenu");
                self.seed_options(&mut nav);
                self.menu = Some(nav);
            }
            None => {
                self.menu = None;
            }
        }
        self.mode = Mode::Pause;
        self.skip_menu_input = true;
        self.set_grab(false);
        if let Some(a) = self.level.as_mut().and_then(|l| l.game.audio.as_mut()).and_then(|a| a.music.as_ref()) {
            a.pause(0.3);
        }
    }

    fn resume(&mut self) {
        self.mode = Mode::Play;
        self.menu = None;
        self.set_grab(true);
        if let Some(a) = self.level.as_mut().and_then(|l| l.game.audio.as_mut()).and_then(|a| a.music.as_ref()) {
            a.resume(0.3);
        }
    }

    /// Navigation inputs of this frame -> menu; returns what the application should do.
    fn menu_input(&mut self) -> Option<Cmd> {
        if std::mem::take(&mut self.skip_menu_input) {
            return None;
        }
        let (Some(ui), Some(gfx)) = (self.ui.as_mut(), self.gfx.as_ref()) else { return None };
        let Some(nav) = self.menu.as_mut() else {
            // no menu document: Esc resumes, Enter starts
            if self.input.pressed(&ui.ctx.input_map, Action::ZW_CANCEL) {
                return Some(if self.mode == Mode::Pause { Cmd::Resume } else { Cmd::Exit });
            }
            if self.input.pressed(&ui.ctx.input_map, Action::ZW_SELECT) {
                return Some(if self.mode == Mode::Pause { Cmd::Resume } else { Cmd::Start });
            }
            return None;
        };
        let map = ui.ctx.input_map.clone();
        let st = &self.input;
        let mut navs: Vec<Nav> = Vec::new();
        for (a, n) in [(Action::ZW_UP, Nav::Up), (Action::ZW_DOWN, Nav::Down), (Action::ZW_LEFT, Nav::Left), (Action::ZW_RIGHT, Nav::Right), (Action::ZW_SELECT, Nav::Select), (Action::ZW_CANCEL, Nav::Back)] {
            if st.pressed(&map, a) {
                navs.push(n);
            }
        }
        let (w, h) = gfx.size();
        let vp = Viewport::new(w as f32, h as f32, 1024.0, 768.0);
        let c = vp.to_canvas(self.cursor_px);
        navs.push(Nav::PointerMove(c));
        if st.pressed(&map, Action::FireWeapon) && self.mode != Mode::Play {
            navs.push(Nav::PointerClick(c));
        }
        let mut cmd = None;
        let mut changed_options = None;
        for n in navs {
            // Back / a CloseWindow button on the root page leaves the pause menu
            let at_root = nav.stack.len() <= 1;
            let closes = match &n {
                Nav::Back => true,
                Nav::Select | Nav::PointerClick(_) => focused_is_close(nav, &ui.ctx),
                _ => false,
            };
            if at_root && closes && self.mode == Mode::Pause {
                return Some(Cmd::Resume);
            }
            let ev = nav.handle(&n, &ui.ctx);
            match ev {
                MenuEvent::PageOpened(w) if self.mode == Mode::Menu && w == "DifficultySelectMenu" => {
                    // The original difficulty page is driven by native code: start the level right away.
                    nav.stack.pop();
                    nav.invalidate();
                    cmd = Some(Cmd::Start);
                }
                // The original save / load pages are native memory-card flows: opening them quick-saves / quick-loads.
                MenuEvent::PageOpened(w) if self.mode == Mode::Pause && SAVE_PAGES.contains(&w.as_str()) => {
                    nav.stack.pop();
                    nav.invalidate();
                    cmd = Some(Cmd::SaveGame);
                }
                MenuEvent::PageOpened(w) if self.mode == Mode::Pause && LOAD_PAGES.contains(&w.as_str()) => {
                    nav.stack.pop();
                    nav.invalidate();
                    cmd = Some(Cmd::LoadGame);
                }
                MenuEvent::OptionChanged { .. } => changed_options = Some(nav.options.clone()),
                MenuEvent::Action { action, .. } => match action {
                    bm_ui::MenuAction::Quit { kind } if kind.eq_ignore_ascii_case("QuitGame") => cmd = Some(Cmd::Exit),
                    bm_ui::MenuAction::Quit { .. } => cmd = Some(Cmd::QuitToMenu),
                    bm_ui::MenuAction::LoadScene { scene, .. } if scene.eq_ignore_ascii_case("CurrentScene") => cmd = Some(Cmd::Restart),
                    other => log::debug!("menu action not handled: {other:?}"),
                },
                _ => {}
            }
        }
        if let Some(o) = changed_options {
            self.apply_audio_options(&o);
        }
        cmd
    }

    // ------------------------------------------------------------------------------------------ levels

    /// Black screen with a caption and a progress bar (`progress` 0..1).
    fn draw_loading(&mut self, what: &str, progress: f32) {
        let (Some(gfx), Some(ui)) = (self.gfx.as_mut(), self.ui.as_ref()) else { return };
        let (w, h) = gfx.size();
        let mut dl = DrawList::new();
        dl.rect(Rect::new(0.0, 0.0, w as f32, h as f32), Color([8, 8, 10, 255]));
        let opts = TextOptions { scale: h as f32 / 720.0, ..TextOptions::new("Header") };
        let l = ui.ctx.layout_text(what, &opts);
        dl.text(&l, [(w as f32 - l.width) * 0.5, h as f32 * 0.5 - l.height * 0.5], Color::WHITE);
        let (bw, bh) = (w as f32 * 0.4, (h as f32 * 0.006).max(3.0));
        let (bx, by) = ((w as f32 - bw) * 0.5, h as f32 * 0.5 + l.height + 16.0);
        dl.rect(Rect::new(bx, by, bw, bh), Color([40, 40, 44, 255]));
        dl.rect(Rect::new(bx, by, bw * progress.clamp(0.0, 1.0), bh), Color([200, 40, 36, 255]));
        gfx.window.set_title(&format!("Blood Money (Rust) - {what}"));
        gfx.render(None, &Camera::default(), &FrameParams::default(), &dl, &ui.ctx.textures, false);
    }

    fn start_level(&mut self, name: &str) {
        self.draw_loading("LOADING...", 0.05);
        let scene = resolve_scene(&self.mounted.vfs, name);
        log::info!("starting level {scene}");
        let t = Instant::now();
        self.draw_loading("LOADING... level data", 0.15);
        let audio = if self.cfg.audio.enabled && !self.opts.no_audio { Some(&self.audio) } else { None };
        let game = match load_game(&self.mounted, &self.cfg, &scene, audio) {
            Ok(g) => g,
            Err(e) => {
                log::error!("{e}");
                if let Some(w) = self.window() {
                    w.set_title(&format!("Blood Money (Rust) - {e}"));
                }
                return;
            }
        };
        self.draw_loading("LOADING... graphics", 0.6);
        let Some(gfx) = self.gfx.as_mut() else { return };
        let lv = match LevelView::new(&mut gfx.renderer, &gfx.particles, game, &UploadOptions::default(), &self.cfg.video) {
            Ok(l) => l,
            Err(e) => {
                log::error!("{e}");
                return;
            }
        };
        if let Some(ui) = self.ui.as_mut() {
            if let Err(e) = ui.load_scene_ui(&scene) {
                log::warn!("level UI resources: {e}");
            }
            // (the in-game menu background sprites are frame tiles, not full pictures: bm-ui draws procedural frames instead)
        }
        log::info!("level ready in {:.2}s", t.elapsed().as_secs_f32());
        let mut lv = lv;
        if let Some(slot) = self.pending_load.take() {
            self.draw_loading("LOADING... saved game", 0.95);
            match lv.game.load_slot(&slot) {
                Ok(r) => log::info!("save {slot} applied: {} NPCs, {} doors, {} pickups", r.npcs, r.doors, r.pickups),
                Err(e) => log::warn!("save {slot} not applied: {e}"),
            }
        }
        self.level = Some(lv);
        self.level_name = name.to_string();
        self.hud = HudState::new();
        self.bridge = HudBridge::default();
        self.menu = None;
        self.mode = Mode::Play;
        self.level_started = Some(Instant::now());
        self.last = Instant::now();
        self.set_grab(true);
    }

    fn do_cmd(&mut self, c: Cmd, el: &ActiveEventLoop) {
        match c {
            Cmd::Start => {
                let n = self.opts.level.clone().unwrap_or_else(|| self.cfg.game.level.clone());
                self.start_level(&n);
            }
            Cmd::Resume => self.resume(),
            Cmd::SaveGame => {
                if let Some(l) = self.level.as_mut() {
                    let label = l.game.save_label();
                    let text = match l.game.save_slot("quick", &label) {
                        Ok(p) => {
                            log::info!("saved to {}", p.display());
                            "Game saved".to_string()
                        }
                        Err(e) => {
                            log::warn!("save failed: {e}");
                            format!("Save failed: {e}")
                        }
                    };
                    l.game.ui.messages.push(bm_game::UiMessage { text, remaining: 3.0 });
                }
                if self.mode == Mode::Pause {
                    self.resume();
                }
            }
            Cmd::LoadGame => {
                let slot = "quick".to_string();
                match bm_game::SaveGame::read(&bm_game::save::slot_path(&slot)) {
                    Ok(s) => {
                        let same = self.level.as_ref().is_some_and(|l| l.game.world.meta.scene == s.level);
                        if same {
                            if let Some(l) = self.level.as_mut() {
                                match l.game.load_slot(&slot) {
                                    Ok(_) => l.game.ui.messages.push(bm_game::UiMessage { text: "Game loaded".into(), remaining: 3.0 }),
                                    Err(e) => log::warn!("load failed: {e}"),
                                }
                            }
                        } else {
                            // the save belongs to another level: load that level first
                            self.pending_load = Some(slot);
                            self.level = None;
                            self.audio.engine_stop_all();
                            self.start_level(&s.level);
                        }
                        if self.mode == Mode::Pause && self.level.is_some() {
                            self.resume();
                        }
                    }
                    Err(e) => {
                        log::warn!("no quick save to load: {e}");
                        if let Some(l) = self.level.as_mut() {
                            l.game.ui.messages.push(bm_game::UiMessage { text: "No saved game".into(), remaining: 3.0 });
                        }
                        if self.mode == Mode::Pause {
                            self.resume();
                        }
                    }
                }
            }
            Cmd::QuitToMenu => {
                if let Some(l) = &mut self.level {
                    if let Some(rt) = l.game.scripts() {
                        rt.shutdown();
                    }
                }
                self.audio.engine_stop_all();
                self.open_main_menu();
            }
            Cmd::Restart => {
                let n = self.level_name.clone();
                self.level = None;
                self.start_level(&n);
            }
            Cmd::Exit => el.exit(),
        }
    }

    // ------------------------------------------------------------------------------------------ frame

    fn poll_hot_reload(&mut self) {
        if self.last_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_poll = Instant::now();
        if let Some(g) = self.gfx.as_mut() {
            g.renderer.poll_hot_reload();
            g.ui.poll_hot_reload();
            g.particles.poll_hot_reload();
        }
        if let Some(ui) = self.ui.as_mut() {
            let r = ui.poll_reload();
            if !r.is_empty() {
                log::info!("ui reloaded: {r:?}");
                if let Some(nav) = self.menu.as_mut() {
                    let name = if self.mode == Mode::Pause { "ingame" } else { "frontend" };
                    if let Some(d) = ui.load_menu(name) {
                        nav.set_doc(d);
                    }
                }
            }
        }
        if let Some(l) = self.level.as_mut() {
            if let Some(rt) = l.game.scripts() {
                for r in rt.poll_hot_reload() {
                    log::info!("mod reload: {r:?}");
                }
                for e in rt.take_errors() {
                    log::warn!("script error: {e:?}");
                }
            }
            for ev in l.game.take_events() {
                log::debug!("event {:?}", ev.event);
            }
        }
    }

    fn redraw(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().clamp(0.0, 0.1);
        self.last = now;

        let mut pad_events = Vec::new();
        self.pads.poll(&mut pad_events);
        if let Some(ui) = self.ui.as_ref() {
            for e in &pad_events {
                self.input.event(e, &ui.ctx.input_map);
            }
        }
        let elapsed = self.started.elapsed().as_secs_f32();
        while let Some((t, key)) = self.opts.inject.get(self.injected).cloned() {
            if elapsed < t {
                break;
            }
            self.injected += 1;
            if let Some(ui) = self.ui.as_ref() {
                log::info!("inject key {key}");
                self.input.event(&InputEvent::Key { code: key.clone(), pressed: true }, &ui.ctx.input_map);
                self.pending_release.push(key);
            }
        }
        self.poll_hot_reload();

        // ---- mode logic
        let t_sim = Instant::now();
        match self.mode {
            Mode::Play => {
                let map = self.ui.as_ref().map(|u| u.ctx.input_map.clone()).unwrap_or_else(bm_ui::InputMap::defaults);
                if self.input.pressed(&map, Action::IngameMenu) {
                    self.open_pause_menu();
                } else if let Some(lv) = self.level.as_mut() {
                    let look = LookConfig { sensitivity: self.cfg.game.mouse_sensitivity * map.mouse_sensitivity / 0.4, invert_y: self.cfg.game.invert_y ^ map.invert_vertical, pad_rate: 2.6 };
                    let mut pi = player_input(&self.input, &map, &look, dt);
                    if let Some(t0) = self.level_started {
                        if t0.elapsed().as_secs_f32() < self.opts.walk_secs {
                            pi.move_y = 1.0;
                        }
                    }
                    lv.game.frame(dt, &pi);
                }
            }
            Mode::Menu | Mode::Pause => {
                if let Some(c) = self.menu_input() {
                    self.do_cmd(c, el);
                }
                if let Some(nav) = self.menu.as_mut() {
                    nav.update(dt);
                }
                if self.mode == Mode::Pause {
                    // keep the world's presentation alive (particles etc. stay frozen)
                }
            }
        }
        let sim_ms = t_sim.elapsed().as_secs_f32() * 1000.0;

        // ---- sync to the renderer
        let t_sync = Instant::now();
        if let Some(lv) = self.level.as_mut() {
            if self.mode == Mode::Play {
                let rs = lv.game.render_state();
                lv.sync(&rs);
            }
        }
        let sync_ms = t_sync.elapsed().as_secs_f32() * 1000.0;

        // ---- UI
        let t_ui = Instant::now();
        let Some(gfx) = self.gfx.as_mut() else { return };
        let (w, h) = gfx.size();
        let screen = [w as f32, h as f32];
        let mut dl = DrawList::new();
        let ui = self.ui.as_mut().expect("ui system");
        let vp = Viewport::new(screen[0], screen[1], 1024.0, 768.0);
        match self.mode {
            Mode::Menu => {
                dl.gradient_v(Rect::new(0.0, 0.0, screen[0], screen[1]), Color([26, 22, 20, 255]), Color([6, 6, 8, 255]));
                dl.push_transform(vp.transform());
                if let Some(bg) = ui.ctx.sprite("HitmanBackground") {
                    bg.draw(&mut dl, [0.0, 0.0], None, Color::WHITE);
                }
                if let Some(logo) = ui.ctx.sprite("logo") {
                    let sz = [logo.size[0] * 0.62, logo.size[1] * 0.62];
                    logo.draw(&mut dl, [1024.0 - sz[0] - 24.0, 20.0], Some(sz), Color::WHITE);
                }
                if let Some(nav) = self.menu.as_mut() {
                    nav.draw(&ui.ctx, &mut dl);
                }
                dl.pop_transform();
            }
            Mode::Play | Mode::Pause => {
                if let Some(lv) = self.level.as_mut() {
                    let heading = lv.camera.yaw.to_degrees().rem_euclid(360.0);
                    self.bridge.update(&mut self.hud, lv.game.ui_state(), heading, if self.mode == Mode::Play { dt } else { 0.0 });
                    Hud::draw(&ui.hud_layout, &self.hud, &ui.ctx, &mut dl, screen);
                    let debug = self.debug.then(|| {
                        let p = lv.game.world.player_pos().unwrap_or_default();
                        let fs = gfx.renderer.last_frame;
                        let f = format!("draws {} tris {} inst {}/{}", fs.draw_calls, fs.triangles, fs.instances_drawn, fs.instances_total);
                        format!("{:.0} fps | pos {:.0} {:.0} {:.0} | room {} | {} | {} {}", self.fps_shown, p.x, p.y, p.z, lv.game.world.player_room_name().unwrap_or("-"), f, gfx.adapter_name, gfx.backend)
                    });
                    draw_overlay(&ui.ctx, &mut dl, screen, lv.game.ui_state(), debug.as_deref());
                    let scr = lv.game.ui_state().screens.last().cloned();
                    match scr {
                        Some(name) => {
                            if self.screens.is_none() {
                                self.screens = Some(ScreenStack::new(&name));
                            }
                            if let Some(st) = &self.screens {
                                let vars = std::collections::HashMap::new();
                                st.draw(&ui.screens, &ui.ctx, &mut dl, &ui.images, &vars);
                            }
                        }
                        None => self.screens = None,
                    }
                }
                if self.mode == Mode::Pause {
                    dl.rect(Rect::new(0.0, 0.0, screen[0], screen[1]), Color([0, 0, 0, 150]));
                    dl.push_transform(vp.transform());
                    if let Some(nav) = self.menu.as_mut() {
                        nav.draw(&ui.ctx, &mut dl);
                    } else if let Some(l) = Some(ui.ctx.layout_text("PAUSED  -  Esc / Enter to resume", &TextOptions::new("Header"))) {
                        dl.text(&l, [512.0 - l.width * 0.5, 360.0], Color::WHITE);
                    }
                    dl.pop_transform();
                }
            }
        }
        let ui_ms = t_ui.elapsed().as_secs_f32() * 1000.0;

        // ---- draw
        let default_cam = Camera::default();
        let default_params = FrameParams::default();
        let (scene, cam, params) = match (&self.level, self.mode) {
            (Some(lv), Mode::Play | Mode::Pause) => (Some(&lv.gpu), &lv.camera, &lv.params),
            _ => (None, &default_cam, &default_params),
        };
        let t = gfx.render(scene, cam, params, &dl, &ui.ctx.textures, self.opts.profile);

        // ---- window screenshot requests (render the same frame offscreen)
        let elapsed = self.started.elapsed().as_secs_f32();
        while let Some((secs, path)) = self.opts.shots.get(self.shots_done).cloned() {
            if elapsed < secs {
                break;
            }
            self.shots_done += 1;
            let rgba = capture(gfx, scene, cam, params, &dl, &ui.ctx.textures);
            match bm_render::offscreen::write_png(&path, w, h, &rgba) {
                Ok(()) => log::info!("wrote {}", path.display()),
                Err(e) => log::error!("screenshot {}: {e}", path.display()),
            }
        }

        self.input.begin_frame();
        if let Some(ui) = self.ui.as_ref() {
            for key in std::mem::take(&mut self.pending_release) {
                self.input.event(&InputEvent::Key { code: key, pressed: false }, &ui.ctx.input_map);
            }
        }

        // ---- stats
        self.prof.frames += 1;
        self.prof.sim += sim_ms;
        self.prof.sync += sync_ms;
        self.prof.ui += ui_ms;
        self.prof.t.record_ms += t.record_ms;
        self.prof.t.present_ms += t.present_ms;
        self.prof.t.gpu_ms += t.gpu_ms;
        let since = *self.prof.since.get_or_insert(now);
        if since.elapsed().as_secs_f32() >= 1.0 {
            let n = self.prof.frames.max(1) as f32;
            self.fps_shown = n / since.elapsed().as_secs_f32();
            let line = format!(
                "{:.0} fps | sim {:.2} sync {:.2} ui {:.2} record {:.2} present {:.2} gpu {:.2} ms",
                self.fps_shown,
                self.prof.sim / n,
                self.prof.sync / n,
                self.prof.ui / n,
                self.prof.t.record_ms / n,
                self.prof.t.present_ms / n,
                self.prof.t.gpu_ms / n
            );
            if self.opts.profile {
                log::info!("frame: {line}");
            }
            let title = match (&self.level, self.mode) {
                (Some(_), Mode::Play | Mode::Pause) => format!("Blood Money (Rust) - {} - {:.0} fps", self.level_name, self.fps_shown),
                _ => "Blood Money (Rust)".to_string(),
            };
            gfx.window.set_title(&title);
            self.prof = Prof::default();
        }

        if let Some(s) = self.opts.exit_after {
            if self.started.elapsed().as_secs_f32() >= s {
                el.exit();
            }
        }
        if let Some(w) = self.window() {
            w.request_redraw();
        }
    }
}

fn capture(gfx: &mut Gfx, scene: Option<&bm_render::SceneGpu>, cam: &Camera, params: &FrameParams, dl: &DrawList, store: &bm_ui::TextureStore) -> Vec<u8> {
    let (w, h) = gfx.size();
    let off = bm_ui::UiOffscreen::new(gfx.renderer.device(), w, h, gfx.config.format);
    let s3 = bm_ui::Scene3d { renderer: &mut gfx.renderer, scene, camera: cam, params };
    off.render_rgba(&mut gfx.ui, Some(s3), [0.0; 4], dl, store)
}

fn focused_is_close(nav: &mut MenuNav, ctx: &bm_ui::UiContext) -> bool {
    use bm_ui::MenuAction;
    let focus = nav.current().map(|p| p.focus).unwrap_or(0);
    let Some(l) = nav.layout(ctx) else { return false };
    let Some(&k) = l.focus_order.get(focus) else { return false };
    matches!(&l.items[k].kind, bm_ui::menu_view::LaidKind::Button(MenuAction::CloseWindow { .. }))
}

pub fn default_attrs(cfg: &Config, opts: &RunOpts) -> WindowAttributes {
    let w = opts.width.unwrap_or(cfg.video.width);
    let h = opts.height.unwrap_or(cfg.video.height);
    let mut a = Window::default_attributes().with_title("Blood Money (Rust)").with_inner_size(LogicalSize::new(w, h)).with_resizable(true);
    if opts.fullscreen || cfg.video.fullscreen {
        a = a.with_fullscreen(Some(Fullscreen::Borderless(None)));
    }
    a
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = default_attrs(&self.cfg, &self.opts);
        let gfx = match Gfx::new(el, attrs, self.mounted.vfs.clone(), self.cfg.video.vsync) {
            Ok(g) => g,
            Err(e) => {
                log::error!("{e}");
                eprintln!("error: {e}");
                el.exit();
                return;
            }
        };
        self.gfx = Some(gfx);
        let mut ui = UiSystem::new(self.mounted.vfs.clone());
        ui.load_user_bindings(&bm_ui::input::user_config_path());
        self.ui = Some(ui);
        self.started = Instant::now();
        // `--load SLOT`: the level comes from the save
        let from_save = self.opts.load_slot.clone().and_then(|slot| match bm_game::SaveGame::read(&bm_game::save::slot_path(&slot)) {
            Ok(sv) => Some((slot, sv.level)),
            Err(e) => {
                log::warn!("--load {slot}: {e}");
                None
            }
        });
        match (from_save, self.opts.level.clone()) {
            (Some((slot, level)), _) => {
                self.pending_load = Some(slot);
                self.start_level(&level);
            }
            (None, Some(l)) => self.start_level(&l),
            (None, None) => self.open_main_menu(),
        }
        if let Some(w) = self.window() {
            w.request_redraw();
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, ev: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = ev {
            if self.grabbed {
                self.input.mouse_delta[0] += delta.0 as f32;
                self.input.mouse_delta[1] += delta.1 as f32;
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, ev: WindowEvent) {
        match &ev {
            WindowEvent::CloseRequested => {
                el.exit();
                return;
            }
            WindowEvent::Resized(sz) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.resize(sz.width, sz.height);
                }
            }
            WindowEvent::Focused(f) => {
                if !*f {
                    self.set_grab(false);
                } else if self.mode == Mode::Play {
                    self.set_grab(true);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_px = [position.x as f32, position.y as f32];
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() && !event.repeat => {
                use winit::keyboard::{KeyCode, PhysicalKey};
                if let PhysicalKey::Code(k) = event.physical_key {
                    match k {
                        KeyCode::F11 => {
                            if let Some(g) = self.gfx.as_ref() {
                                g.toggle_fullscreen();
                            }
                        }
                        KeyCode::F3 => self.debug = !self.debug,
                        KeyCode::F5 => {
                            self.last_poll = Instant::now() - Duration::from_secs(2);
                        }
                        KeyCode::F6 if self.level.is_some() && self.mode != Mode::Menu => self.do_cmd(Cmd::SaveGame, el),
                        KeyCode::F9 if self.level.is_some() && self.mode != Mode::Menu => self.do_cmd(Cmd::LoadGame, el),
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw(el);
                return;
            }
            _ => {}
        }
        let map = self.ui.as_ref().map(|u| u.ctx.input_map.clone());
        if let (Some(e), Some(map)) = (translate(&ev), map) {
            // a click while playing without a grab re-grabs the cursor
            if self.mode == Mode::Play && !self.grabbed {
                if let InputEvent::Mouse { pressed: true, .. } = e {
                    self.set_grab(true);
                }
            }
            self.input.event(&e, &map);
        }
    }
}
