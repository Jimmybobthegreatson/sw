//! The scripting runtime: one sandboxed Luau state per mod, event dispatch with priorities and
//! cancellation, timers, console commands, cvars, persistent save data and hot reload.

use crate::bindings;
use crate::data::{DataReport, DataStore, data_fingerprint};
use crate::events::{DispatchOutcome, ErrorKind, Event, ScriptError, builtin_event};
use crate::host::{LogLevel, ScriptHost};
use crate::manifest::ModManifest;
use crate::package::{ResolvedMod, ResolvedMods};
use mlua::{ChunkMode, Function, Lua, LuaOptions, LuaSerdeExt, StdLib, Value as LuaValue, VmState};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

/// Resource limits applied to every mod.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Memory cap per mod (Luau heap).
    pub memory_bytes: usize,
    /// Wall-clock budget for loading a mod (all entry scripts).
    pub load_time: Duration,
    /// Wall-clock budget for one handler / timer / command call.
    pub call_time: Duration,
    /// Runtime errors tolerated before a mod is disabled.
    pub max_errors: u32,
    /// Timeouts / memory violations tolerated before a mod is disabled.
    pub max_strikes: u32,
    /// Max size of a mod's save file.
    pub max_save_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            memory_bytes: 64 << 20,
            load_time: Duration::from_millis(1000),
            call_time: Duration::from_millis(50),
            max_errors: 50,
            max_strikes: 3,
            max_save_bytes: 1 << 20,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeConfig {
    pub limits: Limits,
    /// Directory for `<mod id>.json` save files. `None` keeps save data in memory only.
    pub save_dir: Option<PathBuf>,
}

pub(crate) struct Cvar {
    pub value: Value,
    pub help: String,
    pub owner: String,
}

pub(crate) struct CommandEntry {
    pub mod_id: String,
    pub gen_id: u64,
    pub help: String,
    pub func: Function,
    pub lua: Lua,
    pub ctx: Rc<ModCtx>,
}

#[derive(Clone)]
pub(crate) struct HandlerEntry {
    pub id: u64,
    pub gen_id: u64,
    pub order: usize,
    pub event: String,
    pub priority: i32,
    pub once: bool,
    pub run_if_cancelled: bool,
    pub func: Function,
    pub lua: Lua,
    pub ctx: Rc<ModCtx>,
}

pub(crate) struct Timer {
    pub id: u64,
    pub due: f64,
    pub interval: Option<f64>,
    pub func: Function,
}

#[derive(Default)]
pub(crate) struct SaveData {
    pub map: serde_json::Map<String, Value>,
    pub dirty: bool,
}

/// Per-mod-instance bookkeeping shared with the Lua closures.
pub(crate) struct ModCtx {
    pub id: String,
    pub gen_id: u64,
    pub order: usize,
    pub manifest: ModManifest,
    pub root: PathBuf,
    pub scripts_dir: PathBuf,
    pub deadline: Cell<Option<Instant>>,
    pub depth: Cell<u32>,
    pub tick: Cell<u32>,
    pub error_count: Cell<u32>,
    pub strikes: Cell<u32>,
    pub disabled: Cell<bool>,
    pub now: Cell<f64>,
    pub timers: RefCell<Vec<Timer>>,
    pub save: RefCell<SaveData>,
    pub save_path: Option<PathBuf>,
    pub last_flush: Cell<f64>,
}

impl ModCtx {
    pub fn has_perm(&self, p: &str) -> bool {
        p.is_empty() || self.manifest.has_permission(p)
    }

    /// Run `f` under a wall-clock budget. Nested calls (a handler emitting an event handled by the
    /// same mod) share the outermost deadline.
    pub fn guarded<R>(&self, budget: Duration, f: impl FnOnce() -> mlua::Result<R>) -> mlua::Result<R> {
        if self.depth.get() == 0 {
            self.deadline.set(Some(Instant::now() + budget));
            self.tick.set(0);
        }
        self.depth.set(self.depth.get() + 1);
        let r = f();
        self.depth.set(self.depth.get() - 1);
        if self.depth.get() == 0 {
            self.deadline.set(None);
        }
        r
    }

    pub fn flush_save(&self, max_bytes: usize) -> Result<(), String> {
        let mut s = self.save.borrow_mut();
        if !s.dirty {
            return Ok(());
        }
        let Some(path) = &self.save_path else {
            s.dirty = false;
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&s.map).map_err(|e| e.to_string())?;
        if bytes.len() > max_bytes {
            return Err(format!("save data too large ({} > {max_bytes} bytes); not written", bytes.len()));
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
        s.dirty = false;
        Ok(())
    }
}

pub(crate) struct LoadedInfo {
    pub version: String,
    pub ctx: Rc<ModCtx>,
}

/// State shared by all mods and by the closures installed in their Lua states.
pub(crate) struct Shared {
    pub host: Rc<dyn ScriptHost>,
    pub limits: Limits,
    pub data: RefCell<DataStore>,
    pub cvars: RefCell<BTreeMap<String, Cvar>>,
    pub handlers: RefCell<Vec<HandlerEntry>>,
    pub commands: RefCell<BTreeMap<String, CommandEntry>>,
    pub loaded: RefCell<BTreeMap<String, LoadedInfo>>,
    pub errors: RefCell<Vec<ScriptError>>,
    pub next_id: Cell<u64>,
    pub emit_depth: Cell<u32>,
}

const MAX_EMIT_DEPTH: u32 = 8;

pub(crate) fn classify(err: &mlua::Error) -> ErrorKind {
    let m = err.to_string().to_lowercase();
    if m.contains("time budget") {
        ErrorKind::Timeout
    } else if m.contains("memory") {
        ErrorKind::Memory
    } else if m.contains("permission denied") || m.contains("host error") {
        ErrorKind::Api
    } else {
        ErrorKind::Runtime
    }
}

/// Convert integral floats to integers so ids and counts survive the Lua number round trip.
pub(crate) fn normalize_json(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15 {
                    *v = Value::from(f as i64);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize_json),
        Value::Object(o) => o.values_mut().for_each(normalize_json),
        _ => {}
    }
}

impl Shared {
    pub fn new_id(&self) -> u64 {
        let i = self.next_id.get();
        self.next_id.set(i + 1);
        i
    }

    pub fn record_error(&self, ctx: &ModCtx, context: &str, err: &mlua::Error) -> ScriptError {
        let kind = classify(err);
        let message = err.to_string();
        match kind {
            ErrorKind::Timeout | ErrorKind::Memory => ctx.strikes.set(ctx.strikes.get() + 1),
            _ => ctx.error_count.set(ctx.error_count.get() + 1),
        }
        let mut note = String::new();
        if (ctx.strikes.get() >= self.limits.max_strikes || ctx.error_count.get() >= self.limits.max_errors)
            && !ctx.disabled.get()
        {
            ctx.disabled.set(true);
            note = " -- mod disabled after repeated failures".to_string();
        }
        self.host.log(&ctx.id, LogLevel::Error, &format!("{context}: {message}{note}"));
        let e = ScriptError { mod_id: ctx.id.clone(), context: context.to_string(), kind, message: format!("{message}{note}") };
        let mut errs = self.errors.borrow_mut();
        if errs.len() >= 200 {
            errs.remove(0);
        }
        errs.push(e.clone());
        e
    }

    /// Dispatch an event to all subscribed (non-disabled) mods, highest priority first.
    pub fn dispatch(&self, name: &str, payload: Value, target: Option<&str>) -> DispatchOutcome {
        let cancelable = builtin_event(name).is_none_or(|d| d.cancelable);
        let mut hs: Vec<HandlerEntry> = self
            .handlers
            .borrow()
            .iter()
            .filter(|h| h.event == name && !h.ctx.disabled.get() && target.is_none_or(|t| h.ctx.id == t))
            .cloned()
            .collect();
        hs.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.order.cmp(&b.order)).then(a.id.cmp(&b.id)));
        let mut out = DispatchOutcome { cancelled: false, payload, handlers_run: 0, errors: Vec::new() };
        if hs.is_empty() {
            return out;
        }
        if self.emit_depth.get() >= MAX_EMIT_DEPTH {
            return out;
        }
        self.emit_depth.set(self.emit_depth.get() + 1);
        for h in hs {
            if h.ctx.disabled.get() || (out.cancelled && !h.run_if_cancelled) {
                continue;
            }
            // Removed by an earlier handler (events.off / unload)?
            if !self.handlers.borrow().iter().any(|x| x.id == h.id) {
                continue;
            }
            if h.once {
                self.handlers.borrow_mut().retain(|x| x.id != h.id);
            }
            let mut p = out.payload.clone();
            if let Value::Object(o) = &mut p {
                o.insert("cancelled".into(), Value::Bool(out.cancelled));
            }
            let call = (|| -> mlua::Result<(Value, bool)> {
                let tbl = bindings::to_lua(&h.lua, &p)?;
                let ret: LuaValue = h.ctx.guarded(self.limits.call_time, || h.func.call(tbl.clone()))?;
                let mut back: Value = bindings::from_lua(&h.lua, tbl)?;
                normalize_json(&mut back);
                Ok((back, matches!(ret, LuaValue::Boolean(false))))
            })();
            match call {
                Ok((mut back, ret_false)) => {
                    out.handlers_run += 1;
                    let mut cancelled = out.cancelled;
                    if let Value::Object(o) = &mut back {
                        if cancelable && o.remove("cancelled").and_then(|c| c.as_bool()).unwrap_or(false) {
                            cancelled = true;
                        }
                        o.remove("cancelled");
                    }
                    if cancelable && ret_false {
                        cancelled = true;
                    }
                    out.cancelled = cancelled;
                    out.payload = back;
                }
                Err(e) => {
                    let se = self.record_error(&h.ctx, &format!("event {name}"), &e);
                    out.errors.push(se);
                }
            }
        }
        self.emit_depth.set(self.emit_depth.get() - 1);
        out
    }

    fn remove_gen(&self, gen_id: u64) {
        self.handlers.borrow_mut().retain(|h| h.gen_id != gen_id);
        self.commands.borrow_mut().retain(|_, c| c.gen_id != gen_id);
    }
}

struct ModInstance {
    ctx: Rc<ModCtx>,
    lua: Lua,
    resolved: ResolvedMod,
    watch: Vec<(PathBuf, Option<SystemTime>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadKind {
    Scripts,
    Data,
}

#[derive(Debug, Clone)]
pub struct ReloadReport {
    pub mod_id: Option<String>,
    pub kind: ReloadKind,
    pub ok: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadReport {
    pub mod_id: String,
    pub ok: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ModStatus {
    pub id: String,
    pub version: String,
    /// Disabled by the runtime after repeated failures.
    pub faulted: bool,
    pub error_count: u32,
    pub strikes: u32,
}

/// Owns all loaded mods. Single-threaded: keep it on the thread that runs the game loop.
pub struct ScriptRuntime {
    shared: Rc<Shared>,
    mods: Vec<ModInstance>,
    config: RuntimeConfig,
    resolved: ResolvedMods,
    data_fp: u64,
    next_gen: u64,
    /// State snapshots of mods whose reload failed; restored on the next successful load.
    pending_state: BTreeMap<String, Value>,
    /// Mods whose reload failed, with the file state at the failure (retry when it changes).
    failed: BTreeMap<String, Vec<(PathBuf, Option<SystemTime>)>>,
}

fn watch_list(m: &ResolvedMod) -> Vec<(PathBuf, Option<SystemTime>)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lua")) {
                out.push(p);
            }
        }
    }
    let mut files = vec![m.root.join("mod.toml")];
    walk(&m.root.join(m.manifest.scripts_dir_name()), &mut files);
    for e in &m.manifest.scripts.entry {
        let p = m.root.join(e);
        if !files.contains(&p) {
            files.push(p);
        }
    }
    files.sort();
    files.into_iter().map(|p| { let t = std::fs::metadata(&p).and_then(|m| m.modified()).ok(); (p, t) }).collect()
}

impl ScriptRuntime {
    pub fn new(host: Rc<dyn ScriptHost>, config: RuntimeConfig) -> ScriptRuntime {
        let shared = Rc::new(Shared {
            host,
            limits: config.limits.clone(),
            data: RefCell::new(DataStore::new()),
            cvars: RefCell::new(BTreeMap::new()),
            handlers: RefCell::new(Vec::new()),
            commands: RefCell::new(BTreeMap::new()),
            loaded: RefCell::new(BTreeMap::new()),
            errors: RefCell::new(Vec::new()),
            next_id: Cell::new(1),
            emit_depth: Cell::new(0),
        });
        ScriptRuntime {
            shared,
            mods: Vec::new(),
            config,
            resolved: ResolvedMods::default(),
            data_fp: 0,
            next_gen: 1,
            pending_state: BTreeMap::new(),
            failed: BTreeMap::new(),
        }
    }

    /// Mutable access to the data store: register base datasets and schemas before `load_mods`.
    pub fn data_mut(&self) -> std::cell::RefMut<'_, DataStore> {
        self.shared.data.borrow_mut()
    }
    pub fn data(&self) -> std::cell::Ref<'_, DataStore> {
        self.shared.data.borrow()
    }

    /// Apply data patches of `set`, then load every mod's scripts in order.
    pub fn load_mods(&mut self, set: &ResolvedMods) -> (DataReport, Vec<LoadReport>) {
        self.resolved = set.clone();
        let drep = self.shared.data.borrow_mut().apply_mods(set);
        for e in &drep.errors {
            self.shared.host.log(&e.mod_id, LogLevel::Error, &format!("data {}: {}", e.file, e.message));
        }
        self.data_fp = data_fingerprint(set);
        let mut reports = Vec::new();
        for (i, m) in set.order.iter().enumerate() {
            if m.manifest.scripts.entry.is_empty() {
                continue;
            }
            reports.push(self.load_one(m, i, false));
        }
        (drep, reports)
    }

    fn load_one(&mut self, m: &ResolvedMod, order: usize, reloaded: bool) -> LoadReport {
        let gen_id = self.next_gen;
        self.next_gen += 1;
        let state = self.pending_state.remove(m.id());
        let id = m.id().to_string();
        match self.build_instance(m, order, gen_id, state.clone(), reloaded) {
            Ok(inst) => {
                self.failed.remove(&id);
                self.shared.loaded.borrow_mut().insert(
                    id.clone(),
                    LoadedInfo { version: m.manifest.version.clone(), ctx: inst.ctx.clone() },
                );
                self.mods.push(inst);
                let me = self.mods.last().unwrap();
                let out = self.shared.dispatch("on_mod_load", serde_json::json!({ "reloaded": reloaded }), Some(&id));
                let errors = out.errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>();
                let _ = me;
                LoadReport { mod_id: id, ok: errors.is_empty(), errors }
            }
            Err(msgs) => {
                self.shared.remove_gen(gen_id);
                if let Some(s) = state {
                    self.pending_state.insert(id.clone(), s);
                }
                self.failed.insert(id.clone(), watch_list(m));
                self.shared.loaded.borrow_mut().remove(&id);
                LoadReport { mod_id: id, ok: false, errors: msgs }
            }
        }
    }

    fn build_instance(
        &self,
        m: &ResolvedMod,
        order: usize,
        gen_id: u64,
        state: Option<Value>,
        reloaded: bool,
    ) -> Result<ModInstance, Vec<String>> {
        let sh = &self.shared;
        let lua = Lua::new_with(
            StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH | StdLib::BIT,
            LuaOptions::default(),
        )
        .map_err(|e| vec![e.to_string()])?;
        lua.set_memory_limit(sh.limits.memory_bytes).map_err(|e| vec![e.to_string()])?;
        // Optimisation level 0: Luau otherwise caches `global.field.field` reads at chunk-load time
        // (GETIMPORT) which makes `mod.state.x` and user global tables return stale values.
        lua.set_compiler(mlua::Compiler::new().set_optimization_level(0));

        let save_path = self.config.save_dir.as_ref().map(|d| d.join(format!("{}.json", m.id())));
        let mut save = SaveData::default();
        if let Some(p) = &save_path {
            if let Ok(t) = std::fs::read_to_string(p) {
                match serde_json::from_str::<Value>(&t) {
                    Ok(Value::Object(o)) => save.map = o,
                    _ => sh.host.log(m.id(), LogLevel::Warn, "ignoring unreadable save file"),
                }
            }
        }
        let ctx = Rc::new(ModCtx {
            id: m.id().to_string(),
            gen_id,
            order,
            manifest: m.manifest.clone(),
            root: m.root.clone(),
            scripts_dir: m.root.join(m.manifest.scripts_dir_name()),
            deadline: Cell::new(None),
            depth: Cell::new(0),
            tick: Cell::new(0),
            error_count: Cell::new(0),
            strikes: Cell::new(0),
            disabled: Cell::new(false),
            now: Cell::new(0.0),
            timers: RefCell::new(Vec::new()),
            save: RefCell::new(save),
            save_path,
            last_flush: Cell::new(0.0),
        });

        {
            let c = ctx.clone();
            lua.set_interrupt(move |_| {
                let t = c.tick.get().wrapping_add(1);
                c.tick.set(t);
                if t % 64 == 0 {
                    if let Some(d) = c.deadline.get() {
                        if Instant::now() > d {
                            return Err(mlua::Error::runtime("script exceeded time budget"));
                        }
                    }
                }
                Ok(VmState::Continue)
            });
        }

        let setup = (|| -> mlua::Result<()> {
            bindings::install(&lua, sh, &ctx, state.as_ref(), reloaded)?;
            lua.sandbox(true)?;
            Ok(())
        })();
        setup.map_err(|e| vec![format!("sandbox setup: {e}")])?;

        // Entry scripts, under the load budget.
        let mut errors = Vec::new();
        for entry in &m.manifest.scripts.entry {
            let path = m.root.join(entry);
            let src = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(format!("{entry}: {e}"));
                    break;
                }
            };
            let r = ctx.guarded(sh.limits.load_time, || {
                lua.load(&src).set_name(format!("={}/{}", m.id(), entry)).set_mode(ChunkMode::Text).exec()
            });
            if let Err(e) = r {
                let se = sh.record_error(&ctx, &format!("loading {entry}"), &e);
                errors.push(se.message);
                break;
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(ModInstance { ctx, lua, resolved: m.clone(), watch: watch_list(m) })
    }

    fn snapshot_state(inst: &ModInstance) -> Option<Value> {
        let g = inst.lua.globals();
        let modt: mlua::Table = g.get("mod").ok()?;
        let st: LuaValue = modt.get("state").ok()?;
        let mut v: Value = bindings::from_lua(&inst.lua, st).ok()?;
        normalize_json(&mut v);
        Some(v)
    }

    fn unload_idx(&mut self, idx: usize, reloading: bool) -> Option<Value> {
        let inst = self.mods.remove(idx);
        let id = inst.ctx.id.clone();
        self.shared.dispatch("on_mod_unload", serde_json::json!({ "reloading": reloading }), Some(&id));
        let state = Self::snapshot_state(&inst);
        if let Err(e) = inst.ctx.flush_save(self.shared.limits.max_save_bytes) {
            self.shared.host.log(&id, LogLevel::Error, &e);
        }
        self.shared.remove_gen(inst.ctx.gen_id);
        self.shared.loaded.borrow_mut().remove(&id);
        state
    }

    pub fn unload_mod(&mut self, id: &str) -> bool {
        match self.mods.iter().position(|m| m.ctx.id == id) {
            Some(i) => {
                self.unload_idx(i, false);
                true
            }
            None => false,
        }
    }

    /// Send an event to all mods.
    pub fn dispatch(&self, ev: &Event) -> DispatchOutcome {
        self.shared.dispatch(&ev.name, ev.payload.clone(), None)
    }

    /// Per-frame tick: runs timers, dispatches `on_update`, periodically flushes save files.
    pub fn update(&self, dt: f32, game_time: f64) {
        for inst in &self.mods {
            let ctx = &inst.ctx;
            if ctx.disabled.get() {
                continue;
            }
            ctx.now.set(game_time);
            let due: Vec<(u64, Function)> = {
                let mut ts = ctx.timers.borrow_mut();
                let mut due = Vec::new();
                ts.retain_mut(|t| {
                    if t.due <= game_time {
                        due.push((t.id, t.func.clone()));
                        match t.interval {
                            Some(iv) => {
                                t.due = (t.due + iv).max(game_time + 0.0005);
                                true
                            }
                            None => false,
                        }
                    } else {
                        true
                    }
                });
                due
            };
            for (id, f) in due {
                if ctx.disabled.get() {
                    break;
                }
                if let Err(e) = ctx.guarded(self.shared.limits.call_time, || f.call::<()>(())) {
                    self.shared.record_error(ctx, &format!("timer {id}"), &e);
                }
            }
            if game_time - ctx.last_flush.get() > 5.0 || game_time < ctx.last_flush.get() {
                ctx.last_flush.set(game_time);
                if let Err(e) = ctx.flush_save(self.shared.limits.max_save_bytes) {
                    self.shared.host.log(&ctx.id, LogLevel::Error, &e);
                }
            }
        }
        self.shared.dispatch("on_update", serde_json::json!({ "dt": dt, "time": game_time }), None);
    }

    /// Check script, manifest and data files for changes; reload what changed.
    pub fn poll_hot_reload(&mut self) -> Vec<ReloadReport> {
        let mut reports = Vec::new();
        // Scripts of running mods.
        let changed: Vec<usize> = (0..self.mods.len())
            .filter(|&i| watch_list(&self.mods[i].resolved) != self.mods[i].watch)
            .collect();
        for i in changed.into_iter().rev() {
            let id = self.mods[i].ctx.id.clone();
            reports.push(self.reload_at(i, &id));
        }
        // Mods whose last (re)load failed.
        let retry: Vec<String> = self
            .failed
            .iter()
            .filter(|(id, w)| self.resolved.get(id).is_some_and(|m| &watch_list(m) != *w))
            .map(|(id, _)| id.clone())
            .collect();
        for id in retry {
            if let Some((order, m)) = self.resolved.order.iter().enumerate().find(|(_, m)| m.id() == id).map(|(i, m)| (i, m.clone())) {
                let m = refresh_manifest(&m).unwrap_or(m);
                let r = self.load_one(&m, order, true);
                reports.push(ReloadReport { mod_id: Some(id), kind: ReloadKind::Scripts, ok: r.ok, errors: r.errors });
            }
        }
        // Data patches.
        let fp = data_fingerprint(&self.resolved);
        if fp != self.data_fp {
            self.data_fp = fp;
            let rep = self.shared.data.borrow_mut().apply_mods(&self.resolved);
            self.shared.dispatch("on_data_reload", serde_json::json!({}), None);
            reports.push(ReloadReport {
                mod_id: None,
                kind: ReloadKind::Data,
                ok: rep.errors.is_empty(),
                errors: rep.errors.iter().map(|e| format!("{}: {}: {}", e.mod_id, e.file, e.message)).collect(),
            });
        }
        reports
    }

    fn reload_at(&mut self, idx: usize, id: &str) -> ReloadReport {
        let order = self.mods[idx].ctx.order;
        let old = self.mods[idx].resolved.clone();
        let state = self.unload_idx(idx, true);
        if let Some(s) = state {
            self.pending_state.insert(id.to_string(), s);
        }
        let m = refresh_manifest(&old).unwrap_or(old);
        let r = self.load_one(&m, order, true);
        // Keep load order stable.
        self.mods.sort_by_key(|i| i.ctx.order);
        ReloadReport { mod_id: Some(id.to_string()), kind: ReloadKind::Scripts, ok: r.ok, errors: r.errors }
    }

    /// Force a reload of one mod regardless of file changes.
    pub fn reload_mod(&mut self, id: &str) -> Option<ReloadReport> {
        let i = self.mods.iter().position(|m| m.ctx.id == id)?;
        Some(self.reload_at(i, id))
    }

    // ---- console & cvars ----

    /// Run a console line against mod commands and cvars. `None` means "not ours": pass it to the
    /// game's built-in console.
    pub fn run_console(&self, line: &str) -> Option<Result<String, String>> {
        let toks = split_args(line);
        let name = toks.first()?.to_lowercase();
        let cmd = {
            let cs = self.shared.commands.borrow();
            cs.get(&name).map(|c| (c.func.clone(), c.lua.clone(), c.ctx.clone()))
        };
        if let Some((func, lua, ctx)) = cmd {
            if ctx.disabled.get() {
                return Some(Err(format!("mod {} is disabled", ctx.id)));
            }
            let args: Vec<String> = toks[1..].to_vec();
            let r = ctx.guarded(self.shared.limits.call_time, || {
                let t = lua.create_sequence_from(args.clone())?;
                func.call::<Option<String>>(t)
            });
            return Some(match r {
                Ok(s) => Ok(s.unwrap_or_default()),
                Err(e) => {
                    self.shared.record_error(&ctx, &format!("command {name}"), &e);
                    Err(e.to_string())
                }
            });
        }
        let is_cvar = self.shared.cvars.borrow().contains_key(&name);
        if is_cvar {
            return Some(if toks.len() == 1 {
                Ok(format!("{} = {}", name, self.cvar_get(&name).unwrap_or(Value::Null)))
            } else {
                let raw = toks[1..].join(" ");
                let v = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
                self.cvar_set(&name, v).map(|_| String::new())
            });
        }
        None
    }

    /// `(name, help)` of all mod commands and cvars, for console auto-complete.
    pub fn console_entries(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> =
            self.shared.commands.borrow().iter().map(|(k, c)| (k.clone(), c.help.clone())).collect();
        v.extend(self.shared.cvars.borrow().iter().map(|(k, c)| (k.clone(), c.help.clone())));
        v.sort();
        v
    }

    pub fn cvar_get(&self, name: &str) -> Option<Value> {
        self.shared.cvars.borrow().get(name).map(|c| c.value.clone())
    }

    pub fn cvar_set(&self, name: &str, value: Value) -> Result<(), String> {
        bindings::cvar_set(&self.shared, name, value)
    }

    // ---- introspection ----

    pub fn mod_status(&self) -> Vec<ModStatus> {
        self.mods
            .iter()
            .map(|m| ModStatus {
                id: m.ctx.id.clone(),
                version: m.ctx.manifest.version.clone(),
                faulted: m.ctx.disabled.get(),
                error_count: m.ctx.error_count.get(),
                strikes: m.ctx.strikes.get(),
            })
            .collect()
    }

    /// Recent script errors (oldest first), clearing the buffer.
    pub fn take_errors(&self) -> Vec<ScriptError> {
        std::mem::take(&mut *self.shared.errors.borrow_mut())
    }

    pub fn mod_ids(&self) -> BTreeSet<String> {
        self.mods.iter().map(|m| m.ctx.id.clone()).collect()
    }

    /// Current `mod.state` of a mod, as JSON.
    pub fn mod_state(&self, id: &str) -> Option<Value> {
        self.mods.iter().find(|m| m.ctx.id == id).and_then(Self::snapshot_state)
    }

    /// Persistent save data of a mod (as the script sees it).
    pub fn mod_save(&self, id: &str) -> Option<Value> {
        self.mods
            .iter()
            .find(|m| m.ctx.id == id)
            .map(|m| Value::Object(m.ctx.save.borrow().map.clone()))
    }

    /// Send `on_shutdown`, flush saves and unload everything.
    pub fn shutdown(&mut self) {
        self.shared.dispatch("on_shutdown", serde_json::json!({}), None);
        while !self.mods.is_empty() {
            let last = self.mods.len() - 1;
            self.unload_idx(last, false);
        }
    }
}

impl Drop for ScriptRuntime {
    fn drop(&mut self) {
        // Break Rc cycles (handlers hold Lua states whose closures hold `Shared`).
        self.shared.handlers.borrow_mut().clear();
        self.shared.commands.borrow_mut().clear();
        self.shared.loaded.borrow_mut().clear();
        for m in &self.mods {
            m.ctx.timers.borrow_mut().clear();
        }
    }
}

fn refresh_manifest(m: &ResolvedMod) -> Option<ResolvedMod> {
    let text = std::fs::read_to_string(m.root.join("mod.toml")).ok()?;
    let manifest = ModManifest::parse(&text).ok()?;
    if manifest.id != m.manifest.id || manifest.validate(&m.root).iter().any(|d| d.severity == crate::manifest::Severity::Error) {
        return None;
    }
    let mut n = m.clone();
    n.assets_root = m.root.join(manifest.assets_dir_name());
    n.data_root = m.root.join(manifest.data_dir_name());
    n.manifest = manifest;
    Some(n)
}

/// Split a console line on whitespace, honouring double quotes.
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut has = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                has = true;
            }
            c if c.is_whitespace() && !in_q => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

#[allow(dead_code)]
fn _assert_serde_ext(l: &Lua) {
    let _ = l.to_value(&1);
}
