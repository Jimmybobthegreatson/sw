//! Event model: built-in event catalogue (also the source for docs and Lua type stubs), the
//! [`Event`] value the game dispatches, and dispatch results.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A field in an event payload. `ty` is a LuaLS type string.
#[derive(Debug, Clone, Copy)]
pub struct FieldDef {
    pub name: &'static str,
    pub ty: &'static str,
    pub doc: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct EventDef {
    pub name: &'static str,
    /// Handlers may set `ev.cancelled = true` (or return `false`) to veto the action.
    pub cancelable: bool,
    pub doc: &'static str,
    pub fields: &'static [FieldDef],
}

const fn f(name: &'static str, ty: &'static str, doc: &'static str) -> FieldDef {
    FieldDef { name, ty, doc }
}

/// Every event the engine sends. The host raises them with [`Event`] and
/// `ScriptRuntime::dispatch`.
pub const BUILTIN_EVENTS: &[EventDef] = &[
    EventDef {
        name: "on_mod_load",
        cancelable: false,
        doc: "Sent only to the mod itself right after its entry scripts ran (also after a hot reload).",
        fields: &[f("reloaded", "boolean", "true when this is a hot reload")],
    },
    EventDef {
        name: "on_mod_unload",
        cancelable: false,
        doc: "Sent only to the mod itself before it is unloaded or hot-reloaded. Last chance to fill `mod.state` / `save`.",
        fields: &[f("reloading", "boolean", "true when followed by a reload")],
    },
    EventDef {
        name: "on_level_load",
        cancelable: false,
        doc: "A level (scene) finished loading.",
        fields: &[f("level", "string", "scene name, e.g. 'M01/M01_main'")],
    },
    EventDef {
        name: "on_level_unload",
        cancelable: false,
        doc: "The current level is being torn down.",
        fields: &[f("level", "string", "scene name")],
    },
    EventDef {
        name: "on_update",
        cancelable: false,
        doc: "Once per frame.",
        fields: &[f("dt", "number", "seconds since last frame"), f("time", "number", "game time in seconds")],
    },
    EventDef {
        name: "on_entity_spawn",
        cancelable: true,
        doc: "An entity is about to be created (cancel to prevent it).",
        fields: &[
            f("entity", "integer?", "entity id (nil if not created yet)"),
            f("template", "string", "template / archetype name"),
            f("kind", "string", "entity kind"),
            f("name", "string?", "entity name"),
            f("position", "Vec3", "spawn position"),
        ],
    },
    EventDef {
        name: "on_entity_despawn",
        cancelable: false,
        doc: "An entity was removed.",
        fields: &[f("entity", "integer", "entity id")],
    },
    EventDef {
        name: "on_damage",
        cancelable: true,
        doc: "An entity is about to take damage. `amount` may be modified; cancel to make it invulnerable to this hit.",
        fields: &[
            f("victim", "integer", "entity taking damage"),
            f("attacker", "integer?", "entity dealing damage"),
            f("amount", "number", "damage (modifiable)"),
            f("weapon", "string?", "weapon / damage source name"),
            f("zone", "string?", "hit zone, e.g. 'head'"),
        ],
    },
    EventDef {
        name: "on_kill",
        cancelable: false,
        doc: "An entity died.",
        fields: &[
            f("victim", "integer", "entity that died"),
            f("killer", "integer?", "entity that killed it"),
            f("weapon", "string?", "weapon / cause"),
            f("accident", "boolean?", "true for staged accidents"),
        ],
    },
    EventDef {
        name: "on_pickup",
        cancelable: true,
        doc: "An entity is about to pick up an item.",
        fields: &[
            f("entity", "integer", "the picker"),
            f("item", "string", "item / weapon name"),
            f("item_entity", "integer?", "world entity of the item"),
        ],
    },
    EventDef {
        name: "on_action",
        cancelable: true,
        doc: "An entity is about to perform a context action (open door, use item, ...).",
        fields: &[
            f("actor", "integer", "who acts"),
            f("action", "string", "action name"),
            f("target", "integer?", "target entity"),
        ],
    },
    EventDef {
        name: "on_mission_event",
        cancelable: false,
        doc: "Mission logic raised an event (objective complete, target killed, alarm, ...).",
        fields: &[f("name", "string", "event name"), f("data", "table?", "event specific data")],
    },
    EventDef {
        name: "on_cvar_changed",
        cancelable: false,
        doc: "A console variable changed.",
        fields: &[f("name", "string", "cvar"), f("value", "any", "new value"), f("old", "any", "previous value")],
    },
    EventDef {
        name: "on_data_reload",
        cancelable: false,
        doc: "Data patches were re-applied (hot reload). Re-read `data.get(..)`.",
        fields: &[],
    },
    EventDef {
        name: "on_shutdown",
        cancelable: false,
        doc: "The game is exiting.",
        fields: &[],
    },
];

pub fn builtin_event(name: &str) -> Option<&'static EventDef> {
    BUILTIN_EVENTS.iter().find(|e| e.name == name)
}

/// A dispatched event. Payload must be a JSON object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    pub payload: Value,
}

impl Event {
    pub fn new(name: impl Into<String>, payload: Value) -> Event {
        let payload = if payload.is_object() { payload } else { json!({ "value": payload }) };
        Event { name: name.into(), payload }
    }
    pub fn level_load(level: &str) -> Event {
        Event::new("on_level_load", json!({ "level": level }))
    }
    pub fn level_unload(level: &str) -> Event {
        Event::new("on_level_unload", json!({ "level": level }))
    }
    pub fn update(dt: f32, time: f64) -> Event {
        Event::new("on_update", json!({ "dt": dt, "time": time }))
    }
    pub fn entity_spawn(template: &str, kind: &str, name: Option<&str>, pos: crate::host::Vec3) -> Event {
        Event::new(
            "on_entity_spawn",
            json!({ "template": template, "kind": kind, "name": name, "position": pos }),
        )
    }
    pub fn damage(victim: u64, attacker: Option<u64>, amount: f32, weapon: Option<&str>, zone: Option<&str>) -> Event {
        Event::new(
            "on_damage",
            json!({ "victim": victim, "attacker": attacker, "amount": amount, "weapon": weapon, "zone": zone }),
        )
    }
    pub fn kill(victim: u64, killer: Option<u64>, weapon: Option<&str>) -> Event {
        Event::new("on_kill", json!({ "victim": victim, "killer": killer, "weapon": weapon }))
    }
    pub fn pickup(entity: u64, item: &str, item_entity: Option<u64>) -> Event {
        Event::new("on_pickup", json!({ "entity": entity, "item": item, "item_entity": item_entity }))
    }
    pub fn action(actor: u64, action: &str, target: Option<u64>) -> Event {
        Event::new("on_action", json!({ "actor": actor, "action": action, "target": target }))
    }
    pub fn mission_event(name: &str, data: Value) -> Event {
        Event::new("on_mission_event", json!({ "name": name, "data": data }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ErrorKind {
    /// Lua runtime / syntax error.
    Runtime,
    /// Exceeded the time budget.
    Timeout,
    /// Exceeded the memory cap.
    Memory,
    /// Bad host call (permission denied, host error).
    Api,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScriptError {
    pub mod_id: String,
    pub context: String,
    pub kind: ErrorKind,
    pub message: String,
}

/// Result of dispatching one event to all subscribed mods.
#[derive(Debug, Clone, Serialize)]
pub struct DispatchOutcome {
    pub cancelled: bool,
    /// Payload after all handlers ran (handlers may modify fields). `cancelled` is stripped.
    pub payload: Value,
    pub handlers_run: usize,
    pub errors: Vec<ScriptError>,
}
