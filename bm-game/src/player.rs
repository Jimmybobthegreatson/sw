//! The player: input, third-person movement, weapons / inventory, interaction prompts and camera.

use crate::anim::AnimState;
use crate::controller::step_body;
use crate::data::{GameData, WeaponKind};
use crate::math::{Vec3, forward, turn_towards, v3, yaw_of};
use crate::recoil::{self, Kick};
use crate::world::{EntityId, FxEvent, Kind, Request, World};
use serde::Serialize;
use serde_json::{Value, json};

/// One frame of player input (device agnostic; `bm-app` maps keys / pads to this).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerInput {
    /// Strafe, -1 (left) .. 1 (right).
    pub move_x: f32,
    /// Forward, -1 (back) .. 1 (forward).
    pub move_y: f32,
    /// Camera yaw delta in radians (positive turns right).
    pub look_dx: f32,
    /// Camera pitch delta in radians (positive looks up).
    pub look_dy: f32,
    pub run: bool,
    /// Held; the stance toggles on the rising edge.
    pub crouch: bool,
    pub aim: bool,
    pub fire: bool,
    /// Held; acts on the rising edge.
    pub interact: bool,
    pub reload: bool,
    /// Held; toggles the weapon drawn / holstered on the rising edge.
    pub draw: bool,
    /// Held; cycles the weapon on the rising edge.
    pub next_weapon: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InvItem {
    pub id: String,
    pub mag: u32,
    pub reserve: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Prompt {
    pub text: String,
    pub entity: u64,
    pub action: String,
}

#[derive(Clone, Debug)]
pub struct PlayerComp {
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub aim: bool,
    pub drawn: bool,
    pub crouching: bool,
    pub running: bool,
    pub items: Vec<InvItem>,
    pub current: usize,
    pub fire_cd: f32,
    pub reload_t: f32,
    /// Native recoil accumulator: set to [`recoil::START`] by a shot, halved every tick (see `crate::recoil`).
    pub recoil_acc: f32,
    /// Kick of this tick, added to the view angles (degrees, see [`PlayerComp::view_angles`]).
    pub recoil: Kick,
    pub prompt: Option<Prompt>,
    pub noise_now: f32,
    prev: PrevInput,
    pub speed_now: f32,
}

#[derive(Clone, Debug, Default)]
struct PrevInput {
    crouch: bool,
    fire: bool,
    interact: bool,
    reload: bool,
    draw: bool,
    next: bool,
}

impl PlayerComp {
    pub fn new(data: &GameData, yaw: f32) -> PlayerComp {
        let mut p = PlayerComp {
            cam_yaw: yaw,
            cam_pitch: -0.15,
            aim: false,
            drawn: false,
            crouching: false,
            running: false,
            items: Vec::new(),
            current: 0,
            fire_cd: 0.0,
            reload_t: 0.0,
            recoil_acc: 0.0,
            recoil: Kick::default(),
            prompt: None,
            noise_now: 0.0,
            prev: PrevInput::default(),
            speed_now: 0.0,
        };
        for w in &data.start_loadout {
            p.give(w, data);
        }
        p
    }

    pub fn give(&mut self, id: &str, data: &GameData) {
        let Some(def) = data.weapon(id) else { return };
        match self.items.iter_mut().find(|i| i.id == id) {
            Some(it) => it.reserve += def.mag_size.max(1) * 2,
            None => self.items.push(InvItem { id: id.to_string(), mag: def.mag_size, reserve: def.reserve }),
        }
    }

    pub fn current_item(&self) -> Option<&InvItem> {
        self.items.get(self.current)
    }

    /// A weapon in hand that bystanders react to.
    pub fn weapon_visible(&self, data: &GameData) -> bool {
        self.drawn
            && self.current_item().and_then(|i| data.weapon(&i.id)).is_some_and(|w| !(w.concealable && w.kind == WeaponKind::Melee))
    }

    pub fn current_noise(&self) -> f32 {
        self.noise_now
    }

    /// Yaw and pitch (radians) the camera and the shots use: the look angles plus this tick's recoil kick. Movement keeps
    /// using the plain look yaw, as the kick is a view offset.
    pub fn view_angles(&self) -> (f32, f32) {
        (self.cam_yaw + self.recoil.right.to_radians(), self.cam_pitch + self.recoil.up.to_radians())
    }

    pub fn inventory_json(&self) -> Value {
        json!({ "current": self.current, "drawn": self.drawn, "items": self.items })
    }
}

/// The third-person camera never gets closer than this to the head pivot (cm); the player model is hidden below
/// [`CAMERA_HIDE_DIST`].
pub const CAMERA_MIN_DIST: f32 = 45.0;
/// See [`CAMERA_MIN_DIST`].
pub const CAMERA_HIDE_DIST: f32 = 85.0;

/// Third-person camera in native space (left-handed, Y up, cm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub pos: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    pub fov_deg: f32,
    pub near: f32,
    pub far: f32,
}

impl World {
    /// Spawns the player entity at `pos` and returns its id.
    pub fn spawn_player(&mut self, pos: Vec3, yaw: f32, node: Option<usize>, prim: Option<u32>) -> EntityId {
        let id = self.spawn("Agent 47", Kind::Player, "player", crate::world::Transform::from_pose(pos, yaw));
        if let Some(i) = self.infos.get_mut(id) {
            i.node = node;
        }
        self.renders.insert(id, crate::world::RenderComp { node, prim_id: prim, visible: true });
        let hp = self.data.player_health.max(1.0);
        self.healths.insert(id, crate::world::Health { hp, max: hp, last_attacker: None, hit_flash: 0.0 });
        self.bodies.insert(id, crate::controller::Body::new(pos));
        self.anims.insert(id, crate::world::AnimComp::default());
        self.players.insert(id, PlayerComp::new(&self.data, yaw));
        self.player = Some(id);
        id
    }

    pub fn camera(&self) -> Option<Camera> {
        let id = self.player?;
        let pl = self.players.get(id)?;
        let tr = self.transforms.get(id)?;
        Some(self.camera_for(pl, tr.pos))
    }

    /// The third-person camera of `pl` standing at `pos` (also used while the component is taken out of the store).
    fn camera_for(&self, pl: &PlayerComp, pos: Vec3) -> Camera {
        let h = if pl.crouching { 115.0 } else { 155.0 };
        let pivot = pos + v3(0.0, h, 0.0);
        let (yaw, pitch) = pl.view_angles();
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let fwd = v3(sy * cp, sp, cy * cp);
        let right = v3(cy, 0.0, -sy);
        let (dist, shoulder) = if pl.aim { (150.0, 45.0) } else { (260.0, 0.0) };
        let want = pivot - fwd * dist + right * shoulder;
        let to = want - pivot;
        let len = to.len();
        let mut pos = want;
        if len > 1.0 {
            if let Some(hit) = self.collision.raycast(pivot, to * (1.0 / len), len, false) {
                // keep a minimum distance so the lens never ends up inside the body (the model is hidden when this is tight)
                pos = pivot + to * ((hit.t - 18.0).max(CAMERA_MIN_DIST) / len);
            }
        }
        Camera { pos, forward: fwd, up: Vec3::UP, fov_deg: if pl.aim { 50.0 } else { 67.4 }, near: 10.0, far: 50000.0 }
    }

    /// Advances the player by one tick.
    pub fn step_player(&mut self, input: &PlayerInput, dt: f32) {
        let Some(id) = self.player else { return };
        let Some(mut pl) = self.players.take(id) else { return };
        let alive = self.is_alive(id);
        let Some(tr) = self.transforms.get(id).copied() else {
            self.players.insert(id, pl);
            return;
        };

        pl.cam_yaw += input.look_dx;
        pl.cam_pitch = (pl.cam_pitch + input.look_dy).clamp(-1.25, 1.1);
        pl.fire_cd = (pl.fire_cd - dt).max(0.0);

        let rising = |now: bool, prev: bool| now && !prev;
        if alive {
            if rising(input.crouch, pl.prev.crouch) {
                pl.crouching = !pl.crouching;
                if let Some(b) = self.bodies.get_mut(id) {
                    b.crouching = pl.crouching;
                }
            }
            if rising(input.draw, pl.prev.draw) {
                pl.drawn = !pl.drawn;
            }
            if rising(input.next_weapon, pl.prev.next) && !pl.items.is_empty() {
                pl.current = (pl.current + 1) % pl.items.len();
                pl.reload_t = 0.0;
            }
        }
        pl.aim = alive && input.aim;
        if pl.aim {
            pl.drawn = true;
        }
        pl.running = alive && input.run && !pl.crouching && !pl.aim;

        // ---- movement ----
        let fwd = forward(pl.cam_yaw);
        let right = v3(fwd.z, 0.0, -fwd.x);
        let mut wish = fwd * input.move_y + right * input.move_x;
        if wish.len() > 1.0 {
            wish = wish.norm();
        }
        let speed = if pl.crouching {
            85.0
        } else if pl.aim {
            100.0
        } else if pl.running {
            self.characters.get(id).map(|c| c.run_speed).unwrap_or(380.0)
        } else {
            150.0
        };
        let desired = if alive { wish * speed } else { Vec3::ZERO };
        let mut yaw = tr.yaw;
        if alive {
            if pl.aim {
                yaw = turn_towards(yaw, pl.cam_yaw, 14.0 * dt);
            } else if wish.len_sq() > 0.01 {
                yaw = turn_towards(yaw, yaw_of(wish), 11.0 * dt);
            }
        }
        let mut moved = 0.0;
        if let Some(mut body) = self.bodies.take(id) {
            let before = body.pos;
            let res = step_body(&self.collision, &self.body_params, &mut body, desired, dt, true);
            moved = (body.pos - before).flat().len();
            let stride = if pl.running { 190.0 } else if pl.crouching { 90.0 } else { 120.0 };
            if body.step_accum > stride && body.grounded {
                body.step_accum = 0.0;
                self.fx.push(FxEvent::Footstep { pos: body.pos, surface: body.surface, run: pl.running, who: id });
            }
            if res.landed && res.land_speed > 700.0 {
                self.fx.push(FxEvent::Footstep { pos: body.pos, surface: body.surface, run: true, who: id });
            }
            // fell out of the world: back to the last safe spot
            if body.pos.y < self.meta.kill_y || !body.pos.is_finite() {
                body.pos = body.last_safe;
                body.vel_y = 0.0;
            }
            let p = body.pos;
            self.bodies.insert(id, body);
            self.set_pose(id, p, yaw);
        }
        let pos = self.pos(id).unwrap_or(tr.pos);
        let spd = moved / dt.max(1e-4);
        pl.speed_now = spd;
        pl.noise_now = if pl.running { 700.0 } else { 0.0 };

        // ---- weapons ----
        let mut firing_state = false;
        if alive {
            if pl.reload_t > 0.0 {
                pl.reload_t -= dt;
                if pl.reload_t <= 0.0 {
                    if let Some(it) = pl.items.get_mut(pl.current) {
                        let def = self.data.weapons.get(&it.id);
                        let mag = def.map(|d| d.mag_size).unwrap_or(0);
                        let need = mag.saturating_sub(it.mag);
                        let take = need.min(it.reserve);
                        it.mag += take;
                        it.reserve -= take;
                    }
                }
            }
            let cur = pl.items.get(pl.current).cloned();
            if let (Some(item), true) = (cur, pl.drawn) {
                if let Some(def) = self.data.weapon(&item.id).cloned() {
                    let melee = matches!(def.kind, WeaponKind::Melee | WeaponKind::Tool);
                    let wants = if def.auto { input.fire } else { rising(input.fire, pl.prev.fire) };
                    let reload_pressed = rising(input.reload, pl.prev.reload);
                    if !melee && pl.reload_t <= 0.0 && (reload_pressed || (wants && item.mag == 0)) && item.mag < def.mag_size && item.reserve > 0 {
                        pl.reload_t = def.reload_time.max(0.1);
                        self.fx.push(FxEvent::Reload { pos, weapon: item.id.clone() });
                    } else if wants && pl.fire_cd <= 0.0 && pl.reload_t <= 0.0 && (melee || item.mag > 0) {
                        let eye = pos + v3(0.0, if pl.crouching { 105.0 } else { 150.0 }, 0.0);
                        let dir = self.aim_dir(&pl, eye, def.range);
                        pl.fire_cd = 1.0 / def.fire_rate.max(0.1);
                        if !melee {
                            if let Some(it) = pl.items.get_mut(pl.current) {
                                it.mag -= 1;
                            }
                        }
                        self.fire_weapon(id, &item.id, eye, dir);
                        if !melee {
                            pl.recoil_acc = recoil::START;
                        }
                        firing_state = true;
                        pl.noise_now = pl.noise_now.max(def.noise);
                    }
                }
            }
        }

        // ---- recoil: this tick's kick from the accumulator (a firearm drawn), then the accumulator decays ----
        let (sx, sy) = if pl.drawn && pl.recoil_acc >= recoil::THRESHOLD {
            (self.rng.range(-1.0, 1.0), self.rng.range(-1.0, 1.0))
        } else {
            (0.0, 0.0)
        };
        pl.recoil = match pl.items.get(pl.current).and_then(|i| self.data.weapon(&i.id)) {
            Some(def) if pl.drawn => recoil::kick(pl.recoil_acc, def, sx, sy),
            _ => Kick::default(),
        };
        recoil::decay(&mut pl.recoil_acc);

        // ---- interaction prompt ----
        pl.prompt = if alive { self.find_prompt(id, pos, yaw) } else { None };
        if alive && rising(input.interact, pl.prev.interact) {
            if let Some(p) = pl.prompt.clone() {
                let target = EntityId(p.entity);
                match p.action.as_str() {
                    "pick_up" => self.requests.push(Request::Pickup { who: id, item: target }),
                    a => self.requests.push(Request::Action { actor: id, action: a.to_string(), target: Some(target) }),
                }
            }
        }

        // ---- animation state ----
        if let Some(a) = self.anims.get_mut(id) {
            let state = if !alive {
                AnimState::Dead
            } else if firing_state || (pl.drawn && pl.fire_cd > 0.15 && pl.aim) {
                AnimState::Fire
            } else if pl.aim {
                AnimState::Aim
            } else if pl.crouching {
                if spd > 15.0 { AnimState::CrouchWalk } else { AnimState::Crouch }
            } else if pl.running && spd > 20.0 {
                AnimState::Run
            } else if spd > 15.0 {
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

        pl.prev = PrevInput {
            crouch: input.crouch,
            fire: input.fire,
            interact: input.interact,
            reload: input.reload,
            draw: input.draw,
            next: input.next_weapon,
        };
        self.players.insert(id, pl);
    }

    /// Direction from the muzzle (`eye`) to whatever the camera crosshair points at: the target is found along the *camera*
    /// ray (the aim camera hangs behind and beside the shoulder), then the bullet flies from the player to that point.
    fn aim_dir(&self, pl: &PlayerComp, eye: Vec3, range: f32) -> Vec3 {
        let (yaw, pitch) = pl.view_angles();
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let fwd = v3(sy * cp, sp, cy * cp);
        let feet = eye - v3(0.0, if pl.crouching { 105.0 } else { 150.0 }, 0.0);
        let cam = self.camera_for(pl, feet);
        // start the ray where it passes the player so the body / geometry behind the shoulder never blocks it
        let behind = (eye - cam.pos).dot(fwd).max(0.0);
        let origin = cam.pos + fwd * behind;
        let target = match self.hitscan(origin, fwd, range, self.player) {
            Some(h) => h.point,
            None => origin + fwd * range,
        };
        let dir = (target - eye).norm_or(fwd);
        // a target right next to the player (or behind the muzzle) must not flip the shot: fall back to the camera direction
        if dir.dot(fwd) < 0.2 { fwd } else { dir }
    }

    /// Nearest interactable in front of the player.
    fn find_prompt(&self, who: EntityId, pos: Vec3, yaw: f32) -> Option<Prompt> {
        let fwd = forward(yaw);
        let mut best: Option<(f32, Prompt)> = None;
        for (id, d) in self.doors.iter() {
            let Some(t) = self.transforms.get(id) else { continue };
            // distance to the door's centre (node origin is the hinge; use the bbox-less hinge + some reach)
            let dist = t.pos.dist_xz(pos);
            if dist > 230.0 || (t.pos.y - pos.y).abs() > 250.0 {
                continue;
            }
            let score = dist;
            let text = if d.locked { "Locked" } else if d.open { "Close door" } else { "Open door" };
            let action = if d.open { "close_door" } else { "open_door" };
            if best.as_ref().map_or(true, |(s, _)| score < *s) {
                best = Some((score, Prompt { text: text.into(), entity: id.0, action: action.into() }));
            }
        }
        for (id, p) in self.pickups.iter() {
            if p.taken {
                continue;
            }
            let Some(t) = self.transforms.get(id) else { continue };
            let to = t.pos - pos;
            let dist = to.flat().len();
            if dist > 170.0 || to.y.abs() > 200.0 {
                continue;
            }
            if dist > 60.0 && to.flat().norm().dot(fwd) < 0.2 {
                continue;
            }
            let name = self.data.weapon(&p.weapon).map(|w| w.name.clone()).unwrap_or_else(|| pretty_item(if p.item.is_empty() { &p.weapon } else { &p.item }));
            let score = dist - 40.0; // prefer items over doors when both are close
            if best.as_ref().map_or(true, |(s, _)| score < *s) {
                best = Some((score, Prompt { text: format!("Pick up {name}"), entity: id.0, action: "pick_up".into() }));
            }
        }
        // clothes of a body (the disguise system, `crate::stealth`)
        for (id, ch) in self.characters.iter() {
            if self.is_alive(id) || self.infos.get(id).is_none_or(|i| i.kind != crate::world::Kind::Npc) {
                continue;
            }
            let Some(t) = self.transforms.get(id) else { continue };
            let to = t.pos - pos;
            if to.flat().len() > 140.0 || to.y.abs() > 150.0 {
                continue;
            }
            if self.stealth.disguise.as_ref().is_some_and(|d| d.colleague_outfit == ch.outfit) {
                continue;
            }
            // GUESS: no prompt where the clothes cannot be taken here (see `stealth::clothes_allowed_in_zone`)
            if !self.can_take_clothes(id) {
                continue;
            }
            let score = to.flat().len() - 20.0;
            if best.as_ref().map_or(true, |(s, _)| score < *s) {
                best = Some((score, Prompt { text: "Take disguise".into(), entity: id.0, action: "take_disguise".into() }));
            }
        }
        let _ = who;
        best.map(|(_, p)| p)
    }
}

/// "Item_PartyInvitation_01" -> "PartyInvitation", "CC_Hammer_01" -> "Hammer" (display text of unmapped items).
pub fn pretty_item(item: &str) -> String {
    let parts: Vec<&str> = item.split('_').filter(|p| !p.is_empty() && !p.chars().all(|c| c.is_ascii_digit())).collect();
    match parts.as_slice() {
        [] => item.to_string(),
        [one] => (*one).to_string(),
        [_, rest @ ..] => rest.join(" "),
    }
}

#[cfg(test)]
mod aim_tests {
    use super::*;
    use crate::collision::{CollisionWorld, Surface, Tri};

    #[test]
    fn shots_converge_on_the_point_under_the_crosshair() {
        // floor and a wall 2000 cm ahead
        let q = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| vec![Tri::new(a, b, c, Surface::Stone), Tri::new(a, c, d, Surface::Stone)];
        let mut tris = q(v3(-3000.0, 0.0, -3000.0), v3(3000.0, 0.0, -3000.0), v3(3000.0, 0.0, 3000.0), v3(-3000.0, 0.0, 3000.0));
        tris.extend(q(v3(-3000.0, 0.0, 2000.0), v3(3000.0, 0.0, 2000.0), v3(3000.0, 600.0, 2000.0), v3(-3000.0, 600.0, 2000.0)));
        let mut w = World::new(CollisionWorld::new(tris), GameData::embedded(), 1);
        let id = w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
        let mut pl = w.players.get(id).unwrap().clone();
        pl.aim = true;
        pl.cam_yaw = 0.0;
        pl.cam_pitch = 0.0;
        let eye = v3(0.0, 150.0, 0.0);
        let cam = w.camera_for(&pl, v3(0.0, 0.0, 0.0));
        let dir = w.aim_dir(&pl, eye, 9000.0);
        // the bullet lands where the camera ray (shoulder offset) meets the wall
        let x_at_wall = eye.x + dir.x / dir.z * (2000.0 - eye.z);
        assert!((x_at_wall - cam.pos.x).abs() < 3.0, "bullet x {x_at_wall} vs crosshair x {}", cam.pos.x);
        assert!(cam.pos.x.abs() > 20.0, "aim camera is offset to the shoulder");
    }

    #[test]
    fn a_shot_kicks_the_view_and_the_kick_decays() {
        // trace values: Remington (recoil_y -5), acc 0.1 -> climb 0.5 degrees; next frame acc 0.05 -> 0.25 degrees
        let q = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| vec![Tri::new(a, b, c, Surface::Stone), Tri::new(a, c, d, Surface::Stone)];
        let tris = q(v3(-3000.0, 0.0, -3000.0), v3(3000.0, 0.0, -3000.0), v3(3000.0, 0.0, 3000.0), v3(-3000.0, 0.0, 3000.0));
        let mut w = World::new(CollisionWorld::new(tris), GameData::embedded(), 1);
        let id = w.spawn_player(v3(0.0, 0.0, 0.0), 0.0, None, None);
        let mut pl = w.players.get(id).unwrap().clone();
        pl.items = vec![InvItem { id: "remington".into(), mag: 6, reserve: 30 }];
        pl.current = 0;
        pl.drawn = true;
        let pitch_before = pl.cam_pitch;
        w.players.insert(id, pl);

        let dt = 1.0 / 60.0;
        w.step_player(&PlayerInput { fire: true, ..Default::default() }, dt);
        let pl = w.players.get(id).unwrap().clone();
        assert!((pl.recoil.up - 0.5).abs() < 1e-5, "{:?}", pl.recoil);
        assert!((pl.recoil_acc - 0.05).abs() < 1e-6, "accumulator {}", pl.recoil_acc);
        assert!((pl.view_angles().1 - (pitch_before + 0.5f32.to_radians())).abs() < 1e-6, "the view climbs with the kick");
        assert!((pl.cam_pitch - pitch_before).abs() < 1e-6, "the look angle itself is not changed by the kick");

        w.step_player(&PlayerInput::default(), dt);
        let pl = w.players.get(id).unwrap().clone();
        assert!((pl.recoil.up - 0.25).abs() < 1e-5, "{:?}", pl.recoil);
    }
}
