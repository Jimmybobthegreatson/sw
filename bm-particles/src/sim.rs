//! CPU particle simulation: pooled particles, deterministic seeded RNG, billboard generation.
//!
//! Renderer agnostic: [`ParticleSystem::collect`] turns the live particles into a depth sorted
//! [`RenderList`] (instances + batches by sprite/blend) for a [`View`], and
//! [`RenderList::vertices`] expands it into classic 4-vertex quads for engines without instancing.

use std::sync::Arc;

use bm_models::math::Mat4;

use crate::curve::Range;
use crate::desc::{BillboardMode, EmitShape, EmitterDesc, FrameMode, ParticleBlend, SimSpace};
use crate::rng::Rng;

pub type Vec3 = [f32; 3];

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
fn norm_or(a: Vec3, fallback: Vec3) -> Vec3 {
    let l = len(a);
    if l < 1e-9 { fallback } else { mul(a, 1.0 / l) }
}

/// Camera basis used to orient billboards. Left-handed like the rest of the engine
/// (`right = cross(up, forward)`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub position: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
}

impl View {
    pub fn look_to(position: Vec3, forward: Vec3, up_hint: Vec3) -> View {
        let forward = norm_or(forward, [0.0, 0.0, 1.0]);
        let mut right = cross(up_hint, forward);
        if len(right) < 1e-6 {
            right = cross([0.0, 0.0, 1.0], forward);
        }
        let right = norm_or(right, [1.0, 0.0, 0.0]);
        let up = cross(forward, right);
        View { position, right, up, forward }
    }
}

/// Handle of an emitter inside a [`ParticleSystem`] (generation checked, stale handles are ignored).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EmitterId {
    index: u32,
    generation: u32,
}

#[derive(Clone, Debug)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    pub age: f32,
    pub life: f32,
    pub size: f32,
    pub rotation: f32,
    pub angular_velocity: f32,
    pub frame: u32,
    pub emitter: u32,
    pub alive: bool,
}

const DEAD: Particle = Particle {
    pos: [0.0; 3],
    vel: [0.0; 3],
    age: 0.0,
    life: 1.0,
    size: 0.0,
    rotation: 0.0,
    angular_velocity: 0.0,
    frame: 0,
    emitter: 0,
    alive: false,
};

struct EmitterState {
    desc: Arc<EmitterDesc>,
    key: Arc<str>,
    xf: Mat4,
    prev_origin: Vec3,
    /// Emitter velocity estimate (cm/s) for `inherit_velocity`-style effects (currently informational).
    time: f32,
    next_batch: f32,
    burst_idx: usize,
    rate_acc: f32,
    spawning: bool,
    alive: u32,
    generation: u32,
    in_use: bool,
    rng: Rng,
    bursts: Vec<crate::desc::Burst>,
}

/// A pooled particle simulation holding any number of emitters.
pub struct ParticleSystem {
    rng: Rng,
    emitters: Vec<EmitterState>,
    free_emitters: Vec<u32>,
    particles: Vec<Particle>,
    free: Vec<u32>,
    capacity: usize,
    alive: usize,
    /// Simulation time in seconds.
    pub time: f64,
    /// Largest internal step in seconds (bigger `dt` values are split).
    pub max_step: f32,
}

impl ParticleSystem {
    /// `capacity` bounds the total number of live particles (the pool never reallocates beyond it).
    pub fn new(seed: u64, capacity: usize) -> ParticleSystem {
        ParticleSystem {
            rng: Rng::new(seed),
            emitters: Vec::new(),
            free_emitters: Vec::new(),
            particles: Vec::new(),
            free: Vec::new(),
            capacity,
            alive: 0,
            time: 0.0,
            max_step: 1.0 / 30.0,
        }
    }

    pub fn alive_count(&self) -> usize {
        self.alive
    }

    pub fn emitter_count(&self) -> usize {
        self.emitters.iter().filter(|e| e.in_use).count()
    }

    pub fn particles(&self) -> impl Iterator<Item = &Particle> {
        self.particles.iter().filter(|p| p.alive)
    }

    /// Adds an emitter at `transform` (emitter space -> world) and starts it.
    pub fn add_emitter(&mut self, desc: impl Into<Arc<EmitterDesc>>, transform: Mat4) -> EmitterId {
        let desc: Arc<EmitterDesc> = desc.into();
        let idx = match self.free_emitters.pop() {
            Some(i) => i,
            None => {
                self.emitters.push(EmitterState {
                    desc: desc.clone(),
                    key: Arc::from(""),
                    xf: transform,
                    prev_origin: [0.0; 3],
                    time: 0.0,
                    next_batch: 0.0,
                    burst_idx: 0,
                    rate_acc: 0.0,
                    spawning: false,
                    alive: 0,
                    generation: 0,
                    in_use: false,
                    rng: Rng::new(0),
                    bursts: Vec::new(),
                });
                (self.emitters.len() - 1) as u32
            }
        };
        let salt = idx as u64 ^ ((self.emitters[idx as usize].generation as u64) << 32);
        let rng = self.rng.fork(salt);
        let e = &mut self.emitters[idx as usize];
        e.key = Arc::from(desc.sprite.source.key());
        e.bursts = desc.spawn.bursts.clone();
        e.bursts.sort_by(|a, b| a.time.total_cmp(&b.time));
        e.desc = desc;
        e.xf = transform;
        e.prev_origin = transform.translation();
        e.time = 0.0;
        e.next_batch = e.desc.spawn.start_delay;
        e.burst_idx = 0;
        e.rate_acc = 0.0;
        e.spawning = true;
        e.alive = 0;
        e.in_use = true;
        e.rng = rng;
        EmitterId { index: idx, generation: e.generation }
    }

    fn state(&mut self, id: EmitterId) -> Option<&mut EmitterState> {
        let e = self.emitters.get_mut(id.index as usize)?;
        (e.in_use && e.generation == id.generation).then_some(e)
    }

    pub fn set_transform(&mut self, id: EmitterId, transform: Mat4) {
        if let Some(e) = self.state(id) {
            e.xf = transform;
        }
    }

    /// Stops new particles; the emitter is released once its particles died.
    pub fn stop(&mut self, id: EmitterId) {
        if let Some(e) = self.state(id) {
            e.spawning = false;
        }
    }

    /// Restarts the spawn schedule (e.g. a looping effect triggered again).
    pub fn restart(&mut self, id: EmitterId) {
        if let Some(e) = self.state(id) {
            e.time = 0.0;
            e.next_batch = e.desc.spawn.start_delay;
            e.burst_idx = 0;
            e.rate_acc = 0.0;
            e.spawning = true;
        }
    }

    /// Spawns `count` extra particles right now (works on stopped emitters too).
    pub fn burst(&mut self, id: EmitterId, count: u32) {
        let Some(e) = self.state(id) else { return };
        let idx = id.index;
        let mut rng = std::mem::replace(&mut e.rng, Rng::new(0));
        for _ in 0..count {
            self.spawn_one(idx, &mut rng);
        }
        self.emitters[idx as usize].rng = rng;
    }

    pub fn is_active(&self, id: EmitterId) -> bool {
        self.emitters.get(id.index as usize).is_some_and(|e| e.in_use && e.generation == id.generation)
    }

    /// Removes every particle and emitter.
    pub fn clear(&mut self) {
        for e in &mut self.emitters {
            if e.in_use {
                e.in_use = false;
                e.generation = e.generation.wrapping_add(1);
            }
        }
        self.free_emitters = (0..self.emitters.len() as u32).rev().collect();
        for p in &mut self.particles {
            p.alive = false;
        }
        self.free = (0..self.particles.len() as u32).rev().collect();
        self.alive = 0;
    }

    fn alloc_particle(&mut self) -> Option<usize> {
        if let Some(i) = self.free.pop() {
            return Some(i as usize);
        }
        if self.particles.len() < self.capacity {
            self.particles.push(DEAD);
            return Some(self.particles.len() - 1);
        }
        None
    }

    fn spawn_one(&mut self, em: u32, rng: &mut Rng) {
        let (desc, xf) = {
            let e = &self.emitters[em as usize];
            if e.alive >= e.desc.max_particles {
                return;
            }
            (e.desc.clone(), e.xf)
        };
        let Some(slot) = self.alloc_particle() else { return };
        // position in emitter space
        let local = match &desc.shape {
            EmitShape::Point => [0.0; 3],
            EmitShape::Box { half_extents: h } => [rng.range(-h[0], h[0]), rng.range(-h[1], h[1]), rng.range(-h[2], h[2])],
            EmitShape::Sphere { radius } => mul(rng.unit_vec3(), radius * rng.f32().cbrt()),
            EmitShape::Disc { radius } => {
                let a = rng.range(0.0, std::f32::consts::TAU);
                let r = radius * rng.f32().sqrt();
                [r * a.cos(), 0.0, r * a.sin()]
            }
        };
        // direction inside the cone around `direction`
        let dir = norm_or(desc.direction, [0.0, 1.0, 0.0]);
        let spread = desc.spread_deg.clamp(0.0, 180.0).to_radians();
        let v_local = if spread > 1e-6 {
            let cos_min = spread.cos();
            let cz = rng.range(cos_min, 1.0);
            let sz = (1.0 - cz * cz).max(0.0).sqrt();
            let phi = rng.range(0.0, std::f32::consts::TAU);
            let helper = if dir[1].abs() < 0.99 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
            let t = norm_or(cross(helper, dir), [1.0, 0.0, 0.0]);
            let b = cross(dir, t);
            add(mul(dir, cz), add(mul(t, sz * phi.cos()), mul(b, sz * phi.sin())))
        } else {
            dir
        };
        let speed = desc.speed.sample(rng);
        let mut vel_local = mul(v_local, speed);
        if desc.velocity_jitter > 0.0 {
            vel_local = add(vel_local, mul(rng.unit_vec3(), desc.velocity_jitter * rng.f32()));
        }
        let (pos, vel) = match desc.space {
            SimSpace::World => (xf.transform_point(local), xf.transform_vector(vel_local)),
            SimSpace::Local => (local, vel_local),
        };
        let frames = desc.sprite.frame_count();
        let frame = match desc.sprite.frame_mode {
            FrameMode::Random => rng.below(frames),
            _ => 0,
        };
        let life = desc.lifetime.sample(rng).max(1e-3);
        let p = &mut self.particles[slot];
        *p = Particle {
            pos,
            vel,
            age: 0.0,
            life,
            size: desc.size.sample(rng),
            rotation: desc.rotation_deg.sample(rng).to_radians(),
            angular_velocity: desc.angular_velocity.sample(rng).to_radians(),
            frame,
            emitter: em,
            alive: true,
        };
        self.emitters[em as usize].alive += 1;
        self.alive += 1;
    }

    /// Advances the simulation. Large steps are split into `max_step` sub steps.
    pub fn update(&mut self, dt: f32) {
        if dt <= 0.0 || !dt.is_finite() {
            return;
        }
        let n = (dt / self.max_step).ceil().max(1.0) as u32;
        let h = dt / n as f32;
        for _ in 0..n {
            self.step(h);
        }
    }

    fn step(&mut self, h: f32) {
        self.time += h as f64;
        // 1. emitters spawn
        for ei in 0..self.emitters.len() {
            if !self.emitters[ei].in_use {
                continue;
            }
            let mut rng = std::mem::replace(&mut self.emitters[ei].rng, Rng::new(0));
            let to_spawn = self.schedule(ei, h);
            for _ in 0..to_spawn {
                self.spawn_one(ei as u32, &mut rng);
            }
            self.emitters[ei].rng = rng;
        }
        // 2. integrate
        let mut freed = Vec::new();
        for (i, p) in self.particles.iter_mut().enumerate() {
            if !p.alive {
                continue;
            }
            let e = &self.emitters[p.emitter as usize];
            p.age += h;
            if p.age >= p.life {
                p.alive = false;
                freed.push((i as u32, p.emitter));
                continue;
            }
            let d = &e.desc;
            if d.drag > 0.0 {
                let k = (-d.drag * h).exp();
                p.vel = mul(p.vel, k);
            }
            p.vel = add(p.vel, mul(d.acceleration, h));
            p.pos = add(p.pos, mul(p.vel, h));
            p.rotation += p.angular_velocity * h;
        }
        for (i, em) in freed {
            self.free.push(i);
            self.alive -= 1;
            self.emitters[em as usize].alive -= 1;
        }
        // 3. release finished emitters
        for (i, e) in self.emitters.iter_mut().enumerate() {
            if e.in_use && !e.spawning && e.alive == 0 {
                e.in_use = false;
                e.generation = e.generation.wrapping_add(1);
                self.free_emitters.push(i as u32);
            }
        }
    }

    /// Number of particles the emitter wants to spawn during the next `h` seconds; updates its clock.
    fn schedule(&mut self, ei: usize, h: f32) -> u32 {
        let e = &mut self.emitters[ei];
        if !e.spawning {
            return 0;
        }
        let sp = &e.desc.spawn;
        let t0 = e.time;
        e.time += h;
        let elapsed = e.time - sp.start_delay;
        if elapsed < 0.0 {
            return 0;
        }
        let mut count = 0u32;
        // game style batches: count at t = 0, then every interval while t < duration
        if sp.batch_count > 0 {
            if sp.batch_interval > 0.0 {
                while e.next_batch - sp.start_delay <= elapsed && (sp.duration <= 0.0 || e.next_batch - sp.start_delay < sp.duration) {
                    count += sp.batch_count;
                    e.next_batch += sp.batch_interval;
                }
            } else if e.next_batch.is_finite() && e.next_batch - sp.start_delay <= elapsed {
                count += sp.batch_count;
                e.next_batch = f32::INFINITY;
            }
        }
        // explicit bursts
        while e.burst_idx < e.bursts.len() && e.bursts[e.burst_idx].time <= elapsed {
            count += e.bursts[e.burst_idx].count;
            e.burst_idx += 1;
        }
        // continuous
        if sp.rate > 0.0 && (sp.duration <= 0.0 || (t0 - sp.start_delay) < sp.duration) {
            e.rate_acc += sp.rate * h;
            let n = e.rate_acc.floor();
            e.rate_acc -= n;
            count += n as u32;
        }
        // end of cycle
        let last_burst = e.bursts.last().map(|b| b.time).unwrap_or(0.0);
        let mut cycle = sp.duration.max(last_burst);
        let batches_done = sp.batch_count == 0 || !e.next_batch.is_finite() || (sp.duration > 0.0 && e.next_batch - sp.start_delay >= sp.duration);
        let continuous = sp.rate > 0.0 && sp.duration <= 0.0;
        if !continuous && batches_done && e.burst_idx >= e.bursts.len() && elapsed >= cycle {
            if sp.looping {
                if cycle <= 0.0 {
                    cycle = e.desc.lifetime.1.max(0.1);
                }
                if elapsed >= cycle {
                    e.time = sp.start_delay;
                    e.next_batch = sp.start_delay;
                    e.burst_idx = 0;
                }
            } else {
                e.spawning = false;
            }
        }
        count
    }

    /// Builds the draw list for `view`: depth sorted (far to near) instances grouped in batches of equal
    /// sprite + blend. `out` is cleared first.
    pub fn collect(&self, view: &View, out: &mut RenderList) {
        out.instances.clear();
        out.batches.clear();
        let mut items: Vec<(f32, ParticleInstance, u32)> = Vec::with_capacity(self.alive);
        for p in self.particles.iter().filter(|p| p.alive) {
            let e = &self.emitters[p.emitter as usize];
            let d = &e.desc;
            let t = (p.age / p.life).clamp(0.0, 1.0);
            let (pos, vel) = match d.space {
                SimSpace::World => (p.pos, p.vel),
                SimSpace::Local => (e.xf.transform_point(p.pos), e.xf.transform_vector(p.vel)),
            };
            let rel = sub(pos, view.position);
            let depth = dot(rel, view.forward);
            if depth <= 0.0 {
                continue;
            }
            let mut c = d.color.eval(t);
            for i in 0..4 {
                c[i] *= d.tint[i];
            }
            c[3] *= d.opacity;
            if d.soft_distance > 0.0 {
                c[3] *= (depth / d.soft_distance).clamp(0.0, 1.0);
            }
            if c[3] <= 0.002 {
                continue;
            }
            let size = ((p.size + d.size_growth * p.age) * d.size_over_life.eval(t)).max(0.0);
            if size <= 0.0 {
                continue;
            }
            let half_w = size * 0.5;
            let half_h = half_w * d.aspect;
            let (ax, ay) = billboard_axes(d.billboard, d.stretch, view, pos, vel, p.rotation, half_w, half_h);
            let frame = match d.sprite.frame_mode {
                FrameMode::OverLife => ((t * d.sprite.frame_count() as f32) as u32).min(d.sprite.frame_count() - 1),
                _ => p.frame,
            };
            items.push((
                depth,
                ParticleInstance { center: pos, axis_x: ax, axis_y: ay, color: c, uv: d.sprite.uv_rect(frame) },
                p.emitter,
            ));
        }
        items.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, inst, em) in items {
            let e = &self.emitters[em as usize];
            let blend = e.desc.blend;
            match out.batches.last_mut() {
                Some(b) if b.blend == blend && b.sprite == e.key => b.count += 1,
                _ => out.batches.push(Batch { sprite: e.key.clone(), blend, first: out.instances.len() as u32, count: 1 }),
            }
            out.instances.push(inst);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn billboard_axes(mode: BillboardMode, stretch: f32, view: &View, pos: Vec3, vel: Vec3, rot: f32, hw: f32, hh: f32) -> (Vec3, Vec3) {
    let camera_axes = || {
        let (s, c) = rot.sin_cos();
        let ax = add(mul(view.right, c), mul(view.up, s));
        let ay = add(mul(view.right, -s), mul(view.up, c));
        (mul(ax, hw), mul(ay, hh))
    };
    match mode {
        BillboardMode::Camera => camera_axes(),
        BillboardMode::Stretched => {
            let (vx, vy) = (dot(vel, view.right), dot(vel, view.up));
            let l2 = (vx * vx + vy * vy).sqrt();
            if l2 < 1e-3 {
                return camera_axes();
            }
            let (dx, dy) = (vx / l2, vy / l2);
            let ay_dir = add(mul(view.right, dx), mul(view.up, dy));
            let ax_dir = add(mul(view.right, dy), mul(view.up, -dx));
            let extra = len(vel) * stretch * 0.5;
            (mul(ax_dir, hw), mul(ay_dir, hh + extra))
        }
        BillboardMode::VelocityAligned => {
            let l = len(vel);
            if l < 1e-3 {
                return camera_axes();
            }
            let dir = mul(vel, 1.0 / l);
            let to_pos = norm_or(sub(pos, view.position), view.forward);
            let side = cross(dir, to_pos);
            if len(side) < 1e-4 {
                return camera_axes();
            }
            (mul(norm_or(side, view.right), hw), mul(dir, hh))
        }
    }
}

/// One camera facing quad, ready for instanced drawing: corners are `center +- axis_x +- axis_y`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "render", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct ParticleInstance {
    pub center: [f32; 3],
    /// Half extent vector along the quad's u axis.
    pub axis_x: [f32; 3],
    /// Half extent vector along the quad's v axis (pointing "up" in the sprite image).
    pub axis_y: [f32; 3],
    /// Straight-alpha RGBA.
    pub color: [f32; 4],
    /// `[u0, v0, u1, v1]`, top-left origin.
    pub uv: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    /// `SpriteSource::key()` of the texture.
    pub sprite: Arc<str>,
    pub blend: ParticleBlend,
    pub first: u32,
    pub count: u32,
}

/// Depth sorted instances + batches produced by [`ParticleSystem::collect`].
#[derive(Clone, Debug, Default)]
pub struct RenderList {
    pub instances: Vec<ParticleInstance>,
    pub batches: Vec<Batch>,
}

/// Classic quad vertex (for CPU rasterisers or non-instanced engines).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BillboardVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl RenderList {
    /// Expands every instance to 4 vertices (order: bottom-left, bottom-right, top-right, top-left) and
    /// 6 indices (`0 1 2 0 2 3`, counter-clockwise as seen from the camera in a right-handed look; flip if needed).
    pub fn vertices(&self) -> (Vec<BillboardVertex>, Vec<u32>) {
        let mut v = Vec::with_capacity(self.instances.len() * 4);
        let mut idx = Vec::with_capacity(self.instances.len() * 6);
        for inst in &self.instances {
            let base = v.len() as u32;
            let corners = [(-1.0, -1.0, inst.uv[0], inst.uv[3]), (1.0, -1.0, inst.uv[2], inst.uv[3]), (1.0, 1.0, inst.uv[2], inst.uv[1]), (-1.0, 1.0, inst.uv[0], inst.uv[1])];
            for (sx, sy, u, w) in corners {
                let p = add(inst.center, add(mul(inst.axis_x, sx), mul(inst.axis_y, sy)));
                v.push(BillboardVertex { pos: p, uv: [u, w], color: inst.color });
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        (v, idx)
    }
}

/// Convenience: samples a [`Range`] with a throw-away RNG (used by tools/tests).
pub fn sample_range(r: Range, seed: u64) -> f32 {
    r.sample(&mut Rng::new(seed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desc::{Burst, SpawnDesc};

    fn view() -> View {
        View::look_to([0.0, 0.0, -500.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
    }

    fn run(seed: u64) -> Vec<(Vec3, f32)> {
        let mut s = ParticleSystem::new(seed, 2000);
        s.add_emitter(EmitterDesc::preset_smoke(), Mat4::IDENTITY);
        s.add_emitter(EmitterDesc::preset_sparks(), Mat4::from_translation([50.0, 0.0, 0.0]));
        for _ in 0..90 {
            s.update(1.0 / 60.0);
        }
        s.particles().map(|p| (p.pos, p.age)).collect()
    }

    #[test]
    fn deterministic_for_same_seed_and_differs_otherwise() {
        let a = run(7);
        let b = run(7);
        let c = run(8);
        assert!(!a.is_empty());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn burst_spawns_exact_count_and_pool_is_reused() {
        let desc = EmitterDesc {
            spawn: SpawnDesc { rate: 0.0, bursts: vec![Burst { time: 0.0, count: 25 }], looping: false, ..Default::default() },
            lifetime: crate::curve::Range(0.5, 0.5),
            ..Default::default()
        };
        let mut s = ParticleSystem::new(1, 100);
        let id = s.add_emitter(desc, Mat4::IDENTITY);
        s.update(0.01);
        assert_eq!(s.alive_count(), 25);
        s.update(0.6);
        assert_eq!(s.alive_count(), 0);
        assert!(!s.is_active(id), "finished non-looping emitter is released");
        // pool slots are reused: a second effect does not grow the pool
        let n = s.particles.len();
        s.add_emitter(EmitterDesc { spawn: SpawnDesc { rate: 0.0, bursts: vec![Burst { time: 0.0, count: 25 }], looping: false, ..Default::default() }, ..Default::default() }, Mat4::IDENTITY);
        s.update(0.01);
        assert_eq!(s.particles.len(), n);
        assert_eq!(s.alive_count(), 25);
    }

    #[test]
    fn capacity_and_per_emitter_cap() {
        let mut s = ParticleSystem::new(1, 10);
        s.add_emitter(EmitterDesc { spawn: SpawnDesc { rate: 1000.0, ..Default::default() }, lifetime: crate::curve::Range(5.0, 5.0), ..Default::default() }, Mat4::IDENTITY);
        s.update(0.5);
        assert_eq!(s.alive_count(), 10);
        let mut s = ParticleSystem::new(1, 1000);
        s.add_emitter(EmitterDesc { max_particles: 7, spawn: SpawnDesc { rate: 1000.0, ..Default::default() }, lifetime: crate::curve::Range(5.0, 5.0), ..Default::default() }, Mat4::IDENTITY);
        s.update(0.5);
        assert_eq!(s.alive_count(), 7);
    }

    #[test]
    fn batch_schedule_matches_game_scheme() {
        // 2 particles every 0.1 s while t < 0.35 -> batches at 0, 0.1, 0.2, 0.3 = 8 particles
        let desc = EmitterDesc {
            spawn: SpawnDesc { rate: 0.0, batch_count: 2, batch_interval: 0.1, duration: 0.35, looping: false, ..Default::default() },
            lifetime: crate::curve::Range(10.0, 10.0),
            ..Default::default()
        };
        let mut s = ParticleSystem::new(3, 100);
        s.add_emitter(desc, Mat4::IDENTITY);
        for _ in 0..60 {
            s.update(1.0 / 60.0);
        }
        assert_eq!(s.alive_count(), 8);
    }

    #[test]
    fn gravity_and_drag_integrate() {
        let desc = EmitterDesc {
            spawn: SpawnDesc { rate: 0.0, bursts: vec![Burst { time: 0.0, count: 1 }], looping: false, ..Default::default() },
            lifetime: crate::curve::Range(10.0, 10.0),
            speed: crate::curve::Range(100.0, 100.0),
            direction: [1.0, 0.0, 0.0],
            acceleration: [0.0, -100.0, 0.0],
            ..Default::default()
        };
        let mut s = ParticleSystem::new(3, 4);
        s.add_emitter(desc, Mat4::IDENTITY);
        for _ in 0..100 {
            s.update(0.01);
        }
        let p = s.particles().next().unwrap();
        assert!((p.pos[0] - 100.0).abs() < 1.5, "x {}", p.pos[0]);
        assert!((p.pos[1] + 50.0).abs() < 2.0, "y {}", p.pos[1]);
    }

    #[test]
    fn curves_drive_size_and_alpha() {
        let desc = EmitterDesc {
            spawn: SpawnDesc { rate: 0.0, bursts: vec![Burst { time: 0.0, count: 1 }], looping: false, ..Default::default() },
            lifetime: crate::curve::Range(2.0, 2.0),
            size: crate::curve::Range(10.0, 10.0),
            size_over_life: crate::curve::Curve::linear(1.0, 3.0),
            color: crate::curve::ColorCurve::evenly_spaced(&[[1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0]]),
            ..Default::default()
        };
        let mut s = ParticleSystem::new(3, 4);
        s.add_emitter(desc, Mat4::from_translation([0.0, 0.0, 0.0]));
        s.update(1.0);
        let mut l = RenderList::default();
        s.collect(&View::look_to([0.0, 0.0, -300.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), &mut l);
        assert_eq!(l.instances.len(), 1);
        let i = &l.instances[0];
        assert!((i.color[3] - 0.5).abs() < 0.02);
        // half width = 0.5 * 10 * 2 = 10
        assert!((len(i.axis_x) - 10.0).abs() < 0.2, "{}", len(i.axis_x));
    }

    #[test]
    fn billboard_modes() {
        let v = view();
        let (ax, ay) = billboard_axes(BillboardMode::Camera, 0.0, &v, [0.0, 0.0, 0.0], [0.0; 3], 0.0, 2.0, 3.0);
        assert!((dot(ax, v.right) - 2.0).abs() < 1e-5 && (dot(ay, v.up) - 3.0).abs() < 1e-5);
        // stretched: long axis follows screen velocity, length grows by speed*stretch
        let (ax, ay) = billboard_axes(BillboardMode::Stretched, 0.01, &v, [0.0; 3], [100.0, 0.0, 0.0], 0.0, 2.0, 3.0);
        assert!((len(ay) - (3.0 + 0.5)).abs() < 1e-4 && (dot(ay, v.right).abs() - 3.5).abs() < 1e-4);
        assert!((len(ax) - 2.0).abs() < 1e-4);
        // velocity aligned: axis along 3D velocity, width perpendicular to view direction
        let (ax, ay) = billboard_axes(BillboardMode::VelocityAligned, 0.0, &v, [0.0; 3], [0.0, 50.0, 0.0], 0.7, 2.0, 3.0);
        assert!((ay[1] - 3.0).abs() < 1e-5);
        assert!(dot(ax, v.forward).abs() < 1e-5 && (len(ax) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn collect_sorts_back_to_front_and_batches() {
        let mut s = ParticleSystem::new(5, 100);
        let one = |blend, tex: &str| EmitterDesc {
            blend,
            sprite: crate::desc::SpriteDesc { source: crate::desc::SpriteSource::Texture(tex.into()), ..Default::default() },
            spawn: SpawnDesc { rate: 0.0, bursts: vec![Burst { time: 0.0, count: 1 }], looping: false, ..Default::default() },
            lifetime: crate::curve::Range(5.0, 5.0),
            ..Default::default()
        };
        s.add_emitter(one(ParticleBlend::Alpha, "a"), Mat4::from_translation([0.0, 0.0, 100.0]));
        s.add_emitter(one(ParticleBlend::Additive, "b"), Mat4::from_translation([0.0, 0.0, 300.0]));
        s.add_emitter(one(ParticleBlend::Alpha, "a"), Mat4::from_translation([0.0, 0.0, 200.0]));
        s.update(0.01);
        let mut l = RenderList::default();
        s.collect(&view(), &mut l);
        let z: Vec<f32> = l.instances.iter().map(|i| i.center[2]).collect();
        assert!(z[0] > z[1] && z[1] > z[2], "{z:?}");
        assert_eq!(l.batches.len(), 2, "far b, then the two adjacent a particles merge");
        let (v, i) = l.vertices();
        assert_eq!((v.len(), i.len()), (12, 18));
    }
}
