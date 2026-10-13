//! Particle effects for gameplay events (muzzle flash, impacts, blood) and the level's ambient emitters,
//! on top of `bm-particles`. The simulation core never reads this; it only consumes [`FxEvent`]s.

use crate::math::{Vec3, pose_matrix, v3};
use crate::world::{EntityId, FxEvent, World};
use bm_models::Mat4;
use bm_particles::{EmitterDesc, EmitterId, ParticleSystem, RenderList, SceneParticles, View};
use std::collections::HashMap;
use std::sync::Arc;

pub struct FxDirector {
    pub system: ParticleSystem,
    pub templates: SceneParticles,
    ambient: HashMap<EntityId, EmitterId>,
    cache: HashMap<String, Option<Arc<EmitterDesc>>>,
    pub spawned: u64,
}

/// Matrix with the Y axis along `up` at `pos` (emitters shoot along their local +Y).
fn basis_up(pos: Vec3, up: Vec3) -> Mat4 {
    let y = up.norm_or(Vec3::UP);
    let helper = if y.y.abs() > 0.95 { v3(1.0, 0.0, 0.0) } else { Vec3::UP };
    let x = helper.cross(y).norm();
    let z = y.cross(x).norm();
    Mat4([[x.x, x.y, x.z, 0.0], [y.x, y.y, y.z, 0.0], [z.x, z.y, z.z, 0.0], [pos.x, pos.y, pos.z, 1.0]])
}

impl FxDirector {
    pub fn new(templates: SceneParticles, seed: u64) -> FxDirector {
        FxDirector { system: ParticleSystem::new(seed, 6000), templates, ambient: HashMap::new(), cache: HashMap::new(), spawned: 0 }
    }

    fn desc(&mut self, name: &str) -> Option<Arc<EmitterDesc>> {
        if let Some(d) = self.cache.get(name) {
            return d.clone();
        }
        let d = self.templates.template_desc(name).map(|mut d| {
            // gameplay one-shots must not loop forever
            d.spawn.looping = false;
            Arc::new(d)
        });
        if d.is_none() {
            log::debug!("particle template '{name}' unknown");
        }
        self.cache.insert(name.to_string(), d.clone());
        d
    }

    pub fn spawn(&mut self, template: &str, m: Mat4) -> Option<EmitterId> {
        let d = self.desc(template)?;
        self.spawned += 1;
        Some(self.system.add_emitter(d, m))
    }

    /// Reacts to a gameplay event; muzzle is where the weapon of a [FxEvent::Shot] really is (else the flash is put in front of the aim origin).
    pub fn handle(&mut self, world: &World, ev: &FxEvent, muzzle: Option<Vec3>) {
        match ev {
            FxEvent::Shot { pos, dir, weapon, .. } => {
                let tpl = world.data.weapon(weapon).map(|w| w.muzzle_particle.clone()).unwrap_or_else(|| "Gun_Flash".into());
                let at = muzzle.unwrap_or(*pos + *dir * 60.0);
                self.spawn(&tpl, basis_up(at, *dir));
                self.spawn("Gun_Smoke", basis_up(at, *dir));
            }
            FxEvent::Impact { pos, normal, surface } => {
                self.spawn(surface.impact_particle(), basis_up(*pos + *normal * 2.0, *normal));
            }
            FxEvent::Blood { pos, dir } => {
                self.spawn("Blood_Smoke", basis_up(*pos, -*dir));
            }
            FxEvent::Particles { template, pos, scale } => {
                let _ = scale;
                self.spawn(template, pose_matrix(*pos, 0.0));
            }
            _ => {}
        }
    }

    /// Starts ambient scene emitters (once) and steps the simulation.
    pub fn update(&mut self, world: &World, dt: f32) {
        if self.ambient.is_empty() {
            for (id, e) in world.emitters.iter() {
                let Some(si) = e.scene_emitter else { continue };
                let Some(pe) = self.templates.emitters.get(si) else { continue };
                let Some(desc) = pe.desc.clone() else { continue };
                let eid = self.system.add_emitter(desc, pe.world);
                self.ambient.insert(id, eid);
            }
        }
        self.system.update(dt);
    }

    pub fn collect(&self, cam_pos: Vec3, forward: Vec3) -> RenderList {
        let mut out = RenderList::default();
        let view = View::look_to(cam_pos.to_array(), forward.to_array(), [0.0, 1.0, 0.0]);
        self.system.collect(&view, &mut out);
        out
    }

    pub fn ambient_count(&self) -> usize {
        self.ambient.len()
    }
}
