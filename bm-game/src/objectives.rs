//! Per-level mission objectives derived from the decoded level data instead of hand-written RON.
//!
//! * The texts and kinds come from the scene's LOC file (`<level>/MissionObjectives/<n>/{Objective, Type, WhoOrWhat}`,
//!   `bm_formats::gameplay::mission_objectives`): `T` kill a target, `E` escape, `R` retrieve, `P` protect, `O` optional.
//! * Who the targets are comes from the level controller: the `ScriptC` module `<level>/<level>_levelcontrol` lists the
//!   actors it supervises in the order of the objectives (verified for M01: Don Fernando, Manuel, then the movie
//!   star's agent who is no target). `T` and `P` objectives take the next actor in that list.
//! * The escape objective is the level's exit area (`bm_formats::gameplay::exit_areas`).
//!
//! This is *not* the original mission script (compiled code in `Scriptcs/*.dll`): it only reproduces "kill the
//! targets, leave through the exit" with the real texts and areas. Anything that cannot be mapped becomes an optional
//! objective that stays open.

use crate::data::{FailCondition, MissionDef, ObjectiveDef, ObjectiveKind};
use crate::world::World;
use bm_formats::gameplay::{self, ObjectiveEntry, ObjectiveType, PathIndex};
use bm_models::Scene;

/// Level key of a scene base: `Scenes/M01/M01_main` -> `M01`.
pub fn level_key(base: &str) -> String {
    base.trim_start_matches("Scenes/").split('/').next().unwrap_or("").to_string()
}

/// Group names (= NPC entity names) of the actors the level controller supervises, in script parameter order.
pub fn controller_actors(scene: &Scene) -> Vec<String> {
    let pidx = PathIndex::new(&scene.graph);
    let mut out: Vec<String> = Vec::new();
    for s in gameplay::scripts(&scene.graph).into_iter().filter(|s| s.module.to_ascii_lowercase().contains("levelcontrol")) {
        for t in s.strings() {
            let Some(i) = pidx.resolve(&t) else { continue };
            let n = &scene.nodes[i];
            if n.type_id != gameplay::TYPE_ACTOR {
                continue;
            }
            let name = n.parent.map(|p| scene.nodes[p].name.clone()).unwrap_or_else(|| n.name.clone());
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// Display name of the level from `AllLevels/Interface/LevelNames/<key>` ("A VINTAGE YEAR"), else the key.
pub fn level_title(scene: &Scene, key: &str) -> String {
    let loc = scene.loc.as_ref();
    for k in [key.to_string(), format!("{}{}", &key[..1].to_ascii_uppercase(), key[1..].to_ascii_lowercase())] {
        if let Some(t) = loc.and_then(|l| l.get(&format!("AllLevels/Interface/LevelNames/{k}"))) {
            return t;
        }
    }
    key.to_string()
}

fn words(s: &str) -> Vec<String> {
    s.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| w.len() >= 4).map(str::to_string).collect()
}

/// Builds a [`MissionDef`] for `base` from the scene's data, if it has `MissionObjectives` in its LOC.
pub fn mission_from_scene(scene: &Scene, world: &World, base: &str) -> Option<MissionDef> {
    let loc = scene.loc.as_ref()?;
    let key = level_key(base);
    let mut entries: Vec<ObjectiveEntry> = gameplay::mission_objectives(loc, &key);
    if entries.is_empty() {
        let alt = key.to_ascii_uppercase();
        let alt2 = format!("{}{}", &key[..1].to_ascii_uppercase(), key[1..].to_ascii_lowercase());
        for k in [alt, alt2] {
            entries = gameplay::mission_objectives(loc, &k);
            if !entries.is_empty() {
                break;
            }
        }
    }
    // numbered entries only; placeholders ("Temp") and unknown targets are dropped
    entries.retain(|e| e.key.chars().all(|c| c.is_ascii_digit()) && !e.text.eq_ignore_ascii_case("temp") && !e.who.eq_ignore_ascii_case("unknown"));
    if entries.is_empty() {
        return None;
    }
    let mut actors = controller_actors(scene).into_iter().filter(|a| world.find(a).is_some());
    let pickups: Vec<String> = world.pickups.iter().map(|(_, p)| p.item.to_ascii_lowercase()).filter(|i| !i.is_empty()).collect();

    let mut objectives: Vec<ObjectiveDef> = Vec::new();
    let mut fail = vec![FailCondition::PlayerDead];
    let mut required: Vec<String> = Vec::new();
    let mut escape: Option<&ObjectiveEntry> = None;
    for e in &entries {
        let id = format!("obj{}", e.key);
        match e.kind {
            ObjectiveType::Target => match actors.next() {
                Some(name) => {
                    objectives.push(ObjectiveDef { id: id.clone(), text: e.text.clone(), kind: ObjectiveKind::KillExact { entity: name }, optional: false, requires: vec![] });
                    required.push(id);
                }
                None => objectives.push(ObjectiveDef { id, text: e.text.clone(), kind: ObjectiveKind::Kill { entity: e.who.clone() }, optional: true, requires: vec![] }),
            },
            ObjectiveType::Protect => match actors.next() {
                Some(name) => {
                    fail.push(FailCondition::EntityKilled { entity: name.clone() });
                    objectives.push(ObjectiveDef { id, text: e.text.clone(), kind: ObjectiveKind::Protect { entity: name }, optional: false, requires: vec![] });
                }
                None => objectives.push(ObjectiveDef { id, text: e.text.clone(), kind: ObjectiveKind::Kill { entity: e.who.clone() }, optional: true, requires: vec![] }),
            },
            ObjectiveType::Retrieve | ObjectiveType::Optional => {
                let ws = words(&e.who);
                let hit = pickups.iter().find(|i| {
                    let flat = i.replace('_', "");
                    ws.iter().any(|w| flat.contains(w.as_str()))
                });
                let optional = e.kind == ObjectiveType::Optional || hit.is_none();
                let item = hit.cloned().unwrap_or_else(|| e.who.to_ascii_lowercase());
                if !optional {
                    required.push(id.clone());
                }
                objectives.push(ObjectiveDef { id, text: e.text.clone(), kind: ObjectiveKind::Pickup { item }, optional, requires: vec![] });
            }
            ObjectiveType::Escape => escape = Some(e),
            ObjectiveType::Other(_) => {}
        }
    }
    if let Some(e) = escape {
        if world.areas.exits().count() > 0 {
            objectives.push(ObjectiveDef { id: format!("obj{}", e.key), text: e.text.clone(), kind: ObjectiveKind::ReachExit { index: 0 }, optional: false, requires: required });
        }
    }
    if objectives.is_empty() {
        return None;
    }
    let title = level_title(scene, &key);
    Some(MissionDef { id: key, title, level: base.to_string(), player_start: None, objectives, fail })
}
