//! NPC AI: perception in the world, decisions in a replaceable [`AiBrain`].
//!
//! The original AI (native `ZHM3Actor` logic, `.OCT`/graph data) is not decoded. This is a small stand-in:
//!
//! * perception (`World::update_ai`): vision cone + line-of-sight rays towards the player, hearing of
//!   weapon noise, witnessing kills -> a *suspicion* value 0..1+,
//! * [`SimpleBrain`]: idle/patrol along waypoints -> suspicious -> alerted (guards shoot, civilians flee)
//!   -> searching -> back to idle,
//! * movement: A* paths over the lazily built walkable grid ([`crate::nav`]) followed by the character
//!   controller.
//!
//! Replace the decision part per NPC role with `World::brain_factory` (any `AiBrain`).

use crate::anim::AnimState;
use crate::controller::step_body;
use crate::math::{Vec3, angle_diff, forward, turn_towards, v3, yaw_of};
use crate::nav::find_path;
use crate::world::{EntityId, FxEvent, World};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Role {
    Guard,
    Civilian,
    Target,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Guard => "guard",
            Role::Civilian => "civilian",
            Role::Target => "target",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Default)]
pub enum AlertState {
    #[default]
    Idle,
    Suspicious,
    Searching,
    Alerted,
    Fleeing,
    Dead,
}

impl AlertState {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertState::Idle => "idle",
            AlertState::Suspicious => "suspicious",
            AlertState::Searching => "searching",
            AlertState::Alerted => "alerted",
            AlertState::Fleeing => "fleeing",
            AlertState::Dead => "dead",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveSpeed {
    Walk,
    Run,
}

/// What a brain wants this tick.
#[derive(Clone, Debug, PartialEq)]
pub struct AiCommand {
    pub state: AlertState,
    pub goal: Option<(Vec3, MoveSpeed)>,
    pub look_at: Option<Vec3>,
    pub fire: bool,
    /// Move on to the next route waypoint (the brain decided it has waited long enough).
    pub advance_route: bool,
    /// Overwrite the suspicion value (used when calming down after a search / escape).
    pub set_suspicion: Option<f32>,
}

impl AiCommand {
    pub fn stay(state: AlertState) -> AiCommand {
        AiCommand { state, goal: None, look_at: None, fire: false, advance_route: false, set_suspicion: None }
    }
}

/// Read-only snapshot a brain decides on.
pub struct AiView<'a> {
    pub me: EntityId,
    pub role: Role,
    pub pos: Vec3,
    pub yaw: f32,
    pub time: f64,
    pub state: AlertState,
    pub state_time: f32,
    pub suspicion: f32,
    pub sees_player: bool,
    pub player_pos: Option<Vec3>,
    pub player_dist: f32,
    pub last_seen: Option<Vec3>,
    pub time_since_seen: f32,
    pub heard: Option<Vec3>,
    pub route: &'a [Vec3],
    pub route_idx: usize,
    /// Help points (`Helplist` of the actor's script): civilians run to the nearest one when alarmed.
    pub help_points: &'a [Vec3],
    /// The previous move goal was reached.
    pub arrived: bool,
    pub home: Vec3,
    pub has_weapon: bool,
    pub weapon_range: f32,
    pub clear_shot: bool,
    pub fire_ready: bool,
    pub hp_frac: f32,
    /// Two uniform random numbers in `[0,1)` from the world RNG (keeps brains stateless w.r.t. determinism).
    pub rand: [f32; 2],
}

pub trait AiBrain: Send {
    fn name(&self) -> &'static str;
    fn think(&mut self, v: &AiView, dt: f32) -> AiCommand;
}

/// Default decision logic (see module docs).
pub struct SimpleBrain {
    pub role: Role,
    wait: f32,
    search_target: Option<Vec3>,
}

impl SimpleBrain {
    pub fn new(role: Role) -> SimpleBrain {
        SimpleBrain { role, wait: 0.0, search_target: None }
    }
}

impl AiBrain for SimpleBrain {
    fn name(&self) -> &'static str {
        "simple"
    }

    fn think(&mut self, v: &AiView, dt: f32) -> AiCommand {
        let armed_role = v.role == Role::Guard && v.has_weapon;
        // escalation, independent of the current state
        if v.state != AlertState::Dead && v.suspicion >= 1.0 {
            let st = if armed_role { AlertState::Alerted } else { AlertState::Fleeing };
            return self.act(v, st, dt);
        }
        match v.state {
            AlertState::Idle => {
                if v.suspicion >= 0.35 {
                    return self.act(v, AlertState::Suspicious, dt);
                }
                self.act(v, AlertState::Idle, dt)
            }
            AlertState::Suspicious => {
                if v.suspicion < 0.12 && v.state_time > 2.0 {
                    if v.heard.is_some() || v.last_seen.is_some() {
                        self.search_target = v.heard.or(v.last_seen);
                        return self.act(v, AlertState::Searching, dt);
                    }
                    return self.act(v, AlertState::Idle, dt);
                }
                self.act(v, AlertState::Suspicious, dt)
            }
            AlertState::Searching => {
                if v.state_time > 14.0 && v.suspicion < 0.3 {
                    return self.act(v, AlertState::Idle, dt);
                }
                self.act(v, AlertState::Searching, dt)
            }
            AlertState::Alerted => {
                if !v.sees_player && v.time_since_seen > 8.0 && v.state_time > 8.0 {
                    self.search_target = v.last_seen.or(v.heard);
                    let mut c = self.act(v, AlertState::Searching, dt);
                    c.set_suspicion = Some(0.6);
                    return c;
                }
                self.act(v, AlertState::Alerted, dt)
            }
            AlertState::Fleeing => {
                if !v.sees_player && v.time_since_seen > 9.0 && v.state_time > 9.0 {
                    let mut c = self.act(v, AlertState::Suspicious, dt);
                    c.set_suspicion = Some(0.5);
                    return c;
                }
                self.act(v, AlertState::Fleeing, dt)
            }
            AlertState::Dead => AiCommand::stay(AlertState::Dead),
        }
    }
}

impl SimpleBrain {
    fn act(&mut self, v: &AiView, st: AlertState, dt: f32) -> AiCommand {
        let mut c = AiCommand::stay(st);
        match st {
            AlertState::Idle => {
                if !v.route.is_empty() {
                    let target = v.route[v.route_idx % v.route.len()];
                    if v.arrived || v.pos.dist_xz(target) < 80.0 {
                        if self.wait <= 0.0 {
                            self.wait = 1.5 + v.rand[0] * 3.0;
                        }
                        self.wait -= dt;
                        if self.wait <= 0.0 {
                            c.advance_route = true;
                            self.wait = 0.0;
                        }
                    } else {
                        c.goal = Some((target, MoveSpeed::Walk));
                    }
                } else if v.pos.dist_xz(v.home) > 120.0 {
                    c.goal = Some((v.home, MoveSpeed::Walk));
                }
            }
            AlertState::Suspicious => {
                c.look_at = v.last_seen.or(v.heard).or(v.player_pos.filter(|_| v.sees_player));
                if v.sees_player && v.role == Role::Guard && v.player_dist > 500.0 {
                    c.goal = v.player_pos.map(|p| (p, MoveSpeed::Walk));
                }
            }
            AlertState::Searching => {
                let t = self.search_target.or(v.last_seen).unwrap_or(v.home);
                if v.arrived || v.pos.dist_xz(t) < 100.0 {
                    // look around: pick a point near the search position
                    let a = v.rand[0] * std::f32::consts::TAU;
                    let r = 300.0 + v.rand[1] * 500.0;
                    self.search_target = Some(t + v3(a.sin() * r, 0.0, a.cos() * r));
                } else {
                    c.goal = Some((t, MoveSpeed::Walk));
                }
            }
            AlertState::Alerted => {
                let want = (v.weapon_range * 0.45).clamp(500.0, 1800.0);
                match (v.sees_player, v.player_pos) {
                    (true, Some(p)) => {
                        c.look_at = Some(p);
                        if v.player_dist > want || !v.clear_shot {
                            c.goal = Some((p, MoveSpeed::Run));
                        }
                        c.fire = v.has_weapon && v.clear_shot && v.fire_ready && v.player_dist < v.weapon_range && v.state_time > 0.5;
                    }
                    _ => {
                        if let Some(p) = v.last_seen.or(v.heard) {
                            c.goal = Some((p, MoveSpeed::Run));
                            c.look_at = Some(p);
                        }
                    }
                }
            }
            AlertState::Fleeing => {
                if let Some(p) = v.player_pos.or(v.last_seen) {
                    // run to the nearest help point that is not nearer to the threat than we are
                    let mine = v.pos.dist_xz(p);
                    let help = v
                        .help_points
                        .iter()
                        .filter(|h| h.dist_xz(p) > mine + 300.0)
                        .min_by(|a, b| a.dist_xz(v.pos).total_cmp(&b.dist_xz(v.pos)))
                        .copied();
                    match help {
                        Some(h) if v.pos.dist_xz(h) > 150.0 => c.goal = Some((h, MoveSpeed::Run)),
                        Some(_) => {}
                        None => {
                            let away = (v.pos - p).flat().norm_or(forward(v.yaw));
                            c.goal = Some((v.pos + away * 1800.0, MoveSpeed::Run));
                        }
                    }
                }
            }
            AlertState::Dead => {}
        }
        c
    }
}

/// AI component of an NPC.
pub struct AiComp {
    pub brain: Box<dyn AiBrain>,
    pub state: AlertState,
    pub state_time: f32,
    pub suspicion: f32,
    pub sees_player: bool,
    pub last_seen: Option<Vec3>,
    pub last_seen_time: f64,
    pub heard: Option<Vec3>,
    pub route: Vec<Vec3>,
    pub route_idx: usize,
    /// `PathFollower` mode RANDOM: waypoints are visited in random order instead of cyclically.
    pub route_random: bool,
    /// Where the actor runs to for help (decoded `Helplist`).
    pub help_points: Vec<Vec3>,
    pub home: Vec3,
    pub home_yaw: f32,
    pub goal: Option<(Vec3, MoveSpeed)>,
    pub path: Vec<Vec3>,
    pub path_idx: usize,
    pub path_goal: Vec3,
    pub arrived: bool,
    pub repath_cd: f32,
    pub think_cd: f32,
    pub vision_range: f32,
    pub fov_deg: f32,
    pub look_at: Option<Vec3>,
    pub stuck: f32,
    pub clear_shot: bool,
    pub dead: bool,
    /// 0..1: how well this NPC has seen through the player's disguise (`crate::stealth`).
    pub disguise_awareness: f32,
}

impl AiComp {
    pub fn new(brain: Box<dyn AiBrain>, home: Vec3, home_yaw: f32, route: Vec<Vec3>) -> AiComp {
        AiComp {
            brain,
            state: AlertState::Idle,
            state_time: 0.0,
            suspicion: 0.0,
            sees_player: false,
            last_seen: None,
            last_seen_time: -100.0,
            heard: None,
            route,
            route_idx: 0,
            route_random: false,
            help_points: Vec::new(),
            home,
            home_yaw,
            goal: None,
            path: Vec::new(),
            path_idx: 0,
            path_goal: home,
            arrived: false,
            repath_cd: 0.0,
            think_cd: 0.0,
            vision_range: 2800.0,
            fov_deg: 110.0,
            look_at: None,
            stuck: 0.0,
            clear_shot: false,
            dead: false,
            disguise_awareness: 0.0,
        }
    }

    pub fn on_damaged(&mut self, attacker: Option<Vec3>, time: f64) {
        if self.dead {
            return;
        }
        self.suspicion = self.suspicion.max(1.2);
        if let Some(p) = attacker {
            self.last_seen = Some(p);
            self.last_seen_time = time;
        }
    }

    pub fn witness(&mut self, killer: Option<Vec3>, time: f64, killer_is_player: bool) {
        if self.dead {
            return;
        }
        if killer_is_player {
            self.suspicion = self.suspicion.max(1.1);
            if let Some(p) = killer {
                self.last_seen = Some(p);
                self.last_seen_time = time;
            }
        } else {
            self.suspicion = self.suspicion.max(0.5);
        }
    }

    pub fn hear(&mut self, pos: Vec3, loud: bool) {
        if self.dead {
            return;
        }
        self.heard = Some(pos);
        self.suspicion = self.suspicion.max(if loud { 1.0 } else { 0.4 });
    }

    pub fn die(&mut self) {
        self.dead = true;
        self.state = AlertState::Dead;
        self.goal = None;
        self.path.clear();
    }

    pub fn to_json(&self) -> Value {
        json!({
            "brain": self.brain.name(),
            "state": self.state.as_str(),
            "suspicion": self.suspicion,
            "sees_player": self.sees_player,
            "route_points": self.route.len(),
        })
    }
}

impl World {
    /// Runs perception, brains and movement for every NPC.
    pub fn update_ai(&mut self, dt: f32) {
        let player = self.player;
        let player_alive = player.is_some_and(|p| self.is_alive(p));
        let player_pos = player.and_then(|p| self.pos(p));
        let player_info = player.and_then(|p| self.players.get(p)).map(|pl| (pl.weapon_visible(&self.data), pl.crouching, pl.running, pl.current_noise()));
        let mut path_budget = self.path_budget;
        let ids = self.ais.ids();
        for id in ids {
            let Some(mut ai) = self.ais.take(id) else { continue };
            if ai.dead {
                self.ais.insert(id, ai);
                continue;
            }
            let Some(tr) = self.transforms.get(id).copied() else {
                self.ais.insert(id, ai);
                continue;
            };
            // simulation bubble around the player
            let far = player_pos.is_some_and(|pp| pp.dist(tr.pos) > self.ai_radius) && ai.state == AlertState::Idle && ai.suspicion < 0.1;
            if far {
                self.ais.insert(id, ai);
                continue;
            }
            ai.state_time += dt;
            ai.repath_cd -= dt;
            ai.think_cd -= dt;
            let role = self.characters.get(id).map(|c| c.role).unwrap_or(Role::Civilian);
            let eye = tr.pos + v3(0.0, 160.0, 0.0);

            // ---- perception + decision at 10 Hz ----
            if ai.think_cd <= 0.0 {
                ai.think_cd += 0.1;
                let think_dt = 0.1;
                let mut sees = false;
                let mut pdist = f32::INFINITY;
                if let (true, Some(pp), Some((armed_visible, crouch, running, _))) = (player_alive, player_pos, player_info) {
                    pdist = pp.dist(tr.pos);
                    let range = if armed_visible { ai.vision_range * 1.3 } else if crouch { ai.vision_range * 0.5 } else { ai.vision_range };
                    if pdist <= range {
                        let to = (pp - tr.pos).flat();
                        let in_cone = to.len() < 200.0 || angle_diff(tr.yaw, yaw_of(to)).abs() <= (ai.fov_deg * 0.5).to_radians();
                        if in_cone {
                            let head = pp + v3(0.0, if crouch { 100.0 } else { 160.0 }, 0.0);
                            let chest = pp + v3(0.0, 110.0, 0.0);
                            sees = self.collision.line_of_sight(eye, head) || self.collision.line_of_sight(eye, chest);
                        }
                    }
                    if sees {
                        ai.last_seen = Some(pp);
                        ai.last_seen_time = self.time;
                        // clothes of somebody else: bystanders accept them, colleagues of the owner slowly see through them
                        let outfit = self.characters.get(id).map(|c| c.outfit).unwrap_or(1);
                        let through = self.disguise_awareness_step(&mut ai.disguise_awareness, role, outfit, pdist, true, think_dt);
                        // zone state 0x80 (eZoneMegaForbidden) sets the detection rate to 100 (`FUN_005bd8a0`, verified)
                        let zone = self.zone_state();
                        // GUESS: clothes out of place are noticed twice as fast and a disguise there is not accepted
                        let oop = self.clothes_out_of_place();
                        let disguised = self.stealth.disguise.is_some() && !through && !oop;
                        let oop_mult = if oop { 2.0 } else { 1.0 };
                        let rate = match role {
                            _ if zone == crate::stealth::ZONE_MEGA_FORBIDDEN => 100.0,
                            _ if armed_visible => 2.2,
                            _ if disguised => 0.0,
                            Role::Guard if pdist < 500.0 => 0.35,
                            Role::Guard if running => 0.18,
                            Role::Guard => 0.06,
                            _ => 0.0,
                        };
                        ai.suspicion += rate * oop_mult * think_dt;
                    }
                }
                if !sees {
                    let outfit = self.characters.get(id).map(|c| c.outfit).unwrap_or(1);
                    self.disguise_awareness_step(&mut ai.disguise_awareness, role, outfit, pdist, false, think_dt);
                }
                ai.sees_player = sees;
                // hearing
                for n in &self.noises {
                    if self.time - n.time < 0.15 && n.source != id && n.pos.dist(tr.pos) <= n.radius && Some(n.source) == player {
                        ai.hear(n.pos, n.radius > 1500.0);
                    }
                }
                if !sees && ai.state != AlertState::Alerted && ai.state != AlertState::Fleeing {
                    ai.suspicion = (ai.suspicion - 0.1 * think_dt).max(0.0);
                } else if !sees {
                    ai.suspicion = ai.suspicion.max(1.0);
                }
                // clear shot
                let ch = self.characters.get(id);
                let weapon = ch.and_then(|c| c.weapon.clone());
                let wdef = weapon.as_ref().and_then(|w| self.data.weapon(w));
                let weapon_range = wdef.map(|w| w.range).unwrap_or(0.0);
                ai.clear_shot = sees
                    && player_pos.is_some_and(|pp| {
                        let d = (pp + v3(0.0, 120.0, 0.0)) - eye;
                        match self.hitscan(eye, d.norm(), d.len(), Some(id)) {
                            None => true,
                            Some(h) => h.entity == player,
                        }
                    });
                let tss = (self.time - ai.last_seen_time) as f32;
                let fire_ready = ch.map(|c| c.fire_cd <= 0.0).unwrap_or(false);
                let hp_frac = self.healths.get(id).map(|h| h.hp / h.max.max(1.0)).unwrap_or(1.0);
                let rand = [self.rng.f32(), self.rng.f32()];
                let view = AiView {
                    me: id,
                    role,
                    pos: tr.pos,
                    yaw: tr.yaw,
                    time: self.time,
                    state: ai.state,
                    state_time: ai.state_time,
                    suspicion: ai.suspicion,
                    sees_player: sees,
                    player_pos,
                    player_dist: pdist,
                    last_seen: ai.last_seen,
                    time_since_seen: tss,
                    heard: ai.heard,
                    route: &ai.route,
                    route_idx: ai.route_idx,
                    help_points: &ai.help_points,
                    arrived: ai.arrived,
                    home: ai.home,
                    has_weapon: weapon.is_some(),
                    weapon_range,
                    clear_shot: ai.clear_shot,
                    fire_ready,
                    hp_frac,
                    rand,
                };
                let cmd = ai.brain.think(&view, think_dt);
                if cmd.state != ai.state {
                    ai.state = cmd.state;
                    ai.state_time = 0.0;
                    ai.arrived = false;
                    ai.path.clear();
                    if matches!(cmd.state, AlertState::Idle) {
                        ai.heard = None;
                    }
                }
                if cmd.advance_route && !ai.route.is_empty() {
                    ai.route_idx = if ai.route_random && ai.route.len() > 1 {
                        let next = self.rng.below(ai.route.len());
                        if next == ai.route_idx { (next + 1) % ai.route.len() } else { next }
                    } else {
                        (ai.route_idx + 1) % ai.route.len()
                    };
                    ai.arrived = false;
                }
                if let Some(s) = cmd.set_suspicion {
                    ai.suspicion = s;
                }
                ai.look_at = cmd.look_at;
                // goal change -> repath
                match (cmd.goal, ai.goal) {
                    (Some((g, _)), Some((og, _))) if g.dist_xz(og) < 150.0 => {}
                    (Some(_), _) => {
                        ai.path.clear();
                        ai.arrived = false;
                    }
                    (None, Some(_)) => ai.path.clear(),
                    _ => {}
                }
                ai.goal = cmd.goal;
                if cmd.fire {
                    if let (Some(w), Some(pp)) = (weapon, player_pos) {
                        let target = pp + v3(0.0, 115.0, 0.0);
                        let dir = (target - eye).norm();
                        self.fire_weapon(id, &w, eye, dir);
                        let rate = self.data.weapon(&w).map(|w| w.fire_rate).unwrap_or(1.0).max(0.1);
                        if let Some(c) = self.characters.get_mut(id) {
                            c.fire_cd = 1.0 / rate;
                        }
                    }
                }
            }

            // ---- movement ----
            let (walk, run) = self.characters.get(id).map(|c| (c.walk_speed, c.run_speed)).unwrap_or((150.0, 380.0));
            let mut desired = Vec3::ZERO;
            let mut speed = 0.0;
            let mut face = None;
            if let Some((goal, ms)) = ai.goal {
                speed = if ms == MoveSpeed::Run { run } else { walk };
                let need_path = ai.path.is_empty() || ai.path_idx >= ai.path.len();
                if need_path && !ai.arrived && ai.repath_cd <= 0.0 && path_budget > 0 {
                    path_budget -= 1;
                    ai.repath_cd = 0.8;
                    let from = tr.pos;
                    match find_path(&self.collision, &self.nav_cfg, &mut self.nav, from, goal) {
                        Some(p) => {
                            ai.path = p.points;
                            ai.path_idx = 0;
                            ai.path_goal = goal;
                            if !p.complete {
                                ai.repath_cd = 2.5;
                            }
                        }
                        None => {
                            // no route: walk straight and let the collision sort it out
                            ai.path = vec![goal];
                            ai.path_idx = 0;
                            ai.repath_cd = 2.0;
                        }
                    }
                }
                if let Some(&wp) = ai.path.get(ai.path_idx) {
                    let to = (wp - tr.pos).flat();
                    let reach = if ai.path_idx + 1 == ai.path.len() { 70.0 } else { 50.0 };
                    if to.len() < reach {
                        ai.path_idx += 1;
                        if ai.path_idx >= ai.path.len() && tr.pos.dist_xz(goal) < 140.0 + (if ms == MoveSpeed::Run { 120.0 } else { 0.0 }) {
                            ai.arrived = true;
                        }
                    } else {
                        desired = to.norm() * speed;
                        face = Some(yaw_of(to));
                    }
                } else if tr.pos.dist_xz(goal) < 120.0 {
                    ai.arrived = true;
                }
            } else {
                ai.arrived = false;
            }
            let look_yaw = ai.look_at.map(|p| yaw_of((p - tr.pos).flat()));
            let target_yaw = match (look_yaw, face) {
                (Some(l), _) if ai.state == AlertState::Alerted || desired.len_sq() < 1.0 => Some(l),
                (_, Some(f)) => Some(f),
                _ => None,
            };
            let mut yaw = tr.yaw;
            if let Some(t) = target_yaw {
                yaw = turn_towards(yaw, t, 6.0 * dt);
            } else if ai.state == AlertState::Idle && ai.route.is_empty() && ai.arrived == false && tr.pos.dist_xz(ai.home) < 120.0 {
                yaw = turn_towards(yaw, ai.home_yaw, 2.0 * dt);
            }
            // keep desired movement mostly along the facing direction
            if desired.len_sq() > 0.0 {
                let fwd = forward(yaw);
                let align = fwd.dot(desired.norm()).max(0.0);
                desired = desired * align.max(0.2);
            }
            // doors on the way: open the closed door the NPC is about to walk into
            if desired.len_sq() > 1.0 && !self.doors.is_empty() && self.tick % 4 == (id.0 % 4) {
                let dir = desired.norm();
                self.npc_open_door_ahead(tr.pos, dir);
            }
            let mut stepped = None;
            if let Some(mut body) = self.bodies.take(id) {
                let before = body.pos;
                // NPCs collide with doors (closed leaves block them until they have swung open)
                let res = step_body(&self.collision, &self.body_params, &mut body, desired, dt, true);
                // stuck detection (wanted to move, did not)
                if desired.len_sq() > 1.0 && res.moved < desired.len() * dt * 0.25 {
                    ai.stuck += dt;
                } else {
                    ai.stuck = 0.0;
                }
                if ai.stuck > 1.5 {
                    ai.stuck = 0.0;
                    ai.path.clear();
                    ai.repath_cd = 0.0;
                    if ai.state == AlertState::Idle && !ai.route.is_empty() {
                        ai.route_idx = (ai.route_idx + 1) % ai.route.len();
                    }
                }
                let stride = if speed > walk + 1.0 { 190.0 } else { 120.0 };
                if body.step_accum > stride {
                    body.step_accum = 0.0;
                    if player_pos.is_some_and(|pp| pp.dist(body.pos) < 2500.0) {
                        self.fx.push(FxEvent::Footstep { pos: body.pos, surface: body.surface, run: speed > walk + 1.0, who: id });
                    }
                }
                stepped = Some((body.pos, (body.pos - before).flat().len()));
                self.bodies.insert(id, body);
            }
            if let Some((p, moved)) = stepped {
                self.set_pose(id, p, yaw);
                if let Some(a) = self.anims.get_mut(id) {
                    let spd = moved / dt.max(1e-4);
                    let state = if a.state == AnimState::Hit && a.time < 0.35 {
                        AnimState::Hit
                    } else if ai.state == AlertState::Alerted && ai.sees_player && spd < 5.0 && ai.look_at.is_some() {
                        AnimState::Aim
                    } else if spd > walk + 40.0 {
                        AnimState::Run
                    } else if spd > 20.0 {
                        AnimState::Walk
                    } else {
                        AnimState::Idle
                    };
                    if a.state != state {
                        a.state = state;
                        a.time = 0.0;
                    }
                    a.speed = spd;
                    a.phase += moved / if state == AnimState::Run { crate::anim::RUN_CYCLE_CM } else { crate::anim::WALK_CYCLE_CM };
                }
            }
            self.ais.insert(id, ai);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view<'a>(role: Role, state: AlertState, route: &'a [Vec3]) -> AiView<'a> {
        AiView {
            me: EntityId(1),
            role,
            pos: Vec3::ZERO,
            yaw: 0.0,
            time: 0.0,
            state,
            state_time: 0.0,
            suspicion: 0.0,
            sees_player: false,
            player_pos: None,
            player_dist: f32::INFINITY,
            last_seen: None,
            time_since_seen: 100.0,
            heard: None,
            route,
            route_idx: 0,
            help_points: &[],
            arrived: false,
            home: Vec3::ZERO,
            has_weapon: role == Role::Guard,
            weapon_range: 5000.0,
            clear_shot: false,
            fire_ready: true,
            hp_frac: 1.0,
            rand: [0.5, 0.5],
        }
    }

    #[test]
    fn guard_escalates_and_civilian_flees() {
        let route = [v3(500.0, 0.0, 0.0)];
        let mut g = SimpleBrain::new(Role::Guard);
        let mut v = view(Role::Guard, AlertState::Idle, &route);
        assert_eq!(g.think(&v, 0.1).state, AlertState::Idle);
        assert!(g.think(&v, 0.1).goal.is_some(), "patrols towards the waypoint");
        v.suspicion = 0.5;
        assert_eq!(g.think(&v, 0.1).state, AlertState::Suspicious);
        v.suspicion = 1.0;
        v.sees_player = true;
        v.player_pos = Some(v3(0.0, 0.0, 1000.0));
        v.player_dist = 1000.0;
        v.clear_shot = true;
        v.state = AlertState::Alerted;
        v.state_time = 1.0;
        let c = g.think(&v, 0.1);
        assert_eq!(c.state, AlertState::Alerted);
        assert!(c.fire);
        let mut civ = SimpleBrain::new(Role::Civilian);
        let mut v = view(Role::Civilian, AlertState::Idle, &[]);
        v.suspicion = 1.2;
        v.player_pos = Some(v3(0.0, 0.0, 500.0));
        assert_eq!(civ.think(&v, 0.1).state, AlertState::Fleeing);
    }
}
