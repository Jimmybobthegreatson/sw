//! Minimal data-driven mission logic: objectives from a [`MissionDef`] (RON) evaluated every tick.

use crate::data::{FailCondition, MissionDef, ObjectiveKind};
use crate::world::{FxEvent, Kind, World};
use bm_script::Event;
use serde::Serialize;
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
pub enum MissionStatus {
    /// No mission definition for this level (free roam).
    #[default]
    None,
    Active,
    Complete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ObjectiveState {
    pub id: String,
    pub text: String,
    pub optional: bool,
    pub done: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MissionState {
    pub def: Option<MissionDef>,
    pub status: MissionStatus,
    pub objectives: Vec<ObjectiveState>,
    pub picked: Vec<String>,
    pub elapsed: f64,
}

impl MissionState {
    pub fn start(def: MissionDef) -> MissionState {
        let objectives = def
            .objectives
            .iter()
            .map(|o| ObjectiveState { id: o.id.clone(), text: o.text.clone(), optional: o.optional, done: false })
            .collect();
        MissionState { def: Some(def), status: MissionStatus::Active, objectives, picked: Vec::new(), elapsed: 0.0 }
    }

    pub fn note_pickup(&mut self, item: &str) {
        if !self.picked.iter().any(|p| p == item) {
            self.picked.push(item.to_string());
        }
    }

    pub fn is_done(&self, id: &str) -> bool {
        self.objectives.iter().any(|o| o.id == id && o.done)
    }

    pub fn title(&self) -> &str {
        self.def.as_ref().map(|d| d.title.as_str()).unwrap_or("")
    }
}

impl World {
    /// Starts a mission definition (objective list reset).
    pub fn start_mission(&mut self, def: MissionDef) {
        self.mission = MissionState::start(def);
        self.outbox.push(Event::mission_event("mission_start", json!({ "id": self.mission.def.as_ref().map(|d| d.id.clone()) })));
    }

    /// Evaluates objectives and fail conditions; emits `on_mission_event` events into the outbox.
    pub fn update_mission(&mut self, dt: f32) {
        if self.mission.status != MissionStatus::Active {
            return;
        }
        self.mission.elapsed += dt as f64;
        let Some(def) = self.mission.def.clone() else { return };
        let player_pos = self.player_pos();
        let player_dead = self.player.map(|p| !self.is_alive(p)).unwrap_or(false);

        for fail in &def.fail {
            let failed = match fail {
                FailCondition::PlayerDead => player_dead,
                FailCondition::EntityKilled { entity } => {
                    let ids = self.find_containing(entity);
                    !ids.is_empty() && ids.iter().any(|id| !self.is_alive(*id) && self.infos.get(*id).is_some_and(|i| i.kind == Kind::Npc))
                }
            };
            if failed {
                self.mission.status = MissionStatus::Failed;
                self.outbox.push(Event::mission_event("mission_failed", json!({ "reason": format!("{fail:?}") })));
                self.fx.push(FxEvent::Message { text: "Mission failed".into(), seconds: 5.0 });
                return;
            }
        }

        for (idx, o) in def.objectives.iter().enumerate() {
            if self.mission.objectives[idx].done {
                continue;
            }
            if !o.requires.iter().all(|r| self.mission.is_done(r)) {
                continue;
            }
            let met = match &o.kind {
                ObjectiveKind::Kill { entity } => {
                    let ids: Vec<_> = self
                        .find_containing(entity)
                        .into_iter()
                        .filter(|id| self.infos.get(*id).is_some_and(|i| i.kind == Kind::Npc))
                        .collect();
                    !ids.is_empty() && ids.iter().all(|id| !self.is_alive(*id))
                }
                ObjectiveKind::KillExact { entity } => self.find(entity).is_some_and(|id| self.infos.get(id).is_some_and(|i| i.kind == Kind::Npc) && !self.is_alive(id)),
                // protect objectives are satisfied by surviving until everything else is done (the fail rule enforces the survival)
                ObjectiveKind::Protect { entity } => {
                    self.find(entity).is_some_and(|id| self.is_alive(id))
                        && def.objectives.iter().enumerate().all(|(j, other)| j == idx || other.optional || matches!(other.kind, ObjectiveKind::Protect { .. }) || self.mission.objectives[j].done)
                }
                ObjectiveKind::ReachExit { index } => self.areas.in_exit(*index),
                ObjectiveKind::ReachNode { node, radius } => {
                    let target = self.find_containing(node).into_iter().find_map(|id| self.pos(id));
                    match (player_pos, target) {
                        (Some(p), Some(t)) => p.dist_xz(t) <= *radius && (p.y - t.y).abs() < 500.0,
                        _ => false,
                    }
                }
                ObjectiveKind::ReachPoint { pos, radius } => player_pos.is_some_and(|p| p.dist(crate::math::Vec3::from_array(*pos)) <= *radius),
                ObjectiveKind::Pickup { item } => self.mission.picked.iter().any(|p| p == item),
            };
            if met && !player_dead {
                self.mission.objectives[idx].done = true;
                self.outbox.push(Event::mission_event("objective_complete", json!({ "id": o.id })));
                self.fx.push(FxEvent::Message { text: format!("Objective complete: {}", o.text), seconds: 4.0 });
            }
        }
        let required = self.mission.objectives.iter().filter(|o| !o.optional).count();
        let all = self.mission.objectives.iter().filter(|o| !o.optional).all(|o| o.done);
        if all && required > 0 {
            self.mission.status = MissionStatus::Complete;
            self.outbox.push(Event::mission_event("mission_complete", json!({ "time": self.mission.elapsed })));
            self.fx.push(FxEvent::Message { text: "Mission complete".into(), seconds: 6.0 });
        }
    }
}
