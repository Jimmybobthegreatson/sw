//! Data-driven game definitions (RON): weapons / pickups and missions.
//!
//! Defaults are embedded (`data/weapons.ron`, `data/missions/*.ron`, our own configuration, not game assets)
//! and can be replaced by loose files in the VFS (mod dirs): `Game/weapons.ron`, `Game/Missions/<id>.ron`.
//! The same tables are exposed to scripts as the `weapons` dataset (see [`GameData::weapons_json`]).

use bm_vfs::Vfs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WeaponKind {
    #[default]
    Pistol,
    Smg,
    Rifle,
    Shotgun,
    Sniper,
    Melee,
    Tool,
}

/// Which recoil axes the original game randomises (`eNoRandom`, `eXRandom`, `eYRandom`, `eXYRandom` item tokens).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RecoilMode {
    #[default]
    None,
    XRandom,
    YRandom,
    XyRandom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeaponDef {
    pub name: String,
    pub kind: WeaponKind,
    /// Native recoil values (see `crate::recoil`): horizontal and vertical kick per accumulator unit, as in the item data.
    pub recoil_x: f32,
    pub recoil_y: f32,
    pub recoil_mode: RecoilMode,
    pub damage: f32,
    /// Damage multiplier for head hits.
    pub headshot_mult: f32,
    /// Shots per second.
    pub fire_rate: f32,
    pub auto: bool,
    pub mag_size: u32,
    pub reserve: u32,
    pub reload_time: f32,
    pub range: f32,
    pub spread_deg: f32,
    pub pellets: u32,
    /// Radius (cm) in which NPCs hear a shot; 0 = silent.
    pub noise: f32,
    /// Can be carried without making guards suspicious.
    pub concealable: bool,
    /// Sound bank name of the shot / swing.
    pub fire_sound: String,
    pub reload_sound: String,
    /// Scene particle template used as muzzle effect.
    pub muzzle_particle: String,
    /// Name of the weapon model group of the original game (`Custom_Pistol`, `SMG_MP5_01` ...), empty = not drawn.
    pub model: String,
    /// Aimed-pose clip of the `Hero_1Person` set (`UB_1P_Hold_USP`), empty = the generic hold of the weapon kind.
    pub hold: String,
    /// Part ids (`bm_models::Mesh::part`) of a modular model that make up the stock weapon, e.g. frame + slide + barrel +
    /// clip of a `Custom_*` weapon (`docs/models.md`). Empty = draw every mesh of the model.
    pub parts: Vec<u32>,
    /// What each named upgrade adds to / removes from `parts` (`silencer`, `laser`, ...), see [`WeaponDef::visible_parts`].
    pub upgrade_parts: BTreeMap<String, PartUpgrade>,
    /// Upgrades the weapon carries (keys of `upgrade_parts`); none by default.
    pub upgrades: Vec<String>,
}

/// Part changes of one weapon upgrade (the original `WeaponUpgradeControl` upgrade nodes add and remove `W_Bone_*` parts).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PartUpgrade {
    pub add: Vec<u32>,
    pub remove: Vec<u32>,
}

impl WeaponDef {
    /// Part ids drawn for this weapon: `parts` with every active upgrade applied (sorted); empty = all meshes of the model.
    pub fn visible_parts(&self) -> Vec<u32> {
        if self.parts.is_empty() {
            return Vec::new();
        }
        let mut p = self.parts.clone();
        for u in self.upgrades.iter().filter_map(|n| self.upgrade_parts.get(n)) {
            p.retain(|x| !u.remove.contains(x));
            p.extend(&u.add);
        }
        p.sort_unstable();
        p.dedup();
        p
    }
}

impl Default for WeaponDef {
    fn default() -> Self {
        WeaponDef {
            name: String::new(),
            kind: WeaponKind::Pistol,
            recoil_x: 0.0,
            recoil_y: 0.0,
            recoil_mode: RecoilMode::None,
            damage: 25.0,
            headshot_mult: 3.0,
            fire_rate: 3.0,
            auto: false,
            mag_size: 8,
            reserve: 40,
            reload_time: 1.8,
            range: 8000.0,
            spread_deg: 1.0,
            pellets: 1,
            noise: 3000.0,
            concealable: true,
            fire_sound: String::new(),
            reload_sound: String::new(),
            muzzle_particle: "Gun_Flash".into(),
            model: String::new(),
            hold: String::new(),
            parts: Vec::new(),
            upgrade_parts: BTreeMap::new(),
            upgrades: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GameData {
    pub weapons: BTreeMap<String, WeaponDef>,
    /// Weapons the player starts with, first one is drawn first.
    pub start_loadout: Vec<String>,
    /// Pickup node-name prefix (lower case, text before the first `_`) -> weapon id.
    pub pickups: BTreeMap<String, String>,
    /// Original item names (`SMG_FamaeSAF_01`, `Gun_SnubNosed_01`, `CC_Hammer_01` ...): lower-case fragment -> weapon id.
    /// The first fragment contained in the item name wins (entries are tried in key order).
    pub item_aliases: BTreeMap<String, String>,
    /// NPC loadouts by role: `guard`, `civilian`, `target`.
    pub npc_weapons: BTreeMap<String, String>,
    pub player_health: f32,
    pub npc_health: BTreeMap<String, f32>,
}

pub const DEFAULT_WEAPONS_RON: &str = include_str!("../data/weapons.ron");
pub const MISSION_M01_RON: &str = include_str!("../data/missions/M01.ron");
pub const MISSION_HIDEOUT_RON: &str = include_str!("../data/missions/hideout.ron");

impl GameData {
    pub fn embedded() -> GameData {
        ron::from_str(DEFAULT_WEAPONS_RON).expect("embedded weapons.ron must parse")
    }

    /// Embedded defaults, replaced by `Game/weapons.ron` from the VFS (mod dirs) when present.
    pub fn load(vfs: Option<&Vfs>) -> GameData {
        if let Some(v) = vfs {
            if let Ok(bytes) = v.open("Game/weapons.ron") {
                match ron::from_str::<GameData>(&String::from_utf8_lossy(&bytes)) {
                    Ok(d) => return d,
                    Err(e) => log::warn!("Game/weapons.ron ignored: {e}"),
                }
            }
        }
        GameData::embedded()
    }

    pub fn weapon(&self, id: &str) -> Option<&WeaponDef> {
        self.weapons.get(id)
    }

    /// Part ids to draw of the weapon model group `model` (case-insensitive): taken from the first weapon definition that
    /// uses that model. Empty = every mesh.
    pub fn parts_for_model(&self, model: &str) -> Vec<u32> {
        self.weapons.values().find(|d| d.model.eq_ignore_ascii_case(model)).map(WeaponDef::visible_parts).unwrap_or_default()
    }

    /// Weapon id for an original item definition name (`CC_Hammer_01` -> `hammer`), if we have a stand-in for it.
    pub fn weapon_for_item(&self, item: &str) -> Option<String> {
        let n = item.to_ascii_lowercase();
        if self.weapons.contains_key(&n) {
            return Some(n);
        }
        // the item definition of the original game is the weapon model group (`Rifle_Remington_01`)
        if let Some((id, _)) = self.weapons.iter().find(|(_, d)| d.model.eq_ignore_ascii_case(item)) {
            return Some(id.clone());
        }
        for (frag, id) in self.item_aliases.iter().chain(self.pickups.iter()) {
            if n.contains(frag.as_str()) && self.weapons.contains_key(id) {
                return Some(id.clone());
            }
        }
        // built-in classes of the original item names
        let class = if n.starts_with("gun_") || n.contains("pistol") || n.starts_with("custom_pistol") {
            "guard_pistol"
        } else if n.starts_with("smg_") || n.starts_with("rifle_") || n.contains("shotgun") || n.contains("assault") {
            "smg"
        } else {
            return None;
        };
        self.weapons.contains_key(class).then(|| class.to_string())
    }

    /// JSON view for the script `data` store (`weapons` dataset).
    pub fn weapons_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.weapons).unwrap_or_default()
    }

    /// Replace the weapon table from a (possibly patched) JSON dataset; unknown/invalid entries are skipped.
    pub fn apply_weapons_json(&mut self, v: &serde_json::Value) {
        if let Ok(m) = serde_json::from_value::<BTreeMap<String, WeaponDef>>(v.clone()) {
            self.weapons = m;
        }
    }
}

// ------------------------------------------------------------------------------------------ missions

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ObjectiveKind {
    /// Kill every NPC whose name contains `entity` (case-insensitive).
    Kill { entity: String },
    /// Kill the one NPC whose name equals `entity` (case-insensitive; used by the objectives derived from the level data).
    KillExact { entity: String },
    /// Keep the NPC `entity` alive (never completes: it only fails the mission when the NPC dies) - `P` objectives.
    Protect { entity: String },
    /// Be inside the `index`-th exit area of the level (`World::triggers`, kind `Exit`).
    ReachExit { index: usize },
    /// Come within `radius` cm of the node whose name contains `node`.
    ReachNode { node: String, radius: f32 },
    ReachPoint { pos: [f32; 3], radius: f32 },
    /// Pick up an item (weapon id).
    Pickup { item: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectiveDef {
    pub id: String,
    pub text: String,
    pub kind: ObjectiveKind,
    pub optional: bool,
    /// Objective ids that must be complete before this one can complete.
    pub requires: Vec<String>,
}

impl Default for ObjectiveDef {
    fn default() -> Self {
        ObjectiveDef {
            id: String::new(),
            text: String::new(),
            kind: ObjectiveKind::ReachPoint { pos: [0.0; 3], radius: 100.0 },
            optional: false,
            requires: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FailCondition {
    PlayerDead,
    /// A named NPC died while not being the player's kill target (e.g. innocent targets).
    EntityKilled { entity: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MissionDef {
    pub id: String,
    pub title: String,
    /// Scene base, e.g. `Scenes/M01/M01_main`.
    pub level: String,
    /// Player start override; otherwise the scene's `ZHitman3` node is used.
    pub player_start: Option<[f32; 3]>,
    pub objectives: Vec<ObjectiveDef>,
    pub fail: Vec<FailCondition>,
}

impl MissionDef {
    pub fn parse(src: &str) -> Result<MissionDef, ron::error::SpannedError> {
        ron::from_str(src)
    }

    /// A mission file from the VFS (mod dirs): `Game/Missions/<dir>.ron` or `<stem>.ron`.
    pub fn vfs_override(vfs: Option<&Vfs>, level: &str) -> Option<MissionDef> {
        let v = vfs?;
        let stem = level.rsplit(['/', '\\']).next().unwrap_or(level);
        let dir = level.trim_start_matches("Scenes/").split('/').next().unwrap_or("");
        for key in [dir, stem] {
            if let Ok(b) = v.open(&format!("Game/Missions/{key}.ron")) {
                match MissionDef::parse(&String::from_utf8_lossy(&b)) {
                    Ok(m) => return Some(m),
                    Err(e) => log::warn!("Game/Missions/{key}.ron ignored: {e}"),
                }
            }
        }
        None
    }

    /// Mission for a scene base: a VFS override `Game/Missions/<stem>.ron`, then the embedded definitions.
    pub fn for_level(vfs: Option<&Vfs>, level: &str) -> Option<MissionDef> {
        if let Some(m) = MissionDef::vfs_override(vfs, level) {
            return Some(m);
        }
        let dir = level.trim_start_matches("Scenes/").split('/').next().unwrap_or("");
        match dir.to_ascii_lowercase().as_str() {
            "m01" => MissionDef::parse(MISSION_M01_RON).ok(),
            "hideout" => MissionDef::parse(MISSION_HIDEOUT_RON).ok(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_data_parses() {
        let d = GameData::embedded();
        assert!(d.weapons.contains_key("silverballer"));
        assert!(!d.start_loadout.is_empty());
        for w in &d.start_loadout {
            assert!(d.weapons.contains_key(w), "{w}");
        }
        let m = MissionDef::parse(MISSION_M01_RON).unwrap();
        assert_eq!(m.id, "M01");
        assert!(m.objectives.len() >= 2);
        assert!(MissionDef::parse(MISSION_HIDEOUT_RON).is_ok());
    }

    #[test]
    fn weapons_json_round_trips() {
        let mut d = GameData::embedded();
        let mut j = d.weapons_json();
        j["silverballer"]["damage"] = serde_json::json!(99.0);
        d.apply_weapons_json(&j);
        assert_eq!(d.weapon("silverballer").unwrap().damage, 99.0);
    }
}
