//! Notoriety and disguises.
//!
//! The numbers are the game's own data files (`notoriety.xml`, `difficultysettings.xml` in every `*_main.ZIP`):
//!
//! * **Notoriety** (`<Notoriety>`): a point total, 0..100, that the HUD bar shows (green <= 50, yellow <= 75, red above,
//!   see `docs/native_hud.md`). A crime (a kill, a weapon fired at somebody) adds `Witness1..4` / `Witnessx` points for
//!   each person who sees it - the first witness counts most - and at most `MaxWitnesses*` people count per difficulty.
//!   A recording camera adds `CaughtOnTape*`. Bribes / a new identity subtract (`BribeInnocent`, `BribePolice`,
//!   `NewIdentity`).
//! * **Disguises** are outfit values of the player model: the combined `Hero` model holds every outfit (`docs/anim.md`
//!   section 6), the `HMAS_*` cloth bundles name them. Taking the clothes of a body switches the outfit value.
//!   `SeeThroughDisguise{Distance,Speed}` / `ForgetDisguiseSpeed` (per difficulty) drive how fast people of the same
//!   class as the clothes see through them.
//!
//! What is *not* from the data: the base distance (`SEE_THROUGH_BASE_CM`) and build-up rate (`SEE_THROUGH_RATE`) that the
//! difficulty multipliers scale (the original logic is native code in `HitmanBloodMoney.exe`; the multipliers are real).

use crate::ai::Role;
use crate::math::Vec3;
use crate::world::{EntityId, World};
use bm_formats::gameplay::DisguiseDef;
use bm_script::Event;
use bm_vfs::Vfs;
use serde_json::json;
use std::collections::HashSet;

/// Length of one recording: the event end of a camera recording (`FUN_005bc0e0` starts event type 10, which ends at
/// `clock + 0x2800`, taken as milliseconds).
pub const RECORD_SECONDS: f64 = 10.24;

/// A security camera of the level (`SecurityCamera` controller of a `ZGROUP` node).
///
/// GUESS for the view (not verified in the original): the lens sits at the node and looks along its forward axis, the
/// half-angle is controller float 4 (degrees) and the range is controller float 5 (metres). The sweep of the head is not
/// modelled.
#[derive(Clone, Debug)]
pub struct SecurityCamera {
    pub pos: Vec3,
    pub dir: Vec3,
    /// cm
    pub range: f32,
    pub half_angle_deg: f32,
    /// Game time the current recording ends (`-inf` when the camera is not recording).
    pub recording_until: f64,
}

impl SecurityCamera {
    /// Does the camera see point `p` (ignoring walls)?
    pub fn sees(&self, p: Vec3) -> bool {
        let d = p - self.pos;
        let dist = d.len();
        if dist > self.range || dist < 1e-3 {
            return false;
        }
        d.dot(self.dir) / dist >= self.half_angle_deg.to_radians().cos()
    }
}

/// The tape kind after a recording start (`FUN_005bc630`): 2 when the hitman is in the room of the previous recording,
/// otherwise `DAT_009b3c24 = (DAT_009b3c24 == 2) + 1`, which is 2 only when the kind was already 2.
pub fn next_tape_kind(kind: u8, same_room: bool) -> u8 {
    if same_room {
        2
    } else if kind == 2 {
        2
    } else {
        1
    }
}

/// Distance (cm) within which a colleague can recognise a disguise at difficulty multiplier 1.
pub const SEE_THROUGH_BASE_CM: f32 = 300.0;
/// Awareness per second at multiplier 1 while a colleague looks at the disguise from inside that distance.
pub const SEE_THROUGH_RATE: f32 = 0.25;
/// Seconds after which a witnessed incident counts as over (a person only counts once per incident).
const INCIDENT_SECONDS: f64 = 20.0;

#[derive(Clone, Debug, PartialEq)]
pub struct NotorietyRules {
    pub caught_on_tape_not_disguised: f32,
    pub caught_on_tape_disguised: f32,
    /// Points of the 1st, 2nd, 3rd, 4th witness.
    pub witness: [f32; 4],
    /// Points of every further witness.
    pub witness_x: f32,
    /// Witness cap per difficulty (Normal, Expert, Professional; Rookie uses Normal).
    pub max_witnesses: [u32; 3],
    pub bribe_innocent: f32,
    pub bribe_police: f32,
    pub new_identity: f32,
}

impl Default for NotorietyRules {
    /// The values shipped in every mission archive.
    fn default() -> Self {
        NotorietyRules {
            caught_on_tape_not_disguised: 20.0,
            caught_on_tape_disguised: 10.0,
            witness: [10.0, 5.0, 3.0, 2.0],
            witness_x: 2.0,
            max_witnesses: [15, 20, 25],
            bribe_innocent: -15.0,
            bribe_police: -40.0,
            new_identity: -100.0,
        }
    }
}

/// The disguise related rows of `difficultysettings.xml`, indexed Rookie, Normal, Expert, Professional.
#[derive(Clone, Debug, PartialEq)]
pub struct DifficultyRules {
    pub see_through_distance: [f32; 4],
    pub see_through_speed: [f32; 4],
    pub forget_disguise_speed: [f32; 4],
    pub run_suspicious_range: [f32; 4],
    pub small_weapon_visible_guard: [f32; 4],
    pub small_weapon_visible_civilian: [f32; 4],
    pub stand_down_time: [f32; 4],
}

impl Default for DifficultyRules {
    fn default() -> Self {
        DifficultyRules {
            see_through_distance: [0.5, 1.0, 1.8, 2.0],
            see_through_speed: [0.5, 1.0, 1.5, 2.0],
            forget_disguise_speed: [2.0, 1.0, 0.8, 0.5],
            run_suspicious_range: [0.0, 5.0, 10.0, 15.0],
            small_weapon_visible_guard: [1.0, 1.0, 2.0, 2.5],
            small_weapon_visible_civilian: [0.4, 0.8, 1.2, 1.6],
            stand_down_time: [20.0, 30.0, 45.0, 60.0],
        }
    }
}

/// `<Tag a="1" b="2"/>` -> attribute lookup over a flat XML text (the files have no nesting that matters here).
fn tag_attrs<'a>(xml: &'a str, tag: &str) -> Vec<(String, &'a str)> {
    let open = format!("<{tag}");
    let Some(start) = xml.find(&open) else { return Vec::new() };
    let rest = &xml[start + open.len()..];
    let end = rest.find('>').unwrap_or(rest.len());
    let body = &rest[..end];
    let mut out = Vec::new();
    let mut s = body;
    while let Some(eq) = s.find("=\"") {
        let name = s[..eq].trim().rsplit(char::is_whitespace).next().unwrap_or("").to_string();
        let val_start = eq + 2;
        let Some(q) = s[val_start..].find('"') else { break };
        out.push((name, &s[val_start..val_start + q]));
        s = &s[val_start + q + 1..];
    }
    out
}

fn tag_value(xml: &str, tag: &str) -> Option<f32> {
    tag_attrs(xml, tag).into_iter().find(|(n, _)| n == "value").and_then(|(_, v)| v.parse().ok())
}

impl NotorietyRules {
    pub fn parse(xml: &str) -> NotorietyRules {
        let mut r = NotorietyRules::default();
        let f = |tag: &str, dst: &mut f32| {
            if let Some(v) = tag_value(xml, tag) {
                *dst = v;
            }
        };
        f("CaughtOnTapeNotDisguised", &mut r.caught_on_tape_not_disguised);
        f("CaughtOnTapeDisguised", &mut r.caught_on_tape_disguised);
        for (i, t) in ["Witness1", "Witness2", "Witness3", "Witness4"].iter().enumerate() {
            f(t, &mut r.witness[i]);
        }
        f("Witnessx", &mut r.witness_x);
        for (i, t) in ["MaxWitnessesNormal", "MaxWitnessesExpert", "MaxWitnessesProfessional"].iter().enumerate() {
            if let Some(v) = tag_value(xml, t) {
                r.max_witnesses[i] = v as u32;
            }
        }
        f("BribeInnocent", &mut r.bribe_innocent);
        f("BribePolice", &mut r.bribe_police);
        f("NewIdentity", &mut r.new_identity);
        r
    }

    /// Points for the `n`-th (1 based) witness of the game.
    pub fn witness_points(&self, n: u32) -> f32 {
        match n {
            0 => 0.0,
            1..=4 => self.witness[(n - 1) as usize],
            _ => self.witness_x,
        }
    }
}

impl DifficultyRules {
    pub fn parse(xml: &str) -> DifficultyRules {
        let mut r = DifficultyRules::default();
        let row = |tag: &str, dst: &mut [f32; 4]| {
            let a = tag_attrs(xml, tag);
            for (i, key) in ["Rookie", "Normal", "Expert", "Professional"].iter().enumerate() {
                if let Some(v) = a.iter().find(|(n, _)| n == key).and_then(|(_, v)| v.parse::<f32>().ok()) {
                    dst[i] = v;
                }
            }
        };
        row("SeeThroughDisguiseDistance", &mut r.see_through_distance);
        row("SeeThroughDisguiseSpeed", &mut r.see_through_speed);
        row("ForgetDisguiseSpeed", &mut r.forget_disguise_speed);
        row("RunSuspiciousRange", &mut r.run_suspicious_range);
        row("SmallWeaponVisibleMultiplierGuard", &mut r.small_weapon_visible_guard);
        row("SmallWeaponVisibleMultiplierCivilian", &mut r.small_weapon_visible_civilian);
        row("StandDownTime", &mut r.stand_down_time);
        r
    }
}

/// `ESecurityZone` name -> zone state value, from the table at 0x789880 of `HitmanBloodMoney.exe` (indexed by the enum
/// ordinal): `eZoneUNDEFINED` 0, `eZone1` 1, `eZone2` 2, `eZone2A` 4, `eZone2B` 8, `eZone3` 0x10, `eZone3A` 0x20,
/// `eZone3B` 0x40, `eZoneMegaForbidden` 0x80. The `_PATCH` variants map to the same values.
pub fn zone_state_of(name: &str) -> u32 {
    match name.trim_end_matches("_PATCH") {
        "eZone1" => 1,
        "eZone2" => 2,
        "eZone2A" => 4,
        "eZone2B" => 8,
        "eZone3" => 0x10,
        "eZone3A" => 0x20,
        "eZone3B" => 0x40,
        "eZoneMegaForbidden" => 0x80,
        _ => 0,
    }
}

/// The zone state of an area nobody restricted (`FUN_005f29b0` returns 1 when neither the zone nor an override is set).
pub const ZONE_PUBLIC: u32 = 1;
/// The state that makes every observer detect the hitman at full rate (`FUN_005bd8a0`).
pub const ZONE_MEGA_FORBIDDEN: u32 = 0x80;

/// GUESS (not verified in the original; user-authorized): clothes can be taken only where their definition lists the
/// current zone state (`safe_zones` is the mask from [`zones_mask`]; `None` = no definition, never restricted). The
/// mega-forbidden zone (0x80) refuses every change.
pub fn clothes_allowed_in_zone(safe_zones: Option<u32>, zone: u32) -> bool {
    if zone & ZONE_MEGA_FORBIDDEN != 0 {
        return false;
    }
    match safe_zones {
        None => true,
        Some(mask) => mask & zone != 0,
    }
}

/// GUESS (not verified; user-authorized): clothes stand out when the zone is not in their safe list. `safe` is the outfit's
/// mask (the own suit uses the `HitmanTemplate` list, `None` = never out of place). The mega-forbidden zone is handled by
/// its own rule, so it is not reported here.
pub fn out_of_place(safe: Option<u32>, zone: u32) -> bool {
    zone & ZONE_MEGA_FORBIDDEN == 0 && safe.is_some_and(|m| m & zone == 0)
}

/// Bit mask of the zone states a definition lists as safe.
pub fn zones_mask(zones: &[String]) -> u32 {
    zones.iter().fold(0, |m, z| m | zone_state_of(z))
}

/// Clothes the player wears that are not his own suit.
#[derive(Clone, Debug, PartialEq)]
pub struct Disguise {
    /// Outfit value of the player model (`Mesh::variant_mask`).
    pub outfit: u8,
    /// Whose clothes: bystanders of this role accept them, colleagues of the same outfit may see through them.
    pub class: Role,
    /// Outfit value of the people that count as colleagues (the victim's `ZHM3Actor` outfit).
    pub colleague_outfit: u8,
    pub name: String,
    /// Zone states (bit mask of [`zone_state_of`]) the outfit is safe in, from its `ZHM3HmAs` definition; `None` when the
    /// clothes have no definition (then no zone restricts them).
    pub safe_zones: Option<u32>,
    /// `EDressQuality` token of the definition.
    pub quality: String,
    /// Scene node and model of the donor: the player is drawn with this model (and `colleague_outfit`).
    pub node: Option<usize>,
    pub prim: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Stealth {
    pub notoriety_rules: NotorietyRules,
    pub difficulty_rules: DifficultyRules,
    /// 0 Rookie, 1 Normal, 2 Expert, 3 Professional.
    pub difficulty: usize,
    /// Notoriety points, 0..=100.
    pub notoriety: f32,
    /// Witnesses counted so far this mission.
    pub witnesses: u32,
    incident: HashSet<EntityId>,
    incident_end: f64,
    /// Outfit value of the player's own suit.
    pub base_outfit: u8,
    pub disguise: Option<Disguise>,
    /// Disguise definitions of the level (`ZHM3HmAs`).
    pub defs: Vec<DisguiseDef>,
    /// Security cameras of the level (see [`SecurityCamera`]).
    pub cameras: Vec<SecurityCamera>,
    /// Zone states the player's own suit is safe in (the `HitmanTemplate` definition; `eZone1` when absent).
    pub suit_safe_zones: u32,
    /// Raised by the security boxes the player stands in (`FUN_005ec350`: never lowered while inside, reset on leaving).
    pub zone_override: u32,
    /// Kind of the latest tape (`DAT_009b3c24`): 0 none, 1 or 2 (see [`next_tape_kind`]).
    pub tape_kind: u8,
    /// Room the latest recording started in (the room word `+0xfd0` of the hitman; the mapping is a guess).
    pub tape_room: Option<usize>,
    /// Notice shown for 10 s when a camera starts a recording (`AllLevels/Info/CaughtOnSuveillanceTape`, `FUN_005bc0e0`).
    pub tape_notice: String,
}

impl Default for Stealth {
    fn default() -> Self {
        Stealth {
            notoriety_rules: NotorietyRules::default(),
            difficulty_rules: DifficultyRules::default(),
            difficulty: 1,
            notoriety: 0.0,
            witnesses: 0,
            incident: HashSet::new(),
            incident_end: -1.0,
            base_outfit: 1,
            disguise: None,
            defs: Vec::new(),
            suit_safe_zones: 1,
            cameras: Vec::new(),
            zone_override: 0,
            tape_kind: 0,
            tape_room: None,
            tape_notice: String::new(),
        }
    }
}

impl Stealth {
    /// Reads `notoriety.xml` / `difficultysettings.xml` of the mounted level archive.
    pub fn load(vfs: &Vfs) -> Stealth {
        let text = |p: &str| vfs.open(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
        let mut s = Stealth::default();
        if let Some(x) = text("notoriety.xml") {
            s.notoriety_rules = NotorietyRules::parse(&x);
        }
        if let Some(x) = text("difficultysettings.xml") {
            s.difficulty_rules = DifficultyRules::parse(&x);
        }
        s
    }

    /// Installs the level's disguise definitions.
    pub fn set_defs(&mut self, defs: Vec<DisguiseDef>) {
        if let Some(t) = defs.iter().find(|d| d.quality == "eQualityHitman" && d.name == "HitmanTemplate") {
            self.suit_safe_zones = zones_mask(&t.zones);
        }
        self.defs = defs;
    }

    /// The definition of clothes that actors with this model and outfit value wear.
    pub fn def_for(&self, prim: Option<u32>, outfit: u8) -> Option<&DisguiseDef> {
        let p = prim? as i32;
        self.defs.iter().find(|d| d.prim == Some(p) && d.outfit == Some(outfit as i32))
    }

    pub fn max_witnesses(&self) -> u32 {
        self.notoriety_rules.max_witnesses[match self.difficulty {
            0 | 1 => 0,
            2 => 1,
            _ => 2,
        }]
    }

    pub fn see_through_distance(&self) -> f32 {
        SEE_THROUGH_BASE_CM * self.difficulty_rules.see_through_distance[self.difficulty.min(3)]
    }

    pub fn see_through_rate(&self) -> f32 {
        SEE_THROUGH_RATE * self.difficulty_rules.see_through_speed[self.difficulty.min(3)]
    }

    pub fn forget_rate(&self) -> f32 {
        SEE_THROUGH_RATE * self.difficulty_rules.forget_disguise_speed[self.difficulty.min(3)]
    }

    /// The outfit value the player model wears now.
    pub fn outfit(&self) -> u8 {
        self.disguise.as_ref().map(|d| d.outfit).unwrap_or(self.base_outfit)
    }

    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "notoriety": self.notoriety,
            "witnesses": self.witnesses,
            "difficulty": self.difficulty,
            "outfit": self.outfit(),
            "disguised": self.disguise.is_some(),
            "disguise": self.disguise.as_ref().map(|d| d.name.clone()),
        })
    }
}

impl World {
    /// Adds (or removes) notoriety points and tells scripts.
    pub fn add_notoriety(&mut self, delta: f32, why: &str) {
        let before = self.stealth.notoriety;
        // `FUN_006d0220`: with difficulty 0 (Rookie) the notoriety is always 0
        if self.stealth.difficulty == 0 {
            self.stealth.notoriety = 0.0;
            return;
        }
        self.stealth.notoriety = (before + delta).clamp(0.0, 100.0);
        if (self.stealth.notoriety - before).abs() > f32::EPSILON {
            self.outbox.push(Event::new("on_notoriety", json!({ "value": self.stealth.notoriety, "delta": self.stealth.notoriety - before, "reason": why })));
        }
    }

    /// People who can see `crime_pos` right now witness the crime of `culprit`; each counts once per incident, the first
    /// one counts most. Returns the NPCs that saw it.
    pub fn register_witnesses(&mut self, crime_pos: Vec3, culprit: EntityId, victim: Option<EntityId>) -> Vec<EntityId> {
        if Some(culprit) != self.player {
            return Vec::new();
        }
        if self.time > self.stealth.incident_end {
            self.stealth.incident.clear();
        }
        self.stealth.incident_end = self.time + INCIDENT_SECONDS;
        let mut seen = Vec::new();
        for id in self.ais.ids() {
            if Some(id) == victim || self.stealth.incident.contains(&id) {
                continue;
            }
            let Some(ai) = self.ais.get(id) else { continue };
            if ai.dead {
                continue;
            }
            let Some(p) = self.pos(id) else { continue };
            if p.dist(crime_pos) < 1800.0 && self.collision.line_of_sight(p + crate::math::v3(0.0, 150.0, 0.0), crime_pos + crate::math::v3(0.0, 100.0, 0.0)) {
                seen.push(id);
            }
        }
        for id in &seen {
            self.stealth.incident.insert(*id);
            if self.stealth.witnesses >= self.stealth.max_witnesses() {
                continue;
            }
            self.stealth.witnesses += 1;
            let pts = self.stealth.notoriety_rules.witness_points(self.stealth.witnesses);
            self.add_notoriety(pts, "witness");
            self.outbox.push(Event::new("on_witness", json!({ "witness": id.0, "count": self.stealth.witnesses })));
        }
        seen
    }

    /// Every security camera watches the player each tick (`ZGEOM_SecurityCamera`, `FUN_005bc7e0` / `FUN_005bc630`). A
    /// sighting of the hitman (inside the view, with line of sight) starts a recording, or extends the running one. A
    /// recording is event type 10 (`FUN_005bc0e0`) and lasts [`RECORD_SECONDS`]. The camera's `Deactivate` and
    /// `ProjectileHit` are not modelled: nothing in the level sends them yet.
    ///
    /// Starting a recording sets the tape kind (`FUN_005bc630`) and adds the notoriety of that kind. The notoriety is
    /// added once per recording start, which is a guess: the decode shows the kind rule and the points, not the call.
    pub(crate) fn update_cameras(&mut self) {
        let Some(pp) = self.player_pos() else { return };
        let head = pp + crate::math::v3(0.0, 100.0, 0.0);
        let now = self.time;
        let room = self.areas.player_room;
        for i in 0..self.stealth.cameras.len() {
            let seen = self.stealth.cameras[i].sees(head) && self.collision.line_of_sight(self.stealth.cameras[i].pos, head);
            if !seen {
                continue;
            }
            let was_recording = self.stealth.cameras[i].recording_until > now;
            self.stealth.cameras[i].recording_until = now + RECORD_SECONDS;
            if was_recording {
                continue;
            }
            let same_room = room.is_some() && room == self.stealth.tape_room;
            self.stealth.tape_kind = next_tape_kind(self.stealth.tape_kind, same_room);
            self.stealth.tape_room = room;
            self.caught_on_tape(i, self.stealth.tape_kind);
            // `FUN_005bc0e0` calls `thunk_FUN_00660ef0(CaughtOnSuveillanceTape, 1)`: a notice for 10 s (`0x41200000`)
            if !self.stealth.tape_notice.is_empty() {
                let text = self.stealth.tape_notice.clone();
                self.fx.push(crate::world::FxEvent::Message { text, seconds: 10.0 });
            }
        }
    }

    /// The camera recorded the player on a tape of `kind`: kind 1 counts the `CaughtOnTapeNotDisguised` points, kind 2
    /// the `CaughtOnTapeDisguised` points (`FUN_006d0220`: `kind == 1` adds `param[0]`, `kind == 2` adds `param[1]`).
    pub fn caught_on_tape(&mut self, camera: usize, kind: u8) {
        let r = &self.stealth.notoriety_rules;
        let pts = if kind == 2 { r.caught_on_tape_disguised } else { r.caught_on_tape_not_disguised };
        self.add_notoriety(pts, "caught_on_tape");
        self.outbox.push(Event::new("on_caught_on_tape", json!({ "camera": camera, "kind": kind })));
    }

    /// Bribes and a new identity (post mission spending of the original) lower the notoriety.
    pub fn spend_notoriety(&mut self, kind: &str) -> bool {
        let r = &self.stealth.notoriety_rules;
        let d = match kind {
            "bribe_innocent" => r.bribe_innocent,
            "bribe_police" => r.bribe_police,
            "new_identity" => r.new_identity,
            _ => return false,
        };
        self.add_notoriety(d, kind);
        true
    }

    /// GUESS: the clothes the player wears (own suit or disguise) are out of place in the current zone.
    pub fn clothes_out_of_place(&self) -> bool {
        let safe = match &self.stealth.disguise {
            None => Some(self.stealth.suit_safe_zones),
            Some(d) => d.safe_zones,
        };
        out_of_place(safe, self.zone_state())
    }

    /// GUESS (not verified in the original; user-authorized): may the player take the clothes of this NPC here?
    /// Uses the outfit's safe-zone list from its `ZHM3HmAs` definition, see [`clothes_allowed_in_zone`].
    pub fn can_take_clothes(&self, victim: EntityId) -> bool {
        let prim = self.renders.get(victim).and_then(|r| r.prim_id);
        let outfit = self.characters.get(victim).map(|c| c.outfit).unwrap_or(1);
        let safe = self.stealth.def_for(prim, outfit).map(|d| zones_mask(&d.zones));
        clothes_allowed_in_zone(safe, self.zone_state())
    }

    /// Puts on the clothes of a dead / unconscious NPC. Returns false when there is nothing to take.
    pub fn take_disguise(&mut self, victim: EntityId) -> bool {
        if self.is_alive(victim) || self.infos.get(victim).is_none_or(|i| i.kind != crate::world::Kind::Npc) {
            return false;
        }
        if !self.can_take_clothes(victim) {
            return false;
        }
        let Some(ch) = self.characters.get(victim) else { return false };
        let (class, their_outfit) = (ch.role, ch.outfit);
        let name = self.infos.get(victim).map(|i| i.name.clone()).unwrap_or_default();
        // the player model carries the same outfit values as the cloth bundle links: use the victim's if the model has it
        let outfit = self.player_outfit_for(their_outfit);
        let node = self.infos.get(victim).and_then(|i| i.node);
        let prim = self.renders.get(victim).and_then(|r| r.prim_id);
        let def = self.stealth.def_for(prim, their_outfit).cloned();
        let safe_zones = def.as_ref().map(|d| zones_mask(&d.zones));
        let quality = def.map(|d| d.quality).unwrap_or_default();
        self.stealth.disguise = Some(Disguise { outfit, class, colleague_outfit: their_outfit, name: name.clone(), safe_zones, quality, node, prim });
        // everybody forgets what they knew about the old clothes
        for (_, ai) in self.ais.iter_mut() {
            ai.disguise_awareness = 0.0;
        }
        self.outbox.push(Event::new("on_disguise", json!({ "outfit": outfit, "class": class.as_str(), "from": name })));
        true
    }

    /// Back into the own suit.
    pub fn drop_disguise(&mut self) {
        if self.stealth.disguise.take().is_some() {
            for (_, ai) in self.ais.iter_mut() {
                ai.disguise_awareness = 0.0;
            }
            self.outbox.push(Event::new("on_disguise", json!({ "outfit": self.stealth.base_outfit, "class": null, "from": null })));
        }
    }

    fn player_outfit_for(&self, want: u8) -> u8 {
        if self.player_outfits.contains(&want) {
            want
        } else {
            self.player_outfits.iter().copied().find(|m| m & 3 == want & 3 && *m != self.stealth.base_outfit).unwrap_or(want)
        }
    }

    /// The zone state the detection logic uses (`FUN_005f29b0`): the override raised by the security boxes the player is in,
    /// else the state of the player's zone, else 1 (public).
    pub fn zone_state(&self) -> u32 {
        if self.stealth.zone_override != 0 {
            return self.stealth.zone_override;
        }
        match self.player_zone_name().map(zone_state_of) {
            Some(z) if z != 0 => z,
            _ => ZONE_PUBLIC,
        }
    }

    /// Per perception tick of one NPC: does it recognise that the player's clothes are not his? Returns true when the NPC
    /// has seen through the disguise (it then treats the player as undisguised).
    pub(crate) fn disguise_awareness_step(&self, ai_awareness: &mut f32, role: Role, outfit: u8, dist: f32, sees: bool, dt: f32) -> bool {
        let Some(d) = &self.stealth.disguise else {
            *ai_awareness = 0.0;
            return false;
        };
        // only colleagues (same outfit, or the same role for outfit-less models) care about the clothes
        let colleague = outfit == d.colleague_outfit || (role == d.class && role == Role::Guard);
        if !colleague {
            *ai_awareness = 0.0;
            return false;
        }
        if sees && dist <= self.stealth.see_through_distance() {
            // closer is faster
            let near = 1.0 + (1.0 - dist / self.stealth.see_through_distance()).clamp(0.0, 1.0);
            *ai_awareness = (*ai_awareness + self.stealth.see_through_rate() * near * dt).min(1.0);
        } else {
            *ai_awareness = (*ai_awareness - self.stealth.forget_rate() * dt).max(0.0);
        }
        *ai_awareness >= 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_shipped_notoriety_file() {
        let xml = "<Notoriety>
	<CaughtOnTapeNotDisguised value=\"20\"/>
	<Witness1 value=\"10\"/>
	<Witness2 value=\"5\"/>
	<MaxWitnessesExpert value=\"21\"/>
	<BribePolice value=\"-40\"/></Notoriety>";
        let r = NotorietyRules::parse(xml);
        assert_eq!(r.witness_points(1), 10.0);
        assert_eq!(r.witness_points(2), 5.0);
        assert_eq!(r.witness_points(9), 2.0);
        assert_eq!(r.max_witnesses, [15, 21, 25]);
        assert_eq!(r.bribe_police, -40.0);
    }

    #[test]
    fn clothes_rule_guess() {
        // no definition: never restricted
        assert!(clothes_allowed_in_zone(None, ZONE_PUBLIC));
        // listed zone allowed, unlisted refused
        let guard = zone_state_of("eZone1") | zone_state_of("eZone2");
        assert!(clothes_allowed_in_zone(Some(guard), zone_state_of("eZone2")));
        assert!(!clothes_allowed_in_zone(Some(guard), zone_state_of("eZone3")));
        // mega-forbidden refuses everything
        assert!(!clothes_allowed_in_zone(Some(0xff), ZONE_MEGA_FORBIDDEN));
    }

    #[test]
    fn out_of_place_guess() {
        let suit = Some(zone_state_of("eZone1"));
        assert!(!out_of_place(suit, zone_state_of("eZone1")));
        assert!(out_of_place(suit, zone_state_of("eZone3")));
        assert!(!out_of_place(None, zone_state_of("eZone3")));
        // the forbidden zone has its own rule
        assert!(!out_of_place(suit, ZONE_MEGA_FORBIDDEN));
    }

    #[test]
    fn camera_view_guess() {
        let cam = SecurityCamera { pos: Vec3::ZERO, dir: crate::math::v3(0.0, 0.0, 1.0), range: 6000.0, half_angle_deg: 20.0, recording_until: f64::NEG_INFINITY };
        assert!(cam.sees(crate::math::v3(0.0, 0.0, 1000.0)));
        assert!(!cam.sees(crate::math::v3(0.0, 0.0, 7000.0)), "beyond range");
        assert!(!cam.sees(crate::math::v3(2000.0, 0.0, 1000.0)), "outside the 20 degree cone");
        assert!(!cam.sees(crate::math::v3(0.0, 0.0, -1000.0)), "behind the lens");
    }

    #[test]
    fn tape_kind_follows_decoded_rule() {
        // same room is always 2
        assert_eq!(next_tape_kind(0, true), 2);
        assert_eq!(next_tape_kind(1, true), 2);
        // another room gives 1 unless the kind is already 2, which then sticks
        assert_eq!(next_tape_kind(0, false), 1);
        assert_eq!(next_tape_kind(1, false), 1);
        assert_eq!(next_tape_kind(2, false), 2);
    }

    #[test]
    fn parses_difficulty_rows() {
        let xml = "<D><SeeThroughDisguiseDistance Rookie=\"0.5\" Normal=\"1.0\" Expert=\"1.8\" Professional=\"2.0\"/><ForgetDisguiseSpeed Rookie=\"2.0\" Normal=\"1.0\" Expert=\"0.7\" Professional=\"0.5\"/></D>";
        let r = DifficultyRules::parse(xml);
        assert_eq!(r.see_through_distance, [0.5, 1.0, 1.8, 2.0]);
        assert_eq!(r.forget_disguise_speed[2], 0.7);
    }
}
