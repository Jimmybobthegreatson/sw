# bm-game - simulation core

Crate `bm-game` (depends on `bm-vfs`, `bm-formats`, `bm-models`, `bm-sound`, `bm-music`, `bm-particles` (without its GPU
feature) and `bm-script`). It is renderer independent: every frame it produces a `RenderState` and a `UiState` that
`bm-app` hands to `bm-render` / `bm-ui`, and `GameHost` implements `bm_script::ScriptHost`.

This is a *vertical slice*: a walkable, shootable level with patrolling NPCs, doors, pickups, a mission, sound, music
and particles, all headless-testable. The original game logic (native `ZHM3*` classes, the mission scripts compiled into `Scriptcs/*.dll`) is
not decoded; what *is* data (rooms, doors, routes, trigger volumes, exits, objectives, pickups) is read from the level files, and wherever this crate
invents behaviour it says so below.

## Quick start

```rust
use bm_game::*;
let vfs = std::sync::Arc::new(bm_vfs::Vfs::from_env()?);
let mut game = Game::load(vfs, "Scenes/M01/M01_main", &LoadOptions::default())?;   // or "Scenes/hideout/hideout_main"
game.enable_audio(Box::new(bm_sound::MixerEngine::headless(48000)), /*music*/ true)?; // optional

// optional scripting: the runtime is built on the game's host
let mut rt = bm_script::ScriptRuntime::new(game.host(), Default::default());
rt.load_mods(&resolved_mods);
game.attach_scripts(rt);

// every frame
game.frame(real_dt, &PlayerInput { move_y: 1.0, ..Default::default() });   // fixed 60 Hz ticks inside
let rs: RenderState = game.render_state();      // camera, posed characters + doors, lights, particles
let ui: &UiState   = game.ui_state();           // HUD vars, prompts, messages, objectives
```

Headless: `game.tick(&input)` runs exactly one 1/60 s tick (no audio/UI); `cargo run --release -p bm-game --example
headless -- Scenes/M01/M01_main 60` prints a simulation report. Tests: `cargo test -p bm-game` (synthetic levels, no
game files) and `tests/real_levels.rs` (M01 + hideout, skipped without `BM_GAME_DIR`).

## Architecture

| module | what |
|---|---|
| `world` | `World`: sparse struct-of-arrays stores (`Store<T>` indexed by the dense `EntityId`, iteration in id order = deterministic) for `Info`, `Transform`, `Hierarchy`, `RenderComp`, `Health`, `Character`, `AiComp`, `Body`, `AnimComp`, `PlayerComp`, `DoorComp`, `PickupComp`, `LightComp`, `SoundEmitter`, `ParticleEmit`; hitscan, damage/kill, door toggle, pickups, request/fx/outbox queues |
| `level` | `load_level` / `Game::load`: scene graph -> world (see *Level loading*) |
| `rooms` | `RoomIndex`: room containment from the decoded `.RMI`, room portals / adjacency from `.SGP` (see *Rooms and areas*) |
| `areas` | trigger volumes (exit areas, `CheckInside`, security zones), the player's room / zone tracker (`World::update_areas`) |
| `doors` | swing / sliding door leaves, door groups, locks, NPCs opening doors |
| `objectives` | `MissionDef` built from the level's LOC objectives + level controller script |
| `save` | versioned serde save games (`SaveGame`, `Game::save_state` / `load_state`, slots in the user dir) |
| `collision` | `CollisionWorld`: static triangle BVH (`StaticMesh`) + dynamic colliders (door leaves); rays, sphere contacts, ground probes, ray-vs-capsule; `Surface` classes |
| `controller` | kinematic capsule (3 spheres) with gravity, ground snapping, step-up, wall sliding (`step_body`) |
| `nav` | lazily built 60 cm grid over walkable collision, A* with string pulling, cached neighbour expansion (`NavCache`) |
| `player` | `PlayerInput`, third-person movement, camera (`World::camera`), weapons / inventory, interaction prompts |
| `ai` | perception in the world, decisions in a replaceable `AiBrain` (`SimpleBrain`) |
| `anim` | `AnimationProvider` trait: `SceneClipProvider` (decoded `.ANM` clips), `ProceduralProvider`, `BindPoseProvider`, `ClipProvider` |
| `mission` | `MissionState` evaluated every tick from a RON `MissionDef` |
| `host` | `GameHost: ScriptHost` (snapshot reads, queued writes) |
| `game` | `Game` facade: fixed-timestep loop, request resolution with script hooks, event log, host command application, `RenderState`, UI vars, dev console |
| `audio`, `fx` | `AudioDirector` (`bm-sound`/`bm-music`), `FxDirector` (`bm-particles`) consuming `FxEvent`s |
| `state` | `RenderState`, `RenderEntity`, `RenderLight`, `UiState` |
| `data` | `GameData` (weapons, pickups, NPC loadouts/health), `MissionDef`; embedded RON in `bm-game/data/`, overridable from the VFS |

**ECS choice.** No external ECS: ~15 component stores as `Vec<Option<T>>` keyed by the dense id. Ids are never reused
(`Arc<AtomicU64>` allocator shared with the script host, so scripts get the id of a spawn immediately). The whole
simulation is single threaded; the only floating-point sources are the tick order and one xorshift `Rng` (`world.rng`).

**Space.** Native engine space everywhere (left-handed, Y up, cm). Heading: `yaw = 0` faces `+Z`, positive yaw turns
towards `+X` (`math::forward`, `math::pose_matrix`); entity matrices are `T(pos) * Ry(yaw)` in the engine's proper-rotation
convention (`docs/models.md`), so they can be passed straight to `SceneGpu::set_world`. The only conversion at a boundary
is `bm_script::Vec3 <-> math::Vec3` (same space) and degrees/radians for script rotations. `bm-sound` positions are cm too.

**Fixed timestep / determinism.** `Game::frame(dt, input)` accumulates `dt` and runs `<= 5` ticks of `TICK_DT = 1/60`
(`Game::tick` runs one). Everything inside a tick is a function of (state, input, rng): the tests simulate M01 twice with
the same seed and compare every entity position (quantised to 1 mm) and the RNG state. Audio, particles, UI and render
state are *presentation* and do not feed back (the particle system has its own seed).

## Level loading (`level.rs`)

* **Collision**: every `NodeKind::Static` node (not under `CloneGroup`, `AllLevels/`, `Controllers!`, `mainsceneincludes`,
  or the `*_MAP.zip` in-game-map quads) contributes its LOD0 rigid meshes, skipping shadow/additive materials, alpha-tested
  foliage (name heuristic), `PosColor10` shadow volumes and the `_Glacier` helper materials except the collision proxies
  (`_Glacier/WorldColi/WC_<n>_<Kind>`, `_Glacier/Collision`). `InsideBound`, `ElevatorBound`, `ObstacleGuide`, `_MISSING` and
  `WaterCollision` are *not* solid. Result for M01: 3 479 nodes, 401 k triangles (door leaves of the decoded `SingleDoor` / `MultiDoor` controllers are dynamic colliders, not part of it; whole level load incl. models, BVH and entities: ~1.9 s cold / 0.5 s warm);
  hideout 514 nodes / 46 k triangles. Raycasts skip dynamic colliders (doors) whose box the ray misses; NPCs collide with doors. Surfaces come from material names (`Surface::from_material_name`).
  **Not used**: `.OCT` (decoded as a list of bounding-volume trees, but its leaf ids are not understood, `docs/formats.md`) and `.RMC` (room
  collision boxes: only the structure is decoded); collision stays the PRM triangle BVH. `.RMI` is used for rooms, `.SGP` for room portals.
  A sanity test checks that no body ends up with a mid-body sphere deeply embedded in geometry (this caught the in-game
  map quads, which are horizontal triangles at y = 0 inside the Hacienda).
* **Player**: the scene's `ZHitman3` node (M01: (-1108, -154, 2488)); `LoadOptions::player_start` overrides.
* **NPCs**: each `ZHM3Actor` node (named `Ground`) becomes an entity named after its parent group
  (`Characters!Guards!Outside!Guard_OutsidePatrol`). Role from the script module (`alllevels/guard`) or the name (`Guards!` -> guard,
  `Special_characters` -> target, else civilian). 71 in M01, 9 in the hideout. Actors authored below/inside the floor (sleepers) are lifted onto the first
  walkable surface above. Decoded parameters (`bm_formats::gameplay::actors`): the **weapon** the actor's `ScriptC` gives it (a reference to a weapon item
  definition such as `SMG_FamaeSAF_01`, mapped to our stand-ins by `GameData::weapon_for_item`), the **patrol route** and mode of its `PathFollower`
  (`LOOPING` = cyclic, `RANDOM` = random waypoint) and its **help list** (`Controllers!Helppoints!...`: where civilians run when alarmed).
* **Patrol routes**: the actor's `PathFollower` controller names its waypoint lists (`ZLIST` nodes whose `props[5]` is a container of node
  paths) and the mode. `route_from_lists` resolves them through `PathIndex`; actors without a `PathFollower` fall back to the old name based
  search (`find_route`). 64 of 71 M01 NPCs get a route (>= 2 points), 67 get decoded parameters (route, weapon or help list). What the
  original *does* with the points (waits, animations at "use points" = `Usepoint*` nodes with native `ScriptC` modules) is not decoded.
* **Doors**: every node with a `SingleDoor` / `MultiDoor` controller (26 + 21 in M01, 68 leaves; `bm_formats::gameplay::doors`). A single door's model
  is the leaf (hinge = node origin, the model spans x = -95..5 so the hinge is at the edge: confirmed by the bounds), a `MultiDoor` group's children
  `D1`, `D2` are the leaves (double doors swing from the outer edges, sliding doors move apart). Leaf triangles go to dynamic colliders. Decoded: the
  initial state (`IDS_LOCKED` doors start locked and carry a lock-pick difficulty `EASY`, `IDS_OPEN` start open), the open direction (away from the
  opener), sliding vs swinging (`DT_MATPOSANIM`). The open angle is not stored (we use 90 deg x 0.95) and sliding distances are derived from the leaf
  bounds. Leaves of one door open together (`DoorComp::group`). Verified visually (double doors in the hacienda, `bm screenshot --exec "door open ..."`).
  The hideout has none.
* **Pickups**: every node with an `Action` / `AT_PICKUP` controller (38 in M01 after excluding the HUD resources; weapons, ammo boxes, items). The item
  definition (`CC_Hammer_01`, `Item_Cello_01`) is mapped to our weapon stand-ins (`GameData::weapon_for_item`, `item_aliases` in `weapons.ron`); unmapped
  items are still pick-up-able (mission items, shown with a prettified name) and count for `Retrieve` objectives.
* **Lights** (295 in M01): position only (colour/range are not decoded, see `docs/render.md`); defaults 900 cm warm white.
* **Sound emitters** (`ZSNDOBJ`, 41), **markers** (`ZBoxPrimitive`, e.g. `Controllers!ExitAreas!PlaneExit`), **ambient
  particle emitters** (`SceneParticles::ambient`, 8) become entities.
* **Hierarchy**: each entity's parent is its nearest ancestor *that is an entity*; `set_world` propagates to children.
* The first second of simulation settles everybody on the floor, then queues/clock are reset.

## Physics / character controller

Capsule r = 30, h = 175 (crouch 120), stack of three spheres. Per tick: horizontal move in sub-steps with push-out from
non-walkable contacts (normal.y < 0.65 counts as wall; the lowest sphere starts above the 40 cm step height so curbs/stairs
are not walls), step-up retry (lift 40 cm, move, drop), then a five-ray ground probe (centre + ring) with snap-down 30 cm and
gravity 981 cm/s^2. Falls below `meta.kill_y` (level min - 4000) teleport back to `last_safe`. Raycasts see static geometry and
closed doors; NPCs ignore doors (they pass closed ones) and each other (no NPC-NPC avoidance). Not done: moving platforms
(elevators), climbing / ladders, jumping, ragdolls, crouch headroom checks, swimming.

## Player

`PlayerInput { move_x, move_y, look_dx, look_dy, run, crouch (toggle on edge), aim, fire, interact, reload, draw, next_weapon }`.
Camera relative walking (150 cm/s, run 380, crouch 85, aim 100), body turns towards motion or the camera when aiming.
Third-person camera: 260 cm behind the head pivot (150 + 45 cm over the shoulder when aiming), clipped by a ray against the
level. Weapons: drawn/holstered, current weapon, magazine + reserve, reload timer, semi/auto fire, melee cone check;
hitscan from the eye towards the crosshair target with spread; zones head/torso/legs from the hit height. Prompts: nearest
door (230 cm) / pickup (170 cm, in front) -> `UiState::prompts`, `interact` queues `Request::Pickup` / `Request::Action`.
Health: `Health { hp, max }`; death sets `AnimState::Dead`, fails the mission (`PlayerDead`).

Weapon data is RON (`bm-game/data/weapons.ron`, loose `Game/weapons.ron` in a mod dir replaces it, and
`GameData::weapons_json` / `apply_weapons_json` expose the table as the script `weapons` dataset). Damage, fire rate, magazine
and range are still our own tuning numbers; only the recoil below is taken from the original game.

### Recoil (`bm-game/src/recoil.rs`)

Verified against the original game with a live trace (`FUN_0064be10`, called every frame a firearm is drawn; the shotgun and
the Snub Nosed revolver were sampled, 2300+ frames each):

* A shot sets the accumulator to 0.1; it is halved every frame; below 2^-13 there is no kick.
* `Y = acc * recoil_y` (exact). `X = acc * recoil_x`, or `s * |acc * recoil_x|` with `s` random in [-1, 1) when the mode
  randomises X (the native `2 * (r - 0.5)`).
* Record fields (sub-object vtable getters): `0x304` random mode (1 = `eXRandom`), `0x308` recoil X at `+0x170`, `0x30c` recoil Y at
  `+0x174`. The Remington shotgun reads -3 / -5 and the revolver 2 / -2.

Our translation: the kick is applied to the view (camera and shot direction) in degrees, climb = -Y (the trace's Y is negative for
both weapons), and the accumulator halves per 60 Hz tick instead of per native frame.

Static checks in Ghidra (no game run): the record is a `ZHM3ItemTemplateWeapon`; its loaded values sit at indices `0x5b` (mode), `0x5c`
(recoil X), `0x5d` (recoil Y) of the source template and are copied to `+0x16c` / `+0x170` / `+0x174` (`FUN_0064b9b0`); the stored Y is
negated (`FUN_00649f20`), which is the sign rule above; the accumulator is halved by a constant 0.5 per call (`FUN_0064a320`, so per frame,
not per second); a shot sets it to `min(1, [0x3d] + [0x3e])` (`FUN_0064e3c0`), which gives 0.1 when the increment is 0.

Not verified yet: the loader that fills indices `0x5b`-`0x5d` from the item property list (so the item-float index 9 / 10 mapping for the
other weapons is still a two-sample check), the source of the shot increment (`[0x3d]`, copied by `FUN_0064ba49`; the rapid-fire result
depends on it), mode 2 / 3 Y randomisation (we use the same symmetric form as X), and the random source (`DAT_00820820` vcall `0xfc`).

## NPC AI (and what is not decoded)

* **Perception** (10 Hz per NPC, only inside `ai_radius` = 90 m of the player unless alerted): vision range 28 m (x1.3 if the
  player holds a visible weapon, x0.5 crouching), 110 degree cone (anything closer than 2 m is "seen"), two line-of-sight rays
  (eye -> head / chest). Seen -> suspicion rises (armed player 2.2/s, guards: close 0.35/s, running 0.18/s, else 0.06/s;
  civilians only react to weapons); unseen -> decays 0.1/s. Weapon noise within its radius sets suspicion to 1 (loud) /
  0.4. A visible kill within 18 m with LOS sets 1.1 for the witness.
* **Brain** (`trait AiBrain: Send`, `fn think(&mut self, &AiView, dt) -> AiCommand`): `SimpleBrain` = Idle (patrol route with
  1.5-4.5 s waits / stand at home) -> Suspicious (stop, face the last sighting) -> Searching (walk to the last known
  position, look around 14 s) -> Idle; suspicion >= 1: guards -> Alerted (close in to 45 % of weapon range, fire when a clear
  shot exists), civilians -> Fleeing (run 18 m away from the player). Replace per role through `World::brain_factory`.
* **Movement**: `nav::find_path` (A* over probed cells, <= 6 000 expansions, cached in `NavCache`) at most 4 new paths per tick,
  followed by the same capsule controller. 17 k cells are cached after one minute of M01 patrols. Paths ignore doors; NPCs collide with doors
  and `World::npc_open_door_ahead` opens the closed, unlocked door they are about to walk into (every 4th tick per NPC, within 70 cm of the leaf and
  roughly ahead); doors an NPC opened close by themselves after 6 s without anybody within 2.2 m. Locked doors stay shut (stuck detection then skips to the
  next waypoint). In a 100 s M01 run NPCs open 16 door leaves (`m01_npcs_open_doors_on_their_patrol`).
* **Routes**: `PathFollower` mode `RANDOM` picks the next waypoint at random, `LOOPING` goes round. **Fleeing** civilians run to the nearest help point
  that is not closer to the player than they are (decoded `Helplist`), else 18 m away from the player as before.
* **Gaps**: no NPC-NPC avoidance, no use points / scripted scenes, no disguises or trespassing consequences (the security zone of the player's room is
  known, `World::player_zone_name`, but nothing reacts to it), no bodies dragging, no unconscious state, no AI hearing of footsteps; perception ignores
  glass. The original AI tables (`ZHM3HmAs`, the routing tables of `.SGP`) and the native script logic are not decoded.

## Animation

`.ANM` key data is decoded (`docs/anim.md`: clip table, bit packed streams, rig solver). `Game::load` installs a
`SceneClipProvider` (clips of the level's own `.ANM` by name, solved onto each character's skeleton once and cached; the
procedural provider is its fallback):

* `SceneClipProvider`: idle / walk / run / crouch / crouch-walk / hit / death clips for the player (`Hero` set), male and female
  NPCs (`Male_Reg` / `Female_Reg`, female recognised by the 82.8 cm legs); `AnimRequest::hero` selects the set. Locomotion is
  driven by the gait phase scaled by the clip's own root motion (feet do not slide), one-shot clips hold their last pose.
  Verified numerically on the player and four NPC skeletons (`decoded_clips_drive_the_real_skeletons`) and visually with
  `bm screenshot --level M06 --walk 74 --ticks 74 --cam=...` (a real heel-toe walk cycle).
* `ProceduralProvider` (fallback): idle sway, walk, run, crouch, crouch-walk, aim, fire kick, hit flinch, fall-back death. It
  rotates `THIGH/CALF/UPPER_ARM/FOREARM/SPINE_1` and moves the pelvis. The bone frames of the real skeletons are not axis
  aligned, so the swing axis and sign of each joint are **calibrated by test-rotating the joint** (cached per skeleton).
* `BindPoseProvider`: T-pose. `ClipProvider`: hand built `bm_models::Animation`s per `AnimState`, fallback provider.
* `RenderEntity::skinning` is `Skeleton::skinning_matrices(&pose)` (one matrix per bone) for `SceneGpu::set_skinning`;
  `RenderEntity::outfit` is the resolved outfit value of the actor (`ZHM3Actor` property 6, `docs/anim.md` section 6).
* Camera: the third-person camera keeps at least `CAMERA_MIN_DIST` (45 cm) from the head pivot when geometry is right behind the
  player and hides the player model when closer than `CAMERA_HIDE_DIST` (85 cm) instead of clipping through it (M06 spawn).

## Scripting integration

`GameHost` works on a **snapshot**: `refresh(&World)` copies entity infos (+ health/ai/inventory/door/light components) and a
clone of the `CollisionWorld` (door leaves included) before scripts run (every tick when a runtime is attached). Reads
(`entity_info`, `find_entity`, `query_entities`, `get_component`, `raycast` incl. character capsules) are answered from it.
Writes become `HostCommand`s applied right after the script call by `Game::apply_host_commands` (spawn, despawn, move, rotate,
set component, damage, kill, sounds, music, particles, screens, HUD vars, messages, console). Script spawn ids are allocated
immediately from the world's allocator and visible in the snapshot at once. Spawn templates: `npc_guard`, `npc_civilian`,
`npc_target`, `pickup[:weapon]`, `marker`, `prop`, `light`. Components readable/writable: `health`, `ai` (read), `inventory`
(read), `door`, `light`, `transform` (read).

Events (all appear in `Game::events()` / `take_events()` as `EventRecord { time, tick, event, cancelled }`, with or without a
runtime):

| event | when | cancelable / modifiable |
|---|---|---|
| `on_level_load`, `on_entity_spawn` (player, NPCs, pickups, doors) | first tick (replayed on `attach_scripts` if later) | no |
| `on_entity_spawn` (script spawns) | before the entity is created | cancel -> not created |
| `on_damage` | every hit, before it is applied | cancel = no damage; `amount` edits apply |
| `on_pickup`, `on_action` (door open/close) | before the action | cancel = nothing happens |
| `on_kill` | after death (`victim`, `killer`, `weapon`) | no |
| `on_mission_event` (`mission_start`, `objective_complete`, `mission_complete`, `mission_failed`) | mission logic | no |
| `on_update` | once per tick through `ScriptRuntime::update` | - |
| `on_room_change` `{from, to, zone}` | the player's room changed (10 Hz check; room names like `M01_Hacienda_Room_101`) | no (custom event: `events.on("on_room_change", fn)`) |
| `on_trigger` `{name, kind, entered}` | the player entered / left a trigger volume (`exit`, `check_inside`, `security`, `action`) | no (custom event) |
| `on_game_loaded` `{level, label}` | a save was applied | no (custom event) |

Cancelable events are dispatched while the world is only borrowed immutably; handlers that call the host see the snapshot
of that moment. Do not call `ScriptRuntime::dispatch` from inside a host method (rule of `bm-script`) - the game never does.

## Audio / music / particles

`AudioDirector::open` (via `Game::enable_audio`) opens the level bank, the `.SND` emitters, optionally music:

* listener = camera; footsteps by surface (`Character/Footsteps/<Stone|Carpet|Asphalt>/...`, random pick), weapon shot / reload
  (`WeaponDef::fire_sound`), bullet impacts (`Impacts_Bullet/<Stone|Metal|Snow|Glass|Wood>/...`), pain grunts on death, script
  `audio.play`. `ZSNDOBJ` emitters within 25 m of the camera loop through `SoundPlayer::play_object` (at most 6; every
  emitter is treated as an ambient loop, the real trigger behaviours are not modelled).
* music: `MissionEvent`s derived from the NPC alert state near the player (Calm / Suspicious / Alert / Combat), plus
  MissionStart, TargetKilled, MissionComplete / Failed, PlayerDead.
* `FxDirector`: scene particle templates (`Gun_Flash`, `Gun_Smoke`, `Metal_Sparks`, `Wood_Debris`, `Stone_Debris`,
  `Blood_Smoke`, ...) at shots (muzzle), impacts (oriented along the surface normal) and hits; the level's ambient emitters
  run from the start. `RenderState::particles` is the depth sorted `bm_particles::RenderList` for the camera.

## Rooms and areas

`rooms::RoomIndex` (built in `load_level` from `<scene>.RMI` + `.SGP`, `World::rooms`): the room boxes of the `.RMI` are assigned to `ZROOM` nodes
(record ids in ascending order = rooms with a positive volume count, checked against the `InsideBound` boxes, `docs/formats.md`), the player's room is the
smallest box that contains the player (10 Hz, `World::update_areas`). M01: 48 rooms, 106 of 107 boxes owned; 12 of 14 levels have an exact
id <-> room match, M02 / M11 fall back to the matched boxes. The `.SGP` planes give **portals** between rooms (`RoomIndex::portals`, `neighbors`,
`route`: breadth first over portals; the Hacienda kitchen reaches the yard, the upper floor too).

What consumes the current room:

* **Reverb** (`AudioDirector::follow_room`): the `.SND` room effect named like the room (`roomeffects/<level>/<group>/<room>`) becomes the mixer reverb
  (`ReverbParams::from_eax`), none outside rooms.
* **Music** (`bm-music` `MusicLocationDef` zones list room names): entering a room that belongs to a zone calls `MusicSystem::set_zone(id)`
  (remembered while a zone-locked alert / combat state plays). Test `m01_room_drives_reverb_and_music_zone`.
* **UI / scripts**: `UiState::{room, location, zone}`, HUD var `room`, the `on_room_change` script event `{from, to, zone}`, console command `room`,
  F3 debug line of `bm run` / `bm screenshot --debug`.
* **Security zones**: `ZoneControl` boxes list the room groups of each zone (`eZone1_PATCH` ... `eZone3B_PATCH`); `Areas::room_zone` maps rooms to zones
  (longest matching path), `World::player_zone_name()`. Nothing reacts to the zone yet.

**Trigger volumes** (`areas::TriggerVolume`, oriented boxes from the `ZBoxPrimitive` size, centred on the node): exit areas (`Action` boxes with the
`escape` label / listed in `...ExitAreas!00_ExitList`, boxes named `Exits!ExitBox`), `CheckInside` boxes and `SecurityZone` boxes. The player entering /
leaving raises `on_trigger {name, kind, entered}`. M01 has 36 volumes, 1 exit (the plane at the airstrip); M06 / M11 / M09 / M02 / M05 / M03 have exits,
M00 / M08 / M10 / M12 / M13 do not (their exits are script driven).

## Mission system

`MissionDef` (RON) objectives: `Kill`, `KillExact`, `Protect`, `ReachNode`, `ReachPoint`, `ReachExit { index }`, `Pickup`, with `requires` chains and `optional`;
fail conditions `PlayerDead`, `EntityKilled`. Order of precedence when a level loads: a mod's `Game/Missions/<dir>.ron`, then **objectives derived from the level
data** (`objectives::mission_from_scene`), then the embedded RON (`data/missions/{M01,hideout}.ron`, only used for the hideout now).

Derived objectives (all 14 levels load a mission; test `objectives_exist_for_every_level`):

* texts and kinds from the LOC (`<Level>/MissionObjectives/<n>/...`: `T` target, `E` escape, `R` retrieve, `P` protect, `O` optional); the level title
  from `AllLevels/Interface/LevelNames/<level>` ("A VINTAGE YEAR");
* targets: the level controller's `ScriptC` (`m01/m01_levelcontrol`) lists the supervised actors in objective order; `T` and `P` objectives take the next
  one (M01: Don Fernando, then Manuel; the movie star's agent is no target; M06: three marks + the politician to protect). Targets we cannot map stay optional;
* escape: the first exit area (`ReachExit`), required after all targets; levels whose exit is script driven (M00, M08, M10, M12, M13) complete when the
  targets are dead; `R` / `O` objectives are matched to pickups by words of `WhoOrWhat` ("Video tape" -> `Item_VideoTape`), else stay optional and open.

This reproduces "kill the targets, leave through the exit" with the real texts, targets and exit; the original scoring, notoriety, bonus rules, disguises,
cut scenes and scripted events are native code and are **not** modelled. Test `m01_mission_comes_from_the_level_data` plays it (kill both targets, walk
into the exit box -> `MissionStatus::Complete`).

## Save games

`save::SaveGame` (serde JSON, `SAVE_VERSION = 1`; newer files are refused, missing fields default): level scene base, label, time / tick / RNG state, the
player (position, health, camera, drawn weapon, inventory), every NPC with a scene node (position, yaw, health, alive, alert state, suspicion, route index,
ammo), every door leaf (open / angle / locked), every pickup (taken), the mission (status, objective flags, picked items, elapsed) and the room.
`Game::save_state(label)` / `Game::load_state(&save)` (applied on top of a freshly loaded level of the same scene, entities matched by scene node; script-spawned
entities are not saved; transient AI memory - current path, wait timers, last sighting - is re-planned), `save_slot(name, label)` / `load_slot(name)` write to
`<user dir>/saves/<slot>.json` (`$BM_USER_DIR` or `%APPDATA%/bm-rs`; written through a temp file), `list_slots()`, console `save [slot]` / `load [slot]`.
`bm run`: **F6** quick save, **F9** quick load (restarts the level first when the save belongs to another one, with a loading bar), the game's own
pause menu entries "Save Game" / "Load Game" (native memory-card pages `SaveGameCheckMenu`, `SelectLoadGameMenu` ... ) are mapped to quick save / load,
`bm run --load SLOT` starts from a save, `bm saves` lists the slots. Tests: `m01_save_and_load_restores_state`, `saves_tolerate_missing_fields`.

## RenderState / UiState

`RenderState { time, tick, camera, entities, hidden_nodes, lights, particles }`.

* `camera`: `Camera { pos, forward, up, fov_deg (67.4 / 50 aiming), near, far }` in native space (left-handed).
* `entities`: only *dynamic* objects (player, NPCs, doors): `node` + `prim_id` (the scene node whose model moves; use
  `SceneGpu::set_world(node, world)`), `world`, `visible`, `skinning` (per-bone matrices), `anim`, `alert`, `hit_flash`, `alive`.
  Static geometry is drawn by the renderer from `Scene::draw_list` as before.
* `hidden_nodes`: scene nodes (with descendants) to hide, e.g. picked-up items.
* `lights`: up to 32 active lights nearest to the camera (position, default colour/range).

`UiState.vars` uses the HUD variable names of `bm-ui`: `health` (0-100), `ammo_clip`, `ammo_reserve`, `weapon`, `notoriety`
(0-99 from the alert level), `alert` (0-3), `time`; plus `prompts`, `messages` (timed), `objectives`, `mission_status`,
`mission_title`, `screens` (script `ui.show_screen`), `crosshair`.

## Dev console

`Game::run_command`: `pos`, `tp x y z`, `give <weapon>`, `heal`, `kill <name-substring>`, `room` (room, location, zone and the rooms it leads to),
`door open|close|lock|unlock <name-substring>`, `save [slot]`, `load [slot]`. Script `console.exec` is forwarded to it.

## Measured on the user's files (release, this machine)

M01: load 0.5-1.9 s (401 k collision triangles, 71 NPCs, 47 doors / 68 leaves, 48 rooms, 36 trigger volumes, 295 lights); 60 s of simulation with all NPCs patrolling in
~8 s (about 2.4 ms per tick including path searches; the first seconds are the most expensive because the nav cache is cold);
the hideout loads in 0.25 s. Test results: player never below the floor, 66 of 71 NPCs grounded, 46 of 71 moved more than 1 m in
60 s, no body embedded in walls, two runs with the same seed are bit-identical, and with game dir absent the real-file tests skip.

## Open questions / next steps

* Facial animation, clip blending / layering, weapon attachment to the hand bones (`docs/anim.md` open items).
* `.OCT` leaf ids and the `.SGP` routing tables (`packed`, `records`): would give real collision cells / room-by-room path planning; climb boxes
  (`ObstacleGuide`, `eClimbBox`, decoded as `gameplay::climb_boxes`) are not used yet (no climbing / vaulting).
* Light colour / range, sun and fog values (`docs/render.md`).
* Original mission logic, scoring and notoriety; real weapon stats; disguises; NPC conversation / use-point behaviour.
* Doors: the open angle and the sliding distance are not stored in the scene (assumed); lock picking, keys and door sounds (`.SND` offsets are decoded) are not
  wired up; `MatPosAnim` curves are not decoded.
* NPC-NPC collision; elevators (`ElevatorHouse` / `ElevatorPanel` controllers are not wired); ladders and `ObstacleGuide` climbing; crouch headroom.
* Facial animation (`ANM` pose bank), lip sync from `bm-sound`, disguise switching (outfit values are known).

## Notoriety and disguises (`bm-game/src/stealth.rs`)

From the game's own data (`notoriety.xml`, `difficultysettings.xml` in every mission archive, read at level load):
notoriety is a 0..100 point total (HUD bar: green <= 50, yellow <= 75, red above). Every person who sees a crime of the player
(kill, or damage dealt) counts once per incident: 1st witness +10, 2nd +5, 3rd +3, 4th +2, every further one +2, capped at
`MaxWitnesses*` per difficulty (15 / 20 / 25). `World::caught_on_tape` adds 20 / 10 (undisguised / disguised),
`World::spend_notoriety` applies the bribe / new identity deltas (-15 / -40 / -100). Console: `notoriety [n]`, `difficulty [0-3]`,
`disguise <body fragment>|off`. Scripts: component `stealth` on the player (`get_component` / `set_component`
`{notoriety, difficulty, disguised=false}`), events `on_notoriety {value, delta, reason}`, `on_witness {witness, count}`,
`on_disguise {outfit, class, from}`.

Disguise: interact with a dead NPC ("Take disguise"): the player is drawn with the donor's model / outfit (the hero model has
only the suit; skeleton of the donor, hero animation set). Colleagues (same outfit value, or guards for a guard disguise)
build up awareness while they look at the player inside `SeeThroughDisguiseDistance` (difficulty multiplier) at
`SeeThroughDisguiseSpeed`, and forget at `ForgetDisguiseSpeed`; others accept the clothes. **Not from data:** the base distance
(300 cm) and rate (0.25 / s) that those multipliers scale (`SEE_THROUGH_BASE_CM`, `SEE_THROUGH_RATE`), zone permissions of
disguises, recording cameras, and persisting notoriety between missions / saves - the original logic is native code that
was not decompiled.

## Security zones, cloth watch and notoriety (Ghidra, partial)

Traced in `HitmanBloodMoney.exe` (headless Ghidra, `hitman-re`); not yet implemented in `bm-game`:

* **Zone enum**: `ESecurityZone` = bit flags `1 << n` (table at 0x789880): `eZoneUNDEFINED`=0, `eZone1`, `eZone2`, `eZone2A`,
  `eZone2B`, `eZone3`, `eZone3A`, `eZone3B`, `eZoneMegaForbidden` (8). The names are in the `.rdata` strings 0x7aada0..0x7aadec.
* **Player zone state** (player object): `+0x12d4` = zone bitmask of the room the player stands in, `+0x12d8` = running
  cloth-alert maximum, `+0x12dd` = dirty flag. `FUN_005f1630` sets `+0x12d4` from the `ZGEOM_SecurityZone` box at the
  player (field 0x3c) or from the `ZGEOM_ZoneControl` (field 0x30), else 0.
* **Cloth watch**: `FUN_005bb5d5` runs per guard: when it sees the player in a forbidden cloth state it calls
  `FUN_005ec350` (raise `+0x12d8` to the zone value, never lower) and resets it with `FUN_005ec370` once the sighting ends.
* **Effective zone state** `FUN_005f29b0`: returns `+0x12d8` if set, else `+0x12d4`, else 1 (normal). The notoriety
  updater `FUN_005bd8a0` uses it (value 4 disables the per-NPC bonus, 0x80 forces 100 notoriety).
* **Saves**: `FUN_005fe2d0` serialises the player's zone / notoriety state (offsets 0x102c..0x103c), so it is in the save file.
* **Still open**: which zone bits forbid which clothes (the cloth deny message `hidclothdenymsg` path at 0x7916a4 /
  `FUN_006acbc0`), and the recording camera trigger `CaughtOnSuveillanceTape` (`ZGEOM_SecurityCamera`, `securitycamera.cpp`
  around 0x5bc118..0x5bcc50). Per-zone permissions are data in the level files (`ZGEOM_SecurityZone` boxes) that were not
  checked against the decoded values yet.

### Cloth change gate (verified in Ghidra)

* `ZHitmanActionChangeClothesFromBody` vtable at 0x7916cc. Methods: start 0x60e580, step 0x60e6a0, **allow check 0x6064a0**,
  stop 0x606470, child compare 0x60df40, and message helpers 0x60e130 / 0x60e160 / 0x60e190.
* Allow check (0x6064a0): if `(hitman+0x3d8) & 0x1e07f == 0` **and** `!FUN_005f6de0()` the change is allowed (return 1).
  Otherwise, once `stage >= 2` (`this+0x20`), the change is refused via notice 0x1e4 and returns 0.
* `FUN_005f6de0`: true when `(hitman+0xb64) & 0x180800 != 0` and `(hitman+0x3d8) & 0x1e01f == 0`.
* `hitman+0x3d8` and `hitman+0xb64` are flag words set from many functions (for example 0x5ed0a0, 0x5f87e0, 0x62c950).
  What each bit means has not been named yet, so the zone rule cannot be implemented without guessing.

**Not done:** naming these bits (needs the functions that set 0x180800 / 0x1e07f to be read with their string
references), the recording camera trigger (`CaughtOnSuveillanceTape`), and notoriety persistence.

### Named flag bits (partial, from class names in RTTI)

| Word | Bit | Set by (class / function) | Meaning (confirmed) |
|---|---|---|---|
| `+0xb64` | 0x800 | `ZHitmanActionPackUnpackRifle` (0x60dde0), `ZHitmanActionPackUnpackSuitcase` (0x614b60), when `this+0x2c` is set | a pack / unpack item action is running |
| `+0x3d8` / `+0xb64` | 0x400 | `ZHM3MovementFall` (0x61fa30) | falling |
| `+0x3d8` | 0x40 / 0x20 | `FUN_0062cab0`: sets 0x40 when the local forward speed `> _DAT_00756e28`, 0x20 when `< _DAT_007570e0` | forward / backward movement (from the sign of the speed) |
| `+0x3d8` | 0x1 / 0x4 / 0x8 / 0x10 | `FUN_0062c950`, `FUN_0062d220`: yaw / heading tests against constants | turning (not yet tied to a class name) |
| `+0xb64` | 0x80000 / 0x100000 | `FUN_005ed0a0`, from the object at vtable slot 0x628 (arg 3): its field `+0x4d4` == 1 / 2 | not yet tied to a name |
| `+0xb64` | 0x20 | `FUN_005ed0f0` | not yet tied to a name |

Not named: the 0x1e07f / 0x1e01f masks used by the cloth gate are therefore only partly understood. The turning bits and
the 0x80000 / 0x100000 source still need their callers resolved through the vtables (the direct-call climb found no
callers, so they are reached indirectly).

### Hitman flag words (`ZHitman3`): what each bit is

`+0x3d8` and `+0xb64` are fields of `ZHitman3` (the vtable slot 0x628 returns `this+0x5d0 + index*4`). Names below come from
the class that sets the bit (RTTI, confirmed) or from the condition the setter tests (decompiled). "unknown" means no setter
with a constant was found, or its condition is not yet tied to a game concept.

`+0xb64` (action / state word):

| Bit | Setter (class) | Meaning |
|---|---|---|
| 0x800 | `ZHitmanActionPackUnpackRifle`, `ZHitmanActionPackUnpackSuitcase`; cleared by `FUN_005f87e0` when unset | packing / unpacking an item action |
| 0x2000 | cleared by `ZHitmanActionHumanShield`, `ZHitmanActionHumanShieldRelease`, `ZHM3MovementHumanShield` | human shield held |
| 0x400 | `ZHM3MovementFall` (0x61fa30) | falling |
| 0x4000 | `FUN_0062c790` when the turn-angle test passes and the cutscene check (vfunc 0x48c) is false, angle > 0 | turning, positive side |
| 0x8000 | same, angle <= 0 | turning, negative side |
| 0x10000 | `FUN_0062c790` when forward component < `_DAT_00795ea8` and angle >= 0 | turning around, positive |
| 0x20000 | `FUN_0062c790` when forward component < `_DAT_00795ea8` and angle < 0 | turning around, negative |
| 0x20 | `FUN_005ed0f0` when speed `+0x7a8` < `_DAT_007629e0` (not in the bounding-volume case) | slow / stopped state (name unconfirmed) |
| 0x80000 / 0x100000 | `FUN_005ed0a0`, from `this+0x5dc` (slot 3): its `+0x4d4` == 1 / == 2 | state of the object in slot 3 (its class is not resolved) |

`+0x3d8` (motion / controller word):

| Bit | Setter (class) | Meaning |
|---|---|---|
| 0x1 | `ZHitman3ControlMouse` (0x62d950) when there is no direction input; cleared when the input is behind | no movement input (direction unresolved) |
| 0x4 | `FUN_0062c950` with `|angle|` >= `_DAT_00795ec8` (sets 0x1 and 0x4) | large turn |
| 0x8 / 0x10 | `FUN_0062c950` (angle side), `FUN_0062d220` (side of strafe) | side of the turn / strafe |
| 0x20 / 0x40 | `FUN_0062cab0`, `FUN_0062c790`, `ZHitman3ControlMouse` (0x62dab0) | lateral input: 0x40 right, 0x20 left (sign convention) |
| 0x400 | `FUN_005f87e0`, `FUN_005ed0f0` | falling-related state (unconfirmed) |
| 0x800 | cleared by a family of `0x5f2xxx` functions (each posts a state event via vfunc 0x390 / 0x37c); set by `ZPlayer` (0x528e80) | action in progress; cleared when a state event is posted |
| 0x20000 | `ZHM3MovementKillTargetAnimLineup` (0x6267b0 sets, 0x62a2d0 clears) | kill-target lineup animation |
| 0x400000 | `ZHM3MovementClimbWall`, `ZHM3MovementHugWall`, `ZHM3MovementClimbVerticalLine`, `ZHM3MovementClimbDrainPipe` | on a wall / pipe |
| 0x20000000 | `ZPlayer` (0x529fc0) | unconfirmed |
| 0x2, 0x2000, 0x4000, 0x8000, 0x10000 | no constant setter found | unknown (written as whole words in another function) |

**Cloth gate, as read from the code:** change of clothes is refused when `+0x3d8 & 0x1e07f` is set, or when
`FUN_005f6de0` is true (`+0xb64 & 0x180800` set and `+0x3d8 & 0x1e01f` clear). With the names above this means: the hitman
is moving, turning, falling, on a wall, in a kill-target lineup, or packing an item (or the slot-3 state is 1 / 2).

**Still open:** the 0x2 and 0x2000 to 0x10000 bits of `+0x3d8`, the class of the object at `this+0x5dc`, and whether the
security-zone ban (zone bitmask `+0x12d4`) is part of the same gate. The gate code above does not read `+0x12d4`.

### Unknown bits resolved (second pass)

Facts used: `FUN_005f87e0` (ZHitman3 update) starts each frame with `+0x3d8 &= 0xfffe1a00`, which clears every motion bit
except 0x200, 0x800, 0x1000 and bits 0x20000 and up. Motion bits are therefore recomputed each frame.

`+0x3d8` (motion word):

| Bit | Setter | Meaning |
|---|---|---|
| 0x2 | no setter | **dead**: cleared each frame, never set |
| 0x2000, 0x4000, 0x8000, 0x10000 | no setter (the turn bits are `+0xb64`, see below) | **dead**: cleared each frame, never set. The cloth gate tests them with `& 0x1e07f` / `& 0x1e01f`, so they never block a change |
| 0x80 | `FUN_005f87e0`: when no motion bit is set and `this+0x103f` is non-zero | idle, with flag `+0x103f` |
| 0x100 | `FUN_005f87e0`: when no motion bit is set | idle (no motion this frame) |
| 0x200 | `FUN_00527b10` sets, `FUN_00527bb0` clears when its counter `+0x5a0` reaches 0 | a counted timed state is running |
| 0x800 | `FUN_00507e..` sets it when the interaction target has class id `DAT_0097bd60`; `FUN_005f87e0` clears it when no interaction | an interaction is in progress |
| 0x40000000 | `FUN_00529fc0`: standing on an object whose `+0x3c` has 0x100 (moving platform) | on a moving platform |
| 0x10000000, 0x20000000 | `FUN_00529fc0`, same branch: sign of the platform's motion against the hitman | platform moves away / towards (sign convention unverified) |

`+0xb64` (action word), new names:

| Bit | Setter | Meaning |
|---|---|---|
| 0x20 | `FUN_00618a30` (0x80020 set), `FUN_005ed0f0` | **crouched**. `FUN_005ecff0(0)` (true when 0x20 is set) makes the body box 60 cm high instead of 100 cm standing |
| 0x80000 | `FUN_00618a30` (state 1) | stance change blocked by headroom (crouching) |
| 0x100000 | `FUN_00618a30` (state 2), which also sets `+0x3d8 0x400` | stance change blocked and no floor under the drop (falling) |

Source of 0x80000 / 0x100000: `FUN_005ed0a0` reads `this+0x5dc` (slot 3 of the actor's `+0x5d0` array), and the state is
the object's `+0x4d4` (state) and `+0x4d8` (flag). `FUN_00618a30` takes `&state` as its first argument. The class of that
object is **not resolved**: the only reference to its update, the thunk `0x61df20`, is not referenced by any pointer or
direct call in the binary.

**Cloth gate, final reading:** `+0x3d8 & 0x1e07f` tests the motion bits 0x1, 0x4, 0x8, 0x10, 0x20 and 0x40 (0x2 is dead),
plus the dead bits 0x2000, 0x4000, 0x8000 and 0x10000. `FUN_005f6de0` tests `+0xb64 & 0x180800` (pack / unpack 0x800, stance
headroom 0x80000 / 0x100000) and `+0x3d8 & 0x1e01f` (the same motion bits without 0x40 / 0x20 / 0x80; still the dead 0x1e000 part).
So in practice a change is refused while the hitman is moving or turning (0x1..0x40), or while packing, or while a stance change
is blocked. Lines of the `+0x3d8` mask beyond 0x40 never apply.

### Corrections and third pass on the flag bits

**Correction 1 (stance headroom is live).** The earlier note that `FUN_00618a30` was dead was wrong. Ghidra was run with
`-noanalysis`, so it split the body of `FUN_0061dee0` at 0x61df20. The bytes show `LEA ECX,[ESI+0x4d4]` immediately before
0x61df20, so control falls straight into the call to `FUN_00618a30`. `FUN_0061dee0` is a method of
`ZHM3MovementPrimaryMove` (vtable 0x794274). Its `this+0x38` is the hitman, and the `PrimaryMove` object is the component at
hitman `+0x5dc` (slot 3 of `+0x5d0`), whose state is at `+0x4d4` and flag at `+0x4d8`. So:

* `+0xb64` 0x80000 is `PrimaryMove` state 1, and 0x100000 is state 2. Both are set by `FUN_00618a30`, and state 2 also sets
  `+0x3d8` 0x400.
* `FUN_00618a30` picks the body box height from the hitman flags: 60 cm when `FUN_005ecff0` is true (`+0xb64` 0x20 set,
  i.e. **crouched**), 82 cm when `FUN_005ed020` is true (`+0x3d8` 0x400, i.e. **airborne**), otherwise 100 cm (standing).
  It sweeps that box (`FUN_004bf4b0`) to see if the stance change fits. State 1 means it is blocked while crouched, state 2
  means it is blocked while airborne. So **0x20 = crouched** is now confirmed by the code that uses it.

**Correction 2 (cloth gate, exact form).** `FUN_006064a0` (the allow check of `ZHitmanActionChangeClothesFromBody`):

* if `hitman+0x3d8 & 0x1e07f == 0` and `FUN_005f6de0() == 0`, return 1 (the action continues);
* else if the action stage (`this+0x20`) is below 2, return 1 (the action continues);
* else refuse (notice 0x1e4), return 0.

`FUN_005f6de0()` is 1 when `hitman+0xb64 & 0x180800` is set and `hitman+0x3d8 & 0x1e01f` is clear. Since `0x1e01f` is a subset of
`0x1e07f`, the refusal condition at stage 2 is: motion bits in `+0x3d8 & 0x1e07f` set, or `+0xb64 & 0x180800` set. In words:
**the change is refused, once it reaches stage 2, while the hitman is moving or turning (`+0x3d8` 0x1..0x40), packing or
unpacking (`+0xb64` 0x800), or blocked by stance headroom (`+0xb64` 0x80000 / 0x100000).** The gate does not read the zone
state (`+0x12d4`).

**Third pass, the remaining unknown bits:**

| Word and bit | Setter (live) | Meaning |
|---|---|---|
| `+0x3d8` 0x2, 0x2000, 0x4000, 0x8000, 0x10000 | none | never set; cleared each frame by `FUN_005f87e0`. Dead in this build |
| `+0x3d8` 0x200 | `FUN_00527b10` (sets) / `FUN_00527bb0` (clears when `+0x5a0` counter reaches 0) | a counted timed state is running |
| `+0x3d8` 0x800 | `FUN_00507a70` (ZActor) when an action target with class id `DAT_0097bd60` is engaged; `FUN_00528e80` (ZPlayer) | action target engaged |
| `+0x3d8` 0x80 | `FUN_005f87e0` when no motion bit is set and `this+0x103f` is non-zero | idle, with flag `+0x103f` (the flag's own meaning is not resolved) |
| `+0x3d8` 0x10000000, 0x20000000, 0x40000000 | `FUN_00529fc0` (ZPlayer) while standing on an object flagged 0x100 | on a moving platform; 0x10000000 / 0x20000000 are the vertical move relative to it (sign convention not confirmed) |
| `+0xb64` 0x800 | `FUN_005f87e0` (when the action predicate `vfunc 0x598` holds and `this+0x1048` is set, or the timed flag `0x200` is set), and `ZHitmanActionPackUnpack*` | an action (interaction / pack / unpack) is in progress |

**Reachability caveat.** These results come from Ghidra with `-noanalysis`. The function boundaries are partly wrong, and some
addresses sit in code Ghidra never split. The reachability check (every data pointer into code and every call / jump from
the program entry) reaches all of the functions above. It also reaches `FUN_005f1630` (zone update) and `FUN_005bb5d5` (cloth
watch), through the code containing their callers. Those callers were attributed by proximity, not by a proper function
boundary, so treat them as reachable but not confirmed. Two callers of the zone state, at `0x6a8eff` and `0x6acb56`, sit in code
that Ghidra did not split. They have not been decoded.

### Zones and disguise definitions (bm-game `stealth.rs`, pass 4)

Verified in `HitmanBloodMoney.exe`:

* `ESecurityZone` values (table at 0x789880, indexed by enum ordinal): `eZoneUNDEFINED` 0, `eZone1` 1, `eZone2` 2, `eZone2A` 4,
  `eZone2B` 8, `eZone3` 0x10, `eZone3A` 0x20, `eZone3B` 0x40, `eZoneMegaForbidden` 0x80 (`zone_state_of`).
* Zone state getter `FUN_005f29b0`: returns `+0x12d8` (running maximum set by `FUN_005ec350`) when non-zero, else `+0x12d4`
  (zone of the player's room / security box, set by `FUN_005f1630`), else 1. `FUN_006a8ef0` is a wrapper of the same value.
* Observer rate (`FUN_005bd8a0`): when the zone state is 0x80 (`eZoneMegaForbidden`) the detection rate is set to 100
  (`ZONE_MEGA_FORBIDDEN`). This is the only zone effect in the observer code that has been verified.
* Disguise definitions: `ZHM3HmAs` nodes (type 0x100446). Property 11 = dress quality token (`eQualityBad` .. `eQualityHitman`),
  property 12 = string array of the zones the outfit is listed for (`bm_formats::gameplay::disguises`). Its `ZLNKOBJ` child
  carries the model id (property 4) and outfit value (property 6). The `HitmanTemplate` definition lists `eZone1` only.
* Notoriety is 0 on Rookie (`FUN_006d0220` only runs when difficulty is not 0).

**Not verified, so not implemented:** which code reads property 12 and what it does when the zone is not listed. The
`ZHM3HmAs` method table (vtable 0x78b4d4) has its zone-specific methods in the 0x5d0xxx range (for example `FUN_005d0190`,
`FUN_005d03b0`, `FUN_005d0a70`), but none of them has been decoded to the zone check yet. The cloth gate `FUN_006064a0` does not
read the zone state. So the clothing ban in a zone is still open.

### Where the clothing ban lives (pass 5)

Verified from `HitmanBloodMoney.exe` (member names come from the serializer strings at 0x790c70..0x790e3c):

* Hitman state fields saved by `FUN_005fe2d0`: `+0x102c` `m_bChangingClothes` (set 1 by the change-clothes action start
  0x60e580, cleared 0 by its stop 0x606470), `+0x102d` `m_bChangingClothesAllowed`, `+0x12d4` `m_eRoomZone`,
  `+0x12d8` `m_eCustomZone`.
* `m_bChangingClothesAllowed` has no writer in the engine code. Its only writers are the property / serializer paths
  (`FUN_005fe2d0`, `FUN_005feb40`), so it is set by the level's scripts.
* The hitman's blocked-action reasons (`FUN_005f7d70`, word `+0xb7c`) include bit 0x400000: "change clothes blocked" when
  `this+0x40b` is set and `m_bChangingClothesAllowed` is 0.
* The level logic is in the script DLLs `Scriptcs/_gamerelease/M01.dll` etc. They export `ISF`, `SF`, `ScriptImports`,
  `Scripts` (the script bytecode of the level), and the zone rule is expected there. The `ChangeClothes` and
  `BodyMissingClothes` strings are script identifiers with no direct code references.

**Consequence for the port:** the zone clothing ban is decided by the level scripts, which are bytecode the Rust port does
not run. The safe-zone lists in the `ZHM3HmAs` data are present (`bm_formats::gameplay::disguises`), but nothing enforces them,
because the script that reads them has not been decoded.

### Level script module (`Scriptcs/_gamerelease/M01.dll`), pass 6

* The module is native x86, not encrypted (the code has normal prologues). Its export `Scripts` (RVA 0x6aa70, in `.data`)
  is a table: a count (0x3b4) followed by 52 pointers to script descriptors (`0x1005bde0` .. `0x1005cb40`). `ISF`, `SF` and
  `ScriptImports` are empty.
* A descriptor starts with a pointer to its class name (`M01_M01_Levelcontrol`, `Alllevels_Guard`, ...), then property lists,
  then pointers to the handlers (for example `M01_M01_Levelcontrol` at `0x10066370` has handlers `0x100503e5`, `0x1005021a`).
* `FUN_100503e5` (the level control's message handler) dispatches on message IDs: `0x824` -> `FUN_10050320` (objective
  triggers), `0x893` -> `FUN_100503ad` (sends message `0x80c`), `0x97e` -> `FUN_10050396` (toggles flag `+4 & 4`). These are
  objective logic, not the clothing rule.
* The clothing rule is not in these handlers. The next step is to list every handler of every descriptor, find the one that
  sets the hitman's `m_bChangingClothesAllowed` (hitman `+0x102d`) or reacts to the zone messages, and decompile it.

### Clothing gate and zone ban, pass 7 (what is ruled out)

* `FUN_006064a0` (the change-clothes allow check) does not read `m_bChangingClothesAllowed` (`+0x102d`) or any zone field.
* `+0x102d` is written only by the hitman's load routine (`FUN_005fe2d0`). Its save routine is `FUN_005feb40`, reached through
  multiple-inheritance adjustor thunks at `0x600f30`..`0x600fb0`. Nothing else writes it, so the level data never sets it.
* The hitman's blocked-action words `+0xb7c` / `+0xb80` (filled by `FUN_005f7d70`) go to the UI through message `0x922`
  (`FUN_005eb8e0`). They do not enforce anything.
* The M01 level script (`M01.dll`, native code, not encrypted) dispatches messages `0x803`..`0x952` (for example `0x824`,
  `0x893`, `0x97e` in the level control). The handlers checked so far (`FUN_10050320`, `FUN_100503ad`, `FUN_10050396`,
  `FUN_10009ecb`, `FUN_100381f6`) handle objectives and flags, not clothing or zones.

**Conclusion so far:** the zone ban is not implemented as a single engine rule that can be read from the code paths checked.
Either it is spread over the message handlers in the level DLLs and the UI, or it is not enforced for the change itself.
Decoding it fully needs the message-ID table of the engine (which message means what), which is not in the binaries I've read.

### Message-name table and the clothing lead (pass 8, ruled out)

* The engine has a table of 557 message-name pointers at VA 0x7f3850 (`.data`). Each entry points to a tail of a name in
  the string pool (for example `nterssecurezone`, `eavessecurezone`, `hangeclothes`, `lidclothdenymsg`). The mapping from
  table index to message ID is **not verified**. The script ids used by the level DLL (0x80c..0x97e) fall inside the
  range 0x800..0xa2c that the table spans, which makes `id = index + 0x800` plausible but unconfirmed.
* The `0x961` values in the executable (`PUSH 0x961` at 0x45fec8 and 0x49cc76) are **line numbers** passed to the allocator
  `FUN_00446720` (with the file names at 0x760b88 and 0x764fc0). They are not messages. The same applies to other constants
  found this way, so the lead to `ChangeClothes` through those pushes is false.
* In the level DLL, `FUN_1003916f` sends `0x96c` to the NPC's target and `FUN_10038e0d` handles `0x96c`. This is NPC-side
  script logic and does not show a zone check.
* The `ZHM3HmAs` getters at 0x5d0a00..0x5d0a30 return constants. They do not read the zone list.

**Still open:** the zone check that stops a clothes change (or a disguise pickup) in a forbidden zone. No engine or script
code that performs it has been located.

### Zone clothing ban: GUESS, implemented on user request (no game testing)

Verified data this rule uses: the `ZHM3HmAs` safe-zone lists and the zone state (`zone_state_of`, `zone_state`).
**Not verified:** the rule itself. The original's enforcement is not located (see the sections above).

Implemented (`bm-game/src/stealth.rs`, `player.rs`):
* `clothes_allowed_in_zone(safe_zones, zone)`: clothes can be taken only where the outfit's definition lists the current zone
  state. The mega-forbidden zone (0x80) refuses every change. An outfit without a definition is never restricted.
* `World::can_take_clothes(victim)`: applies the rule to the victim's outfit. `take_disguise` refuses when it returns false,
  and the "Take disguise" prompt is not offered.
* Wearing the own suit back (`drop_disguise`) is always allowed. This is a guess too.
* Test: `clothes_rule_guess` in `stealth.rs`.

Nothing here has been checked against the original game.

### Guessed zone behaviour, round 2 (no game testing)

All of the following are GUESSES. The only verified inputs are the safe-zone lists, the zone values and the `eZoneMegaForbidden`
rate of 100.

* **Out of place.** The clothes the player wears (own suit, or disguise) are out of place when the current zone is not in their
  safe list. The own suit uses the `HitmanTemplate` list (`eZone1` in the data). `out_of_place` in `stealth.rs`, the mega-forbidden
  zone excluded.
* **Effect on detection.** Observers' detection rate doubles while the clothes are out of place. A disguise that is out of place
  is not accepted by bystanders; it is treated like the own suit (`clothes_out_of_place`, `ai.rs`).
* **Clothes change.** Taking clothes is refused where the outfit's list does not contain the zone, and in the mega-forbidden zone
  (`clothes_allowed_in_zone`).
* Tests: `clothes_rule_guess`, `out_of_place_guess`.

### Recording cameras (`ZGEOM_SecurityCamera`, `securitycamera.cpp`)

Verified from `HitmanBloodMoney.exe`:
* Class vtable 0x78950c. The camera's message names: `Activate` (id 0x49), `Deactivate` (0x4a), `ProjectileHit` (0x4b),
  `ObjectVisible` (0x4c), `ObjectNotVisible` (0x4d), `ObjectVisibleUpdate` (0x4e), looked up in `FUN_005bba80`.
* `FUN_005bc7e0` dispatches on those ids. `ObjectVisible` runs `FUN_005bc630` on the seen object (it copies the object's action words
  `+0xb7c`/`+0xb80` and `+0xb64` flags into the message) and then sends `0x814`; `ObjectNotVisible` sends `0x815`;
  `ObjectVisibleUpdate` sends `0x816`; `Activate` sends `0x817`; `Deactivate` sends `0x818`.
* The per-tick check `FUN_005bbdc0` sends a visibility message to the owner when the seen object's flag `0x400` is clear.
* The meaning of `0x814`..`0x818` and the tape itself are in the level script, which is not decoded.
* Scene data: `SecurityCamera` controller props are floats `[0..7]`, an int, then the camera-head, lamp and sound node
  references. Levels with cameras: M02 (entrance cameras), M04, M05 (two), M10 (casino and lobby), M11, M12 (many).

**Guesses (implemented, not verified):**
* Camera view: the lens is at the node, facing its forward axis. Half-angle = props float 4 (degrees). Range = props float 5
  (metres). No sweep is modelled.
* A crime (a kill or damage the player makes) that a camera sees with line of sight records the player: the game's
  `CaughtOnTape` notoriety (20, or 10 while disguised). Each camera records at most once per 10 seconds (`SecurityCamera`,
  `cameras_record`, `TAPE_COOLDOWN`).
* Tests: `camera_view_guess`.

Not implemented: stealing the tape (`StealTape`), and the camera's head sweep and alarm.

### Camera alarm and tape stealing: partial decode (pass 9)

Verified:
* Camera message senders: `0x814` at 0x5bb940, `0x815` at 0x5bb980, `0x816` at 0x5bb9c0, `0x817` at 0x5bba00 (the camera
  `FUN_005bc7e0` dispatch). `0x818` is sent by `FUN_005bba30`.
* Receivers: no level DLL (`Scriptcs/_gamerelease/*.dll`) contains the immediates `0x814`..`0x818`. The receivers are in the
  executable: `0x814` is also used by `FUN_00643154`, which sends notice `0x86c` to the level object and calls `+0x164` on the
  mission object (`DAT_0082083c+0xa48`) when more than one entry is present (`in_EAX > 1`). This is the candidate camera alarm,
  not yet confirmed. `FUN_0045cb20` and `FUN_00477750` use `0x818`.
* The steal-tape action (`ZHitmanActionStealTape`, vtable 0x793354): start `FUN_0060d660` sets the hitman's animation flag
  `+0x1030` and `+0x1034`, stop `FUN_0060d800` clears them. The tape effect is not in these two functions.
* The tape objects are `VideoTape` (M02), `SurveillenceTapes` (M05), `Surveillance_Tapes` (M10) and `POISurveillanceTape` (M11)
  (`ZBoxPrimitive` with an `InterestPoint`).
* The notoriety calculation (`FUN_006d0220`) adds 20 or 10 on a tape event, selected by the object's field `+0xec` (1 undisguised,
  2 disguised). Who sets `+0xec` has not been found.

Open for 1:1: which function sets `+0xec` (the tape event), the effect of stealing a tape on it, and the full alarm chain
(`FUN_00643154` and the guards' response). The camera recording and the 10-second cooldown in `stealth.rs` are still guesses.

### Camera alarm and tape: decode pass 10

* The `0x814` send is inside a method of `ZHM3ActorRepairUtilBox` (vtable 0x797644; the class name only appears in the
  executable, and no level file uses it). Its method at 0x643100 is a small state machine on the counter `+0x3c`:
  count 0 -> set count to 1 and copy `+0x48` to `+0x40`; count 1 -> call `FUN_00641570` (walks the list at owner `+0x34`,
  `+0x3c`, calling virtual slots `0x58` / `0x5c` on each entry's `+0x60` when `FUN_00438f70()` says so) and count++.
  `FUN_00643154` (inside the same method) sends `0x814` and `0x86c` and calls mission slot `+0x164` when count >= 2.
* Not established: that this util is the camera alarm; the owner of `+0x34`; what `0x814`/`0x86c` do to the guards.
* The message-name table has no `NearByGuardsAlarmed`. Its neighbours `ealarm` (0x951), `obalalarm` (0x956) and
  `turnedalarm` (0x968) are alarm names, but the index-to-id mapping is still unverified.

### Decode pass 11 (camera alarm and tape)

* `ZHM3ActorRepairUtilBox` has type id `0x2000000f`. Its constructor `FUN_00642e70` takes an owner pointer and stores the
  owner at `+0x08`. Its check `FUN_00642ed0` compares an actor's type with `0x2000000f` and its class name with the string
  `ZHM3ActorRepairUtilBox`. No level node has type `0x2000000f`, so the box is created at run time. Its creator is the generic
  factory `FUN_00637ca0`.
* The mission stats `Alarm_global` and `Alarm_local` are only named in the stats table (`.data` 0x806100, 0x8060ec, 0x818b84,
  0x818b98). Their increments are by stat index and are not yet found.
* Still open: the function that writes the tape-type field `+0xec` on the notoriety object, the effect of stealing a tape, and
  the alarm chain from `0x814`.

### Decode pass 12: the camera's sighting path (verified from the binary)

* `FUN_005bc630` (called when the camera sees an actor). For a player-type actor (`type & 0x...e84 == 0x...e80`):
  it copies the hitman's action words `+0x2df` / `+0x2e0` (the `+0xb7c` / `+0xb80` reasons) into the message; sets
  `DAT_009b3c04 = 1`; sets `DAT_009b3c24 = 2` when the hitman's current room word `+0xfd4` equals `+0xfd0`, or toggles it
  `(prev == 2) + 1` otherwise (when the level object flag `+0x1b0` is clear). `DAT_009b3c24` is the tape kind read by the
  notoriety updater (1 = undisguised, 2 = disguised). It then calls `FUN_005bc0e0` when `+0x1b0` is clear.
* `FUN_005bc0e0` (camera): when `FUN_005e83b0()` (byte `+0x1bc` of the level object) is 0 and the game clock is past the
  camera's time check: stores the camera's time, starts a recording through the security manager (`DAT_0082083c+0x6974`,
  vfunc `0x26c`, handle stored at `+0x240`), calls `thunk_FUN_00660ef0(&DAT_00789600, 1)`, and sends the camera's
  `Activate`-slot message (`+0x30`) to its target (`+0xb0`) through the owner (`+0x24`, vfunc `0x124`).
* `FUN_005bbf70` (camera): a sighting ring of 10 entries (stride 0xc at `+0xd0`, count `+0x238`, next index `+0x23c`). Each
  entry holds the actor, a flag byte `+0xda` (set when the actor is unrecognised) and three short positions.
* `FUN_005bbb90` (camera): builds the flags for a query on an actor: `0x20` (entry not flagged and not recognised), `0x40`
  (the actor is not the hitman), `0x80` (position beyond the threshold), `0x100` (distance below threshold), `0x4000`
  (`FUN_005029a0() == 2`), `0x8000` (not recognised), `0x40` / `0x8000` as above.
* Predicates on the actor: vfunc `0x6c8` (ZHM3Actor) = bit 2 of the byte at `+0xa3c` (recognition); `FUN_005029a0` = the byte at
  `+0x5e2` (alert level; the ZActor constructor sets it to 1). `FUN_005e83b0` = the byte at `+0x1bc` of the level object.

Still open: what the security manager does with the handle (the recording and its tape), what stealing the tape removes,
and the guards' reaction to `thunk_FUN_00660ef0` and to the `Activate` message.

### Decode pass 13: security camera recording and the tape (verified unless marked)

* Camera message switch `FUN_005bc7e0` (`ZGEOM_SecurityCamera`). Message ids come from the registration calls in
  `FUN_005bba80`: `+0x30` Activate (0x49), `+0x32` Deactivate (0x4a), `+0x34` ProjectileHit (0x4b),
  `+0x36` ObjectVisible (0x4c), `+0x38` ObjectNotVisible (0x4d), `+0x3a` ObjectVisibleUpdate (0x4e).
* Sighting `FUN_005bc630(object, msg)`. The hitman path is taken when the object's class flag matches. It reads the
  hitman's `+0xb7c`/`+0xb80` words into the message, and sets the tape kind `DAT_009b3c24`:
  `= 2` when `hitman+0xfd4 == hitman+0xfd0` (level flag `+0x1b0` clear), otherwise
  `DAT_009b3c24 = (DAT_009b3c24 == 2) + 1`, i.e. 1, or 2 when it was already 2 (sticky). The same code is a separate
  routine at `0x6a9222` (`mov edx,[DAT_009b3c24]; cmp edx,2; sete al; inc eax; mov [DAT_009b3c24],eax`).
  `fd0`/`fd4` are the hitman's room words (documented above as the current room word); the Rust port takes
  "same room" as the room of the previous recording (guess).
  It then calls `FUN_005bc0e0` (start recording) only when the level flag `+0x1b0` is clear.
* Start `FUN_005bc0e0`: only if the clock check passes and `FUN_005e83b0()==0` (returns `this+0x1bc`). It stores the
  end time `camera+0x244 = clock * K`, calls the event-camera start `0x26c(camera+0xc4, 10, 0, 0, 0, 0)` and stores
  the handle in `camera+0x240`, runs `thunk_FUN_00660ef0(&DAT_00789600, 1)` (notice), and sends `Activate`-family
  message `+0x30` to `camera+0xb0` through the owner (vfunc `+0x124`).
  **The camera passes event type 10, not 8.**
* Event type 10 (`FUN_005eaba0`): `param[8]=1` if unset; end time = now + 0x2800 ticks; zoom `param[0xe] = 25.0`;
  flag `param[0xf] = 1`. Type 8 (250.0 duration, end now + 0x1000) is a different event.
* `ObjectNotVisible` (+0x38) does not stop the recording. It ends at its end time (`clock + 0x2800`).
* Stop: `Deactivate` (+0x32) and `ProjectileHit` (+0x34) while active clear `camera+0xb4`, call `0x270(camera+0x240)`
  (`FUN_005e9ee0`, remove by handle), and then send message `+0x32` to `camera+0xb0`. Notifiers: `FUN_005bb940` 0x814
  (ObjectVisible), `FUN_005bb980` 0x815 (ObjectNotVisible), `FUN_005bb9c0` 0x816 (ObjectVisibleUpdate), `FUN_005bba00`
  0x817 (recording started), `FUN_005bba30` 0x818 (recording stopped).
* Tape clear (resolved): `DAT_009b3c6c` is the message id of `StealTape`. The registration is in the static init
  block at `0x74a840`: `push "StealTape" (0x78a8d0); mov ecx, 0x9b3c64; call FUN_0043a6e0; push FUN_00750920; call ...`.
  `FUN_0043a6e0` writes `obj[0]=0`, `obj[1]=name`, `obj[2]=0` and links the object into the registry
  (`FUN_0043a690()` = `DAT_00820988`, `FUN_005a2a50`). `obj[2]` is the id, which the registry assigns at runtime, so
  there is no direct store to `DAT_009b3c6c` (that is why the search found none). The same block registers
  `CompleteLevel` (`0x9b3c4c`, handler `0x7508e0`), `FailLevel` (`0x9b3c58`, `0x7508f0`) and `ZGEOM_NotorietyPapers`
  (`0x9b3c70`). `FUN_006ac5e0(msg, data)` is the router: when `msg == StealTape`, `DAT_009b3c24 = 0` (tape kind cleared),
  then it forwards to `FUN_006cd080`. The `StealTape` handler `FUN_00750920` sets the byte at `DAT_00820990`
  (`FUN_0043a690()+8`) to 1. What reads that byte is not yet known.
* The steal action is `ZHitmanActionStealTape` (vtable `0x793354`). Start `0x60d660` and stop `0x60d800` only drive the
  animation and the hitman's `+0x1030/+0x1034` flags. Its prompt name is `AllLevels/Actions/StealTape`.
* Notoriety: `FUN_006d0220` reads the tape kind (`DAT_009b3c24`, i.e. `0x9b3b38+0xec`). The caller that runs it on a
  real tape event is not yet confirmed (it is also called from the mission-statistics routine `FUN_006af684`).
* Rust port (`bm-game/src/stealth.rs`, `update_cameras`): sighting-driven recording for `RECORD_SECONDS` (10.24, clock
  units assumed ms), tape kind via `next_tape_kind`, notoriety by kind on each recording start (the call site of the
  notoriety is a guess). `Deactivate` / `ProjectileHit` are not modelled.
* Camera alarm chain (partly verified):
  * `ZSecurityMonitor` (vtable `0x7896b4`, constructor area `0x5bcxxx`) registers `Deactivate` (`0x2f`), `Activate`
    (`0x30`) and `SecurityCameraNotification` (`0x31`). `Activate` sets `monitor+0x68 = 1` and `Deactivate` clears it.
    While on, `FUN_005bcd40` forwards each message to the watchers whose bit `+0x6a & 1` is set (`FUN_005bce20` sets
    the bits through `FUN_005bcca0`). Its `+0x4c` list holds the monitor's `SecurityCamera`s (`FUN_005bcc00`).
  * The camera's `Activate` (`camera+0x30`) goes to `camera+0xb0`. That target is the monitor if the link is right, but
    the writer of `camera+0xb0` is not found yet (store scan of `.text`: 104 sites; checked `0x5c..0x5e` candidates, none
    of them the camera).
  * Guard reaction: `FUN_00501910` (class `ZActionArbiter`, vtable `0x77075c`, slot `0x164`) runs while the actor's
    pending timer `+0x58` is > 0 and not expired. It sends an alarm message to its colleagues (`+0x124`, the colleague
    list at `+0x10`), copies the alarm name, shows `AllLevels/Warnings/NearByGuardsAlarmed` (`0x7709b8`), sets the
    alert level `+0xec = 999` and the alarm end `now + 0x400`. The setter of `+0x58` is not found yet.
  * `CaughtOnSuveillanceTape` (`0x789600`) is shown by `FUN_005bc0e0` for 10 s on each recording start (done in Rust).
* Still open: the reader of `DAT_00820990` (the StealTape flag), the writer of `camera+0xb0`, and the setter of the
  guard's `+0x58` timer.

### Decode pass 14: monitors, level data and the StealTape id

* Level data (`camera_survey` / controller scans): `SecurityCamera` controllers are in M02 (2), M04 (1), M05 (2), M10 (6),
  M11 (3), M12 (18), total 31. M00, M01, M03, M06, M08, M09, M13 and hideout have none.
  `SecurityCamera` props: F32 0 (10 to 60, unverified), F32 1 (5), F32 2 (1 or 2), F32 3 (0), F32 4 = half angle,
  F32 5 = range (m), F32 6 (sweep, unverified), F32 7 (offset, unverified), I32 8 (group, unverified),
  Str 9 = head node, Str 10 = lamp node, Str 12/13 = sounds, F32 14 = 1.
* `SecurityMonitor` controllers sit on the guard (or security room) node and are in M02 (2), M04 (1), M05 (1), M10 (1),
  M11 (1), M12 (4). Props: `Container[0]` = watcher nodes (guards, `…\Ground`), `Container[1]` = camera node paths.
  So the link camera to monitor is explicit in the data: the monitor's `+0x4c` list is `Container[1]`
  (`FUN_005bcc00`), and the watchers `+0x30` are `Container[0]`.
* Camera start `FUN_005bc0e0` shows `AllLevels/Info/CaughtOnSuveillanceTape` for 10 s (string at `0x789600`,
  reference at `0x5bc16c`). Done in Rust (`stealth.tape_notice`).
* `FUN_00501910` (class `ZActionArbiter`, slot `0x164` of the vtable at `0x770764`) is the guard alarm: it alerts the
  colleagues (`+0x124` with the alarm name), shows `AllLevels/Warnings/NearByGuardsAlarmed` (`0x7709b8`), sets the
  alert level `+0x3b = 999` and the alarm end `now + 0x400`. It runs while the pending timer `+0x58` is > 0.
  The setter of `+0x58` and the watcher test (`FUN_005bcca0`, vfunc `+0x170`) are the open parts.
* Numeric camera ids `0x814` to `0x818` also occur in many unrelated functions, so the watcher reaction cannot be found
  by immediate search alone.
