# Blood Money (Rust)

A clean-room reimplementation of the **Glacier 1** runtime that powers *Hitman: Blood Money* (IO Interactive, 2006), written in
Rust, built to read **your own installed copy of the game** and to be modded easily: loose-file overrides, hot reload, data
patches and a sandboxed Lua scripting API.

> **Legal note.** This repository contains **no game assets and no decompiled game code**: only original Rust source and notes
> on file formats worked out from the files themselves. You need your own legitimate copy of *Hitman: Blood Money* (PC). The
> program only reads that install (it never writes into it). Hitman and Blood Money are trademarks of their respective owners;
> this project is not affiliated with or endorsed by them. Mods should only ship their own content, never game files.

## What works today

* Loads every shipped level (`M00`..`M13`, `hideout`) straight from the original `Scenes/*/*.ZIP` archives and renders it with
  `wgpu` (textured, lit, skinned characters, sky, fog, particles).
* The game's **own main menu and pause menu** (`menu/frontend.xml`, `menu/ingame.xml`, its fonts, localisation and background
  art) drive the front end.
* A playable vertical slice: third-person Agent 47 (walk / run / crouch / aim / shoot / reload / doors / pickups), patrolling
  NPCs with vision-cone AI that follow the level's own waypoint lists and open doors, 400 k-triangle collision on the Hacienda, the real
  mission objectives / targets / exit of every level, room tracking from the original room maps (reverb + music zones follow the room), HUD,
  sound effects, dynamic music, particles and quick save / load.
* Modding: mod folders / zips with `mod.toml`, asset overrides through the VFS, JSON/RON/TOML data patches, Luau scripts with
  event hooks, hot reload (scripts, data, menus, fonts, HUD, shaders), `bm mods ...` management commands.
* Tools: scene file listing, glTF export, free-fly viewer, sound / music inspection, headless screenshots.

Status of every area, known gaps and open questions are tracked in [`PLAN.md`](PLAN.md) and the per-area notes in [`docs/`](docs).

## Requirements

* Windows 10/11 with a Vulkan or DirectX 12 capable GPU (developed on Windows; the code is portable but only tested there).
* Rust stable >= 1.85 (edition 2024) and the MSVC toolchain (the scripting crate vendors Luau and builds it with a C++ compiler).
* An installed copy of *Hitman: Blood Money* (PC).

## Build and run

```powershell
cargo build --release                       # -> target\release\bm.exe

# point it at your install (any one of these):
$env:BM_GAME_DIR = "C:\Games\Hitman Blood Money"     # environment variable
bm --game-dir "C:\Games\Hitman Blood Money" run       # flag
#   or  game.game_dir = "..."  in  %APPDATA%\bm-rs\config.toml   (see `bm config`)

bm run                          # main menu (game's own front end) -> Start Game
bm run --level M01              # straight into the Hacienda
bm run --level hideout --windowed --width 1600 --height 900
bm run --level M01 --profile    # logs per-stage frame timings once a second
```

Controls (rebindable through `%APPDATA%\bm-rs\input.ron`, written by `bm-ui`): `WASD` move, `Shift` run, `Ctrl` crouch,
mouse look, `LMB` fire, `RMB` aim, `R` reload, `E` use door / pick up, `H` draw / holster weapon, `1`-`7` cycle weapons,
`Esc` pause menu, `F3` debug line (shows the current room), `F5` force a hot-reload poll, `F6` / `F9` quick save / load, `F11` fullscreen. A gamepad works too (left stick move,
right stick look, `A` use, triggers fire / aim, `Start` menu).

### Command line

| command | what it does |
|---|---|
| `bm run [--level M01] [--windowed] [--fullscreen] [--width W --height H] [--profile] [--no-audio]` | the game |
| `bm view <scene> [--cam x,y,z,yaw,pitch]` | free-fly viewer (`WASD`/`QE`, hold right mouse to look, `1/2/3` debug views) |
| `bm dump <scene> [--filter txt] [--tree] [--extract NAME --out DIR]` | list the files of a scene archive, print its scene graph, extract one file |
| `bm export <scene> out.glb [--node NAME] [--tex 512] [--native]` | glTF export of a scene or a subtree |
| `bm mods list \| order \| validate [path] \| conflicts \| enable ID \| disable ID \| stubs FILE` | mod management |
| `bm sound list\|info\|dump\|play [NAME] [--scene M01]`, `bm music list\|ron\|render\|play [--scene M01]` | sound bank and music inspection |
| `bm screenshot --level M01 --out x.png [--ticks N --walk N --cam=x,y,z,yaw,pitch --exec "tp x y z;door open NAME" --debug --frames N --profile]` | headless offscreen render (CI) |
| `bm saves`, `bm run --load SLOT` | list save slots / continue from one (in game: `F6` quick save, `F9` quick load, or the pause menu Save / Load) |
| `bm config [--init]` | print / create the user config |

Global flags: `--game-dir DIR`, `--mods-dir DIR` (repeatable), `--no-mods`. Scene names are `M01`, `hideout`, ... or a
`Scenes/..` path. Automation helpers for `bm run`: `--exit-after SECS`, `--shot SECS:file.png` (repeatable), `--key SECS:KeyName`
(synthetic key taps such as `3:Enter`, `9.5:Escape`), `--walk SECS`. Logging uses `RUST_LOG` (default: info for our crates).

### Configuration

`%APPDATA%\bm-rs\config.toml` (override the folder with `BM_USER_DIR`): `[video]` width / height / fullscreen / vsync,
`[audio]` device on/off, music on/off, bus volumes (also changed by the in-game sound options), `[game]` game dir, default level,
mouse sensitivity, `[mods]` enable switch and extra mod folders. The enable list of mods lives in `mods.toml` next to it.

## Modding quickstart

```
%APPDATA%\bm-rs\Mods\my_mod\
    mod.toml                  # id, name, version, dependencies, permissions
    scripts\main.lua          # Luau: events, console commands, timers, UI messages
    data\weapons.json         # JSON merge patch over the game's weapon table
    assets\Music\M01_main\alert.ogg    # anything under assets\ is mounted over the game files
```

`bm mods validate`, `bm mods order` and `bm mods conflicts` check a setup; scripts and data hot reload while the game runs.
Full guide, API reference and type stubs for editor completion: [`docs/modding.md`](docs/modding.md),
`examples/mods/` and `bm mods stubs bm_api.d.lua`. Override paths for each subsystem (sounds, music, menus, fonts, HUD,
shaders, input defaults) are listed in `docs/sound.md`, `docs/music.md`, `docs/ui.md` and `docs/render.md`.

## Crate map

| crate | role | notes |
|---|---|---|
| `bm-vfs` | virtual file system: game dir + mod dirs, scene ZIPs, `.WAV/.WHD` banks, `pc_eng.str` | [`docs/vfs.md`](docs/vfs.md) |
| `bm-formats` | parsers for `.gms .prp .prm .mat .tex .loc .anm .oct .rmc .rmi .sgp .snd ...` and typed gameplay nodes (doors, routes, volumes, objectives) | [`docs/formats.md`](docs/formats.md) |
| `bm-models` | meshes, skeletons, materials, scene graph, glTF / OBJ export + import | [`docs/models.md`](docs/models.md) |
| `bm-render` | `wgpu` renderer (materials, lights, fog, skinning, sky, shader hot reload, hooks) | [`docs/render.md`](docs/render.md) |
| `bm-particles` | `.PRP` particle templates, CPU simulation, GPU billboard renderer | [`docs/particles.md`](docs/particles.md) |
| `bm-sound` | sound banks, `.SND` objects, 3D mixer, reverb, cpal output (`device` feature) | [`docs/sound.md`](docs/sound.md) |
| `bm-music` | data-driven dynamic music streamed from `pc_eng.str` | [`docs/music.md`](docs/music.md) |
| `bm-ui` | menus (game XML), fonts, localisation, HUD, input bindings, UI renderer | [`docs/ui.md`](docs/ui.md) |
| `bm-script` | mod manager, Luau runtime, events, data patches, hot reload | [`docs/modding.md`](docs/modding.md) |
| `bm-game` | simulation: collision, character controller, nav, AI, doors, rooms / areas, weapons, missions, saves, `RenderState` | [`docs/game.md`](docs/game.md) |
| `bm-app` | the `bm` binary: window, main loop, menus flow, CLI, screenshots | [`docs/app.md`](docs/app.md) |

## Known limitations

* Animation is decoded (`docs/anim.md`): characters walk, run, idle, crouch and die with the original `.ANM` clips and wear the
  outfit stored in their actor node. Open: facial poses (mouths stay in the bind expression), blending/layering of clips, weapons in hands.
* Light colours/ranges, the real sun/fog values and shadow data are unknown: lighting is a plausible default, there are no shadows,
  reflections or water; large parts of terrain (scatter objects, ground) and some hideout surfaces render incorrectly.
* No original mission scripts (native code in `Scriptcs/*.dll`), AI tables, scoring, notoriety or disguises: the mission is "kill the targets, leave through the
  exit" with the real texts, targets and exit areas decoded from the level files (levels whose exit is script driven complete when the targets are dead).
  Only the Hacienda (`M01`), M06 and the hideout have been walked in detail; all 14 levels load and render.
* Saves are quick-slot style (player, NPCs, doors, pickups, mission; AI memory is re-planned); no cut-scenes, no character dialogue playback, no controller rebinding UI beyond the game's own pages.
* The difficulty selection of the front end is native-code driven in the original and is skipped (Start Game goes straight in).

## Development

```powershell
cargo check --workspace
cargo test --workspace --release       # real-file tests use BM_GAME_DIR and skip without it
cargo build --release
bm screenshot --level M01 --out m01.png --frames 30 --profile    # CI-style smoke test + timings
```

There is no license file yet: add one before publishing.


Status update (agent #13): faces animate (rest pose, blinking, lip-sync hook), window glass is transparent/reflective, interior ambient tuned, M01 horizon haze fixed. See docs/anim.md section 7 and docs/render.md (lighting audit).


Status update (agent #14): room maps (`.RMI` / `.RMC`), the room portal graph (`.SGP`), `.OCT` block structure and the gameplay nodes (doors, `PathFollower` routes, actor scripts, trigger volumes, exits, pickups, LOC objectives) are decoded (`docs/formats.md`) and used by `bm-game` (rooms -> reverb / music zones / HUD, real doors incl. NPCs opening them, decoded routes, level-derived missions, trigger volumes) plus versioned save games.
