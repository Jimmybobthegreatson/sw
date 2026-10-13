//! Lua bindings: installs the sandboxed API (see `spec.rs`) into a mod's Lua state.

use crate::events::builtin_event;
use crate::host::*;
use crate::manifest::safe_rel;
use crate::runtime::{CommandEntry, Cvar, HandlerEntry, ModCtx, Shared, Timer, normalize_json};
use mlua::{DeserializeOptions, Function, Lua, LuaSerdeExt, MultiValue, SerializeOptions, Table, Value as LuaValue};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

type LuaResult<T> = mlua::Result<T>;

fn rt_err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

fn host_err(e: HostError) -> mlua::Error {
    rt_err(format!("host error: {e}"))
}

pub(crate) fn to_lua<T: serde::Serialize + ?Sized>(lua: &Lua, t: &T) -> LuaResult<LuaValue> {
    lua.to_value_with(t, SerializeOptions::new().serialize_none_to_null(false).serialize_unit_to_null(false))
}

pub(crate) fn from_lua<T: DeserializeOwned>(lua: &Lua, v: LuaValue) -> LuaResult<T> {
    lua.from_value_with(v, DeserializeOptions::new().deny_unsupported_types(false))
}

fn json_from_lua(lua: &Lua, v: LuaValue) -> LuaResult<Value> {
    let mut j: Value = from_lua(lua, v)?;
    normalize_json(&mut j);
    Ok(j)
}

/// `{x=,y=,z=}` or `{x,y,z}`.
#[derive(Deserialize)]
#[serde(untagged)]
enum V3 {
    Obj {
        #[serde(default)]
        x: f32,
        #[serde(default)]
        y: f32,
        #[serde(default)]
        z: f32,
    },
    Arr([f32; 3]),
}

impl From<V3> for Vec3 {
    fn from(v: V3) -> Vec3 {
        match v {
            V3::Obj { x, y, z } => Vec3 { x, y, z },
            V3::Arr([x, y, z]) => Vec3 { x, y, z },
        }
    }
}

#[derive(Deserialize, Default)]
struct SpawnOpts {
    name: Option<String>,
    pos: Option<V3>,
    rot: Option<V3>,
    model: Option<String>,
    components: Option<serde_json::Map<String, Value>>,
}

#[derive(Deserialize, Default)]
struct QueryOpts {
    kind: Option<String>,
    name_contains: Option<String>,
    near: Option<V3>,
    radius: Option<f32>,
    #[serde(default)]
    alive_only: bool,
    limit: Option<usize>,
}

#[derive(Deserialize, Default)]
struct SoundOpts {
    pos: Option<V3>,
    entity: Option<u64>,
    volume: Option<f32>,
    pitch: Option<f32>,
    #[serde(default)]
    looping: bool,
}

#[derive(Deserialize, Default)]
struct ParticleOpts {
    entity: Option<u64>,
    scale: Option<f32>,
    lifetime: Option<f32>,
}

const PRELUDE: &str = r#"
local function new(x, y, z) return { x = x or 0, y = y or 0, z = z or 0 } end
local M = {}
M.new = new
function M.add(a, b) return new(a.x + b.x, a.y + b.y, a.z + b.z) end
function M.sub(a, b) return new(a.x - b.x, a.y - b.y, a.z - b.z) end
function M.scale(a, s) return new(a.x * s, a.y * s, a.z * s) end
function M.dot(a, b) return a.x * b.x + a.y * b.y + a.z * b.z end
function M.length(a) return math.sqrt(a.x * a.x + a.y * a.y + a.z * a.z) end
function M.distance(a, b) return M.length(M.sub(a, b)) end
function M.lerp(a, b, t) return new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t) end
vec3 = M
"#;

const MAX_HANDLERS_PER_MOD: usize = 2000;
const MAX_TIMERS_PER_MOD: usize = 2000;
const MAX_READ_BYTES: u64 = 4 << 20;

/// Register `f` as `table[name]`; if the mod lacks `perm`, register a function that raises
/// "permission denied" instead (so the error message is actionable).
fn reg<A, R, F>(lua: &Lua, ctx: &ModCtx, t: &Table, module: &str, name: &str, perm: &str, f: F) -> LuaResult<()>
where
    A: mlua::FromLuaMulti,
    R: mlua::IntoLuaMulti,
    F: Fn(&Lua, A) -> LuaResult<R> + 'static,
{
    let func = if ctx.has_perm(perm) {
        lua.create_function(f)?
    } else {
        let msg = format!(
            "permission denied: {module}.{name} needs permission \"{perm}\" in mod.toml (mod '{}')",
            ctx.id
        );
        lua.create_function(move |_, _: MultiValue| -> LuaResult<()> { Err(rt_err(msg.clone())) })?
    };
    t.set(name, func)
}

fn join_args(args: MultiValue) -> String {
    args.iter()
        .map(|v| match v {
            LuaValue::String(s) => s.to_string_lossy(),
            LuaValue::Nil => "nil".to_string(),
            other => other.to_string().unwrap_or_else(|_| format!("<{}>", other.type_name())),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.' || b == b'-')
}

pub(crate) fn cvar_set(sh: &Shared, name: &str, value: Value) -> Result<(), String> {
    let old = {
        let mut cv = sh.cvars.borrow_mut();
        let c = cv.get_mut(name).ok_or_else(|| format!("unknown cvar {name:?}"))?;
        let ok = matches!(
            (&c.value, &value),
            (Value::Bool(_), Value::Bool(_)) | (Value::Number(_), Value::Number(_)) | (Value::String(_), Value::String(_))
        );
        if !ok {
            return Err(format!("cvar {name:?} expects {}", type_name(&c.value)));
        }
        std::mem::replace(&mut c.value, value.clone())
    };
    if old != value {
        sh.dispatch("on_cvar_changed", serde_json::json!({ "name": name, "value": value, "old": old }), None);
    }
    Ok(())
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        _ => "a scalar",
    }
}

/// Build the whole API into `lua`'s globals. Must run before `lua.sandbox(true)`.
pub(crate) fn install(
    lua: &Lua,
    sh: &Rc<Shared>,
    ctx: &Rc<ModCtx>,
    restored_state: Option<&Value>,
    reloaded: bool,
) -> LuaResult<()> {
    let g = lua.globals();
    for n in [
        "loadstring", "load", "dofile", "loadfile", "getfenv", "setfenv", "debug", "os", "io", "package", "require",
        "newproxy", "gcinfo",
    ] {
        g.set(n, LuaValue::Nil)?;
    }
    lua.load(PRELUDE).set_name("=prelude").exec()?;

    // ---- log ----
    let log = lua.create_table()?;
    for (name, level) in [("debug", LogLevel::Debug), ("info", LogLevel::Info), ("warn", LogLevel::Warn), ("error", LogLevel::Error)] {
        let (sh2, id) = (sh.clone(), ctx.id.clone());
        log.set(
            name,
            lua.create_function(move |_, args: MultiValue| {
                sh2.host.log(&id, level, &join_args(args));
                Ok(())
            })?,
        )?;
    }
    g.set("log", log)?;
    {
        let (sh2, id) = (sh.clone(), ctx.id.clone());
        g.set(
            "print",
            lua.create_function(move |_, args: MultiValue| {
                sh2.host.log(&id, LogLevel::Info, &join_args(args));
                Ok(())
            })?,
        )?;
    }

    // ---- mod ----
    let modt = lua.create_table()?;
    modt.set("id", ctx.id.as_str())?;
    modt.set("name", ctx.manifest.name.as_str())?;
    modt.set("version", ctx.manifest.version.as_str())?;
    modt.set("api_version", crate::API_VERSION)?;
    modt.set("reloaded", reloaded)?;
    let state_tbl = match restored_state {
        Some(s) if s.is_object() => match to_lua(lua, s)? {
            LuaValue::Table(t) => t,
            _ => lua.create_table()?,
        },
        _ => lua.create_table()?,
    };
    modt.set("state", state_tbl)?;
    let read_file = {
        let root = ctx.root.clone();
        move |rel: &str| -> LuaResult<Option<String>> {
            if !safe_rel(rel) {
                return Err(rt_err(format!("invalid path {rel:?}: must be relative and inside the mod")));
            }
            let p = root.join(rel);
            match std::fs::metadata(&p) {
                Ok(m) if m.is_file() && m.len() <= MAX_READ_BYTES => Ok(std::fs::read_to_string(&p).ok()),
                Ok(m) if m.is_file() => Err(rt_err("file too large")),
                _ => Ok(None),
            }
        }
    };
    {
        let rf = read_file.clone();
        reg(lua, ctx, &modt, "mod", "read_text", "", move |_, p: String| rf(&p))?;
        reg(lua, ctx, &modt, "mod", "read_json", "", move |lua, p: String| match read_file(&p)? {
            Some(t) => {
                let v: Value = serde_json::from_str(&t).map_err(|e| rt_err(format!("{p}: {e}")))?;
                to_lua(lua, &v)
            }
            None => Ok(LuaValue::Nil),
        })?;
    }
    g.set("mod", modt)?;

    // ---- mods ----
    let mods = lua.create_table()?;
    {
        let s = sh.clone();
        reg(lua, ctx, &mods, "mods", "is_loaded", "", move |_, id: String| {
            Ok(s.loaded.borrow().get(&id).is_some_and(|l| !l.ctx.disabled.get()))
        })?;
        let s = sh.clone();
        reg(lua, ctx, &mods, "mods", "version", "", move |_, id: String| {
            Ok(s.loaded.borrow().get(&id).map(|l| l.version.clone()))
        })?;
        let s = sh.clone();
        reg(lua, ctx, &mods, "mods", "list", "", move |_, ()| {
            let l = s.loaded.borrow();
            let mut v: Vec<_> = l.iter().filter(|(_, i)| !i.ctx.disabled.get()).map(|(k, i)| (i.ctx.order, k.clone())).collect();
            v.sort();
            Ok(v.into_iter().map(|x| x.1).collect::<Vec<_>>())
        })?;
    }
    g.set("mods", mods)?;

    // ---- events ----
    let events = lua.create_table()?;
    {
        let (s, c) = (sh.clone(), ctx.clone());
        reg(lua, ctx, &events, "events", "on", "", move |lua, (event, func, opts): (String, Function, Option<Table>)| {
            if s.handlers.borrow().iter().filter(|h| h.gen_id == c.gen_id).count() >= MAX_HANDLERS_PER_MOD {
                return Err(rt_err("too many event handlers"));
            }
            let (mut priority, mut once, mut ric) = (0i32, false, false);
            if let Some(o) = opts {
                priority = o.get::<Option<i32>>("priority")?.unwrap_or(0);
                once = o.get::<Option<bool>>("once")?.unwrap_or(false);
                ric = o.get::<Option<bool>>("run_if_cancelled")?.unwrap_or(false);
            }
            let id = s.new_id();
            s.handlers.borrow_mut().push(HandlerEntry {
                id,
                gen_id: c.gen_id,
                order: c.order,
                event,
                priority,
                once,
                run_if_cancelled: ric,
                func,
                lua: lua.clone(),
                ctx: c.clone(),
            });
            Ok(id)
        })?;
        let (s, c) = (sh.clone(), ctx.clone());
        reg(lua, ctx, &events, "events", "off", "", move |_, id: u64| {
            s.handlers.borrow_mut().retain(|h| !(h.id == id && h.gen_id == c.gen_id));
            Ok(())
        })?;
        let s = sh.clone();
        reg(lua, ctx, &events, "events", "emit", "", move |lua, (name, payload): (String, Option<LuaValue>)| {
            if builtin_event(&name).is_some() {
                return Err(rt_err(format!("event {name:?} is reserved for the engine")));
            }
            let mut p = match payload {
                Some(v @ LuaValue::Table(_)) => json_from_lua(lua, v)?,
                None | Some(LuaValue::Nil) => serde_json::json!({}),
                Some(_) => return Err(rt_err("events.emit payload must be a table")),
            };
            if !p.is_object() {
                p = serde_json::json!({});
            }
            let out = s.dispatch(&name, p, None);
            let t = lua.create_table()?;
            t.set("cancelled", out.cancelled)?;
            t.set("payload", to_lua(lua, &out.payload)?)?;
            Ok(t)
        })?;
    }
    g.set("events", events)?;

    // ---- timer ----
    let timer = lua.create_table()?;
    for (name, every) in [("after", false), ("every", true)] {
        let (s, c) = (sh.clone(), ctx.clone());
        reg(lua, ctx, &timer, "timer", name, "", move |_, (secs, func): (f64, Function)| {
            if !secs.is_finite() || secs < 0.0 || (every && secs < 0.001) {
                return Err(rt_err("invalid timer interval"));
            }
            if c.timers.borrow().len() >= MAX_TIMERS_PER_MOD {
                return Err(rt_err("too many timers"));
            }
            let id = s.new_id();
            let base = if c.now.get() > 0.0 { c.now.get() } else { s.host.game_time() };
            c.timers.borrow_mut().push(Timer { id, due: base + secs, interval: every.then_some(secs), func });
            Ok(id)
        })?;
    }
    {
        let c = ctx.clone();
        reg(lua, ctx, &timer, "timer", "cancel", "", move |_, id: u64| {
            c.timers.borrow_mut().retain(|t| t.id != id);
            Ok(())
        })?;
    }
    g.set("timer", timer)?;

    // ---- data ----
    let data = lua.create_table()?;
    {
        let s = sh.clone();
        reg(lua, ctx, &data, "data", "get", "", move |lua, (name, path): (String, Option<String>)| {
            let d = s.data.borrow();
            let v = match path {
                Some(p) => d.get_path(&name, &p),
                None => d.get(&name),
            };
            match v {
                Some(v) => to_lua(lua, v),
                None => Ok(LuaValue::Nil),
            }
        })?;
        let s = sh.clone();
        reg(lua, ctx, &data, "data", "names", "", move |_, ()| Ok(s.data.borrow().names()))?;
    }
    g.set("data", data)?;

    // ---- save ----
    let save = lua.create_table()?;
    {
        let c = ctx.clone();
        reg(lua, ctx, &save, "save", "get", "save", move |lua, (k, default): (String, Option<LuaValue>)| {
            match c.save.borrow().map.get(&k) {
                Some(v) => to_lua(lua, v),
                None => Ok(default.unwrap_or(LuaValue::Nil)),
            }
        })?;
        let c = ctx.clone();
        reg(lua, ctx, &save, "save", "set", "save", move |lua, (k, v): (String, LuaValue)| {
            if k.len() > 256 {
                return Err(rt_err("save key too long"));
            }
            let mut s = c.save.borrow_mut();
            if matches!(v, LuaValue::Nil) {
                s.map.remove(&k);
            } else {
                s.map.insert(k, json_from_lua(lua, v)?);
            }
            s.dirty = true;
            Ok(())
        })?;
        let c = ctx.clone();
        reg(lua, ctx, &save, "save", "delete", "save", move |_, k: String| {
            let mut s = c.save.borrow_mut();
            s.map.remove(&k);
            s.dirty = true;
            Ok(())
        })?;
        let c = ctx.clone();
        reg(lua, ctx, &save, "save", "keys", "save", move |_, ()| {
            Ok(c.save.borrow().map.keys().cloned().collect::<Vec<_>>())
        })?;
        let (c, s) = (ctx.clone(), sh.clone());
        reg(lua, ctx, &save, "save", "flush", "save", move |_, ()| {
            c.flush_save(s.limits.max_save_bytes).map_err(rt_err)
        })?;
    }
    g.set("save", save)?;

    // ---- world ----
    let world = lua.create_table()?;
    {
        let s = sh.clone();
        reg(lua, ctx, &world, "world", "level", "world", move |_, ()| Ok(s.host.level_name()))?;
        let s = sh.clone();
        reg(lua, ctx, &world, "world", "time", "world", move |_, ()| Ok(s.host.game_time()))?;
        let s = sh.clone();
        reg(lua, ctx, &world, "world", "player", "world", move |_, ()| Ok(s.host.player().map(|e| e.0)))?;
        let s = sh.clone();
        reg(lua, ctx, &world, "world", "raycast", "world", move |lua, (a, b): (LuaValue, LuaValue)| {
            let (a, b): (V3, V3) = (from_lua(lua, a)?, from_lua(lua, b)?);
            match s.host.raycast(a.into(), b.into()).map_err(host_err)? {
                Some(h) => to_lua(lua, &h),
                None => Ok(LuaValue::Nil),
            }
        })?;
    }
    g.set("world", world)?;

    // ---- entity ----
    let entity = lua.create_table()?;
    {
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "info", "entities.read", move |lua, id: u64| match s.host.entity_info(EntityId(id)) {
            Some(i) => to_lua(lua, &i),
            None => Ok(LuaValue::Nil),
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "exists", "entities.read", move |_, id: u64| {
            Ok(s.host.entity_info(EntityId(id)).is_some())
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "find", "entities.read", move |_, name: String| {
            Ok(s.host.find_entity(&name).map(|e| e.0))
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "query", "entities.read", move |lua, q: Option<LuaValue>| {
            let q: QueryOpts = match q {
                Some(v @ LuaValue::Table(_)) => from_lua(lua, v)?,
                _ => QueryOpts::default(),
            };
            let res = s.host.query_entities(&EntityQuery {
                kind: q.kind,
                name_contains: q.name_contains,
                near: q.near.map(Into::into),
                radius: q.radius,
                alive_only: q.alive_only,
                limit: Some(q.limit.unwrap_or(1000).min(10_000)),
            });
            to_lua(lua, &res)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "get_component", "entities.read", move |lua, (id, c): (u64, String)| {
            match s.host.get_component(EntityId(id), &c).map_err(host_err)? {
                Some(v) => to_lua(lua, &v),
                None => Ok(LuaValue::Nil),
            }
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "spawn", "entities.write", move |lua, (template, opts): (String, Option<LuaValue>)| {
            let o: SpawnOpts = match opts {
                Some(v @ LuaValue::Table(_)) => from_lua(lua, v)?,
                _ => SpawnOpts::default(),
            };
            let req = SpawnRequest {
                template,
                name: o.name,
                position: o.pos.map(Into::into).unwrap_or_default(),
                rotation: o.rot.map(Into::into).unwrap_or_default(),
                model: o.model,
                components: o.components.unwrap_or_default(),
            };
            Ok(s.host.spawn_entity(&req).map_err(host_err)?.0)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "despawn", "entities.write", move |_, id: u64| {
            s.host.despawn_entity(EntityId(id)).map_err(host_err)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "set_position", "entities.write", move |lua, (id, p): (u64, LuaValue)| {
            let p: V3 = from_lua(lua, p)?;
            s.host.set_entity_position(EntityId(id), p.into()).map_err(host_err)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "set_rotation", "entities.write", move |lua, (id, p): (u64, LuaValue)| {
            let p: V3 = from_lua(lua, p)?;
            s.host.set_entity_rotation(EntityId(id), p.into()).map_err(host_err)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "set_model", "entities.write", move |_, (id, m): (u64, String)| {
            s.host.set_entity_model(EntityId(id), &m).map_err(host_err)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "set_component", "entities.write", move |lua, (id, c, v): (u64, String, LuaValue)| {
            let v = json_from_lua(lua, v)?;
            s.host.set_component(EntityId(id), &c, v).map_err(host_err)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "damage", "entities.write", move |_, (id, a, src): (u64, f32, Option<u64>)| {
            s.host.damage_entity(EntityId(id), a, src.map(EntityId)).map_err(host_err)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &entity, "entity", "kill", "entities.write", move |_, (id, src): (u64, Option<u64>)| {
            s.host.kill_entity(EntityId(id), src.map(EntityId)).map_err(host_err)
        })?;
    }
    g.set("entity", entity)?;

    // ---- audio ----
    let audio = lua.create_table()?;
    {
        let s = sh.clone();
        reg(lua, ctx, &audio, "audio", "play", "audio", move |lua, (name, opts): (String, Option<LuaValue>)| {
            let o: SoundOpts = match opts {
                Some(v @ LuaValue::Table(_)) => from_lua(lua, v)?,
                _ => SoundOpts::default(),
            };
            let p = SoundParams {
                position: o.pos.map(Into::into),
                entity: o.entity.map(EntityId),
                volume: o.volume,
                pitch: o.pitch,
                looping: o.looping,
            };
            s.host.play_sound(&name, &p).map_err(host_err)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &audio, "audio", "stop", "audio", move |_, h: u64| s.host.stop_sound(h).map_err(host_err))?;
        let s = sh.clone();
        reg(lua, ctx, &audio, "audio", "music", "audio", move |_, c: String| s.host.play_music_cue(&c).map_err(host_err))?;
        let s = sh.clone();
        reg(lua, ctx, &audio, "audio", "music_state", "audio", move |_, c: String| s.host.set_music_state(&c).map_err(host_err))?;
        let s = sh.clone();
        reg(lua, ctx, &audio, "audio", "music_event", "audio", move |_, c: String| s.host.music_event(&c).map_err(host_err))?;
    }
    g.set("audio", audio)?;

    // ---- fx ----
    let fx = lua.create_table()?;
    {
        let s = sh.clone();
        reg(lua, ctx, &fx, "fx", "particles", "fx", move |lua, (name, pos, opts): (String, LuaValue, Option<LuaValue>)| {
            let pos: V3 = from_lua(lua, pos)?;
            let o: ParticleOpts = match opts {
                Some(v @ LuaValue::Table(_)) => from_lua(lua, v)?,
                _ => ParticleOpts::default(),
            };
            let p = ParticleParams { entity: o.entity.map(EntityId), scale: o.scale, lifetime: o.lifetime };
            s.host.spawn_particles(&name, pos.into(), &p).map_err(host_err)
        })?;
    }
    g.set("fx", fx)?;

    // ---- ui ----
    let ui = lua.create_table()?;
    {
        let s = sh.clone();
        reg(lua, ctx, &ui, "ui", "show_screen", "ui", move |_, n: String| s.host.show_screen(&n).map_err(host_err))?;
        let s = sh.clone();
        reg(lua, ctx, &ui, "ui", "hide_screen", "ui", move |_, n: String| s.host.hide_screen(&n).map_err(host_err))?;
        let s = sh.clone();
        reg(lua, ctx, &ui, "ui", "set_hud_var", "ui", move |lua, (n, v): (String, LuaValue)| {
            let v = json_from_lua(lua, v)?;
            s.host.set_hud_var(&n, v).map_err(host_err)
        })?;
        let s = sh.clone();
        reg(lua, ctx, &ui, "ui", "message", "ui", move |_, (t, secs): (String, Option<f32>)| {
            s.host.show_message(&t, secs.unwrap_or(3.0)).map_err(host_err)
        })?;
    }
    g.set("ui", ui)?;

    // ---- console / cvar ----
    let console = lua.create_table()?;
    {
        let (s, c) = (sh.clone(), ctx.clone());
        reg(lua, ctx, &console, "console", "register", "console", move |lua, (name, help, func): (String, String, Function)| {
            let name = name.to_lowercase();
            if !valid_name(&name) {
                return Err(rt_err(format!("invalid command name {name:?}")));
            }
            let mut cmds = s.commands.borrow_mut();
            if let Some(e) = cmds.get(&name) {
                if e.mod_id != c.id {
                    return Err(rt_err(format!("command {name:?} is already registered by mod '{}'", e.mod_id)));
                }
            }
            cmds.insert(
                name,
                CommandEntry { mod_id: c.id.clone(), gen_id: c.gen_id, help, func, lua: lua.clone(), ctx: c.clone() },
            );
            Ok(())
        })?;
        let s = sh.clone();
        reg(lua, ctx, &console, "console", "print", "console", move |_, t: String| {
            s.host.console_print(&t);
            Ok(())
        })?;
        let s = sh.clone();
        reg(lua, ctx, &console, "console", "exec", "console", move |_, l: String| {
            s.host.run_console_command(&l).map_err(host_err)
        })?;
    }
    g.set("console", console)?;

    let cvar = lua.create_table()?;
    {
        let (s, c) = (sh.clone(), ctx.clone());
        reg(lua, ctx, &cvar, "cvar", "register", "console", move |lua, (name, default, help): (String, LuaValue, Option<String>)| {
            let name = name.to_lowercase();
            if !valid_name(&name) {
                return Err(rt_err(format!("invalid cvar name {name:?}")));
            }
            let dv = json_from_lua(lua, default)?;
            if !matches!(dv, Value::Bool(_) | Value::Number(_) | Value::String(_)) {
                return Err(rt_err("cvar default must be boolean, number or string"));
            }
            if s.commands.borrow().contains_key(&name) {
                return Err(rt_err(format!("{name:?} is a console command")));
            }
            let mut cv = s.cvars.borrow_mut();
            match cv.get_mut(&name) {
                Some(e) if e.owner != c.id => {
                    return Err(rt_err(format!("cvar {name:?} is owned by mod '{}'", e.owner)));
                }
                Some(e) => {
                    // Re-registration (hot reload) keeps the current value when the type matches.
                    let same = std::mem::discriminant(&e.value) == std::mem::discriminant(&dv);
                    if !same {
                        e.value = dv;
                    }
                    e.help = help.unwrap_or_default();
                }
                None => {
                    cv.insert(name, Cvar { value: dv, help: help.unwrap_or_default(), owner: c.id.clone() });
                }
            }
            Ok(())
        })?;
        let s = sh.clone();
        reg(lua, ctx, &cvar, "cvar", "get", "console", move |lua, name: String| {
            match s.cvars.borrow().get(&name.to_lowercase()) {
                Some(c) => to_lua(lua, &c.value),
                None => Ok(LuaValue::Nil),
            }
        })?;
        let s = sh.clone();
        reg(lua, ctx, &cvar, "cvar", "set", "console", move |lua, (name, v): (String, LuaValue)| {
            let v = json_from_lua(lua, v)?;
            cvar_set(&s, &name.to_lowercase(), v).map_err(rt_err)
        })?;
    }
    g.set("cvar", cvar)?;

    // ---- require (mod-local modules only) ----
    {
        let cache: Rc<RefCell<HashMap<String, LuaValue>>> = Rc::default();
        let loading: Rc<RefCell<HashSet<String>>> = Rc::default();
        let c = ctx.clone();
        g.set(
            "require",
            lua.create_function(move |lua, name: String| {
                if let Some(v) = cache.borrow().get(&name) {
                    return Ok(v.clone());
                }
                let rel = name.replace('.', "/");
                if !safe_rel(&rel) {
                    return Err(rt_err(format!("invalid module name {name:?}")));
                }
                let cands = [c.scripts_dir.join(format!("{rel}.lua")), c.scripts_dir.join(&rel).join("init.lua")];
                let Some(path) = cands.iter().find(|p| p.is_file()) else {
                    return Err(rt_err(format!("module {name:?} not found in {}", c.manifest.scripts_dir_name())));
                };
                if !loading.borrow_mut().insert(name.clone()) {
                    return Err(rt_err(format!("circular require of {name:?}")));
                }
                let src = std::fs::read_to_string(path).map_err(|e| rt_err(e.to_string()));
                let r = src.and_then(|src| {
                    lua.load(&src)
                        .set_name(format!("={}/{}", c.id, name))
                        .set_mode(mlua::ChunkMode::Text)
                        .eval::<LuaValue>()
                });
                loading.borrow_mut().remove(&name);
                let v = match r? {
                    LuaValue::Nil => LuaValue::Boolean(true),
                    v => v,
                };
                cache.borrow_mut().insert(name, v.clone());
                Ok(v)
            })?,
        )?;
    }
    Ok(())
}
