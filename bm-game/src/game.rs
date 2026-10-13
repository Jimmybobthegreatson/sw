//! [`Game`]: the façade `bm-app` drives. Owns the [`World`], optional audio / particle directors and the script
//! runtime, runs the fixed-timestep loop and produces [`RenderState`] / [`UiState`] every frame.

use crate::ai::{AlertState, Role};
use crate::anim::{AnimState, AnimRequest, AnimationProvider, ProceduralProvider, SceneClipProvider};
use crate::audio::{AlertLevel, AudioDirector};
use crate::data::WeaponKind;
use crate::fx::FxDirector;
use crate::host::{GameHost, HostCommand};
use crate::level::{GameError, Level, LevelStats, LoadOptions, load_level};
use crate::math::{Vec3, v3};
use crate::mission::MissionStatus;
use crate::player::PlayerInput;
use crate::props::{PropDraw, PropSlot, WeaponModels};
use crate::sim::TICK_DT;
use crate::state::{RenderEntity, RenderLight, RenderState, UiMessage, UiState};
use crate::world::*;
use bm_models::{Scene, Skeleton};
use bm_script::{Event, ScriptRuntime, SpawnRequest};
use bm_sound::AudioEngine;
use bm_vfs::Vfs;
use serde_json::json;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// One entry of the script event log (also kept when no script runtime is attached).
#[derive(Clone, Debug)]
pub struct EventRecord {
    pub time: f64,
    pub tick: u64,
    pub event: Event,
    /// A script handler vetoed the action this event announced.
    pub cancelled: bool,
}

pub struct Game {
    pub world: World,
    pub scene: Option<Arc<Scene>>,
    pub vfs: Option<Arc<Vfs>>,
    pub stats: LevelStats,
    pub audio: Option<AudioDirector>,
    pub fx: Option<FxDirector>,
    pub ui: UiState,
    pub provider: Box<dyn AnimationProvider>,
    host: Rc<GameHost>,
    script: Option<ScriptRuntime>,
    log: Vec<EventRecord>,
    startup: Vec<Event>,
    started: bool,
    frame_fx: Vec<FxEvent>,
    acc: f64,
    skel_cache: HashMap<u32, Option<Arc<Skeleton>>>,
    weapon_models: Option<Arc<WeaponModels>>,
    /// World position of the muzzle of each character's visible weapon as of the last [Game::render_state] (PosBox_MuzzleExit
    /// through the hand matrix): where shot effects start.
    muzzles: HashMap<EntityId, crate::math::Vec3>,
    faces: HashMap<u64, crate::face::FaceController>,
    /// Debug: facial pose weights by name that replace the automatic face animation of every character.
    pub face_debug: Vec<(String, f32)>,
    /// Maximum number of retained event records (oldest are dropped).
    pub max_log: usize,
    last_mission_status: MissionStatus,
}

impl Game {
    /// Wraps an existing world (synthetic worlds in tests, tools). No scene, audio or particles.
    pub fn from_world(world: World) -> Game {
        let host = Rc::new(GameHost::new(world.id_allocator()));
        let mut g = Game {
            world,
            scene: None,
            vfs: None,
            stats: LevelStats::default(),
            audio: None,
            fx: None,
            ui: UiState::default(),
            provider: Box::new(ProceduralProvider::new()),
            host,
            script: None,
            log: Vec::new(),
            startup: Vec::new(),
            started: false,
            frame_fx: Vec::new(),
            acc: 0.0,
            skel_cache: HashMap::new(),
            weapon_models: None,
            muzzles: HashMap::new(),
            faces: HashMap::new(),
            face_debug: Vec::new(),
            max_log: 4096,
            last_mission_status: MissionStatus::None,
        };
        g.build_startup_events();
        g
    }

    /// Loads a level from the game files (`base` like `Scenes/M01/M01_main`).
    pub fn load(vfs: Arc<Vfs>, base: &str, opts: &LoadOptions) -> Result<Game, GameError> {
        let Level { world, scene, particles, stats } = load_level(&vfs, base, opts)?;
        let mut g = Game::from_world(world);
        g.fx = Some(FxDirector::new(particles, opts.seed ^ 0x5eed));
        g.provider = Box::new(SceneClipProvider::new(scene.clone(), Box::new(ProceduralProvider::new())));
        g.scene = Some(scene);
        g.vfs = Some(vfs);
        g.stats = stats;
        Ok(g)
    }

    /// Opens sound (+ optionally music) for the loaded level on `engine`. Needs [`Game::load`].
    pub fn enable_audio(&mut self, engine: Box<dyn AudioEngine>, with_music: bool) -> Result<(), String> {
        let (Some(vfs), Some(scene)) = (self.vfs.clone(), self.scene.clone()) else {
            return Err("audio needs a level loaded from game files".into());
        };
        let base = self.world.meta.scene.clone();
        self.audio = Some(AudioDirector::open(vfs, &scene, &base, engine, with_music)?);
        Ok(())
    }

    fn build_startup_events(&mut self) {
        self.startup.clear();
        self.startup.push(Event::level_load(&self.world.meta.scene));
        for (id, info) in self.world.infos.iter() {
            if matches!(info.kind, Kind::Npc | Kind::Pickup | Kind::Player | Kind::Door) {
                let p = self.world.pos(id).unwrap_or(Vec3::ZERO);
                self.startup.push(Event::entity_spawn(&info.template, info.kind.as_str(), Some(&info.name), p.into()));
            }
        }
    }

    fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let evs = self.startup.clone();
        for e in evs {
            self.emit(e);
        }
    }

    // ------------------------------------------------------------------------------ scripting

    /// The `ScriptHost` to construct a `ScriptRuntime` with: `ScriptRuntime::new(game.host(), cfg)`.
    pub fn host(&self) -> Rc<GameHost> {
        self.host.clone()
    }

    /// Takes over a runtime built on [`Game::host`]. Level-load / spawn events are (re)sent to it.
    pub fn attach_scripts(&mut self, rt: ScriptRuntime) {
        self.script = Some(rt);
        self.host.refresh(&self.world);
        if self.started {
            if let Some(rt) = &self.script {
                for e in &self.startup {
                    rt.dispatch(e);
                }
            }
        }
    }

    pub fn scripts(&mut self) -> Option<&mut ScriptRuntime> {
        self.script.as_mut()
    }

    pub fn detach_scripts(&mut self) -> Option<ScriptRuntime> {
        self.script.take()
    }

    /// Events raised since the last call (kill, damage, pickup, mission ...), oldest first.
    pub fn take_events(&mut self) -> Vec<EventRecord> {
        std::mem::take(&mut self.log)
    }

    pub fn events(&self) -> &[EventRecord] {
        &self.log
    }

    fn record(&mut self, event: Event, cancelled: bool) {
        self.log.push(EventRecord { time: self.world.time, tick: self.world.tick, event, cancelled });
        if self.log.len() > self.max_log {
            let n = self.log.len() - self.max_log;
            self.log.drain(0..n);
        }
    }

    /// Raises a script event from outside the simulation (save / load notifications).
    pub(crate) fn log_event(&mut self, ev: Event) {
        self.emit(ev);
    }

    /// Logs an event and sends it to the scripts (non-cancelable ones).
    fn emit(&mut self, ev: Event) {
        if let Some(rt) = &self.script {
            self.host.refresh(&self.world);
            rt.dispatch(&ev);
        }
        self.record(ev, false);
    }

    // ------------------------------------------------------------------------------ simulation

    /// One fixed simulation tick (1/60 s).
    pub fn tick(&mut self, input: &PlayerInput) {
        self.ensure_started();
        self.world.step(input, TICK_DT);
        self.settle();
        if self.script.is_some() {
            self.host.refresh(&self.world);
            if let Some(rt) = &self.script {
                rt.update(TICK_DT, self.world.time);
            }
            self.apply_host_commands();
            self.settle();
        }
        let fx: Vec<FxEvent> = std::mem::take(&mut self.world.fx);
        self.frame_fx.extend(fx);
    }

    /// Variable frame: accumulates `dt` and runs up to 5 fixed ticks, then updates audio / particles / UI.
    /// Returns the number of ticks run.
    pub fn frame(&mut self, dt: f32, input: &PlayerInput) -> usize {
        self.acc += dt.clamp(0.0, 0.25) as f64;
        let mut n = 0;
        // look deltas must only be applied once per frame, not per tick
        let mut inp = input.clone();
        while self.acc >= TICK_DT as f64 && n < 5 {
            self.acc -= TICK_DT as f64;
            self.tick(&inp);
            inp.look_dx = 0.0;
            inp.look_dy = 0.0;
            n += 1;
        }
        if n == 5 {
            self.acc = 0.0;
        }
        self.present(dt);
        n
    }

    /// Audio / particles / UI for the events collected since the last call.
    pub fn present(&mut self, dt: f32) {
        let events = std::mem::take(&mut self.frame_fx);
        let cam = self.world.camera();
        if let Some(a) = &mut self.audio {
            a.update(&self.world, cam.as_ref(), &events);
        }
        if let Some(f) = &mut self.fx {
            for e in &events {
                // a shot starts at the muzzle of the shooter's visible weapon when it is near the aim origin
                let muzzle = match e {
                    FxEvent::Shot { pos, shooter, .. } => self.muzzles.get(shooter).copied().filter(|m| (*m - *pos).len() < MUZZLE_MAX_DIST),
                    _ => None,
                };
                f.handle(&self.world, e, muzzle);
            }
            f.update(&self.world, dt);
        }
        for e in &events {
            if let FxEvent::Message { text, seconds } = e {
                self.ui.messages.push(UiMessage { text: text.clone(), remaining: *seconds });
            }
        }
        self.update_ui(dt);
    }

    /// Resolves queued requests (with script veto hooks), post events and script commands until quiet.
    fn settle(&mut self) {
        for _ in 0..6 {
            let had_script = self.script.is_some();
            if had_script {
                self.host.refresh(&self.world);
            }
            {
                let Game { world, script, log, host, .. } = self;
                let time = world.time;
                let tick = world.tick;
                world.resolve_requests(&mut |w, r| {
                    let ev = r.to_event(w);
                    match script.as_ref() {
                        Some(rt) => {
                            let out = rt.dispatch(&ev);
                            let _ = host;
                            if out.cancelled {
                                log.push(EventRecord { time, tick, event: ev, cancelled: true });
                                return false;
                            }
                            r.apply_payload(&out.payload);
                            log.push(EventRecord { time, tick, event: Event { name: ev.name.clone(), payload: out.payload }, cancelled: false });
                            true
                        }
                        None => {
                            log.push(EventRecord { time, tick, event: ev, cancelled: false });
                            true
                        }
                    }
                });
            }
            let out: Vec<Event> = std::mem::take(&mut self.world.outbox);
            let quiet_outbox = out.is_empty();
            for e in out {
                self.after_event(&e);
                self.emit(e);
            }
            let had_cmds = self.host.has_commands();
            self.apply_host_commands();
            if quiet_outbox && !had_cmds && self.world.requests.is_empty() {
                break;
            }
        }
        // mission status -> music
        let st = self.world.mission.status;
        if st != self.last_mission_status {
            self.last_mission_status = st;
            if let Some(a) = &mut self.audio {
                match st {
                    MissionStatus::Complete => a.mission_event(bm_music::MissionEvent::MissionComplete),
                    MissionStatus::Failed => a.mission_event(bm_music::MissionEvent::MissionFailed),
                    _ => {}
                }
            }
        }
    }

    /// Reactions to simulation events that are not part of the world itself.
    fn after_event(&mut self, ev: &Event) {
        if ev.name == "on_kill" {
            let victim = ev.payload.get("victim").and_then(|v| v.as_u64()).map(EntityId);
            if let (Some(v), Some(a)) = (victim, self.audio.as_mut()) {
                let is_target = self.world.characters.get(v).is_some_and(|c| c.role == Role::Target);
                if is_target {
                    a.mission_event(bm_music::MissionEvent::TargetKilled);
                }
                if self.world.player == Some(v) {
                    a.mission_event(bm_music::MissionEvent::PlayerDead);
                }
            }
        }
    }

    /// Applies what scripts asked for.
    pub fn apply_host_commands(&mut self) {
        for c in self.host.drain_commands() {
            match c {
                HostCommand::Spawn { id, req } => {
                    let kind = template_kind(&req.template);
                    let ev = Event::entity_spawn(&req.template, kind, req.name.as_deref(), req.position);
                    let cancelled = match &self.script {
                        Some(rt) => rt.dispatch(&ev).cancelled,
                        None => false,
                    };
                    if cancelled {
                        self.record(ev, true);
                        continue;
                    }
                    if self.world.spawn_template(id, &req).is_ok() {
                        self.record(ev, false);
                    }
                }
                HostCommand::Despawn(id) => {
                    self.world.despawn(id);
                }
                HostCommand::SetPosition(id, p) => {
                    let yaw = self.world.transforms.get(id).map(|t| t.yaw).unwrap_or(0.0);
                    let p: Vec3 = p.into();
                    if let Some(b) = self.world.bodies.get_mut(id) {
                        b.pos = p;
                        b.vel_y = 0.0;
                        b.last_safe = p;
                    }
                    self.world.set_pose(id, p, yaw);
                }
                HostCommand::SetRotation(id, e) => {
                    let p = self.world.pos(id).unwrap_or(Vec3::ZERO);
                    self.world.set_pose(id, p, e.y.to_radians());
                }
                HostCommand::SetModel(id, m) => {
                    if let Some(i) = self.world.infos.get_mut(id) {
                        i.model = Some(m);
                    }
                }
                HostCommand::SetComponent(id, name, v) => {
                    self.world.set_component_json(id, &name, &v);
                }
                HostCommand::Damage { id, amount, source } => {
                    let p = self.world.pos(id).unwrap_or(Vec3::ZERO) + v3(0.0, 110.0, 0.0);
                    self.world.requests.push(Request::Damage { victim: id, attacker: source, amount, weapon: Some("script".into()), zone: None, point: p, dir: Vec3::UP });
                }
                HostCommand::Kill { id, source } => self.world.kill_now(id, source, Some("script")),
                HostCommand::PlaySound { handle, name, params } => self.frame_fx.push(FxEvent::Sound {
                    name,
                    pos: params.position.map(Into::into),
                    entity: params.entity,
                    volume: params.volume.unwrap_or(1.0),
                    pitch: params.pitch.unwrap_or(1.0),
                    looping: params.looping,
                    handle,
                }),
                HostCommand::StopSound(h) => self.frame_fx.push(FxEvent::StopSound { handle: h }),
                HostCommand::MusicCue(cue) => {
                    if let Some(m) = self.audio.as_ref().and_then(|a| a.music.as_ref()) {
                        m.trigger_stinger(&cue);
                    }
                }
                HostCommand::MusicState(s) => {
                    if let Some(m) = self.audio.as_ref().and_then(|a| a.music.as_ref()) {
                        m.set_state(&s);
                    }
                }
                HostCommand::MusicEvent(e) => {
                    if let Some(m) = self.audio.as_ref().and_then(|a| a.music.as_ref()) {
                        m.event(&e);
                    }
                }
                HostCommand::Particles { effect, pos, params } => {
                    self.frame_fx.push(FxEvent::Particles { template: effect, pos: pos.into(), scale: params.scale.unwrap_or(1.0) })
                }
                HostCommand::ShowScreen(s) => {
                    if !self.ui.screens.contains(&s) {
                        self.ui.screens.push(s);
                    }
                }
                HostCommand::HideScreen(s) => self.ui.screens.retain(|x| *x != s),
                HostCommand::HudVar(k, v) => {
                    self.ui.vars.insert(k, v);
                }
                HostCommand::Message(t, s) => self.ui.messages.push(UiMessage { text: t, remaining: s }),
                HostCommand::Console(t) => log::info!("[script] {t}"),
                HostCommand::ConsoleCommand(line) => {
                    let out = self.run_command(&line);
                    log::info!("[console] {line} -> {out}");
                }
            }
        }
    }

    /// Built-in developer console: `tp x y z`, `give <weapon>`, `heal`, `kill <name>`, `pos`, `room`, `door open|close|lock|unlock <name>`, `save [slot]`, `load [slot]`.
    pub fn run_command(&mut self, line: &str) -> String {
        let mut it = line.split_whitespace();
        let Some(cmd) = it.next() else { return String::new() };
        let args: Vec<&str> = it.collect();
        let Some(pid) = self.world.player else { return "no player".into() };
        match cmd {
            "pos" => self.world.pos(pid).map(|p| format!("{:.1} {:.1} {:.1}", p.x, p.y, p.z)).unwrap_or_default(),
            "tp" if args.len() == 3 => {
                let v: Vec<f32> = args.iter().filter_map(|a| a.parse().ok()).collect();
                if v.len() != 3 {
                    return "usage: tp x y z".into();
                }
                let p = v3(v[0], v[1], v[2]);
                if let Some(b) = self.world.bodies.get_mut(pid) {
                    b.pos = p;
                    b.vel_y = 0.0;
                    b.last_safe = p;
                }
                let yaw = self.world.transforms.get(pid).map(|t| t.yaw).unwrap_or(0.0);
                self.world.set_pose(pid, p, yaw);
                "ok".into()
            }
            "give" if args.len() == 1 => {
                let data = self.world.data.clone();
                match self.world.players.get_mut(pid) {
                    Some(p) if data.weapons.contains_key(args[0]) => {
                        p.give(args[0], &data);
                        "ok".into()
                    }
                    _ => format!("unknown weapon '{}'", args[0]),
                }
            }
            "weapon" => {
                // weapon [id | list]: gives a weapon of the table and makes it the one in hand (no id: lists the ids)
                let data = self.world.data.clone();
                match args.first() {
                    None => format!("weapons: {}", data.weapons.keys().cloned().collect::<Vec<_>>().join(" ")),
                    Some(&"list") => data.weapons.keys().cloned().collect::<Vec<_>>().join("\n"),
                    Some(id) => match self.world.players.get_mut(pid) {
                        Some(p) if data.weapons.contains_key(*id) => {
                            p.give(id, &data);
                            if let Some(i) = p.items.iter().position(|it| it.id == *id) {
                                p.current = i;
                            }
                            format!("gave {id} (reserve {})", p.items[p.current].reserve)
                        }
                        _ => format!("unknown weapon '{id}' (try: weapon list)"),
                    },
                }
            }
            "save" => {
                let slot = args.first().copied().unwrap_or("quick");
                let label = self.save_label();
                match self.save_slot(slot, &label) {
                    Ok(p) => format!("saved to {}", p.display()),
                    Err(e) => format!("save failed: {e}"),
                }
            }
            "load" => {
                let slot = args.first().copied().unwrap_or("quick");
                match self.load_slot(slot) {
                    Ok(r) => format!("loaded: {} NPCs, {} doors, {} pickups", r.npcs, r.doors, r.pickups),
                    Err(e) => format!("load failed: {e}"),
                }
            }
            "door" if args.len() >= 2 => {
                // door open|close|lock|unlock <name substring>: affects every door leaf whose name matches
                let needle = args[1].to_ascii_lowercase();
                let from = self.world.pos(pid).unwrap_or(Vec3::ZERO);
                let ids: Vec<EntityId> = self.world.doors.iter().filter(|(i, _)| self.world.infos.get(*i).is_some_and(|n| n.name.to_ascii_lowercase().contains(&needle))).map(|(i, _)| i).collect();
                let mut n = 0;
                for id in ids {
                    match args[0] {
                        "open" => n += self.world.set_door(id, true, from, false) as usize,
                        "close" => n += self.world.set_door(id, false, from, false) as usize,
                        "lock" | "unlock" => {
                            if let Some(d) = self.world.doors.get_mut(id) {
                                d.locked = args[0] == "lock";
                                n += 1;
                            }
                        }
                        _ => {}
                    }
                }
                format!("{n} door leaves changed")
            }
            "room" => {
                let idx = &self.world.rooms;
                let neighbors: Vec<&str> = self.world.areas.player_room.map(|r| idx.neighbors(r)).unwrap_or_default().into_iter().filter_map(|n| idx.room(n)).map(|r| r.name.as_str()).collect();
                format!(
                    "{} | location {} | zone {} | leads to {}",
                    self.world.player_room_name().unwrap_or("(none)"),
                    self.ui.location.as_deref().unwrap_or("(none)"),
                    self.world.player_zone_name().unwrap_or("(none)"),
                    if neighbors.is_empty() { "(unknown)".to_string() } else { neighbors.join(", ") }
                )
            }
            "heal" => {
                if let Some(h) = self.world.healths.get_mut(pid) {
                    h.hp = h.max;
                }
                "ok".into()
            }
            "notoriety" => {
                if let Some(v) = args.first().and_then(|a| a.parse::<f32>().ok()) {
                    let d = v - self.world.stealth.notoriety;
                    self.world.add_notoriety(d, "console");
                }
                format!("notoriety {:.0}, {} witnesses", self.world.stealth.notoriety, self.world.stealth.witnesses)
            }
            "zone" => format!(
                "zone state {:#x} ({}), override {:#x}",
                self.world.zone_state(),
                self.world.player_zone_name().unwrap_or("-"),
                self.world.stealth.zone_override
            ),
            "difficulty" => {
                if let Some(v) = args.first().and_then(|a| a.parse::<usize>().ok()) {
                    self.world.stealth.difficulty = v.min(3);
                }
                format!("difficulty {} (0 Rookie .. 3 Professional)", self.world.stealth.difficulty)
            }
            "disguise" => match args.first().copied() {
                Some("off") => {
                    self.world.drop_disguise();
                    "own suit".into()
                }
                Some(name) => {
                    let ids = self.world.find_containing(name);
                    let ok = ids.into_iter().any(|id| {
                        if self.world.is_alive(id) {
                            self.world.kill_now(id, Some(pid), Some("console"));
                        }
                        self.world.take_disguise(id)
                    });
                    if ok { "disguised".into() } else { "no such body".into() }
                }
                None => format!("{}", self.world.stealth.to_json()),
            },
            "kill" if !args.is_empty() => {
                let ids = self.world.find_containing(args[0]);
                let mut n = 0;
                for id in ids {
                    if self.world.infos.get(id).is_some_and(|i| i.kind == Kind::Npc) && self.world.is_alive(id) {
                        self.world.kill_now(id, Some(pid), Some("console"));
                        n += 1;
                    }
                }
                format!("killed {n}")
            }
            _ => format!("unknown command '{cmd}'"),
        }
    }

    // ------------------------------------------------------------------------------ presentation

    fn update_ui(&mut self, dt: f32) {
        for m in &mut self.ui.messages {
            m.remaining -= dt;
        }
        self.ui.messages.retain(|m| m.remaining > 0.0);
        let w = &self.world;
        let level = AudioDirector::compute_alert(w);
        self.ui.alert_level = match level {
            AlertLevel::Calm => 0,
            AlertLevel::Suspicious => 1,
            AlertLevel::Alert => 2,
            AlertLevel::Combat => 3,
        };
        self.ui.mission_status = w.mission.status;
        self.ui.mission_title = w.mission.title().to_string();
        self.ui.objectives = w.mission.objectives.clone();
        self.ui.prompts.clear();
        if let Some(pid) = w.player {
            if let Some(h) = w.healths.get(pid) {
                self.ui.vars.insert("health".into(), json!((h.hp / h.max.max(1.0) * 100.0).round()));
            }
            if let Some(p) = w.players.get(pid) {
                if let Some(pr) = &p.prompt {
                    self.ui.prompts.push(pr.text.clone());
                }
                let (clip, reserve, name) = match p.current_item() {
                    Some(it) => (it.mag, it.reserve, w.data.weapon(&it.id).map(|d| d.name.clone()).unwrap_or_else(|| it.id.clone())),
                    None => (0, 0, String::new()),
                };
                self.ui.vars.insert("ammo_clip".into(), json!(clip));
                self.ui.vars.insert("ammo_reserve".into(), json!(reserve));
                self.ui.vars.insert("weapon".into(), json!(name));
                self.ui.crosshair = p.aim || p.drawn;
            }
        }
        self.ui.room = w.player_room_name().map(str::to_string);
        self.ui.location = w.areas.player_room.and_then(|r| w.rooms.room(r)).map(|r| r.location.clone());
        self.ui.zone = w.player_zone_name().map(str::to_string);
        self.ui.vars.insert("room".into(), json!(self.ui.room.clone().unwrap_or_default()));
        self.ui.vars.insert("notoriety".into(), json!(w.stealth.notoriety));
        self.ui.vars.insert("disguised".into(), json!(w.stealth.disguise.is_some()));
        self.ui.vars.insert("zone_state".into(), json!(w.zone_state()));
        self.ui.vars.insert("alert".into(), json!(self.ui.alert_level));
        self.ui.vars.insert("time".into(), json!(w.time));
    }

    fn skeleton_for(&mut self, prim: u32) -> Option<Arc<Skeleton>> {
        if let Some(s) = self.skel_cache.get(&prim) {
            return s.clone();
        }
        let sk = self.scene.as_ref().and_then(|sc| sc.model(prim)).and_then(|m| m.skeleton.clone());
        // attach the pose names of the level's ANM to skeletons that carry a face bank
        let sk = match (sk, self.scene.as_ref().and_then(|sc| sc.anim_bank())) {
            (Some(s), Some(b)) if !s.face.is_empty() && s.facial_poses.is_empty() => {
                let mut c = (*s).clone();
                c.facial_poses = b.facial_poses.clone();
                Some(Arc::new(c))
            }
            (s, _) => s,
        };
        self.skel_cache.insert(prim, sk.clone());
        sk
    }

    /// Weapon model groups of the scene (built on first use).
    fn weapon_models(&mut self) -> Option<Arc<WeaponModels>> {
        if self.weapon_models.is_none() {
            self.weapon_models = self.scene.as_ref().map(|s| Arc::new(WeaponModels::build(s)));
        }
        self.weapon_models.clone().filter(|m| !m.is_empty())
    }

    /// `(node, group -> model matrix, model name, part ids)` of the model a pickup shows: the original item's model, else the
    /// stand-in weapon's.
    fn pickup_model(&self, wm: &WeaponModels, p: &crate::world::PickupComp) -> Option<(usize, bm_models::Mat4, String, Vec<u32>)> {
        let data = &self.world.data;
        if let Some((n, m)) = wm.get(&p.item) {
            return Some((n, m, p.item.clone(), data.parts_for_model(&p.item)));
        }
        let d = data.weapon(&p.weapon)?;
        wm.get(&d.model).map(|(n, m)| (n, m, d.model.clone(), d.visible_parts()))
    }

    /// Every weapon model the renderer has to create (one per owner and weapon): all weapons the player may hold, the weapon of
    /// each NPC and each pickup. `bm-app` uploads them once as extra scene items, [`RenderState::props`] says which are visible.
    pub fn prop_slots(&mut self) -> Vec<PropSlot> {
        let Some(wm) = self.weapon_models() else { return Vec::new() };
        let w = &self.world;
        let mut out = Vec::new();
        let model_of = |wid: &str| w.data.weapon(wid).and_then(|d| wm.get(&d.model).map(|(node, _)| (node, d.model.clone(), d.visible_parts())));
        for (id, _) in w.players.iter() {
            for wid in w.data.weapons.keys() {
                if let Some((node, model, parts)) = model_of(wid) {
                    out.push(PropSlot { key: format!("h{}:{}", id.0, wid), node, model, parts });
                }
            }
        }
        for (id, c) in w.characters.iter() {
            if let Some(wid) = &c.weapon {
                if let Some((node, model, parts)) = model_of(wid) {
                    out.push(PropSlot { key: format!("h{}:{}", id.0, wid), node, model, parts });
                }
            }
        }
        for (id, p) in w.pickups.iter() {
            if let Some((node, _, model, parts)) = self.pickup_model(&wm, p) {
                out.push(PropSlot { key: format!("g{}", id.0), node, model, parts });
            }
        }
        out
    }

    /// Plays a dialogue line's lip sync on a character's face (see [`crate::face`]).
    pub fn speak(&mut self, entity: u64, lip: Arc<bm_sound::LipSync>) {
        let now = self.world.time as f32;
        self.faces.entry(entity).or_insert_with(|| crate::face::FaceController::new(entity)).speak(lip, now);
    }

    /// Everything the renderer needs for the current state (poses are sampled from the animation provider).
    pub fn render_state(&mut self) -> RenderState {
        let camera = self.world.camera();
        let ids: Vec<EntityId> = self.world.renders.iter().map(|(i, _)| i).collect();
        let mut entities = Vec::with_capacity(ids.len());
        let weapon_models = self.weapon_models();
        let mut props: Vec<PropDraw> = Vec::new();
        for id in ids {
            let w = &self.world;
            let (Some(r), Some(info), Some(tr)) = (w.renders.get(id), w.infos.get(id), w.transforms.get(id)) else { continue };
            if !matches!(info.kind, Kind::Player | Kind::Npc | Kind::Door) {
                continue;
            }
            let anim = w.anims.get(id).cloned();
            let (node, prim, mut visible) = (r.node, r.prim_id, r.visible);
            if info.kind == Kind::Player {
                // camera squeezed against geometry right behind the player: hide the model instead of clipping through it
                if let (Some(c), Some(pos)) = (&camera, w.transforms.get(id).map(|t| t.pos)) {
                    let head = pos + crate::math::v3(0.0, if w.players.get(id).is_some_and(|p| p.crouching) { 115.0 } else { 155.0 }, 0.0);
                    if (c.pos - head).len() < crate::player::CAMERA_HIDE_DIST {
                        visible = false;
                    }
                }
            }
            let world_m = if matches!(info.kind, Kind::Player | Kind::Npc) { crate::math::character_render(&tr.world) } else { tr.world };
            let (kind, name, alive) = (info.kind, info.name.clone(), info.alive);
            let alert = w.ais.get(id).map(|a| a.state);
            let flash = w.healths.get(id).map(|h| (h.hit_flash / 0.25).clamp(0.0, 1.0)).unwrap_or(0.0);
            // weapon in hand: the player's drawn weapon, an NPC's weapon once it is alerted
            let held: Option<String> = match kind {
                Kind::Player => w.players.get(id).filter(|p| p.drawn && alive).and_then(|p| p.current_item()).map(|i| i.id.clone()),
                Kind::Npc if alive && matches!(alert, Some(AlertState::Alerted)) => w.characters.get(id).and_then(|c| c.weapon.clone()),
                _ => None,
            };
            let held_model = held.as_ref().and_then(|wid| w.data.weapon(wid).map(|d| (wid.clone(), d.model.clone())));
            let mut hand = None;
            let mut grip = None;
            let mut skinning = None;
            let donor = if kind == Kind::Player { self.world.stealth.disguise.as_ref().and_then(|d| d.node.zip(d.prim).map(|(n, p)| (n, p, d.colleague_outfit))) } else { None };
            if let (Some(a), Some(p)) = (&anim, donor.map(|d| d.1).or(prim)) {
                if matches!(kind, Kind::Player | Kind::Npc) {
                    if let Some(sk) = self.skeleton_for(p) {
                        let aiming = matches!(a.state, AnimState::Aim | AnimState::Fire);
                        let hold: Option<Arc<str>> = match (&held_model, kind) {
                            (Some((wid, _)), Kind::Player) => self.world.data.weapon(wid).and_then(|d| hold_clip_name(d, aiming)).map(Arc::from),
                            _ => None,
                        };
                        let aim_pitch = if kind == Kind::Player { self.world.players.get(id).map(|p| p.cam_pitch).unwrap_or(0.0) } else { 0.0 };
                        let req = AnimRequest { state: a.state, time: a.time, phase: a.phase, speed: a.speed, dead_blend: a.dead_blend, hero: kind == Kind::Player, hold, aim_pitch };
                        let mut pose = self.provider.sample(&sk, &req);
                        if !sk.face.is_empty() {
                            let now = self.world.time as f32;
                            let wts = if self.face_debug.is_empty() {
                                self.faces.entry(id.0 as u64).or_insert_with(|| crate::face::FaceController::new(id.0 as u64)).weights(&sk, id.0 as u64, now)
                            } else {
                                self.face_debug.iter().filter_map(|(n, w)| sk.face_pose_index(n).map(|i| (i, *w))).collect()
                            };
                            sk.apply_face(&mut pose, &wts);
                        }
                        skinning = Some(sk.skinning_matrices(&pose));
                        hand = crate::props::hand_matrix(&sk, &pose);
                        grip = self.provider.weapon_grip(&sk);
                    }
                }
            }
            if let (Some((wid, model)), Some(hand), Some(wm)) = (held_model, hand, &weapon_models) {
                if visible {
                    if let Some((_, local)) = wm.get(&model) {
                        let g = grip.unwrap_or_else(|| crate::props::grip(HOLD_GRIP_ROT, HOLD_GRIP_POS));
                        let grip_world = world_m.mul(&hand).mul(&g);
                        props.push(PropDraw { key: format!("h{}:{}", id.0, wid), world: grip_world.mul(&local) });
                        match wm.muzzle(&model) {
                            Some(m) => {
                                let p = grip_world.transform_point(m);
                                self.muzzles.insert(id, crate::math::v3(p[0], p[1], p[2]));
                            }
                            None => {
                                self.muzzles.remove(&id);
                            }
                        }
                    }
                }
            }
            let outfit = match (&self.scene, node, prim) {
                (Some(sc), Some(_), Some(p)) if kind == Kind::Player => sc.model(p).map(|m| m.resolve_outfit(self.world.stealth.outfit())).unwrap_or(1),
                (Some(sc), Some(n), Some(p)) => match (sc.actor_outfit(n), sc.model(p)) {
                    (Some(o), Some(m)) => m.resolve_outfit(o),
                    (None, Some(m)) => m.resolve_outfit(1),
                    _ => 1,
                },
                _ => 1,
            };
            let disguise = donor.and_then(|(n, p, o)| self.scene.as_ref().and_then(|sc| sc.model(p)).map(|m| (n, m.resolve_outfit(o))));
            entities.push(RenderEntity {
                id: id.0,
                disguise,
                name,
                kind,
                node,
                prim_id: prim,
                outfit,
                world: world_m,
                visible,
                skinning,
                anim: anim.map(|a| a.state).unwrap_or_default(),
                alert,
                hit_flash: flash,
                alive,
            });
        }
        // hidden nodes: picked-up items
        let mut hidden = Vec::new();
        if let Some(sc) = &self.scene {
            for (id, p) in self.world.pickups.iter() {
                if p.taken {
                    if let Some(n) = self.world.infos.get(id).and_then(|i| i.node) {
                        hidden.extend(sc.subtree(n));
                    }
                } else if let (Some(wm), Some(n)) = (&weapon_models, self.world.infos.get(id).and_then(|i| i.node)) {
                    // the weapon lying there: the original item model, else the stand-in weapon's
                    if let Some((_, local, _, _)) = self.pickup_model(wm, p) {
                        props.push(PropDraw { key: format!("g{}", id.0), world: sc.nodes[n].world.mul(&local) });
                    }
                }
            }
        }
        // lights near the camera
        let mut lights: Vec<(f32, RenderLight)> = Vec::new();
        if let Some(c) = &camera {
            for (id, l) in self.world.lights.iter() {
                if !l.on {
                    continue;
                }
                let Some(t) = self.world.transforms.get(id) else { continue };
                let d = t.pos.dist(c.pos);
                if d < 6000.0 {
                    lights.push((d, RenderLight { position: t.pos.to_array(), color: l.color, range: l.range, range_start: l.range_start, intensity: l.intensity, spot: l.kind == LightKind::Spot, direction: l.direction, cone: l.cone }));
                }
            }
            lights.sort_by(|a, b| a.0.total_cmp(&b.0));
            lights.truncate(32);
        }
        let particles = match (&self.fx, &camera) {
            (Some(f), Some(c)) => f.collect(c.pos, c.forward),
            _ => Default::default(),
        };
        RenderState {
            time: self.world.time,
            tick: self.world.tick,
            camera,
            entities,
            hidden_nodes: hidden,
            props,
            lights: lights.into_iter().map(|l| l.1).collect(),
            particles,
        }
    }

    pub fn ui_state(&self) -> &UiState {
        &self.ui
    }

    /// Highest alert state among living NPCs (handy for tests / debug overlays).
    pub fn max_npc_alert(&self) -> AlertState {
        let rank = |s: AlertState| match s {
            AlertState::Dead | AlertState::Idle => 0,
            AlertState::Suspicious => 1,
            AlertState::Searching => 2,
            AlertState::Fleeing => 3,
            AlertState::Alerted => 4,
        };
        self.world.ais.iter().filter(|(_, a)| !a.dead).map(|(_, a)| a.state).max_by_key(|s| rank(*s)).unwrap_or(AlertState::Idle)
    }
}

fn template_kind(t: &str) -> &'static str {
    if t.starts_with("npc_") {
        "npc"
    } else if t.starts_with("pickup") {
        "pickup"
    } else if t == "light" {
        "light"
    } else if t == "marker" {
        "marker"
    } else {
        "prop"
    }
}

impl World {
    /// Creates an entity from a script template (`npc_guard`, `npc_civilian`, `npc_target`, `pickup[:weapon]`,
    /// `marker`, `prop`, `light`) with a pre-allocated id.
    pub fn spawn_template(&mut self, id: EntityId, req: &SpawnRequest) -> Result<(), String> {
        let pos: Vec3 = req.position.into();
        let yaw = req.rotation.y.to_radians();
        let name = req.name.clone().unwrap_or_else(|| format!("{}#{}", req.template, id.0));
        let t = req.template.as_str();
        match t {
            "npc_guard" | "npc_civilian" | "npc_target" => {
                let role = match t {
                    "npc_guard" => Role::Guard,
                    "npc_target" => Role::Target,
                    _ => Role::Civilian,
                };
                self.spawn_npc_as(id, &name, role, pos, yaw, Vec::new(), None, None);
            }
            "light" => {
                self.spawn_with_id(id, &name, Kind::Light, "light", Transform::from_pose(pos, yaw));
                self.lights.insert(id, LightComp::omni_default());
            }
            "marker" | "prop" => {
                let kind = if t == "marker" { Kind::Marker } else { Kind::Prop };
                self.spawn_with_id(id, &name, kind, t, Transform::from_pose(pos, yaw));
            }
            _ if t.starts_with("pickup") => {
                let w = t.strip_prefix("pickup:").unwrap_or("hammer");
                if !self.data.weapons.contains_key(w) {
                    return Err(format!("unknown weapon '{w}'"));
                }
                self.spawn_with_id(id, &name, Kind::Pickup, "pickup", Transform::from_pose(pos, yaw));
                self.renders.insert(id, RenderComp { node: None, prim_id: None, visible: true });
                self.pickups.insert(id, PickupComp { weapon: w.to_string(), taken: false, item: String::new() });
            }
            _ => return Err(format!("unknown template '{t}'")),
        }
        if let Some(i) = self.infos.get_mut(id) {
            i.model = req.model.clone();
        }
        for (k, v) in &req.components {
            self.set_component_json(id, k, v);
        }
        Ok(())
    }
}

/// A visible muzzle farther than this from the shot origin (cm) is stale (weapon holstered / re-drawn) and ignored.
const MUZZLE_MAX_DIST: f32 = 150.0;

/// Rotation (degrees about X, Y, Z) and offset (cm) that put a weapon's grip space into the right hand attacher frame.
const HOLD_GRIP_ROT: [f32; 3] = [0.0, 0.0, 0.0];
const HOLD_GRIP_POS: [f32; 3] = [0.0, 0.0, 0.0];

/// Upper body clip for a held weapon: the aimed 1P pose of the weapon, else the generic hold of its kind.
fn hold_clip_name(d: &crate::data::WeaponDef, aiming: bool) -> Option<String> {
    if aiming && !d.hold.is_empty() {
        return Some(format!("Hero/Hero_1Person#{}", d.hold));
    }
    match d.kind {
        WeaponKind::Pistol if aiming => Some("Hero/Hero_UB#UB_Hold_Gun_Front".into()),
        WeaponKind::Pistol => Some("Hero/Hero_UB#UB_Hold_Gun_Side".into()),
        WeaponKind::Smg => Some("Hero/Hero_UB_WeaponHandling#UB_Hold_SMG".into()),
        WeaponKind::Rifle | WeaponKind::Shotgun | WeaponKind::Sniper => Some("Hero/Hero_UB_WeaponHandling#UB_Hold_Rifle_Special".into()),
        _ => None,
    }
}
