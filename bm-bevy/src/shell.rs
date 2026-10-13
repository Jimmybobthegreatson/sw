//! The application shell: main menu (the game's own `frontend.xml`) -> level -> pause menu (`ingame.xml`), the HUD,
//! input routing and level loading. A port of `bm-app`'s `app.rs` state machine onto Bevy's window, input and ECS.

use crate::args::Args;
use crate::hud::{HudBridge, draw_overlay};
use crate::input::{self, LookConfig, Pads, player_input};
use crate::level::{self, Level, Rig, Targets};
use crate::native_hud::{ItemPanel, NativeHud, OsdAssets, WeaponClass};
use crate::ui_render::{UiQuad, UiRenderer};
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorMoved, CursorOptions, PrimaryWindow, WindowMode};
use bm_game::{Game, LoadOptions};
use bm_script::{ModDirs, ResolvedMods, RuntimeConfig, ScriptRuntime};
use bm_sound::mixer::Bus;
use bm_sound::{AudioEngine, MixerEngine};
use bm_ui::{Action, Color, DrawList, Hud, HudState, InputEvent, InputState, MenuEvent, MenuNav, Nav, Rect, ScreenStack, TextOptions, UiSystem, Viewport};
use std::sync::Arc;

/// Only prompts and subtitles: the native HUD elements are drawn by `native_hud`.
const HUD_SUBTITLES_RON: &str = r##"(
    name: "subtitles",
    reference_height: 720.0,
    margin: 24.0,
    widgets: [
        (id: "prompts", anchor: Bottom, offset: (0.0, -150.0), size: (400.0, 60.0), kind: Prompts(align: Center)),
        (id: "subtitles", anchor: Bottom, offset: (0.0, -60.0), size: (760.0, 60.0), kind: Subtitles(max_lines: 2)),
    ],
)"##;

/// Windows of the game menu XML that start the (native, memory card based) save / load flows.
const SAVE_PAGES: [&str; 5] = ["SaveGameCheckMenu", "SaveConfirmationMenu", "SelectSaveGameMenu", "SaveGameMenu", "EnumerateSaveGamesMenu"];
const LOAD_PAGES: [&str; 4] = ["LoadGameCheckMenu", "SelectLoadGameMenu", "LoadGameConfirmation", "LoadGameMenu"];

/// The mounted game files and the mods that went into them.
pub struct Mounted {
    pub vfs: Arc<bm_vfs::Vfs>,
    pub dirs: ModDirs,
    pub set: ResolvedMods,
}

/// Opens the game directory and mounts the enabled mods (must happen before anything reads through the VFS).
pub fn mount(dir: Option<std::path::PathBuf>) -> Result<Mounted, String> {
    let mut vfs = match dir {
        Some(d) => bm_vfs::Vfs::new(d),
        None => bm_vfs::Vfs::from_env(),
    }
    .map_err(|e| format!("cannot open the game directory: {e}\nSet --game-dir or the BM_GAME_DIR environment variable."))?;
    let dirs = ModDirs::default_dirs();
    let mut set = ResolvedMods::default();
    match bm_script::ModManager::new(dirs.clone()) {
        Ok(mgr) => {
            set = mgr.resolve();
            for p in &set.problems {
                log::warn!("mod problem: {p:?}");
            }
            match mgr.mount(&mut vfs, &set) {
                Ok(rep) if !rep.mounted.is_empty() => log::info!("mounted mods: {}", rep.mounted.join(", ")),
                Ok(_) => {}
                Err(e) => log::warn!("mod mount failed: {e}"),
            }
        }
        Err(e) => log::warn!("mod manager: {e}"),
    }
    Ok(Mounted { vfs: Arc::new(vfs), dirs, set })
}

/// `M01` -> `Scenes/M01/M01_main`; `Scenes/..` paths and `.ZIP` suffixes are normalised.
pub fn resolve_scene(vfs: &bm_vfs::Vfs, name: &str) -> String {
    let n = name.trim().replace('\\', "/");
    let n = n.trim_end_matches(".ZIP").trim_end_matches(".zip").to_string();
    if n.to_ascii_lowercase().starts_with("scenes/") {
        return n;
    }
    let l = n.to_ascii_lowercase();
    let all = vfs.list("Scenes/*/*_main.zip");
    let dir_of = |e: &bm_vfs::EntryInfo| {
        let p = e.path.trim_end_matches(".ZIP").trim_end_matches(".zip").to_string();
        let d = p.split('/').nth(1).unwrap_or("").to_ascii_lowercase();
        (p, d)
    };
    for e in &all {
        let (p, d) = dir_of(e);
        if d == l {
            return p;
        }
    }
    for e in &all {
        let (p, d) = dir_of(e);
        if d.starts_with(&l) {
            return p;
        }
    }
    format!("Scenes/{n}/{n}_main")
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Menu,
    /// Waiting for the "loading" frame to be shown before the (blocking) load starts.
    Loading { name: String, frames: u32 },
    Play,
    Pause,
}

enum Cmd {
    Start,
    Resume,
    SaveGame,
    LoadGame,
    QuitToMenu,
    Restart,
    Exit,
}

#[derive(Clone)]
struct AudioCfg {
    master: f32,
    sfx: f32,
    voice: f32,
    ambient: f32,
    music: f32,
}

/// The shared mixer and the cpal stream pulling from it.
struct AudioOut {
    engine: MixerEngine,
    _stream: bm_sound::device::DeviceOutput,
}

fn open_audio() -> Option<AudioOut> {
    let rate = bm_sound::device::DeviceOutput::default_rate().map_err(|e| log::warn!("no audio device ({e}); running silent")).ok()?;
    let engine = MixerEngine::headless(rate);
    let stream = bm_sound::device::DeviceOutput::start(engine.mixer()).map_err(|e| log::warn!("audio device failed ({e}); running silent")).ok()?;
    log::info!("audio device open: {} Hz, {} ch", stream.sample_rate, stream.channels);
    Some(AudioOut { engine, _stream: stream })
}

pub struct Shell {
    args: Args,
    mounted: Mounted,
    rig: Rig,
    ui: UiSystem,
    input: InputState,
    pads: Pads,
    menu: Option<MenuNav>,
    mode: Mode,
    level: Option<Level>,
    level_name: String,
    hud: HudState,
    bridge: HudBridge,
    screens: Option<ScreenStack>,
    dl: DrawList,
    ui_renderer: UiRenderer,
    audio: Option<AudioOut>,
    audio_cfg: AudioCfg,
    cursor_px: [f32; 2],
    grabbed: bool,
    skip_menu_input: bool,
    pending_load: Option<String>,
    elapsed: f32,
    level_started: Option<f32>,
    look_frames: u32,
    started_menu: bool,
    hud_layout: bm_ui::HudLayout,
    osd: OsdAssets,
    panel_timer: f32,
    debug: bool,
    fps: f32,
    /// Developer console (backtick): open while typing; keys and mouse do not reach the game then.
    console_open: bool,
    console_line: String,
    console_log: Vec<String>,
}

impl Shell {
    pub fn new(args: Args, mounted: Mounted, rig: Rig) -> Shell {
        let mut ui = UiSystem::new(mounted.vfs.clone());
        ui.load_user_bindings(&bm_ui::input::user_config_path());
        let audio = (!args.no_audio).then(open_audio).flatten();
        let audio_cfg = AudioCfg { master: 1.0, sfx: 1.0, voice: 1.0, ambient: 1.0, music: 0.8 };
        let mut s = Shell {
            args,
            mounted,
            rig,
            ui,
            input: InputState::new(),
            pads: Pads::new(),
            menu: None,
            mode: Mode::Menu,
            level: None,
            level_name: String::new(),
            hud: HudState::new(),
            bridge: HudBridge::default(),
            screens: None,
            dl: DrawList::new(),
            ui_renderer: UiRenderer::default(),
            audio,
            audio_cfg,
            cursor_px: [0.0; 2],
            grabbed: false,
            skip_menu_input: false,
            pending_load: None,
            elapsed: 0.0,
            level_started: None,
            look_frames: 0,
            started_menu: false,
            hud_layout: bm_ui::HudLayout::from_ron(HUD_SUBTITLES_RON).expect("subtitle layout"),
            osd: OsdAssets::default(),
            panel_timer: 0.0,
            debug: false,
            fps: 0.0,
            console_open: false,
            console_line: String::new(),
            console_log: Vec::new(),
        };
        s.apply_audio();
        s
    }

    fn apply_audio(&mut self) {
        let c = self.audio_cfg.clone();
        if let Some(a) = self.audio.as_mut() {
            a.engine.set_master_volume(c.master);
            a.engine.set_bus_volume(Bus::Sfx, c.sfx);
            a.engine.set_bus_volume(Bus::Voice, c.voice);
            a.engine.set_bus_volume(Bus::Ambient, c.ambient);
            a.engine.set_bus_volume(Bus::Music, c.music);
        }
    }

    fn apply_audio_options(&mut self, options: &std::collections::HashMap<String, f32>) {
        let a = &mut self.audio_cfg;
        let g = |n: &str, d: f32| options.get(n).map(|v| v / 100.0).unwrap_or(d);
        a.sfx = g("Audio_Volume_Sfx", a.sfx);
        a.voice = g("Audio_Volume_Dialog", a.voice);
        a.music = g("Audio_Volume_Music", a.music);
        a.ambient = g("Audio_Volume_Ambience", a.ambient);
        self.apply_audio();
    }

    fn seed_options(&self, nav: &mut MenuNav) {
        let a = &self.audio_cfg;
        nav.set_option("Audio_Volume_Sfx", a.sfx * 100.0);
        nav.set_option("Audio_Volume_Dialog", a.voice * 100.0);
        nav.set_option("Audio_Volume_Music", a.music * 100.0);
        nav.set_option("Audio_Volume_Ambience", a.ambient * 100.0);
    }

    fn stop_audio(&mut self) {
        if let Some(a) = self.audio.as_mut() {
            a.engine.stop_all(None);
        }
    }

    fn music_pause(&mut self, pause: bool) {
        if let Some(m) = self.level.as_mut().and_then(|l| l.game.audio.as_mut()).and_then(|a| a.music.as_ref()) {
            if pause {
                m.pause(0.3);
            } else {
                m.resume(0.3);
            }
        }
    }

    // ------------------------------------------------------------------------------------------ menus

    fn open_main_menu(&mut self) {
        self.level = None;
        let Some(doc) = ({
            if let Err(e) = self.ui.load_scene_ui("Scenes/HitmanBloodMoney") {
                log::warn!("front-end scene unavailable: {e}");
            }
            self.ui.prefetch_sprites(&["HitmanBackground", "HitmanBackgroundFaded", "MouseBackLight", "SaveLoadDialog"]);
            self.ui.add_scene_texture_sprite("logo", "Bitmaps/Screens/MainMenu/HMBMLogo_01");
            self.ui.load_menu("frontend")
        }) else {
            log::warn!("menu/frontend.xml missing: starting the default level");
            self.menu = None;
            self.mode = Mode::Loading { name: "M01".into(), frames: 0 };
            return;
        };
        self.grabbed = false;
        let mut nav = MenuNav::new(doc, "MainMenu");
        self.seed_options(&mut nav);
        self.menu = Some(nav);
        self.mode = Mode::Menu;
    }

    fn open_pause_menu(&mut self) {
        self.menu = self.ui.load_menu("ingame").map(|doc| {
            let mut nav = MenuNav::new(doc, "IngameMenu");
            self.seed_options(&mut nav);
            nav
        });
        self.mode = Mode::Pause;
        self.grabbed = false;
        self.skip_menu_input = true;
        self.music_pause(true);
    }

    fn resume(&mut self) {
        self.mode = Mode::Play;
        self.grabbed = true;
        self.menu = None;
        self.music_pause(false);
    }

    /// Navigation inputs of this frame -> menu; returns what the application should do.
    fn menu_input(&mut self, screen: Vec2) -> Option<Cmd> {
        if std::mem::take(&mut self.skip_menu_input) {
            return None;
        }
        let Some(nav) = self.menu.as_mut() else {
            let map = &self.ui.ctx.input_map;
            if self.input.pressed(map, Action::ZW_CANCEL) {
                return Some(if self.mode == Mode::Pause { Cmd::Resume } else { Cmd::Exit });
            }
            if self.input.pressed(map, Action::ZW_SELECT) {
                return Some(if self.mode == Mode::Pause { Cmd::Resume } else { Cmd::Start });
            }
            return None;
        };
        let map = self.ui.ctx.input_map.clone();
        let st = &self.input;
        let mut navs: Vec<Nav> = Vec::new();
        for (a, n) in [(Action::ZW_UP, Nav::Up), (Action::ZW_DOWN, Nav::Down), (Action::ZW_LEFT, Nav::Left), (Action::ZW_RIGHT, Nav::Right), (Action::ZW_SELECT, Nav::Select), (Action::ZW_CANCEL, Nav::Back)] {
            if st.pressed(&map, a) {
                navs.push(n);
            }
        }
        let vp = Viewport::new(screen.x, screen.y, 1024.0, 768.0);
        let c = vp.to_canvas(self.cursor_px);
        navs.push(Nav::PointerMove(c));
        if st.pressed(&map, Action::FireWeapon) {
            navs.push(Nav::PointerClick(c));
        }
        let mut cmd = None;
        let mut changed_options = None;
        for n in navs {
            let at_root = nav.stack.len() <= 1;
            let closes = match &n {
                Nav::Back => true,
                Nav::Select | Nav::PointerClick(_) => focused_is_close(nav, &self.ui.ctx),
                _ => false,
            };
            if at_root && closes && self.mode == Mode::Pause {
                return Some(Cmd::Resume);
            }
            match nav.handle(&n, &self.ui.ctx) {
                MenuEvent::PageOpened(w) if self.mode == Mode::Menu && w == "DifficultySelectMenu" => {
                    // The original difficulty page is driven by native code: start the level right away.
                    nav.stack.pop();
                    nav.invalidate();
                    cmd = Some(Cmd::Start);
                }
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

    fn load_game(&mut self, scene: &str) -> Result<Game, String> {
        let t = std::time::Instant::now();
        let opts = LoadOptions { seed: 1, ..Default::default() };
        let mut game = Game::load(self.mounted.vfs.clone(), scene, &opts).map_err(|e| format!("loading {scene}: {e}"))?;
        log::info!("level {scene} simulated-loaded in {:.2}s ({} NPCs)", t.elapsed().as_secs_f32(), game.stats.npcs);
        if let Some(a) = self.audio.as_ref() {
            let mut engine = a.engine.clone();
            engine.stop_all(None);
            if let Err(e) = game.enable_audio(Box::new(engine), true) {
                log::warn!("audio disabled for this level: {e}");
            }
        }
        self.attach_mods(&mut game);
        Ok(game)
    }

    fn attach_mods(&self, game: &mut Game) {
        let m = &self.mounted;
        if m.set.order.is_empty() {
            return;
        }
        let mut rt = ScriptRuntime::new(game.host(), RuntimeConfig { save_dir: Some(m.dirs.save_dir.clone()), ..Default::default() });
        rt.data_mut().set_base("weapons", game.world.data.weapons_json());
        let (data, reports) = rt.load_mods(&m.set);
        for e in &data.errors {
            log::warn!("mod data {}: {}: {}", e.mod_id, e.file, e.message);
        }
        for r in &reports {
            if !r.ok {
                log::warn!("mod script load failed: {r:?}");
            }
        }
        if let Some(w) = rt.data().get("weapons").cloned() {
            game.world.data.apply_weapons_json(&w);
        }
        log::info!("{} mod(s) active", m.set.order.len());
        game.attach_scripts(rt);
    }

    fn shutdown_level(&mut self, commands: &mut Commands) {
        if let Some(mut l) = self.level.take() {
            if let Some(rt) = l.game.scripts() {
                rt.shutdown();
            }
            l.despawn(commands);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_level(&mut self, name: &str, commands: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>) {
        self.shutdown_level(commands);
        let scene = resolve_scene(&self.mounted.vfs, name);
        log::info!("starting level {scene}");
        let t = std::time::Instant::now();
        let game = match self.load_game(&scene) {
            Ok(g) => g,
            Err(e) => {
                log::error!("{e}");
                self.open_main_menu();
                return;
            }
        };
        let mut lv = match level::spawn(game, &self.rig, commands, meshes, images, mats) {
            Ok(l) => l,
            Err(e) => {
                log::error!("{e}");
                self.open_main_menu();
                return;
            }
        };
        if let Err(e) = self.ui.load_scene_ui(&scene) {
            log::warn!("level UI resources: {e}");
        }
        self.osd = OsdAssets::load(&lv.scene, &mut self.ui.ctx.textures);
        self.ui.prefetch_sprites(&["HealthFrame", "FrameLarge", "CompassMarker", "ButtonSymbol", "BigButtonSymbol_PcOnly"]);
        log::info!("level ready in {:.2}s", t.elapsed().as_secs_f32());
        if let Some(slot) = self.pending_load.take() {
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
        self.grabbed = true;
        self.look_frames = 60;
        self.level_started = Some(self.elapsed);
    }

    #[allow(clippy::too_many_arguments)]
    fn do_cmd(&mut self, c: Cmd, commands: &mut Commands, exit: &mut MessageWriter<AppExit>) {
        match c {
            Cmd::Start => {
                let n = self.args.level.clone().unwrap_or_else(|| "M01".into());
                self.mode = Mode::Loading { name: n, frames: 0 };
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
                            if self.mode == Mode::Pause {
                                self.resume();
                            }
                        } else {
                            // the save belongs to another level: load that level first
                            self.pending_load = Some(slot);
                            self.stop_audio();
                            self.mode = Mode::Loading { name: s.level, frames: 0 };
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
                self.shutdown_level(commands);
                self.stop_audio();
                self.open_main_menu();
            }
            Cmd::Restart => {
                let n = self.level_name.clone();
                self.mode = Mode::Loading { name: n, frames: 0 };
            }
            Cmd::Exit => {
                exit.write(AppExit::Success);
            }
        }
    }

    fn draw_loading(&mut self, screen: Vec2, what: &str) {
        let (w, h) = (screen.x, screen.y);
        let dl = &mut self.dl;
        dl.rect(Rect::new(0.0, 0.0, w, h), Color([8, 8, 10, 255]));
        let opts = TextOptions { scale: h / 720.0, ..TextOptions::new("Header") };
        let l = self.ui.ctx.layout_text(what, &opts);
        dl.text(&l, [(w - l.width) * 0.5, h * 0.5 - l.height * 0.5], Color::WHITE);
        let (bw, bh) = (w * 0.4, (h * 0.006).max(3.0));
        let (bx, by) = ((w - bw) * 0.5, h * 0.5 + l.height + 16.0);
        dl.rect(Rect::new(bx, by, bw, bh), Color([40, 40, 44, 255]));
        dl.rect(Rect::new(bx, by, bw * 0.35, bh), Color([200, 40, 36, 255]));
    }

    /// Opens or closes the developer console. Held keys and buttons are dropped on either switch.
    fn toggle_console(&mut self) {
        self.console_open = !self.console_open;
        self.console_line.clear();
        self.input = InputState::new();
    }

    /// One pressed key while the console is open: typing, Backspace, Enter runs the line, Escape closes.
    fn console_key(&mut self, k: &bevy::input::keyboard::KeyboardInput) {
        use bevy::input::keyboard::KeyCode;
        match k.key_code {
            KeyCode::Escape => self.toggle_console(),
            KeyCode::Enter | KeyCode::NumpadEnter => {
                let line = std::mem::take(&mut self.console_line);
                self.console_run(&line);
            }
            KeyCode::Backspace => {
                self.console_line.pop();
            }
            _ => {
                if let Some(t) = &k.text {
                    for ch in t.chars().filter(|c| !c.is_control()) {
                        if self.console_line.len() < 120 {
                            self.console_line.push(ch);
                        }
                    }
                }
            }
        }
    }

    /// Runs one console line: `help`, or a game command (`weapon remington`, `weapon list`, `tp`, `heal`, ...).
    fn console_run(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.console_log.push(format!("> {line}"));
        let out = if line == "help" {
            "commands: weapon [id|list] | give <id> | tp x y z | pos | heal | kill <name> | room | notoriety <n> | door open|close|lock|unlock <name> | save [slot] | load [slot]".to_string()
        } else {
            match self.level.as_mut() {
                Some(lv) => lv.game.run_command(line),
                None => "no level loaded".to_string(),
            }
        };
        if out.trim().is_empty() {
            self.console_log.push("ok".into());
        }
        for l in out.lines() {
            self.console_log.push(l.to_string());
        }
        let excess = self.console_log.len().saturating_sub(200);
        self.console_log.drain(..excess);
        log::info!("console '{line}': {out}");
    }

    fn draw_ui(&mut self, screen: Vec2) {
        let sc = [screen.x, screen.y];
        let vp = Viewport::new(sc[0], sc[1], 1024.0, 768.0);
        let dl = &mut self.dl;
        let ui = &mut self.ui;
        match &self.mode {
            Mode::Menu => {
                dl.gradient_v(Rect::new(0.0, 0.0, sc[0], sc[1]), Color([26, 22, 20, 255]), Color([6, 6, 8, 255]));
                dl.push_transform(vp.transform());
                if let Some(bg) = ui.ctx.sprite("HitmanBackground") {
                    bg.draw(dl, [0.0, 0.0], None, Color::WHITE);
                }
                if let Some(logo) = ui.ctx.sprite("logo") {
                    let sz = [logo.size[0] * 0.62, logo.size[1] * 0.62];
                    logo.draw(dl, [1024.0 - sz[0] - 24.0, 20.0], Some(sz), Color::WHITE);
                }
                if let Some(nav) = self.menu.as_mut() {
                    nav.draw(&ui.ctx, dl);
                }
                dl.pop_transform();
            }
            Mode::Play | Mode::Pause => {
                if let Some(lv) = self.level.as_mut() {
                    self.bridge.update(&mut self.hud, lv.game.ui_state(), lv.heading, if self.mode == Mode::Play { 1.0 / 60.0 } else { 0.0 });
                    let st = lv.game.ui_state();
                    let weapon = st.vars.get("weapon").and_then(|v| v.as_str()).unwrap_or("");
                    let num = |k: &str| st.vars.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let def = lv.game.world.data.weapons.values().find(|d| d.name == weapon);
                    let class = match def.map(|d| d.kind) {
                        Some(bm_game::data::WeaponKind::Rifle) => WeaponClass::Rifle,
                        Some(bm_game::data::WeaponKind::Smg) => WeaponClass::Smg,
                        Some(bm_game::data::WeaponKind::Pistol) => WeaponClass::Pistol,
                        Some(bm_game::data::WeaponKind::Sniper) => WeaponClass::Sniper,
                        Some(bm_game::data::WeaponKind::Shotgun) => WeaponClass::Shotgun,
                        Some(bm_game::data::WeaponKind::Melee) => WeaponClass::CloseCombat,
                        _ => WeaponClass::Item,
                    };
                    let firearm = matches!(class, WeaponClass::Rifle | WeaponClass::Smg | WeaponClass::Pistol | WeaponClass::Sniper | WeaponClass::Shotgun);
                    let bullets = if firearm && !weapon.is_empty() { format!("{} / {}", num("ammo_clip") as u32, num("ammo_reserve") as u32) } else { String::new() };
                    let panel_alpha = (self.panel_timer / 0.3).clamp(0.0, 1.0);
                    let panel = (self.panel_timer > 0.0 && !weapon.is_empty()).then(|| {
                        let loc = lv.scene.loc.as_ref();
                        let leaf = |id: &str, k: &str| loc.and_then(|l| l.get(&format!("AllLevels/Items/{id}/{k}")));
                        let id = lv.scene.loc.as_ref().and_then(|l| {
                            l.leaves().into_iter().find_map(|(p, lf)| {
                                let pl = p.to_ascii_lowercase();
                                (pl.starts_with("allLevels/items/".to_ascii_lowercase().as_str()) && pl.ends_with("/shortname") && lf.text().eq_ignore_ascii_case(weapon)).then(|| p.split('/').nth(2).unwrap_or("").to_string())
                            })
                        });
                        let (name, desc) = match &id {
                            Some(id) => (leaf(id, "Shortname").unwrap_or_else(|| weapon.to_string()), leaf(id, "Description").unwrap_or_default()),
                            None => (weapon.to_string(), String::new()),
                        };
                        let state = if st.crosshair { "HOLSTER" } else { "" };
                        (name, state.to_string(), desc)
                    });
                    crate::native_hud::draw(
                        &ui.ctx,
                        &self.osd,
                        dl,
                        sc,
                        &NativeHud {
                            health: num("health") / 100.0,
                            stealth: num("notoriety").min(100.0),
                            heading: lv.heading,
                            weapon,
                            class,
                            bullets,
                            pointer: st.crosshair,
                            can_act: !st.prompts.is_empty(),
                            panel: panel.as_ref().map(|(n, st, d)| ItemPanel { name: n, state: st, description: d, alpha: panel_alpha }),
                        },
                    );
                    // subtitles and prompts keep the generic layout
                    Hud::draw(&self.hud_layout, &self.hud, &ui.ctx, dl, sc);
                    let debug = self.debug.then(|| {
                        let p = lv.game.world.player_pos().unwrap_or_default();
                        format!("{:.0} fps | pos {:.0} {:.0} {:.0} | room {}", self.fps, p.x, p.y, p.z, lv.game.world.player_room_name().unwrap_or("-"))
                    });
                    draw_overlay(&ui.ctx, dl, sc, lv.game.ui_state(), debug.as_deref());
                    match lv.game.ui_state().screens.last().cloned() {
                        Some(name) => {
                            if self.screens.is_none() {
                                self.screens = Some(ScreenStack::new(&name));
                            }
                            if let Some(st) = &self.screens {
                                let vars = std::collections::HashMap::new();
                                st.draw(&ui.screens, &ui.ctx, dl, &ui.images, &vars);
                            }
                        }
                        None => self.screens = None,
                    }
                }
                if self.console_open && self.level.is_some() {
                    crate::hud::draw_console(&ui.ctx, dl, sc, &self.console_log, &self.console_line);
                }
                if self.mode == Mode::Pause {
                    dl.rect(Rect::new(0.0, 0.0, sc[0], sc[1]), Color([0, 0, 0, 150]));
                    dl.push_transform(vp.transform());
                    if let Some(nav) = self.menu.as_mut() {
                        nav.draw(&ui.ctx, dl);
                    } else {
                        let l = ui.ctx.layout_text("PAUSED  -  Esc / Enter to resume", &TextOptions::new("Header"));
                        dl.text(&l, [512.0 - l.width * 0.5, 360.0], Color::WHITE);
                    }
                    dl.pop_transform();
                }
            }
            Mode::Loading { .. } => {}
        }
    }
}

fn focused_is_close(nav: &mut MenuNav, ctx: &bm_ui::UiContext) -> bool {
    use bm_ui::MenuAction;
    let focus = nav.current().map(|p| p.focus).unwrap_or(0);
    let Some(l) = nav.layout(ctx) else { return false };
    let Some(&k) = l.focus_order.get(focus) else { return false };
    matches!(&l.items[k].kind, bm_ui::menu_view::LaidKind::Button(MenuAction::CloseWindow { .. }))
}

/// Per frame: input -> menus / simulation -> ECS -> UI draw list -> overlay entities.
#[allow(clippy::too_many_arguments)]
pub fn frame(
    mut commands: Commands,
    shell: Option<NonSendMut<Shell>>,
    time: Res<Time>,
    mut keys: MessageReader<KeyboardInput>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut wheel: MessageReader<MouseWheel>,
    mut cursor: MessageReader<CursorMoved>,
    motion: Res<AccumulatedMouseMotion>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut targets: Targets,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut exit: MessageWriter<AppExit>,
    mut ui_quads: Query<(&mut Mesh3d, &mut MeshMaterial3d<StandardMaterial>, &mut Transform, &mut Visibility), With<UiQuad>>,
) {
    use bevy::input::keyboard::KeyCode;
    let Some(mut sh) = shell else { return };
    let sh = &mut *sh;
    let Ok((mut window, mut cursor_opts)) = windows.single_mut() else { return };
    let dt = time.delta_secs().clamp(0.0, 0.1);
    sh.elapsed += dt;
    let screen = Vec2::new(window.width(), window.height());

    // ---- input
    let map = sh.ui.ctx.input_map.clone();
    let want_exit = false;
    let mut pending: Vec<Cmd> = Vec::new();
    for k in keys.read() {
        if k.state == bevy::input::ButtonState::Pressed && !k.repeat && k.key_code == KeyCode::Backquote && sh.level.is_some() {
            sh.toggle_console();
            continue;
        }
        if sh.console_open {
            if k.state == bevy::input::ButtonState::Pressed {
                sh.console_key(k);
            }
            continue;
        }
        if k.state == bevy::input::ButtonState::Pressed && !k.repeat {
            match k.key_code {
                KeyCode::F11 => window.mode = if window.mode == WindowMode::Windowed { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed },
                KeyCode::F3 => sh.debug = !sh.debug,
                KeyCode::F6 if sh.level.is_some() && sh.mode != Mode::Menu => pending.push(Cmd::SaveGame),
                KeyCode::F9 if sh.level.is_some() && sh.mode != Mode::Menu => pending.push(Cmd::LoadGame),
                _ => {}
            }
        }
        if let Some(e) = input::key_event(k) {
            sh.input.event(&e, &map);
        }
    }
    for b in buttons.read() {
        if sh.console_open {
            continue;
        }
        if let Some(e) = input::mouse_event(b) {
            // a click while playing without a grab re-grabs the cursor
            if sh.mode == Mode::Play && !sh.grabbed {
                if let InputEvent::Mouse { pressed: true, .. } = e {
                    sh.grabbed = true;
                }
            }
            sh.input.event(&e, &map);
        }
    }
    for w in wheel.read() {
        if !sh.console_open {
            sh.input.event(&input::wheel_event(w), &map);
        }
    }
    for c in cursor.read() {
        sh.cursor_px = [c.position.x, c.position.y];
        sh.input.event(&input::cursor_event(c), &map);
    }
    let mut pad_events = Vec::new();
    sh.pads.poll(&mut pad_events);
    for e in &pad_events {
        if !sh.console_open {
            sh.input.event(e, &map);
        }
    }
    if sh.grabbed && !sh.console_open {
        sh.input.mouse_delta[0] += motion.delta.x;
        sh.input.mouse_delta[1] += motion.delta.y;
    }

    // ---- first frame: main menu or straight into a level
    if !sh.started_menu {
        sh.started_menu = true;
        match sh.args.level.clone() {
            Some(l) => sh.mode = Mode::Loading { name: l, frames: 0 },
            None => sh.open_main_menu(),
        }
    }

    // ---- mode logic
    for c in pending {
        sh.do_cmd(c, &mut commands, &mut exit);
    }
    match sh.mode.clone() {
        Mode::Play => {
            if sh.input.pressed(&map, Action::IngameMenu) {
                sh.open_pause_menu();
            } else if let Some(lv) = sh.level.as_mut() {
                let look = LookConfig { sensitivity: sh.args.sensitivity * map.mouse_sensitivity / 0.4, invert_y: map.invert_vertical, pad_rate: 2.6 };
                let mut pi = player_input(&sh.input, &map, &look, dt);
                if let Some(t0) = sh.level_started {
                    if sh.elapsed - t0 < sh.args.walk {
                        pi.move_y = 1.0;
                    }
                }
                if let Some(t0) = sh.level_started {
                    let now = sh.elapsed - t0;
                    let due: Vec<String> = sh.args.cmds.iter().filter(|(at, _)| *at <= now).map(|(_, c)| c.clone()).collect();
                    sh.args.cmds.retain(|(at, _)| *at > now);
                    for c in due {
                        let out = lv.game.run_command(&c);
                        log::info!("cmd '{c}': {out}");
                    }
                }
                if sh.look_frames > 0 {
                    // spread over many frames: the controller limits how far one tick may turn
                    pi.look_dx += sh.args.turn.to_radians() / 60.0;
                    pi.look_dy += sh.args.pitch.to_radians() / 60.0;
                    sh.look_frames -= 1;
                }
                // holding the right mouse button opens the item selector (a drawn firearm keeps it for aiming)
                let drawn_gun = lv.game.ui_state().crosshair && lv.game.ui_state().vars.get("ammo_reserve").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0;
                let want_panel = (sh.input.down(&map, Action::FireWeapon2) && !drawn_gun) || std::env::var_os("BM_PANEL").is_some();
                if want_panel {
                    pi.aim = false;
                    sh.panel_timer = 0.3;
                } else {
                    sh.panel_timer = (sh.panel_timer - dt).max(0.0);
                }
                lv.preview_on = sh.panel_timer > 0.0;
                lv.preview_name = lv.game.ui_state().vars.get("weapon").and_then(|v| v.as_str()).unwrap_or("").to_string();
                lv.game.frame(dt, &pi);
                let rs = lv.game.render_state();
                {
                    let (m, i, ma) = (&mut *targets.meshes, &mut *images, &mut *mats);
                    lv.ensure_disguise(&rs, &mut commands, m, i, ma);
                }
                level::sync(lv, &rs, &mut targets);
                for ev in lv.game.take_events() {
                    log::debug!("event {:?}", ev.event);
                }
                if let Some(rt) = lv.game.scripts() {
                    for e in rt.take_errors() {
                        log::warn!("script error: {e:?}");
                    }
                }
            }
        }
        Mode::Menu | Mode::Pause => {
            if let Some(c) = sh.menu_input(screen) {
                sh.do_cmd(c, &mut commands, &mut exit);
            }
            if let Some(nav) = sh.menu.as_mut() {
                nav.update(dt);
            }
        }
        Mode::Loading { name, frames } => {
            if frames >= 2 {
                let (m, i, ma) = (&mut *targets.meshes, &mut *images, &mut *mats);
                sh.start_level(&name, &mut commands, m, i, ma);
            } else {
                sh.mode = Mode::Loading { name, frames: frames + 1 };
            }
        }
    }
    if want_exit {
        exit.write(AppExit::Success);
    }

    // ---- cursor grab follows the mode
    if sh.mode == Mode::Play && sh.grabbed {
        cursor_opts.grab_mode = CursorGrabMode::Locked;
        cursor_opts.visible = false;
    } else {
        cursor_opts.grab_mode = CursorGrabMode::None;
        cursor_opts.visible = true;
    }

    // ---- UI -> overlay entities
    sh.dl.clear();
    if let Mode::Loading { .. } = sh.mode {
        sh.draw_loading(screen, "LOADING...");
    } else {
        sh.draw_ui(screen);
    }
    if dt > 0.0 {
        sh.fps += (1.0 / dt - sh.fps) * 0.1;
    }
    let Shell { dl, ui_renderer, ui, .. } = sh;
    ui_renderer.draw(dl, &ui.ctx.textures, screen, &mut commands, &mut targets.meshes, &mut images, &mut mats, &mut ui_quads);
    sh.input.begin_frame();
}
