# Modding guide (`bm-script`)

Blood Money (Rust) is built to be modded. A mod is a **folder or `.zip`** with a `mod.toml` manifest that can
contain any mix of:

* **assets** - loose files that override or extend the game's files through the VFS (textures, models, sounds, music cues, menus, fonts, shaders, ...);
* **data patches** - JSON/RON/TOML files that tune gameplay data (weapons, NPC parameters, loadouts, ...);
* **Lua scripts** - sandboxed Luau scripts that hook game events and call the engine API.

API version: **`1.0.0`** (`bm_script::API_VERSION`). Minor versions only add things; a major bump may break scripts.

## Quick start

```
Mods/
  my_mod/
    mod.toml
    scripts/main.lua
    assets/Music/M01_main/alert.ogg      # mounted into the VFS
    data/weapons.json                    # merge patch
```

`mod.toml`:

```toml
id = "my_mod"                 # [a-z][a-z0-9_-]{0,47}, unique
name = "My Mod"
version = "1.0.0"             # semver
authors = ["you"]
description = "..."
api_version = "^1.0"          # semver requirement on the scripting API (default "^1")
game_version = "Blood Money 1.0 (PC)"   # informational
priority = 0                  # higher loads later => wins conflicts
load_after = []               # soft ordering
load_before = []
conflicts = ["other_mod"]     # cannot be enabled together (lower priority one is disabled)
permissions = ["ui", "world"] # see below
assets_dir = "assets"         # defaults shown
data_dir = "data"

[dependencies]                # hard: missing/wrong version => mod is skipped
base_pack = ">=1.2, <2"
[optional_dependencies]       # ordering + version warning only
hello_mod = "*"

[scripts]
entry = ["scripts/main.lua"]  # run in order at load
dir = "scripts"               # root for require("x")
```

Two complete examples live in `examples/mods/`:

* `hello_mod` - logging, `require`, a console command, a cvar, `ui.message`, a data patch.
* `hooks_demo` - `on_damage` scaling (priorities, modifying `ev` fields), cancelling `on_pickup`, kill counter in `save`, timers, custom events, hot-reload state, soft dependency.

Type stubs for editor completion: `examples/types/bm_api.d.lua` (LuaLS: add the folder to `workspace.library`; regenerate with
`cargo run -p bm-script --example mods -- stubs bm_api.d.lua`).

## Where mods are found

`ModDirs::default_dirs()`: `<folder of the exe>/Mods` and `<user dir>/Mods`
(`$BM_USER_DIR`, else `%APPDATA%\bm-rs`, else `~/.bm-rs`). Each entry is a folder containing `mod.toml` or a `.zip`
(`mod.toml` at the zip root or inside a single top-level folder). Zips are extracted once into `<user dir>/mod_cache`
(zip-slip and size/entry-count limits enforced). If the same id appears twice the higher version wins (ties: later root),
and a warning is reported.

Enable/disable list: `<user dir>/mods.toml`

```toml
disabled = ["some_mod"]       # everything else that is installed is enabled
[priority]
other_mod = 25                # user override of the manifest priority
```

### Load order and conflicts

`bm_script::package::resolve` produces the load order: a topological sort over `dependencies`, enabled
`optional_dependencies`, `load_after`/`load_before`; ties are broken by `(priority, id)` so the result is deterministic.
**Later mods override earlier ones** (this is exactly `Vfs::add_mod_dir` order, and the order in which data patches apply).
Problems never abort loading of unrelated mods; the offending mod is skipped and a `Problem` is reported:

| kind | meaning |
|---|---|
| `Invalid` | manifest error (bad id/version, unsafe path, missing entry script, API requirement not met) |
| `MissingDependency` / `DependencyVersion` | hard dependency absent, disabled or wrong version (cascades) |
| `Conflict` | two enabled mods declare a conflict; the lower-priority one is disabled |
| `Cycle` | dependency / `load_after` / `load_before` cycle; the mods on the cycle are disabled |
| `DuplicateId`, `DisabledByUser`, `OptionalDependencyVersion` | informational |

`asset_conflicts` / `ModManager::mount` report every VFS path provided by more than one mod (`AssetConflict { path, mods,
winner, overrides_game }`), which mod replaces base-game files, and how many files each mod provides. (Archives inside
`assets/`, e.g. a custom `Scenes/Foo/Foo.ZIP`, are mounted by the VFS; conflicts *inside* archives are not analysed.)

### Command line (`bm mods`)

`bm_script::cli::run_mods_command(&dirs, &args, game_dir)` implements the sub-commands for `bm-app`; the same is available now as
`cargo run -p bm-script --example mods -- <cmd>`:

```
list                installed mods, state, load order
order               resolved load order + problems
validate [path]     validate one folder/zip or everything installed (non-zero exit on errors)
conflicts           which mods override which VFS paths (and game files, if the game dir is known)
enable|disable <id> edit mods.toml
stubs <file>        write the LuaLS stubs
```

## Assets

Anything under the mod's `assets/` dir is addressed like a game file (case-insensitive, `/` or `\`).
Put `assets/Scenes/M01/M01_main.TEX` to replace a texture in the scene archive, or use the other crates' mod paths:
`Sounds/<bank>/<name>.ogg`, `Music/<level>/<cue>.ogg` + `*.ron`, `menu/<n>.xml|.add.xml`, `Fonts/*.fnt`,
`Localisation/<lang>.json|ron`, `UI/hud.ron`, `UI/Screens/*.ron`, `Config/input.ron`, `Shaders/WGSL/*.wgsl` (see `docs/sound.md`,
`docs/music.md`, `docs/ui.md`, `docs/render.md`). Never ship game files in a mod; ship your own content only.

## Data patches

Files `data/<dataset>.json|ron|toml` (sub-folders allowed: `data/npc/guard.toml` -> dataset `npc/guard`) are
**JSON Merge Patches (RFC 7396)** applied in load order on top of the base dataset registered by the game:

* objects merge recursively, `null` deletes a key, arrays/scalars replace;
* `"$replace": true` at the root replaces the whole dataset instead of merging;
* RON files must use map syntax, e.g. `{ "damage": 30, "tags": ["a"] }` or `(damage: 30)` named-field syntax (both parse to the same JSON value);
* `data/<dataset>.schema.json` ships a schema (JSON-Schema subset: `type`, `properties`, `required`, `additionalProperties`,
  `items`, `enum`, `const`, `minimum/maximum`, `exclusiveMin/Max`, `minLength/maxLength`, `minItems/maxItems`, `anyOf/oneOf/allOf`);
  the game's own schema (set with `DataStore::set_schema`) always wins;
* a patch that makes the merged result violate the schema is **rejected as a whole** and reported (`DataReport.errors`) - other patches still apply.

Lua reads the result with `data.get("weapons")` / `data.get("weapons", "ak47.damage")`. Data files are watched for hot reload
(`on_data_reload` is sent).

### How bm-game should consume data

```rust
let mut rt = ScriptRuntime::new(host, RuntimeConfig { save_dir: Some(user_dir.join("mod_saves")), ..Default::default() });
{
    let mut d = rt.data_mut();
    d.set_base("weapons", serde_json::to_value(&weapon_table)?);   // game defaults
    d.set_schema("weapons", schema_json);                           // optional validation
}
let (data_report, load_reports) = rt.load_mods(&resolved);          // applies patches, then runs scripts
let weapons: WeaponTable = rt.data().get_as("weapons").unwrap();    // typed access to the merged result
```

Re-read datasets whenever `poll_hot_reload()` reports `ReloadKind::Data` (or on `on_data_reload`). Base datasets should be built from the game's
real data (or your own defaults file), never hard-coded in mods.

## Scripting

Scripts are **Luau** (Lua 5.1-compatible with types/`continue`/compound assignment), embedded with `mlua` (vendored, builds with plain MSVC, no external toolchain).

### Sandbox and limits

* Each mod gets its **own Lua state** (own globals, own memory budget); mods can only talk through `events.emit` and `mods.*`.
* Only `coroutine`, `table`, `string`, `utf8`, `math`, `bit32` and the safe base functions are present. **No** `os`, `io`, `debug`, `package`, `loadstring`, `load`, `dofile`, `getfenv/setfenv`, `newproxy`. Standard library tables and string metatables are read-only. Bytecode chunks are refused (text only).
* `require("name")` / `require("a.b")` only loads `<scripts dir>/name.lua` or `name/init.lua` inside the mod; `mod.read_text/read_json` read files inside the mod folder only (no `..`, 4 MiB cap).
* **Time**: loading a mod gets 1 s, every handler/timer/command call 50 ms (wall clock, checked at calls and loop back-edges; `pcall` cannot swallow it for long). **Memory**: 64 MiB per mod. Defaults live in `Limits` and can be changed per runtime.
* A mod is **disabled** (others keep running) after 3 limit violations or 50 runtime errors; errors are logged through the host and kept in `ScriptRuntime::take_errors()`.
* Scripts are compiled at optimisation level 0 on purpose: Luau's import caching would otherwise serve stale `mod.state.x` / global-table reads.
* Numbers are doubles; entity ids are exact up to 2^53. Integral numbers coming back from Lua are normalised to JSON integers.

### Permissions

Always available: `log`, `events`, `timer`, `data`, `mod`, `mods`, `vec3`, `require`. Everything else needs a permission
declared in `mod.toml` (`world`, `entities.read`, `entities.write`, `audio`, `fx`, `ui`, `console`, `save`); calling an API without
it raises `permission denied: ... needs permission "x" in mod.toml`.

### Events

```lua
events.on("on_damage", function(ev)
  if ev.victim ~= world.player() then ev.amount = ev.amount * 1.5 end
end, { priority = 10 })      -- higher runs first; ties by load order
```

* Handlers receive a table; **modify its fields** to change the outcome (the host reads the final values from `DispatchOutcome.payload`).
* Cancelable events: set `ev.cancelled = true` or `return false`. Later handlers are skipped unless subscribed with `run_if_cancelled = true`; they see `ev.cancelled`.
* `once = true` unsubscribes after the first call; `events.off(handle)` removes a subscription.
* Custom events: `events.emit("my_mod.something", {..})` reaches all mods (and returns `{cancelled, payload}`); built-in names are reserved. Emit recursion is limited to 8 levels.
* `on_mod_load` / `on_mod_unload` go only to the mod itself.

### Persistence

* `save.get/set/delete/keys/flush` (permission `save`): JSON values, stored in `<save_dir>/<mod id>.json`, written at most every 5 game-seconds, on `flush()`, on unload and shutdown. 1 MiB cap per mod (an oversized save is refused and logged).
* `mod.state`: a table that **survives hot reload** (snapshot as JSON before the reload and restored before the entry scripts run again, `mod.reloaded == true`). Use `mod.state.x = mod.state.x or default`.

### Hot reload

`ScriptRuntime::poll_hot_reload()` (call it once per frame or on a timer) watches each mod's `mod.toml`, entry scripts and every `*.lua` under its scripts dir, plus all data files. On change the mod is unloaded (`on_mod_unload`, handlers/timers/commands removed, save flushed, `mod.state` captured), reloaded and `on_mod_load{reloaded=true}` is sent. cvar values are kept. If the new script fails to load, the mod stays unloaded, its state is kept, and loading is retried automatically as soon as a file changes again.

### Console

`console.register("name", "help", function(args) return "text" end)` adds a command (`args` is a list of strings, quotes respected);
`cvar.register("mod.speed", 1.0, "help")` adds a typed variable. The game console should call `rt.run_console(line)` first - it returns `None` when the line is not a mod command/cvar (`name` shows, `name value` sets) - and use `rt.console_entries()` for auto-complete.

## Integrating `bm-script` into bm-game / bm-app

`bm-script` depends only on `bm-vfs`; `bm-game` depends on it.

```rust
use bm_script::*;

// 1. discovery, order, VFS mounting (once at start-up, before opening scenes)
let dirs  = ModDirs::default_dirs();
let mgr   = ModManager::new(dirs.clone())?;
let set   = mgr.resolve();                       // log set.problems
let mount = mgr.mount(&mut vfs, &set)?;          // log mount.conflicts

// 2. host: implement ScriptHost for the game (all methods have defaults; &self + interior mutability)
struct GameHost { /* queues, world handle */ }
impl ScriptHost for GameHost { /* level_name, entity_info, spawn_entity, play_sound, ... */ }

// 3. runtime
let mut rt = ScriptRuntime::new(Rc::new(GameHost::new()), RuntimeConfig { save_dir: Some(dirs.save_dir.clone()), ..Default::default() });
rt.data_mut().set_base("weapons", ...);
let (_data, reports) = rt.load_mods(&set);

// 4. per frame / on game events
rt.update(dt, game_time);                        // timers + on_update + save flush
for r in rt.poll_hot_reload() { /* log */ }
let out = rt.dispatch(&Event::damage(victim.0, attacker.map(|a| a.0), amount, Some("Silverballer"), None));
if !out.cancelled { apply(out.payload["amount"].as_f64().unwrap()); }
```

Rules for host implementors: (1) `ScriptRuntime` is single-threaded (`Rc`, Luau); keep it on the game thread. (2) Do not call `dispatch` from inside a `ScriptHost` method;
queue the event instead (the Lua call stack is live). (3) Dispatch cancelable events *before* applying the action and honour `cancelled` and modified fields. (4) Raise `on_entity_spawn` / `on_damage` / `on_pickup` / `on_action` / `on_kill` / `on_mission_event` / `on_level_load` / `on_level_unload` / `on_update` at the matching points; they are the stable surface of API 1.x. `MockHost` implements the whole trait and records calls (`HostCall`), which is how the tests in `bm-script/tests/modding.rs` exercise scripts.

## Testing mods

`MockHost` + `ScriptRuntime` run headless: see `bm-script/tests/modding.rs` (sandbox escape attempts, infinite loops, memory bombs, hot reload, dependency cycles, conflicts, zips, data patches, the example mods).

# API reference (1.0.0)

Generated from `bm_script::spec` (`cargo run -p bm-script --example mods -- api-md`); a test checks that every function listed exists in Lua.
Vectors are plain tables `{x=, y=, z=}` (arrays `{x, y, z}` are accepted as arguments); `vec3.*` has helpers. Space is the engine's native one: left-handed, Y up, centimetres.
Tables passed to the API for options: `EventOptions{priority, once, run_if_cancelled}`, `EntityQuery{kind, name_contains, near, radius, alive_only, limit}`,
`SpawnOptions{name, pos, rot, model, components}`, `SoundOptions{pos, entity, volume, pitch, looping}`, `ParticleOptions{entity, scale, lifetime}`;
returned `EntityInfo{id, name, kind, position, rotation, model, alive}`, `RayHit{position, normal, distance, entity}`. `mod` holds `id, name, version, api_version, reloaded, state, read_text, read_json`.

### Events

Subscribe with `events.on("name", function(ev) ... end, {priority = 0})`. Fields of `ev` may be modified by handlers; the host reads the final values. Cancelable events honour `ev.cancelled = true` (or returning `false`).

#### `on_mod_load`

Sent only to the mod itself right after its entry scripts ran (also after a hot reload).

| field | type | description |
|---|---|---|
| `reloaded` | `boolean` | true when this is a hot reload |

#### `on_mod_unload`

Sent only to the mod itself before it is unloaded or hot-reloaded. Last chance to fill `mod.state` / `save`.

| field | type | description |
|---|---|---|
| `reloading` | `boolean` | true when followed by a reload |

#### `on_level_load`

A level (scene) finished loading.

| field | type | description |
|---|---|---|
| `level` | `string` | scene name, e.g. 'M01/M01_main' |

#### `on_level_unload`

The current level is being torn down.

| field | type | description |
|---|---|---|
| `level` | `string` | scene name |

#### `on_update`

Once per frame.

| field | type | description |
|---|---|---|
| `dt` | `number` | seconds since last frame |
| `time` | `number` | game time in seconds |

#### `on_entity_spawn` (cancelable)

An entity is about to be created (cancel to prevent it).

| field | type | description |
|---|---|---|
| `entity` | `integer?` | entity id (nil if not created yet) |
| `template` | `string` | template / archetype name |
| `kind` | `string` | entity kind |
| `name` | `string?` | entity name |
| `position` | `Vec3` | spawn position |

#### `on_entity_despawn`

An entity was removed.

| field | type | description |
|---|---|---|
| `entity` | `integer` | entity id |

#### `on_damage` (cancelable)

An entity is about to take damage. `amount` may be modified; cancel to make it invulnerable to this hit.

| field | type | description |
|---|---|---|
| `victim` | `integer` | entity taking damage |
| `attacker` | `integer?` | entity dealing damage |
| `amount` | `number` | damage (modifiable) |
| `weapon` | `string?` | weapon / damage source name |
| `zone` | `string?` | hit zone, e.g. 'head' |

#### `on_kill`

An entity died.

| field | type | description |
|---|---|---|
| `victim` | `integer` | entity that died |
| `killer` | `integer?` | entity that killed it |
| `weapon` | `string?` | weapon / cause |
| `accident` | `boolean?` | true for staged accidents |

#### `on_pickup` (cancelable)

An entity is about to pick up an item.

| field | type | description |
|---|---|---|
| `entity` | `integer` | the picker |
| `item` | `string` | item / weapon name |
| `item_entity` | `integer?` | world entity of the item |

#### `on_action` (cancelable)

An entity is about to perform a context action (open door, use item, ...).

| field | type | description |
|---|---|---|
| `actor` | `integer` | who acts |
| `action` | `string` | action name |
| `target` | `integer?` | target entity |

#### `on_mission_event`

Mission logic raised an event (objective complete, target killed, alarm, ...).

| field | type | description |
|---|---|---|
| `name` | `string` | event name |
| `data` | `table?` | event specific data |

#### `on_cvar_changed`

A console variable changed.

| field | type | description |
|---|---|---|
| `name` | `string` | cvar |
| `value` | `any` | new value |
| `old` | `any` | previous value |

#### `on_data_reload`

Data patches were re-applied (hot reload). Re-read `data.get(..)`.

#### `on_shutdown`

The game is exiting.

### Functions

#### `log`

- `log.debug(...: any)` - Log at debug level.
- `log.info(...: any)` - Log at info level (arguments are tostring'ed and joined by spaces).
- `log.warn(...: any)` - Log at warning level.
- `log.error(...: any)` - Log at error level.

#### `events`

- `events.on(event: string, handler: fun(ev: table), opts?: EventOptions) -> integer` - Subscribe to a built-in or custom event. Higher `priority` runs first. Returns a handle for `events.off`.
- `events.off(handle: integer)` - Remove a subscription.
- `events.emit(event: string, payload?: table) -> EmitResult` - Raise a custom event for all mods (including this one). Built-in event names are reserved. Returns `{cancelled, payload}`.

#### `timer`

- `timer.after(seconds: number, fn: fun()) -> integer` - Call `fn` once after `seconds` of game time. Returns a timer id.
- `timer.every(seconds: number, fn: fun()) -> integer` - Call `fn` repeatedly every `seconds`. Returns a timer id.
- `timer.cancel(id: integer)` - Cancel a timer.

#### `mod`

- `mod.read_text(path: string) -> string?` - Read a text file from inside the mod folder (no `..`). Returns nil if missing.
- `mod.read_json(path: string) -> any` - Read and parse a JSON file from inside the mod folder. Returns nil if missing.

#### `mods`

- `mods.is_loaded(id: string) -> boolean` - Is another mod loaded (and not faulted)?
- `mods.version(id: string) -> string?` - Version string of a loaded mod.
- `mods.list() -> string[]` - Ids of all loaded mods in load order.

#### `data`

- `data.get(dataset: string, path?: string) -> any` - Merged gameplay dataset (base + all mod patches), optionally a dotted sub-path such as 'ak47.damage'. Returns a fresh copy.
- `data.names() -> string[]` - Names of all datasets.

#### `save` (permission `save`)

- `save.get(key: string, default?: any) -> any` - Read a persistent per-mod value.
- `save.set(key: string, value: any)` - Write a persistent per-mod value (JSON-compatible).
- `save.delete(key: string)` - Remove a key.
- `save.keys() -> string[]` - All keys.
- `save.flush()` - Write the save file now (otherwise written periodically and at unload).

#### `world` (permission `world`)

- `world.level() -> string?` - Name of the loaded level.
- `world.time() -> number` - Game time in seconds.
- `world.player() -> integer?` - Entity id of the player (Agent 47).
- `world.raycast(from: Vec3, to: Vec3) -> RayHit?` - Cast a ray; returns the first hit or nil.

#### `entity` (permission `entities.read`)

- `entity.info(id: integer) -> EntityInfo?` - Snapshot of an entity (nil if it does not exist).
- `entity.exists(id: integer) -> boolean` - Does the entity exist?
- `entity.find(name: string) -> integer?` - Find an entity by exact (case-insensitive) name.
- `entity.query(q?: EntityQuery) -> EntityInfo[]` - List entities matching kind / name / radius filters.
- `entity.get_component(id: integer, component: string) -> any` - Read a component value (JSON-like).
- `entity.spawn(template: string, opts?: SpawnOptions) -> integer` - Create an entity from a template. Returns its id.
- `entity.despawn(id: integer)` - Remove an entity.
- `entity.set_position(id: integer, pos: Vec3)` - Teleport an entity.
- `entity.set_rotation(id: integer, euler_deg: Vec3)` - Set Euler rotation in degrees.
- `entity.set_model(id: integer, model: string)` - Swap the visual model.
- `entity.set_component(id: integer, component: string, value: any)` - Write a component value.
- `entity.damage(id: integer, amount: number, source?: integer)` - Apply damage (goes through the normal damage pipeline and `on_damage`).
- `entity.kill(id: integer, source?: integer)` - Kill an entity.

#### `audio` (permission `audio`)

- `audio.play(name: string, opts?: SoundOptions) -> integer` - Play a sound (bank name, `#sound` id or mod file). Returns a handle.
- `audio.stop(handle: integer)` - Stop a sound.
- `audio.music(cue: string)` - Start a music cue.
- `audio.music_state(state: string)` - Set the dynamic-music state.
- `audio.music_event(name: string)` - Send a mission event to the music system.

#### `fx` (permission `fx`)

- `fx.particles(effect: string, pos: Vec3, opts?: ParticleOptions) -> integer` - Spawn a particle effect.

#### `ui` (permission `ui`)

- `ui.show_screen(name: string)` - Push a UI screen.
- `ui.hide_screen(name: string)` - Pop / hide a UI screen.
- `ui.set_hud_var(name: string, value: any)` - Set a HUD variable.
- `ui.message(text: string, seconds?: number)` - Show a transient message line (default 3 s).

#### `console` (permission `console`)

- `console.register(name: string, help: string, handler: fun(args: string[]): string?)` - Register a console command. The return value is printed.
- `console.print(text: string)` - Print a line to the console.
- `console.exec(line: string) -> string` - Run a built-in console command line.

#### `cvar` (permission `console`)

- `cvar.register(name: string, default: boolean|number|string, help?: string)` - Register a console variable (type is fixed by the default).
- `cvar.get(name: string) -> boolean|number|string|nil` - Read a cvar.
- `cvar.set(name: string, value: boolean|number|string)` - Set a cvar (fires `on_cvar_changed`).

#### `vec3`

- `vec3.new(x: number, y: number, z: number) -> Vec3` - Construct a vector table.
- `vec3.add(a: Vec3, b: Vec3) -> Vec3` - a + b
- `vec3.sub(a: Vec3, b: Vec3) -> Vec3` - a - b
- `vec3.scale(a: Vec3, s: number) -> Vec3` - a * s
- `vec3.dot(a: Vec3, b: Vec3) -> number` - Dot product.
- `vec3.length(a: Vec3) -> number` - Euclidean length.
- `vec3.distance(a: Vec3, b: Vec3) -> number` - Distance between points.
- `vec3.lerp(a: Vec3, b: Vec3, t: number) -> Vec3` - Linear interpolation.

### Permissions

Declare in `mod.toml`: `permissions = [..]`. Known: `world`, `entities.read`, `entities.write`, `audio`, `fx`, `ui`, `console`, `save`.
