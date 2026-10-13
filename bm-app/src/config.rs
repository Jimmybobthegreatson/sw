//! User configuration (`<user dir>/config.toml`) and game directory / VFS / mod setup shared by every command.

use bm_script::package::{ModDirs, ModManager, ResolvedMods, user_dir};
use bm_vfs::Vfs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub vsync: bool,
    /// Sun shadow maps (cascaded, only where the zone has a sun).
    pub shadows: bool,
    /// Texture size of one shadow cascade (1024 / 2048 / 4096).
    pub shadow_resolution: u32,
    /// Farthest shadowed distance in cm.
    pub shadow_distance: f32,
    /// FXAA post pass.
    pub fxaa: bool,
    /// Environment reflections on glass / chrome / polished surfaces.
    pub reflections: bool,
}

impl Default for Video {
    fn default() -> Self {
        Video { width: 1280, height: 720, fullscreen: false, vsync: true, shadows: true, shadow_resolution: 2048, shadow_distance: 9000.0, fxaa: true, reflections: true }
    }
}

impl Video {
    /// Applies the quality switches to render parameters.
    pub fn apply(&self, p: &mut bm_render::FrameParams) {
        p.shadows.enabled = self.shadows;
        p.shadows.resolution = self.shadow_resolution;
        p.shadows.distance = self.shadow_distance;
        p.fxaa = self.fxaa;
        p.reflections = if self.reflections { 1.0 } else { 0.0 };
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    /// false = never open the sound device (headless mixer, silent).
    pub enabled: bool,
    pub music: bool,
    pub master: f32,
    pub sfx: f32,
    pub voice: f32,
    pub ambient: f32,
    pub music_volume: f32,
}

impl Default for Audio {
    fn default() -> Self {
        Audio { enabled: true, music: true, master: 1.0, sfx: 1.0, voice: 1.0, ambient: 1.0, music_volume: 0.8 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GameCfg {
    /// Install directory of the user's own copy of the game (overridden by `--game-dir` / `BM_GAME_DIR`).
    pub game_dir: Option<PathBuf>,
    /// Level started by `bm run` / the main menu's Start Game (`M01`, `hideout`, or a `Scenes/..` path).
    pub level: String,
    /// Radians of camera rotation per pixel of mouse motion.
    pub mouse_sensitivity: f32,
    pub invert_y: bool,
    pub seed: u64,
}

impl Default for GameCfg {
    fn default() -> Self {
        GameCfg { game_dir: None, level: "M01".into(), mouse_sensitivity: 0.0022, invert_y: false, seed: 1 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ModsCfg {
    pub enabled: bool,
    /// Extra mod roots scanned in addition to `<exe>/Mods` and `<user dir>/Mods`.
    pub extra_dirs: Vec<PathBuf>,
}

impl Default for ModsCfg {
    fn default() -> Self {
        ModsCfg { enabled: true, extra_dirs: Vec::new() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub video: Video,
    pub audio: Audio,
    pub game: GameCfg,
    pub mods: ModsCfg,
}

pub fn config_path() -> PathBuf {
    user_dir().join("config.toml")
}

impl Config {
    /// Loads the config; a missing or broken file yields the defaults (broken files are reported).
    pub fn load() -> Config {
        let p = config_path();
        match std::fs::read_to_string(&p) {
            Ok(t) => match toml::from_str(&t) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("{}: {e}; using defaults", p.display());
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = config_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(p, text)
    }
}

/// Command line options common to everything that touches the game files.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct GameArgs {
    /// Game install directory (default: $BM_GAME_DIR, then config.toml, then the built-in default path).
    #[arg(long, global = true)]
    pub game_dir: Option<PathBuf>,
    /// Additional mod directory (a folder that contains mod folders / zips). May be repeated.
    #[arg(long = "mods-dir", global = true)]
    pub mods_dir: Vec<PathBuf>,
    /// Do not load any mods.
    #[arg(long, global = true)]
    pub no_mods: bool,
}

pub fn resolve_game_dir(args: &GameArgs, cfg: &Config) -> PathBuf {
    if let Some(d) = &args.game_dir {
        return d.clone();
    }
    if let Some(d) = std::env::var_os("BM_GAME_DIR") {
        return PathBuf::from(d);
    }
    if let Some(d) = &cfg.game.game_dir {
        return d.clone();
    }
    bm_vfs::game_dir_from_env()
}

pub fn mod_dirs(args: &GameArgs, cfg: &Config) -> ModDirs {
    let mut dirs = ModDirs::default_dirs();
    for d in cfg.mods.extra_dirs.iter().chain(args.mods_dir.iter()) {
        dirs.roots.push(d.clone());
    }
    dirs
}

/// A mounted VFS plus the mod set that was mounted into it.
pub struct Mounted {
    pub vfs: Arc<Vfs>,
    pub game_dir: PathBuf,
    pub dirs: ModDirs,
    pub set: ResolvedMods,
    pub notes: Vec<String>,
}

/// Opens the game directory and mounts the enabled mods (must happen before anything reads through the VFS).
pub fn mount(args: &GameArgs, cfg: &Config) -> Result<Mounted, String> {
    let game_dir = resolve_game_dir(args, cfg);
    let mut vfs = Vfs::new(&game_dir).map_err(|e| {
        format!(
            "cannot open the game directory {}: {e}\nSet --game-dir, the BM_GAME_DIR environment variable or `game.game_dir` in {}",
            game_dir.display(),
            config_path().display()
        )
    })?;
    let dirs = mod_dirs(args, cfg);
    let mut notes = Vec::new();
    let mut set = ResolvedMods::default();
    if cfg.mods.enabled && !args.no_mods {
        match ModManager::new(dirs.clone()) {
            Ok(mgr) => {
                set = mgr.resolve();
                for p in &set.problems {
                    notes.push(format!("mod problem: {p:?}"));
                }
                match mgr.mount(&mut vfs, &set) {
                    Ok(rep) => {
                        if !rep.mounted.is_empty() {
                            notes.push(format!("mounted mods: {}", rep.mounted.join(", ")));
                        }
                    }
                    Err(e) => notes.push(format!("mod mount failed: {e}")),
                }
            }
            Err(e) => notes.push(format!("mod manager: {e}")),
        }
    }
    Ok(Mounted { vfs: Arc::new(vfs), game_dir, dirs, set, notes })
}

/// `M01` -> `Scenes/M01/M01_main`; `Scenes/..` paths and `.ZIP` suffixes are normalised.
pub fn resolve_scene(vfs: &Vfs, name: &str) -> String {
    let n = name.trim().replace('\\', "/");
    let n = n.trim_end_matches(".ZIP").trim_end_matches(".zip").to_string();
    if n.to_ascii_lowercase().starts_with("scenes/") {
        return n;
    }
    let l = n.to_ascii_lowercase();
    // exact directory name match first (M01 -> Scenes/M01/M01_main), then a prefix match (M01 -> M01_main)
    let all = vfs.list("Scenes/*/*_main.zip");
    for e in &all {
        let p = e.path.trim_end_matches(".ZIP").trim_end_matches(".zip");
        let dir = p.split('/').nth(1).unwrap_or("").to_ascii_lowercase();
        if dir == l {
            return p.to_string();
        }
    }
    for e in &all {
        let p = e.path.trim_end_matches(".ZIP").trim_end_matches(".zip");
        let dir = p.split('/').nth(1).unwrap_or("").to_ascii_lowercase();
        if dir.starts_with(&l) {
            return p.to_string();
        }
    }
    format!("Scenes/{n}/{n}_main")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_tolerates_partial_files() {
        let c = Config::default();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.video.width, 1280);
        let partial: Config = toml::from_str("[audio]\nmusic_volume = 0.1\n").unwrap();
        assert!((partial.audio.music_volume - 0.1).abs() < 1e-6);
        assert_eq!(partial.game.level, "M01");
    }

    #[test]
    fn game_dir_precedence_flag_first() {
        let args = GameArgs { game_dir: Some(PathBuf::from("x")), ..Default::default() };
        assert_eq!(resolve_game_dir(&args, &Config::default()), PathBuf::from("x"));
    }
}
