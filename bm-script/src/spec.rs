//! Machine-readable description of the Lua API. It is the single source for the LuaLS type stubs
//! (`lua_stubs`) and the API reference in `docs/modding.md` (`api_markdown`); a test verifies that
//! every entry exists in a live Lua state, so docs cannot drift from the bindings.

use crate::events::BUILTIN_EVENTS;
use crate::manifest::PERMISSIONS;
use std::fmt::Write;

pub struct FnSpec {
    pub module: &'static str,
    pub name: &'static str,
    /// `(name, LuaLS type)`; a trailing `?` on the name marks an optional parameter.
    pub params: &'static [(&'static str, &'static str)],
    pub ret: &'static str,
    /// Permission needed (`""` = always available).
    pub perm: &'static str,
    pub doc: &'static str,
}

const fn f(
    module: &'static str,
    name: &'static str,
    params: &'static [(&'static str, &'static str)],
    ret: &'static str,
    perm: &'static str,
    doc: &'static str,
) -> FnSpec {
    FnSpec { module, name, params, ret, perm, doc }
}

pub const API_FUNCTIONS: &[FnSpec] = &[
    f("log", "debug", &[("...", "any")], "", "", "Log at debug level."),
    f("log", "info", &[("...", "any")], "", "", "Log at info level (arguments are tostring'ed and joined by spaces)."),
    f("log", "warn", &[("...", "any")], "", "", "Log at warning level."),
    f("log", "error", &[("...", "any")], "", "", "Log at error level."),
    f("events", "on", &[("event", "string"), ("handler", "fun(ev: table)"), ("opts?", "EventOptions")], "integer", "",
      "Subscribe to a built-in or custom event. Higher `priority` runs first. Returns a handle for `events.off`."),
    f("events", "off", &[("handle", "integer")], "", "", "Remove a subscription."),
    f("events", "emit", &[("event", "string"), ("payload?", "table")], "EmitResult", "",
      "Raise a custom event for all mods (including this one). Built-in event names are reserved. Returns `{cancelled, payload}`."),
    f("timer", "after", &[("seconds", "number"), ("fn", "fun()")], "integer", "", "Call `fn` once after `seconds` of game time. Returns a timer id."),
    f("timer", "every", &[("seconds", "number"), ("fn", "fun()")], "integer", "", "Call `fn` repeatedly every `seconds`. Returns a timer id."),
    f("timer", "cancel", &[("id", "integer")], "", "", "Cancel a timer."),
    f("mod", "read_text", &[("path", "string")], "string?", "", "Read a text file from inside the mod folder (no `..`). Returns nil if missing."),
    f("mod", "read_json", &[("path", "string")], "any", "", "Read and parse a JSON file from inside the mod folder. Returns nil if missing."),
    f("mods", "is_loaded", &[("id", "string")], "boolean", "", "Is another mod loaded (and not faulted)?"),
    f("mods", "version", &[("id", "string")], "string?", "", "Version string of a loaded mod."),
    f("mods", "list", &[], "string[]", "", "Ids of all loaded mods in load order."),
    f("data", "get", &[("dataset", "string"), ("path?", "string")], "any", "",
      "Merged gameplay dataset (base + all mod patches), optionally a dotted sub-path such as 'ak47.damage'. Returns a fresh copy."),
    f("data", "names", &[], "string[]", "", "Names of all datasets."),
    f("save", "get", &[("key", "string"), ("default?", "any")], "any", "save", "Read a persistent per-mod value."),
    f("save", "set", &[("key", "string"), ("value", "any")], "", "save", "Write a persistent per-mod value (JSON-compatible)."),
    f("save", "delete", &[("key", "string")], "", "save", "Remove a key."),
    f("save", "keys", &[], "string[]", "save", "All keys."),
    f("save", "flush", &[], "", "save", "Write the save file now (otherwise written periodically and at unload)."),
    f("world", "level", &[], "string?", "world", "Name of the loaded level."),
    f("world", "time", &[], "number", "world", "Game time in seconds."),
    f("world", "player", &[], "integer?", "world", "Entity id of the player (Agent 47)."),
    f("world", "raycast", &[("from", "Vec3"), ("to", "Vec3")], "RayHit?", "world", "Cast a ray; returns the first hit or nil."),
    f("entity", "info", &[("id", "integer")], "EntityInfo?", "entities.read", "Snapshot of an entity (nil if it does not exist)."),
    f("entity", "exists", &[("id", "integer")], "boolean", "entities.read", "Does the entity exist?"),
    f("entity", "find", &[("name", "string")], "integer?", "entities.read", "Find an entity by exact (case-insensitive) name."),
    f("entity", "query", &[("q?", "EntityQuery")], "EntityInfo[]", "entities.read", "List entities matching kind / name / radius filters."),
    f("entity", "get_component", &[("id", "integer"), ("component", "string")], "any", "entities.read", "Read a component value (JSON-like)."),
    f("entity", "spawn", &[("template", "string"), ("opts?", "SpawnOptions")], "integer", "entities.write", "Create an entity from a template. Returns its id."),
    f("entity", "despawn", &[("id", "integer")], "", "entities.write", "Remove an entity."),
    f("entity", "set_position", &[("id", "integer"), ("pos", "Vec3")], "", "entities.write", "Teleport an entity."),
    f("entity", "set_rotation", &[("id", "integer"), ("euler_deg", "Vec3")], "", "entities.write", "Set Euler rotation in degrees."),
    f("entity", "set_model", &[("id", "integer"), ("model", "string")], "", "entities.write", "Swap the visual model."),
    f("entity", "set_component", &[("id", "integer"), ("component", "string"), ("value", "any")], "", "entities.write", "Write a component value."),
    f("entity", "damage", &[("id", "integer"), ("amount", "number"), ("source?", "integer")], "", "entities.write", "Apply damage (goes through the normal damage pipeline and `on_damage`)."),
    f("entity", "kill", &[("id", "integer"), ("source?", "integer")], "", "entities.write", "Kill an entity."),
    f("audio", "play", &[("name", "string"), ("opts?", "SoundOptions")], "integer", "audio", "Play a sound (bank name, `#sound` id or mod file). Returns a handle."),
    f("audio", "stop", &[("handle", "integer")], "", "audio", "Stop a sound."),
    f("audio", "music", &[("cue", "string")], "", "audio", "Start a music cue."),
    f("audio", "music_state", &[("state", "string")], "", "audio", "Set the dynamic-music state."),
    f("audio", "music_event", &[("name", "string")], "", "audio", "Send a mission event to the music system."),
    f("fx", "particles", &[("effect", "string"), ("pos", "Vec3"), ("opts?", "ParticleOptions")], "integer", "fx", "Spawn a particle effect."),
    f("ui", "show_screen", &[("name", "string")], "", "ui", "Push a UI screen."),
    f("ui", "hide_screen", &[("name", "string")], "", "ui", "Pop / hide a UI screen."),
    f("ui", "set_hud_var", &[("name", "string"), ("value", "any")], "", "ui", "Set a HUD variable."),
    f("ui", "message", &[("text", "string"), ("seconds?", "number")], "", "ui", "Show a transient message line (default 3 s)."),
    f("console", "register", &[("name", "string"), ("help", "string"), ("handler", "fun(args: string[]): string?")], "", "console", "Register a console command. The return value is printed."),
    f("console", "print", &[("text", "string")], "", "console", "Print a line to the console."),
    f("console", "exec", &[("line", "string")], "string", "console", "Run a built-in console command line."),
    f("cvar", "register", &[("name", "string"), ("default", "boolean|number|string"), ("help?", "string")], "", "console", "Register a console variable (type is fixed by the default)."),
    f("cvar", "get", &[("name", "string")], "boolean|number|string|nil", "console", "Read a cvar."),
    f("cvar", "set", &[("name", "string"), ("value", "boolean|number|string")], "", "console", "Set a cvar (fires `on_cvar_changed`)."),
    f("vec3", "new", &[("x", "number"), ("y", "number"), ("z", "number")], "Vec3", "", "Construct a vector table."),
    f("vec3", "add", &[("a", "Vec3"), ("b", "Vec3")], "Vec3", "", "a + b"),
    f("vec3", "sub", &[("a", "Vec3"), ("b", "Vec3")], "Vec3", "", "a - b"),
    f("vec3", "scale", &[("a", "Vec3"), ("s", "number")], "Vec3", "", "a * s"),
    f("vec3", "dot", &[("a", "Vec3"), ("b", "Vec3")], "number", "", "Dot product."),
    f("vec3", "length", &[("a", "Vec3")], "number", "", "Euclidean length."),
    f("vec3", "distance", &[("a", "Vec3"), ("b", "Vec3")], "number", "", "Distance between points."),
    f("vec3", "lerp", &[("a", "Vec3"), ("b", "Vec3"), ("t", "number")], "Vec3", "", "Linear interpolation."),
];

const STUB_CLASSES: &str = r#"---@meta
-- Blood Money (Rust) modding API type stubs for LuaLS. Generated by bm-script; do not edit.

---@class Vec3
---@field x number
---@field y number
---@field z number

---@class EventOptions
---@field priority? integer       -- higher runs first (default 0)
---@field once? boolean           -- auto-unsubscribe after the first call
---@field run_if_cancelled? boolean -- still run when an earlier handler cancelled the event

---@class EmitResult
---@field cancelled boolean
---@field payload table

---@class EntityInfo
---@field id integer
---@field name string
---@field kind string
---@field position Vec3
---@field rotation Vec3
---@field model string?
---@field alive boolean

---@class EntityQuery
---@field kind? string
---@field name_contains? string
---@field near? Vec3
---@field radius? number
---@field alive_only? boolean
---@field limit? integer

---@class SpawnOptions
---@field name? string
---@field pos? Vec3
---@field rot? Vec3
---@field model? string
---@field components? table<string, any>

---@class RayHit
---@field position Vec3
---@field normal Vec3
---@field distance number
---@field entity integer?

---@class SoundOptions
---@field pos? Vec3
---@field entity? integer
---@field volume? number
---@field pitch? number
---@field looping? boolean

---@class ParticleOptions
---@field entity? integer
---@field scale? number
---@field lifetime? number

---@class ModInfo
---@field id string
---@field name string
---@field version string
---@field api_version string
---@field reloaded boolean  -- true after a hot reload
---@field state table       -- survives hot reload (JSON-compatible values only)
---@field read_text fun(path: string): string?
---@field read_json fun(path: string): any
---@field read_text fun(path: string): string?
---@field read_json fun(path: string): any

--- Information about the running mod.
---@type ModInfo
mod = nil

"#;

fn lua_type_name(event: &str) -> String {
    let mut s = String::new();
    for part in event.split('_') {
        let mut c = part.chars();
        if let Some(f) = c.next() {
            s.push(f.to_ascii_uppercase());
            s.push_str(c.as_str());
        }
    }
    format!("{s}Event")
}

/// Generate the LuaLS `.d.lua` stub file.
pub fn lua_stubs() -> String {
    let mut o = String::from(STUB_CLASSES);
    writeln!(o, "---@alias ApiVersion \"{}\"\n", crate::API_VERSION).ok();
    for e in BUILTIN_EVENTS {
        writeln!(o, "---{}{}", e.doc, if e.cancelable { " (cancelable)" } else { "" }).ok();
        writeln!(o, "---@class {}", lua_type_name(e.name)).ok();
        if e.cancelable {
            writeln!(o, "---@field cancelled boolean? -- set to true to cancel").ok();
        }
        for fld in e.fields {
            let (ty, opt) = match fld.ty.strip_suffix('?') {
                Some(t) => (t, "?"),
                None => (fld.ty, ""),
            };
            writeln!(o, "---@field {}{} {} -- {}", fld.name, opt, ty, fld.doc).ok();
        }
        o.push('\n');
    }
    let mut modules: Vec<&str> = Vec::new();
    for fs in API_FUNCTIONS {
        if !modules.contains(&fs.module) {
            modules.push(fs.module);
        }
    }
    for m in modules {
        if m == "mod" {
            continue; // declared as fields of ModInfo
        }
        writeln!(o, "---@class {}Api", m).ok();
        writeln!(o, "{} = {{}}\n", m).ok();
        for fs in API_FUNCTIONS.iter().filter(|x| x.module == m) {
            writeln!(o, "--- {}", fs.doc).ok();
            if !fs.perm.is_empty() {
                writeln!(o, "--- Requires permission `{}`.", fs.perm).ok();
            }
            if fs.module == "events" && fs.name == "on" {
                for e in BUILTIN_EVENTS {
                    writeln!(
                        o,
                        "---@overload fun(event: \"{}\", handler: fun(ev: {}), opts?: EventOptions): integer",
                        e.name,
                        lua_type_name(e.name)
                    )
                    .ok();
                }
            }
            for (n, t) in fs.params {
                if *n == "..." {
                    writeln!(o, "---@param ... {t}").ok();
                } else {
                    writeln!(o, "---@param {n} {t}").ok();
                }
            }
            if !fs.ret.is_empty() {
                writeln!(o, "---@return {}", fs.ret).ok();
            }
            let pnames: Vec<String> = fs.params.iter().map(|(n, _)| n.trim_end_matches('?').to_string()).collect();
            writeln!(o, "function {}.{}({}) end\n", m, fs.name, pnames.join(", ")).ok();
        }
    }
    o
}

/// Generate the API reference as Markdown (embedded in `docs/modding.md`).
pub fn api_markdown() -> String {
    let mut o = String::new();
    writeln!(o, "### Events\n").ok();
    writeln!(o, "Subscribe with `events.on(\"name\", function(ev) ... end, {{priority = 0}})`. Fields of `ev` may be modified by handlers; the host reads the final values. Cancelable events honour `ev.cancelled = true` (or returning `false`).\n").ok();
    for e in BUILTIN_EVENTS {
        writeln!(o, "#### `{}`{}\n", e.name, if e.cancelable { " (cancelable)" } else { "" }).ok();
        writeln!(o, "{}\n", e.doc).ok();
        if !e.fields.is_empty() {
            writeln!(o, "| field | type | description |\n|---|---|---|").ok();
            for fld in e.fields {
                writeln!(o, "| `{}` | `{}` | {} |", fld.name, fld.ty, fld.doc).ok();
            }
            o.push('\n');
        }
    }
    writeln!(o, "### Functions\n").ok();
    let mut modules: Vec<&str> = Vec::new();
    for fs in API_FUNCTIONS {
        if !modules.contains(&fs.module) {
            modules.push(fs.module);
        }
    }
    for m in modules {
        let perm = API_FUNCTIONS.iter().find(|x| x.module == m).map(|x| x.perm).unwrap_or("");
        writeln!(
            o,
            "#### `{}`{}\n",
            m,
            if perm.is_empty() { String::new() } else { format!(" (permission `{perm}`)") }
        )
        .ok();
        for fs in API_FUNCTIONS.iter().filter(|x| x.module == m) {
            let ps: Vec<String> = fs.params.iter().map(|(n, t)| format!("{n}: {t}")).collect();
            let ret = if fs.ret.is_empty() { String::new() } else { format!(" -> {}", fs.ret) };
            writeln!(o, "- `{}.{}({}){}` - {}", fs.module, fs.name, ps.join(", "), ret, fs.doc).ok();
        }
        o.push('\n');
    }
    writeln!(o, "### Permissions\n").ok();
    writeln!(o, "Declare in `mod.toml`: `permissions = [..]`. Known: {}.", PERMISSIONS.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ")).ok();
    o
}
