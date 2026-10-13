//! One fixed simulation tick: player, doors, AI, timers, mission. Deterministic given the same inputs.

use crate::anim::AnimState;
use crate::player::PlayerInput;
use crate::world::World;

/// Fixed timestep of the simulation (seconds).
pub const TICK_DT: f32 = 1.0 / 60.0;

impl World {
    /// Advances the world by one tick of `dt` seconds with the given player input.
    pub fn step(&mut self, input: &PlayerInput, dt: f32) {
        self.dt = dt;
        self.time += dt as f64;
        self.tick += 1;

        self.step_player(input, dt);
        self.update_areas();
        self.update_cameras();
        self.update_doors(dt);
        self.update_ai(dt);

        for (_, c) in self.characters.iter_mut() {
            c.fire_cd = (c.fire_cd - dt).max(0.0);
        }
        for (_, h) in self.healths.iter_mut() {
            h.hit_flash = (h.hit_flash - dt).max(0.0);
        }
        for (_, a) in self.anims.iter_mut() {
            a.time += dt;
            if a.state == AnimState::Dead {
                a.dead_blend = (a.dead_blend + dt * 2.5).min(1.0);
            }
        }
        let now = self.time;
        self.noises.retain(|n| now - n.time < 0.5);

        // NPC fall-out-of-world recovery
        let kill_y = self.meta.kill_y;
        let ids: Vec<_> = self.ais.ids();
        for id in ids {
            if let Some(b) = self.bodies.get_mut(id) {
                if b.pos.y < kill_y || !b.pos.is_finite() {
                    b.pos = b.last_safe;
                    b.vel_y = 0.0;
                    let (p, y) = (b.pos, self.transforms.get(id).map(|t| t.yaw).unwrap_or(0.0));
                    self.set_pose(id, p, y);
                }
            }
        }
        self.update_mission(dt);
    }

    /// Runs `ticks` steps with a constant input (test / tooling helper).
    pub fn run_ticks(&mut self, input: &PlayerInput, ticks: usize) {
        for _ in 0..ticks {
            self.step(input, TICK_DT);
            // without a driving `Game` the request queue has no script hook: resolve directly
            self.resolve_requests(&mut |_, _| true);
        }
    }
}
