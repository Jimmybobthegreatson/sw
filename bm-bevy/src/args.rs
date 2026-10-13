//! Command line.

use bevy::prelude::Resource;
use clap::Parser;

#[derive(Parser, Resource, Clone)]
#[command(name = "bmb", version, about = "Blood Money (Rust) on Bevy")]
pub struct Args {
    /// Start this level immediately (M01, hideout, ... or a Scenes/.. path) instead of showing the main menu.
    #[arg(long)]
    pub level: Option<String>,
    /// Game directory (default: BM_GAME_DIR, the folder the exe sits in, or the usual install folder).
    #[arg(long)]
    pub game_dir: Option<std::path::PathBuf>,
    #[arg(long, default_value_t = 1280)]
    pub width: u32,
    #[arg(long, default_value_t = 720)]
    pub height: u32,
    /// Quit after this many seconds (automated runs).
    #[arg(long)]
    pub exit_after: Option<f32>,
    /// Walk forward for this many seconds after the level starts (automated runs).
    #[arg(long, default_value_t = 0.0)]
    pub walk: f32,
    /// Save a screenshot after this many seconds: `--shot 8:out.png`. May be repeated.
    #[arg(long = "shot", value_parser = parse_timed)]
    pub shots: Vec<(f32, String)>,
    /// Run a console command after this many seconds in the level: `--cmd 3:notoriety 40`. May be repeated.
    #[arg(long = "cmd", value_parser = parse_timed)]
    pub cmds: Vec<(f32, String)>,
    /// Initial camera turn in degrees (positive = right), for automated runs.
    #[arg(long, default_value_t = 0.0)]
    pub turn: f32,
    /// Initial camera pitch in degrees (positive = up), for automated runs.
    #[arg(long, default_value_t = 0.0)]
    pub pitch: f32,
    /// Do not open the audio device.
    #[arg(long)]
    pub no_audio: bool,
    /// Mouse look sensitivity in radians per pixel.
    #[arg(long, default_value_t = 0.0022)]
    pub sensitivity: f32,
}

fn parse_timed(s: &str) -> Result<(f32, String), String> {
    let (t, v) = s.split_once(':').ok_or("expected SECONDS:VALUE")?;
    Ok((t.parse().map_err(|e| format!("{e}"))?, v.to_string()))
}
