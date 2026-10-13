//! `bmb` - Blood Money (Rust) on Bevy.
//!
//! Bevy owns the window, the input, the renderer and the main loop; the `bm-*` crates still read the game files, run
//! the simulation and lay out the menus and HUD. Needs the user's own installed copy of Hitman: Blood Money.

mod args;
mod convert;
mod hud;
mod input;
mod level;
mod native_hud;
mod shell;
mod ui_render;

use args::Args;
use bevy::prelude::*;
use bevy::light::GlobalAmbientLight;
use clap::Parser;

/// The game files being mounted on a worker thread while Bevy initialises the window and the GPU.
struct MountJob(std::thread::JoinHandle<Result<shell::Mounted, String>>);

/// `--game-dir`, then `BM_GAME_DIR`, then the folder the exe sits in (or any parent / the current folder) that holds `Scenes`.
fn find_game_dir(args: &Args) -> Option<std::path::PathBuf> {
    if let Some(d) = &args.game_dir {
        return Some(d.clone());
    }
    if std::env::var_os("BM_GAME_DIR").is_some() {
        return None;
    }
    let starts = [std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())), std::env::current_dir().ok()];
    starts.into_iter().flatten().find_map(|s| s.ancestors().find(|a| a.join("Scenes").is_dir()).map(|a| a.to_path_buf()))
}

fn main() {
    let args = Args::parse();
    let dir = find_game_dir(&args);
    let mount = std::thread::spawn(move || shell::mount(dir));
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Hitman: Blood Money (Rust / Bevy)".into(),
                resolution: (args.width, args.height).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(args)
        .insert_resource(GlobalAmbientLight::default())
        .insert_resource(ClearColor(Color::BLACK))
        .insert_non_send(MountJob(mount))
        .add_systems(Startup, setup)
        .add_systems(Update, (shell::frame, scripted_exit))
        .run();
}

fn setup(world: &mut World) {
    let args = world.resource::<Args>().clone();
    let mounted = world.remove_non_send::<MountJob>().map(|j| j.0.join().unwrap_or_else(|_| Err("mount thread panicked".into())));
    let mounted = match mounted.unwrap_or_else(|| Err("no mount job".into())) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            world.write_message(AppExit::error());
            return;
        }
    };
    let rig = {
        use bevy::ecs::system::SystemState;
        let mut state: SystemState<(Commands, ResMut<Assets<Mesh>>, ResMut<Assets<StandardMaterial>>)> = SystemState::new(world);
        let rig = {
            let (mut commands, mut meshes, mut mats) = state.get_mut(world).expect("system state");
            ui_render::spawn_camera(&mut commands);
            level::Rig::spawn(&mut commands, &mut meshes, &mut mats)
        };
        state.apply(world);
        rig
    };
    world.insert_non_send(shell::Shell::new(args, mounted, rig));
}

/// `--exit-after` and `--shot` for automated runs.
fn scripted_exit(mut commands: Commands, args: Res<Args>, time: Res<Time>, mut exit: MessageWriter<AppExit>, mut done: Local<Vec<bool>>) {
    let t = time.elapsed_secs();
    if done.len() != args.shots.len() {
        *done = vec![false; args.shots.len()];
    }
    for (i, (at, path)) in args.shots.iter().enumerate() {
        if !done[i] && t >= *at {
            done[i] = true;
            commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path.clone()));
        }
    }
    if let Some(limit) = args.exit_after {
        if t >= limit {
            exit.write(AppExit::Success);
        }
    }
}
