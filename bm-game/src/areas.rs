//! Areas of a level: trigger boxes (exit areas, `CheckInside` boxes, security zones), the room the player is in and the
//! security zone that room belongs to. Everything comes from decoded level data (`bm_formats::gameplay`, `.RMI`).

use crate::math::Vec3;
use crate::world::World;
use bm_models::Mat4;
use bm_script::Event;
use serde_json::json;

#[derive(Clone, Debug, PartialEq)]
pub enum TriggerKind {
    /// Exit area (`Action` with the `escape` label): standing inside completes `Escape` objectives.
    Exit,
    /// `CheckInside`: the watched node (the player's `Hero` actor) entering / leaving notifies `target`.
    Check { enter: String, leave: String, target: String },
    /// `SecurityZone` box carrying a zone token (`eZone1_PATCH`).
    Security { zone: String },
    /// Other `Action` volume (mission specific events).
    Action { event: String, target: String },
}

impl TriggerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriggerKind::Exit => "exit",
            TriggerKind::Check { .. } => "check_inside",
            TriggerKind::Security { .. } => "security",
            TriggerKind::Action { .. } => "action",
        }
    }
}

/// An oriented box centred on a scene node.
#[derive(Clone, Debug)]
pub struct TriggerVolume {
    pub node: usize,
    pub name: String,
    pub kind: TriggerKind,
    /// Half extents along the node's local axes (cm).
    pub half: [f32; 3],
    pub world: Mat4,
    pub inverse: Mat4,
}

impl TriggerVolume {
    pub fn new(node: usize, name: &str, kind: TriggerKind, size: [f32; 3], world: Mat4) -> TriggerVolume {
        TriggerVolume { node, name: name.to_string(), kind, half: [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5], world, inverse: world.inverse_affine() }
    }

    pub fn contains(&self, p: Vec3) -> bool {
        let l = self.inverse.transform_point(p.to_array());
        (0..3).all(|k| l[k].abs() <= self.half[k])
    }

    pub fn center(&self) -> Vec3 {
        Vec3::from_array(self.world.translation())
    }
}

/// A security zone token and the rooms (by lower-case node path prefixes) it covers.
#[derive(Clone, Debug)]
pub struct SecurityZone {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Areas {
    pub triggers: Vec<TriggerVolume>,
    /// Per trigger: is the player inside.
    pub inside: Vec<bool>,
    pub zones: Vec<SecurityZone>,
    /// Per room of [`crate::rooms::RoomIndex`]: index into `zones`.
    pub room_zone: Vec<Option<usize>>,
    pub player_room: Option<usize>,
    /// Zone of the player's current room.
    pub player_zone: Option<usize>,
    /// Time (world seconds) the player entered the current room.
    pub room_since: f64,
}

impl Areas {
    pub fn exits(&self) -> impl Iterator<Item = (usize, &TriggerVolume)> {
        self.triggers.iter().enumerate().filter(|(_, t)| t.kind == TriggerKind::Exit)
    }

    /// Is the player inside the `n`-th exit area?
    pub fn in_exit(&self, n: usize) -> bool {
        self.exits().nth(n).is_some_and(|(i, _)| self.inside.get(i).copied().unwrap_or(false))
    }
}

impl World {
    /// Name of the room the player is in (`M01_Hacienda_Room_101`).
    pub fn player_room_name(&self) -> Option<&str> {
        self.areas.player_room.and_then(|r| self.rooms.room(r)).map(|r| r.name.as_str())
    }

    /// Name of the security zone the player is in.
    pub fn player_zone_name(&self) -> Option<&str> {
        self.areas.player_zone.and_then(|z| self.areas.zones.get(z)).map(|z| z.name.as_str())
    }

    /// Room name at any position.
    pub fn room_name_at(&self, p: Vec3) -> Option<&str> {
        self.rooms.name_at(p)
    }

    /// Tracks the player against trigger volumes (every tick) and rooms (10 Hz); raises `on_trigger` / `on_room_change`.
    pub fn update_areas(&mut self) {
        let Some(pp) = self.player_pos() else { return };
        for i in 0..self.areas.triggers.len() {
            let now = self.areas.triggers[i].contains(pp);
            if now == self.areas.inside[i] {
                continue;
            }
            self.areas.inside[i] = now;
            let t = &self.areas.triggers[i];
            self.outbox.push(Event {
                name: "on_trigger".into(),
                payload: json!({ "name": t.name, "kind": t.kind.as_str(), "entered": now }),
            });
        }
        // security boxes raise the zone state while the player is inside (`FUN_005ec350`), see `crate::stealth`
        let mut over = 0;
        for (i, t) in self.areas.triggers.iter().enumerate() {
            if let TriggerKind::Security { zone } = &t.kind {
                if self.areas.inside[i] {
                    over = over.max(crate::stealth::zone_state_of(zone));
                }
            }
        }
        self.stealth.zone_override = over;
        if self.tick % 6 != 0 && self.tick != 1 {
            return;
        }
        let room = self.rooms.room_at(pp);
        if room != self.areas.player_room {
            let from = self.player_room_name().map(str::to_string);
            self.areas.player_room = room;
            self.areas.room_since = self.time;
            self.areas.player_zone = room.and_then(|r| self.areas.room_zone.get(r).copied().flatten());
            let to = self.player_room_name().map(str::to_string);
            let zone = self.player_zone_name().map(str::to_string);
            self.outbox.push(Event { name: "on_room_change".into(), payload: json!({ "from": from, "to": to, "zone": zone }) });
        }
    }
}
