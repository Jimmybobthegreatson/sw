# Blood Money in Rust — Plan

Goal: clean-room reimplementation of the Glacier 1 runtime used by Hitman: Blood Money, in Rust,
that reads the user's OWN installed game files (env `BM_GAME_DIR`, default
`C:\Users\Shadow\Downloads\Hitman Blood Money`) and is built for first-class modding
(loose-file overrides, hot reload, scripting API, documented formats).

Rules
- Never copy game assets or decompiled code into this repo. Only our own code + notes on formats.
- No unsafe unless justified. `cargo check --workspace` must pass at end of every agent run.
- Work only in your own crate(s); changes to shared APIs go via the owner crate and are noted below.
- Each agent appends findings to `docs/<area>.md` and updates the status table here.
- Format knowledge: reverse-engineer from the files themselves (hex dumps, tests) and public
  community knowledge (e.g. web search for Glacier/Hitman BM format docs). Prove with round-trip tests.

Observed layout: `Scenes/<level>/<name>.ZIP` (+ `.WAV`/`.WHD` sound banks), `pc_eng.str` (1.8GB stream),
`HitmanBloodMoney.ini`. Inside scene ZIPs: .gms .prm .tex .mat .anm .oct .sgp .sgd .zgf .snd .loc .prp
.rmc .rmi .buf .sup .fx .xml .dat

Crates (dependency order)
1. bm-vfs       — mounts game dir + mod dirs; ZIP scene archives, loose-file override, .WAV/.WHD banks, .str stream
2. bm-formats   — low-level parsers: .gms .prp .mat .tex .loc .anm .oct .zgf .sgp .sgd .snd .buf etc.
3. bm-models    — .prm primitives -> mesh, skeletons, animation (.anm), material binding
4. bm-render    — wgpu renderer: scene graph, materials/.fx equivalents, textures, lighting, skinning
5. bm-particles — .prp/particle emitters, sim + render hooks
6. bm-sound     — .WHD/.WAV banks, .snd, 3D positional audio (rodio/cpal/kira)
7. bm-music     — dynamic music from .str / cue system, layering/crossfade
8. bm-ui        — HUD, menus, fonts, localisation (.loc), input bindings
9. bm-script    — modding API: mod manifest, hot reload, Lua (mlua) or WASM, event hooks
10. bm-game     — entity/component world, AI, physics/collision, mission logic
11. bm-app      — winit window, main loop, CLI (`bm dump`, `bm view`, `bm run`)

Entry points for later agents (stable API of the first two crates)
- `bm_vfs::Vfs::from_env()` / `add_mod_dir()` / `open()` / `list(glob)`; `SoundBank`, `StrIndex` (sound agent).
- `bm_formats::loader::SceneFiles::load(&vfs, "Scenes/M01/M01_main")` parses a whole scene; `.graph()` joins
  GMS+BUF+PRP into `scene::SceneGraph` (names, types, hierarchy, position/rotation, `stdobj_prim_id()`).
- Models: `prm::Prm::{model, model_objects, prim_vertices, prim_indices}`; materials: `mat::Mat::instance(id)
  .textures()/.props()`; textures: `tex::Tex::image(id).decode_rgba(level)`; text: `loc::Loc::get(path)`.
- Models (bm-models): `bm_models::Scene::load(&vfs, "Scenes/M01/M01_main")` -> `scene.draw_list(&[NodeKind::Static, ..])`
  gives `DrawItem { node, model: Arc<Model>, world: Mat4 }`; `Model.meshes[i]` has SoA `VertexData` (positions, normals,
  tangents, uvs, RGBA colors, joints/weights), `indices` (triangle list, native clockwise winding) and
  `submeshes[].material`; `scene.materials.desc(id)` -> plain-data `MaterialDesc`, `.image(tex_id)` -> lazily decoded RGBA8.
  Native space is left-handed Y-up, cm. Skeleton/skinning: `Model.skeleton`, `Skeleton::skinning_matrices(&Pose)`.
  Exporters/importer/PRM writer: `gltf_export`, `obj`, `import` (`Model::from_glb`), `prm_write::write_model`.
- Rendering (bm-render): `Renderer::with_vfs(device, queue, fmt, Some(vfs))`, `SceneGpu::upload(&mut r, &scene, &items, &UploadOptions::default())`, `r.render(&mut encoder, &RenderTarget{..}, Some(&gpu), &Camera, &FrameParams)`, `OffscreenTarget::render_png`, `RenderHooks` + `Shaders/WGSL/main.wgsl` loose override with `poll_hot_reload()`; `gpu.set_world/set_visible/set_pose`. Example: `--example render_scene`.
- Sound (bm-sound): `SoundLibrary::open_level(Arc<Vfs>, "Scenes/M01/M01_main")` -> `.find/.info/.pcm(id)/.lip_sync(id)/.snd(bank)`; `SoundScene::from_scene(&Scene, &SndDb)`; `MixerEngine::headless(48000)` implements `AudioEngine` (`play/play_stream/set_listener/set_bus_volume/set_reverb`), `SoundPlayer` for id/name/`#sound` playback; music hook = `play_stream(.., VoiceParams::on_bus(Bus::Music))`; mod override paths `Sounds/<bank stem>/<name>.{ogg,flac,wav}`, `Sounds/_global/...`. Examples `play_sound`, `render_mix`.
- Music (bm-music): `bm_music::load_level_def(vfs, lib, "Scenes/M01/M01_main")?.system(MusicOptions::for_engine(&engine))?` -> `MusicSystem` (`attach(&mut engine)` plays on `Bus::Music`; `set_state/set_zone/mission_event(MissionEvent)/set_intensity/trigger_stinger/duck/pause/resume/poll_events`, `render(frames)` offline); data model `MusicDef` (RON/JSON, `docs/music.md`); mod paths `Music/<level>/<cue>[.<stem>].{ogg,flac,wav}`, `Music/<level>/*.ron|json`; examples `music_list`, `music_render` (headless WAV + click check).
- UI (bm-ui): `let mut ui = bm_ui::UiSystem::new(vfs_arc); ui.load_scene_ui("Scenes/HitmanBloodMoney")?;` (game fonts, LOC strings; `prefetch_sprites(&[..])`), `ui.load_menu("frontend")` -> `Arc<MenuDoc>`; `MenuNav::new(doc, "MainMenu")` with `.handle(&Nav::Down/Select/Back/PointerClick(p), &ui.ctx) -> MenuEvent`, `.draw(&ui.ctx, &mut DrawList)`; HUD: `Hud::draw(&ui.hud_layout, &HudState, &ui.ctx, &mut dl, [w,h])` (game sets variables `health`, `ammo_clip`, `notoriety`, ... + `say()` subtitles, `prompts`); screens `ScreenStack`; input `InputMap::load_or_default(user_config_path())` + `InputState::event/pressed/down/move_vector`; GPU: `UiRenderer::for_renderer(&renderer, fmt, Some(vfs))`, `ur.render(&mut encoder, &view, (w,h), &dl, &ui.ctx.textures)` after `Renderer::render`, `ur.poll_hot_reload()`, `ui.poll_reload()`; screenshots `UiOffscreen::render_rgba`. Mod paths `menu/<n>.xml|.add.xml`, `Fonts/*.fnt`, `Localisation/<lang>.json|ron`, `UI/hud.ron`, `UI/Screens/*.ron`, `UI/Images/*.png`, `Config/input.ron`, `Shaders/WGSL/ui.wgsl`.
- Format knowledge + open questions: `docs/formats.md`. Real-file tests skip when the game dir is absent.
- Gameplay data (bm-formats, agent #14): `roommap::RoomMap` (`.RMI` / `.RMC` room boxes, `records_at`, `smallest_at`), `sgp::Sgp::portals()` (room portal graph), `oct::Oct` (blocks of packed trees), `gameplay::{doors, actors, path_followers, volumes, exit_areas, pickups, climb_boxes, zone_rooms, mission_objectives, PathIndex}`. bm-game: `rooms::RoomIndex` (`room_at`, `neighbors`, `route`), `World::{areas, rooms}`, `World::set_door / toggle_door / npc_open_door_ahead`, `objectives::mission_from_scene`, `save::SaveGame` + `Game::{save_state, load_state, save_slot, load_slot}`.

Agent #11 (done): `.ANM` animation decoded (`docs/anim.md`: `AnmBank`, `anm_decode`, `rig::RigSolver`), per-actor outfit selection
(`ZHM3Actor` property 6, `Scene::actor_outfit`), the `.MAT` reference bias fix (PRM material reference = instance id + 1), `U8V8` normal
maps decoded unsigned, `SceneClipProvider` in bm-game, camera clipping fix, `bm screenshot --focus`.
Next steps: (1) light/sun/fog + shadows, (2) facial poses / clip blending / weapons in hands, (3) terrain/hideout render fixes,
(4) .OCT/.RMC/.RMI collision/AI, missions, saves.

Status (agents update)
| Agent | Crate(s) | Status |
|---|---|---|
| vfs+formats recon | bm-vfs, bm-formats | **done** (see docs/vfs.md, docs/formats.md; open items listed at the end of formats.md) |
| models | bm-models | **done** (see docs/models.md, docs/anim.md; `.ANM` clips + humanoid rig solver, outfits, material bias fix; open: facial poses, ZScatterObj cell tables, material->effect mapping) |
| rendering | bm-render | **done** (see docs/render.md; per-actor outfits done; open: shadow maps, env/reflection, water, scatter, light params) |
| particles | bm-particles | **done** (see docs/particles.md; ZParticleTemplate/Emitter decoded, CPU sim + GPU hook, `--example render_particles`; open: unknown template props, flipbooks, collision, bLighting) |
| sounds | bm-sound | **done** (see docs/sound.md; `SoundLibrary` banks+cache+mod overrides, `.SND` object graph (waves/sounds/room-effect EAX blocks), LIP lip-sync, `SoundScene` ZSNDOBJ/controllers, `Mixer`/`AudioEngine` 3D+buses+reverb, headless WAV render, cpal behind `--features device`; open: seq/set/def bodies, surface props, room maps) |
| music | bm-music | **done** (see docs/music.md; 64-slot per-level track table + `MusicLocationDef` zones decoded, native `ZMusicController` logic documented as not-data; data-driven `MusicSystem` (RON/JSON cues, stems, intensity layers, states/transitions with beat/loop sync, curves, loop points + crossfade, playlists, stingers, ducking, pause) streaming from `pc_eng.str` with read-ahead; generated per-level defaults + mission hooks; loose `Music/<level>/<cue>.ogg` + custom `*.ron`; examples `music_list`, `music_render`, `music_play`; open: native event->slot mapping, `#sound` mode/aux) |
| ui | bm-ui | **done** (see docs/ui.md; tolerant menu-XML loader -> typed `MenuDoc`/`Window`/`Item`/`Action` with Template/Platform/MainIniOption resolution, page-stack `MenuNav` + layout/focus/sliders/toggles/transitions, game `ZFONT` atlases decoded + BMFont mod fonts, LOC strings with markup (`{locale}`, `{if}`, `<font>`, wrapping/justify), `ZWINPIC` multi-tile sprites decoded, `DrawList` batcher + wgpu `UiRenderer` (UI pass after the 3D pass, `Shaders/WGSL/ui.wgsl` override + hot reload), data-driven HUD (RON/JSON), RON/JSON mod screens, rebindable `InputMap` + user config, `UiSystem` VFS loading + `poll_reload`; examples `render_ui`, `menu_dump`; open: font page descriptor chain, ButtonGraphic state->node binding, original bindings/HUD placement) |
| scripting/modding | bm-script | **done** (see docs/modding.md; `mod.toml` folders/zips, discovery + `mods.toml` enable list, dep/cycle/conflict resolution + load order, asset-conflict report + `ModManager::mount` into `Vfs`; sandboxed Luau (mlua vendored, builds with MSVC only) one state per mod with time/memory limits, permissions, hot reload preserving `mod.state`/cvars; `ScriptHost` trait + `MockHost`, events with priorities/cancellation, timers, console cmds/cvars, per-mod JSON saves; RFC7396 data patches + schema; LuaLS stubs `examples/types/bm_api.d.lua`; examples `examples/mods/{hello_mod,hooks_demo}`; `--example mods`) |
| game logic | bm-game | **done**, extended by agent #14 with decoded room / door / route / trigger / objective data and saves (see docs/game.md; vertical slice: `Game::load` builds a deterministic fixed-timestep world from the scene graph (M01 Hacienda: 401 k-triangle BVH collision, 71 NPCs with decoded patrol routes, 47 decoded doors, pickups, lights, emitters, hierarchy; hideout too), kinematic capsule controller (stairs, slopes, gravity), lazy-grid A* nav, third-person player/camera/weapons/hitscan/prompts, vision-cone+LOS AI with replaceable `AiBrain`, RON weapons + missions (M01 example), `GameHost: ScriptHost` (snapshot reads, queued writes, cancelable `on_damage/on_pickup/on_action/on_entity_spawn`), audio/music/particles directors, `RenderState`/`UiState`; headless tests incl. real M01/hideout; `SceneClipProvider` plays the decoded `.ANM` clips (procedural provider as fallback); open: `.OCT` leaf ids / `.SGP` routing tables, light params, original (native) mission/AI logic) |
| app integration | bm-app | **done**; agent #14 added quick save / load (F6 / F9, pause menu pages, `--load`, `bm saves`), `screenshot --exec`, a loading bar (see docs/app.md and README.md; `bm` binary: winit+wgpu window, game's own main/pause menus, level play with HUD/audio/music/particles/mods, hot reload polling, CLI `run view dump export mods sound music screenshot config`, headless screenshots + frame profiling, all 14 levels load and render; `bm screenshot --focus <name>` renders a character close-up; open: facial poses, lights/shadows, original missions/AI, saves, clickable script screens) |
- Game (bm-game): `let mut game = bm_game::Game::load(Arc::new(vfs), "Scenes/M01/M01_main", &LoadOptions::default())?;` (optional `game.enable_audio(Box::new(MixerEngine::headless(48000)), true)?`, `game.attach_scripts(ScriptRuntime::new(game.host(), cfg))`), per frame `game.frame(dt, &PlayerInput{..})` (fixed 1/60 s ticks inside; `game.tick(&input)` = one tick for headless), then `game.render_state() -> RenderState{camera, entities{node, prim_id, world, skinning, ..}, hidden_nodes, lights, particles}` and `game.ui_state() -> &UiState{vars(health, ammo_clip, notoriety..), prompts, messages, objectives}`; events via `game.take_events()`; dev console `game.run_command("tp x y z")`; swap AI/animation with `World::brain_factory` / `Game::provider`. See `docs/game.md`.
- Scripting (bm-script, API 1.0.0): `let mgr = ModManager::new(ModDirs::default_dirs())?; let set = mgr.resolve(); mgr.mount(&mut vfs, &set)?;` then `ScriptRuntime::new(Rc<dyn ScriptHost>, RuntimeConfig)`, `rt.data_mut().set_base("weapons", json)`, `rt.load_mods(&set)`, per frame `rt.update(dt, t)` + `rt.poll_hot_reload()`, game events via `rt.dispatch(&Event::damage(..)) -> DispatchOutcome{cancelled, payload}`, console `rt.run_console(line)`. bm-game implements `ScriptHost` (all methods defaulted, `&self`). `bm_script::cli::run_mods_command` backs `bm mods`. bm-script must not depend on bm-game. See `docs/modding.md`.


Agent #12/#13: lights/sun/ambient/fog/zones decoded (`bm-models::lighting`), cascaded shadows, reflections, FXAA, scatter, post pass.
Agent #13 (done): facial pose banks decoded and applied (`Skeleton::face/apply_face`, `bm_game::face`: rest `D_Closed`, blinking, LIP-sync
hook `Game::speak`), opaque window glass drawn blended by its reflection mask, self-lit glow cards, interior ambient tuning, M01 horizon
haze fix; `bm screenshot --face/--set/--focus-dist=-N`. Remaining: light maps, original expression triggers, M13/M03 spawn placement.

## Final status (agent #10)

All eleven crates are done and wired together: `cargo build --release` produces `target
eleasem.exe`; `bm run` shows the game's own main menu
and plays any level; `bm screenshot` is the CI smoke test. Cross-crate changes made during integration are listed in `docs/app.md`
(`SceneGpu::set_skinning`, `InputState::axis`, `World::path_budget`). Biggest remaining gaps, in order (after agent #11 closed the `.ANM` / outfit gap): facial poses and clip blending,
light/sun/fog data and shadows, terrain/scatter objects and the hideout surface issue, original mission/AI data (`.OCT/.RMC/.RMI`), save games.


Agent #14 (done): decoded the spatial / gameplay data the vertical slice approximated and wired it in (`docs/formats.md`, `docs/game.md`):
* `.RMI` / `.RMC` = room boxes (record id = room, ids in order = `ZROOM`s with volumes), `.SGP` = room portal graph, `.OCT` = list of packed bounding-volume
  trees (container decoded, leaf ids not), `ZGF/SUP/SGD` confirmed empty stubs; typed decoders for doors, `PathFollower`, `ScriptC`, trigger boxes, exits,
  pickups, climb boxes, security zones and the LOC mission objectives.
* bm-game: room tracking (-> reverb, music zones, HUD, `on_room_change`), real doors (swing / double / sliding, locks, NPCs open them), decoded patrol routes /
  weapons / help points, trigger volumes + exit areas, missions derived from the level data (all 14 levels), versioned save games, console `room/door/save/load`.
* bm-app: F6 / F9 quick save-load, pause-menu save/load pages, `--load`, `bm saves`, `screenshot --exec/--debug`, loading bar.
Still undecoded: native mission / AI scripts (`Scriptcs/*.dll`), `.OCT` leaf ids, `.SGP` / `.RMC` tree and routing tables, `MatPosAnim` door curves, `Usepoint`
behaviours. Next steps: use the portal graph for room-by-room NPC search / pathing, react to security zones (trespassing), lock picking, elevators, climb boxes.
