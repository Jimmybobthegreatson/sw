# bm-app - the `bm` binary

Crate `bm-app` (binary `bm`) ties the other crates together: `winit` window + `wgpu` surface, the main loop, the game's own menus,
audio output, mod loading and the command line. Usage is in the top-level `README.md`; this file records how it is put together and
what was verified.

## Modules

| file | role |
|---|---|
| `main.rs` | clap CLI (`run view dump export mods sound music screenshot saves config`), logging (`RUST_LOG`, default info for our crates) |
| `config.rs` | `config.toml` (`<user dir>` = `$BM_USER_DIR` / `%APPDATA%\bm-rs`), game dir resolution (`--game-dir` > `$BM_GAME_DIR` > config > default), `mount()` = `Vfs::new` + `ModManager::resolve` + `mount` **before** anything reads the VFS, `resolve_scene("M01")` |
| `level.rs` | `AudioOut` (cpal device through `bm-sound/device`, silent headless mixer fallback), `load_game` (Game + audio + mods), `attach_mods` (`ScriptRuntime` on `game.host()`, `weapons` dataset patched by mods and applied back to `GameData`), `LevelView` = Game + uploaded `SceneGpu` + `sync(RenderState)` |
| `gfx.rs` | window/surface/depth, `Renderer` + `UiRenderer` + `ParticleRenderer` hook, frame record (3D pass -> UI pass -> present) with CPU/GPU timings |
| `app.rs` | `bm run`: state machine Menu -> Play <-> Pause, input, hot-reload polling (1 Hz: shaders, UI files, particle shader, mod scripts/data), profiling log, automation (`--exit-after/--shot/--key/--walk`) |
| `input.rs` | winit/gilrs events -> `bm_ui::InputEvent`; `InputState` + `InputMap` -> `PlayerInput` (mouse look, stick look, cycle keys) |
| `hud.rs` | `UiState` -> `HudState` variables / prompts / subtitles; overlay (mission title, objectives, banner, debug line) |
| `shot.rs`, `viewer.rs`, `tools.rs` | `bm screenshot` (`--focus <name> [--focus-dist cm] [--focus-nth n] [--focus-height cm]` puts the camera in front of a character, `--cam=x,y,z,yaw,pitch` free camera, `--walk N` >= `--ticks` keeps walking through the capture frame), `bm view`, `dump/export/mods/sound/music` |

## Frame flow (`bm run`)

1. events -> `InputState` (one `begin_frame` per frame); gamepad polled through gilrs;
2. Play: `game.frame(dt, &PlayerInput)` (fixed 60 Hz ticks inside) -> `game.render_state()` -> `LevelView::sync`;
   Menu / Pause: `MenuNav` fed with `ZW_*` actions and pointer events;
3. `sync`: for every `RenderEntity` the delta `entity.world * inverse(node.world)` is applied to the node's whole subtree of
   `SceneGpu` items (`set_world`), skinning matrices go to `SceneGpu::set_skinning`, `hidden_nodes` -> `set_visible`, the game camera
   becomes the `bm_render::Camera` (yaw/pitch from `forward`), `RenderState::lights` become the point lights, `RenderState::particles`
   goes to `ParticleHandle::submit_list`;
4. one command buffer: `Renderer::render` (sky, opaque, transparent, particles hook) then `UiRenderer::render` (HUD from
   `bm_ui::Hud`, overlay, menus), present. `--profile` waits for the GPU after submit and logs CPU stages + GPU time.

Menu flow: main menu = `menu/frontend.xml` page `MainMenu` over the `HitmanBackground` picture. The `DifficultySelectMenu` page is an
empty window driven by native code in the original, so *opening it starts the level* (`--level` or `game.level` from the config).
Pause menu = `menu/ingame.xml` page `IngameMenu`; `QuitLevel` returns to the main menu, `LoadScene CurrentScene` restarts, a
`CloseWindow` button / Esc on the root page resumes; the sound sliders change the mixer bus volumes live (and the config).

Draw list filtering: nodes under `herosetup*` (except `Hero/Ground`, the player model), `herocutscenes*` and `ColiPlanes` are left
out - they are cut-scene setups (a second posed Agent 47 lying in a blood pool, camera targets) and collision proxies of the hero that
the native game never draws.

## Verification done (this machine: RTX 2000 Ada, Vulkan, Windows 11)

* `bm screenshot` of all 14 levels (`M00..M13` except M07 which does not exist, `hideout`): load, simulate, render, no panics.
  M01 and the hideout were inspected visually (HUD, compass, health/notoriety/ammo, objectives, third-person camera, NPCs moving).
* Windowed `bm run` with synthetic keys: main menu -> Start Game -> level -> Esc pause menu -> Options -> Esc / Esc -> resumed;
  window frames written through `--shot`. No errors in the log; `--mods-dir examples/mods` loads `hello_mod` + `hooks_demo`
  (the mod's message shows as a subtitle, the data patch is applied to the weapon table).
* Frame timing M01 window, 1080p, vsync off by the driver: ~330 fps; CPU per frame: sim 0.5 ms, sync 0.7 ms, UI 0.07 ms, record
  0.15 ms; GPU ~1.3 ms. Level load: 0.5 s simulation (was 1.8 s), 0.5-0.7 s GPU upload (730 textures), 0.45 s UI resources.
* `cargo test --workspace --release` green (incl. real-file tests), `bm-app` has config unit tests and an end-to-end `bm screenshot` smoke test.

## Changes made in other crates during integration

* `bm-render`: `SceneGpu::set_skinning(item, &[Mat4])` (the game hands out ready skinning matrices, not a `Pose`) and `SceneGpu::item_world`.
* `bm-ui`: `InputState::axis(PadAxis)` (raw stick value for camera look).
* `bm-game`: `World::path_budget` (max path searches per tick, default 4); `load_level` settles the world with budget 0. The cold nav grid made
  the 30 settle ticks cost 1.3 s of the 1.8 s level load; NPCs still fall to the floor, they just start pathing when play starts.
  Added `debug` timing logs to `load_level`.

## Saves, rooms and screenshots (agent #14)

* **Saves**: `F6` quick save / `F9` quick load (slot `quick`, `<user dir>/saves/quick.json`), the pause menu entries "Save Game" / "Load Game" (the original pages
  `SaveGameCheckMenu`, `SelectSaveGameMenu`, `LoadGameCheckMenu`, `SelectLoadGameMenu` ... are native memory-card flows: `MenuEvent::PageOpened` of those windows is
  turned into `Cmd::SaveGame` / `Cmd::LoadGame`, the page is popped and the game resumes), `bm run --load SLOT`, `bm saves`. Loading a save of another level restarts
  that level first (`pending_load`, applied when the level is ready). Format and behaviour: `docs/game.md`, *Save games*.
* The F3 debug line and `bm screenshot --debug` show the current room and security zone.
* `bm screenshot --exec "tp x y z;door open HO_DoorDA_01;room"` runs console commands before the simulation (console: `docs/game.md`).
  Verified with it: closed / open double doors of the hacienda, the DrugLab staircase (`M01_DrugLab_Staircase_B101`, zone `eZone3B_PATCH`), the M06 hotel lobby
  (`M06_Hotel_Lobby_101`, zone `eZone1_PATCH`) and the hideout (`Hitman_HideOut_large`); the HUD lists the objectives decoded from the level data.

## Known gaps / next steps

* Characters play the decoded `.ANM` clips and wear exactly one outfit; open: facial poses (mouths stay slightly open), upper-body layering,
  blending between states, weapons attached to the hands (`docs/anim.md`).
* Window and offscreen paths share the same renderer; there is no MSAA, no resolution scaling option and no in-game options page
  beyond the game's own XML pages (graphics/controls pages are displayed but only the sound sliders are wired).
* Level loading blocks the event loop for ~1.5-3 s; the loading screen is a caption plus a three-step progress bar (level data -> graphics -> saved game), not a live progress.
* Script-driven UI screens (`ui.show_screen`) are drawn but cannot be clicked yet.
* Mouse look and gamepad input were implemented but only keyboard (synthetic) and rendering paths were exercised in automated runs.
