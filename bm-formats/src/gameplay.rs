//! Typed views of the gameplay related scene nodes and controllers (doors, patrol routes, actor scripts, trigger
//! boxes, security zones, exit areas, pickups, climb boxes) plus the per-level mission objectives in the LOC file.
//!
//! The PRP stores property values without field names; the layouts below were identified by reading every node of the
//! 14 shipped levels (`docs/formats.md`, section "Gameplay nodes"). Nothing here needs world transforms: node
//! references are returned as scene node indices (resolved through [`PathIndex`]) and the caller composes matrices.

use std::collections::HashMap;

use crate::loc::Loc;
use crate::prp::Value;
use crate::scene::{SceneGraph, SceneNode};

pub const TYPE_BOX: u32 = 0x2000e1;
pub const TYPE_ZLIST: u32 = 0x800001a;
pub const TYPE_CLIMB_GUIDE: u32 = 0x200435;
pub const TYPE_USEPOINT: u32 = 0x200445;
pub const TYPE_ACTOR: u32 = 0x2003e9;
pub const TYPE_HMAS: u32 = 0x100446;
pub const TYPE_LINK: u32 = 0x200006;

/// Normalises a reference string (`ROOT\M01/M01_CHARACTERS.zip\Controllers!Levelcontroller`) to the form of
/// [`SceneGraph::path`] (forward slashes, no `ROOT/` prefix, lower case).
pub fn normalize_ref(s: &str) -> String {
    let s = s.replace('\\', "/");
    let s = s.strip_prefix("ROOT/").unwrap_or(&s);
    s.trim_start_matches('/').to_ascii_lowercase()
}

/// Case-insensitive lookup of nodes by full path / by path suffix.
pub struct PathIndex {
    by_path: HashMap<String, usize>,
}

impl PathIndex {
    pub fn new(g: &SceneGraph) -> PathIndex {
        let mut by_path = HashMap::with_capacity(g.nodes.len());
        for n in &g.nodes {
            by_path.entry(g.path(n.index).to_ascii_lowercase()).or_insert(n.index);
        }
        PathIndex { by_path }
    }

    /// Resolves a `ROOT\...` reference string to a node index.
    pub fn resolve(&self, s: &str) -> Option<usize> {
        self.by_path.get(&normalize_ref(s)).copied()
    }
}

fn strs(v: &Value) -> Vec<String> {
    match v {
        Value::Container(c) | Value::Array(c) => c.iter().filter_map(|x| if let Value::Str(s) = x { Some(s.clone()) } else { None }).collect(),
        Value::Str(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn token(v: Option<&Value>) -> Option<&str> {
    match v? {
        Value::Token(t) => Some(t),
        _ => None,
    }
}

fn f32_of(v: Option<&Value>) -> Option<f32> {
    match v? {
        Value::F32(f) => Some(*f),
        _ => None,
    }
}

fn bool_of(v: Option<&Value>) -> Option<bool> {
    match v? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn str_of(v: Option<&Value>) -> Option<&str> {
    match v? {
        Value::Str(s) => Some(s),
        _ => None,
    }
}

fn controller<'a>(n: &'a SceneNode, name: &str) -> Option<&'a crate::scene::Controller> {
    n.controllers.iter().find(|c| c.name == name)
}

// ------------------------------------------------------------------------------------------------ doors

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorKind {
    /// `SingleDoor` controller on a model node: one leaf hinged at the node origin.
    Single,
    /// `MultiDoor` controller on a group: the leaves are the children (`D1`, `D2` ...).
    Multi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorState {
    Closed,
    Locked,
    Open,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DoorDef {
    pub node: usize,
    pub kind: DoorKind,
    /// `IDS_CLOSED` / `IDS_LOCKED` / `IDS_OPEN` (props[0]).
    pub initial: DoorState,
    /// Lock-picking difficulty token (`EASY`, ... ; `IMPOSSIBLE` = cannot be picked) (props[11]).
    pub lock_difficulty: String,
    /// `AWAY` (props[7]): the door swings away from whoever opens it.
    pub open_direction: String,
    /// True for `DT_MATPOSANIM` (sliding doors animated by a position animation) instead of `DT_NORMAL` swing doors.
    pub sliding: bool,
    /// Animation speed factor (props[20]: 1.0 for swing doors, 0.8 for sliding ones).
    pub speed: f32,
    /// `.SND` object offsets of the open / close sounds (props[8], props[9]; 0 = none).
    pub open_sound: u32,
    pub close_sound: u32,
    /// Raw boolean flags props[1..7] (meaning unknown; e.g. `true` for ordinary doors, `false` for cellar hatches).
    pub flags: Vec<bool>,
}

pub fn doors(g: &SceneGraph) -> Vec<DoorDef> {
    let mut out = Vec::new();
    for n in &g.nodes {
        for c in &n.controllers {
            let kind = match c.name.as_str() {
                "SingleDoor" => DoorKind::Single,
                "MultiDoor" => DoorKind::Multi,
                _ => continue,
            };
            let p = &c.props;
            let initial = match token(p.first()) {
                Some("IDS_LOCKED") => DoorState::Locked,
                Some("IDS_OPEN") => DoorState::Open,
                _ => DoorState::Closed,
            };
            out.push(DoorDef {
                node: n.index,
                kind,
                initial,
                lock_difficulty: token(p.get(11)).unwrap_or("IMPOSSIBLE").to_string(),
                open_direction: token(p.get(7)).unwrap_or("AWAY").to_string(),
                sliding: token(p.get(19)) == Some("DT_MATPOSANIM"),
                speed: f32_of(p.get(20)).unwrap_or(1.0),
                open_sound: p.get(8).and_then(|v| v.as_i32()).unwrap_or(0) as u32,
                close_sound: p.get(9).and_then(|v| v.as_i32()).unwrap_or(0) as u32,
                flags: (1..7).filter_map(|i| bool_of(p.get(i))).collect(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------- routes / actors

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathMode {
    /// Walks the list in order and wraps around.
    Looping,
    /// Picks waypoints at random.
    Random,
    /// Any other token (kept for completeness).
    Other,
}

/// `PathFollower` controller of an actor node.
#[derive(Clone, Debug, PartialEq)]
pub struct PathFollowerDef {
    pub node: usize,
    /// Waypoint list (`ZLIST`) node references.
    pub lists: Vec<String>,
    pub mode: PathMode,
    pub mode_token: String,
    /// props[2..5] booleans (unknown).
    pub flags: Vec<bool>,
}

pub fn path_followers(g: &SceneGraph) -> Vec<PathFollowerDef> {
    let mut out = Vec::new();
    for n in &g.nodes {
        let Some(c) = controller(n, "PathFollower") else { continue };
        let mode_token = token(c.props.get(1)).unwrap_or("").to_string();
        let mode = match mode_token.as_str() {
            "LOOPING" => PathMode::Looping,
            "RANDOM" => PathMode::Random,
            _ => PathMode::Other,
        };
        out.push(PathFollowerDef {
            node: n.index,
            lists: c.props.first().map(strs).unwrap_or_default(),
            mode,
            mode_token,
            flags: (2..5).filter_map(|i| bool_of(c.props.get(i))).collect(),
        });
    }
    out
}

/// Node references of a `ZLIST` node (waypoint lists, exit lists, elevator target lists ...).
pub fn zlist_items(n: &SceneNode) -> Vec<String> {
    if n.type_id != TYPE_ZLIST {
        return Vec::new();
    }
    n.props.get(5).map(strs).unwrap_or_default()
}

/// `ScriptC` controller: the native script module attached to a node plus its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptDef {
    pub node: usize,
    /// Module name (backslashes turned into `/`) such as `m01/m01_donchilean` / `m01/m01_useable_getautograph` (compiled code in `Scriptcs/*.dll`).
    pub module: String,
    /// Remaining parameter values (module specific layout).
    pub params: Vec<Value>,
}

impl ScriptDef {
    /// All string parameters (node references, weapon names, event names ...).
    pub fn strings(&self) -> Vec<String> {
        self.params.iter().flat_map(strs).filter(|s| !s.is_empty()).collect()
    }
}

pub fn scripts(g: &SceneGraph) -> Vec<ScriptDef> {
    let mut out = Vec::new();
    for n in &g.nodes {
        for c in n.controllers.iter().filter(|c| c.name == "ScriptC") {
            if let Some(Value::Str(m)) = c.props.first() {
                out.push(ScriptDef { node: n.index, module: m.replace('\\', "/"), params: c.props[1..].to_vec() });
            }
        }
    }
    out
}

/// Gameplay parameters of a `ZHM3Actor`.
#[derive(Clone, Debug, PartialEq)]
pub struct ActorDef {
    pub node: usize,
    /// Animation database name (`M01_Don`), from the `anmcol:animationdatabase#...` property.
    pub anim_db: Option<String>,
    /// Outfit selector (property 6, see `docs/anim.md`).
    pub outfit: Option<i32>,
    pub script: Option<String>,
    pub path: Option<PathFollowerDef>,
    /// Weapon the script gives the actor (a string equal to the name of a weapon item definition node).
    pub weapon: Option<String>,
    /// `Controllers!Helppoints!...` list the actor runs to when alarmed (first `Helplist` reference of the script).
    pub help_list: Option<String>,
}

pub fn actors(g: &SceneGraph) -> Vec<ActorDef> {
    let idx = PathIndex::new(g);
    let all_scripts = scripts(g);
    let all_paths = path_followers(g);
    let mut out = Vec::new();
    for n in g.nodes.iter().filter(|n| n.type_id == TYPE_ACTOR) {
        let anim_db = n.props.iter().find_map(|v| match v {
            Value::Str(s) if s.starts_with("anmcol:animationdatabase#") => Some(s["anmcol:animationdatabase#".len()..].to_string()),
            _ => None,
        });
        let outfit = n.props.get(6).and_then(|v| v.as_i32());
        let script = all_scripts.iter().find(|s| s.node == n.index);
        // the weapon is a reference to a weapon item definition (type 0x100429)
        let weapon = script.and_then(|s| {
            s.strings().into_iter().find_map(|t| {
                let i = idx.resolve(&t)?;
                (g.nodes[i].type_id == 0x100429).then(|| g.nodes[i].name.clone())
            })
        });
        let help_list = script.and_then(|s| s.strings().into_iter().find(|t| t.to_ascii_lowercase().contains("helplist")));
        let path = all_paths.iter().find(|p| p.node == n.index).cloned();
        out.push(ActorDef { node: n.index, anim_db, outfit, script: script.map(|s| s.module.clone()), path, weapon, help_list });
    }
    out
}

// ------------------------------------------------------------------------------------------------ volumes

/// What a `ZBoxPrimitive` trigger volume does.
#[derive(Clone, Debug, PartialEq)]
pub enum VolumeKind {
    /// `CheckInside`: when `watched` nodes enter / leave the box, `target` receives `enter_msg` / `leave_msg`.
    CheckInside { watched: Vec<String>, target: String, enter_msg: String, leave_msg: String, flags: Vec<bool> },
    /// `SecurityZone`: trespassing zone token (`eZone1_PATCH` ...), props `[Bool, Token]`.
    Security { zone: String, enabled: bool },
    /// `ZoneControl`: names the room groups that make up a security zone.
    ZoneRooms { zone: String, rooms: Vec<String> },
    /// `Action` with `AT_GENERIC` (exit areas, mission actions): `event` is delivered to `target`.
    Action { label: String, token: String, event: String, target: String },
    /// `Action` with `AT_PICKUP` on a box.
    Pickup,
    /// Box without a gameplay controller we know (sound volumes, markers ...).
    Plain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VolumeDef {
    pub node: usize,
    /// Full box size along the node's local axes (props[6..9]); the box is centred on the node origin.
    pub size: [f32; 3],
    pub kind: VolumeKind,
}

/// Every `ZBoxPrimitive` with the gameplay meaning of its controllers (one entry per controller of interest).
pub fn volumes(g: &SceneGraph) -> Vec<VolumeDef> {
    let mut out = Vec::new();
    for n in g.nodes.iter().filter(|n| n.type_id == TYPE_BOX) {
        let size = match (f32_of(n.props.get(6)), f32_of(n.props.get(7)), f32_of(n.props.get(8))) {
            (Some(x), Some(y), Some(z)) => [x, y, z],
            _ => continue,
        };
        for c in &n.controllers {
            let p = &c.props;
            let kind = match c.name.as_str() {
                "CheckInside" => {
                    let paths: Vec<String> = p.iter().take(2).filter_map(|v| str_of(Some(v)).map(str::to_string)).collect();
                    VolumeKind::CheckInside {
                        watched: paths.first().cloned().into_iter().collect(),
                        target: paths.get(1).cloned().unwrap_or_default(),
                        enter_msg: str_of(p.get(2)).unwrap_or("").to_string(),
                        leave_msg: str_of(p.get(3)).unwrap_or("").to_string(),
                        flags: p.iter().skip(4).filter_map(|v| match v { Value::I32(i) => Some(*i != 0), Value::Bool(b) => Some(*b), _ => None }).collect(),
                    }
                }
                "SecurityZone" => VolumeKind::Security { enabled: p.first().and_then(|v| v.as_i32()).map(|i| i != 0).or_else(|| bool_of(p.first())).unwrap_or(true), zone: token(p.get(1)).unwrap_or("").to_string() },
                "ZoneControl" => VolumeKind::ZoneRooms { zone: token(p.first()).unwrap_or("").to_string(), rooms: p.get(1).map(strs).unwrap_or_default() },
                "Action" => match token(p.get(2)) {
                    Some("AT_PICKUP") => VolumeKind::Pickup,
                    Some(t) => VolumeKind::Action {
                        label: str_of(p.first()).unwrap_or("").to_string(),
                        token: t.to_string(),
                        event: str_of(p.get(3)).unwrap_or("").to_string(),
                        target: str_of(p.get(4)).unwrap_or("").to_string(),
                    },
                    None => continue,
                },
                _ => continue,
            };
            out.push(VolumeDef { node: n.index, size, kind });
        }
    }
    out
}

/// `ZoneControl` boxes are the only carriers of the zone -> rooms table; this collapses them into `(zone, rooms)`.
pub fn zone_rooms(g: &SceneGraph) -> Vec<(String, Vec<String>)> {
    volumes(g).into_iter().filter_map(|v| if let VolumeKind::ZoneRooms { zone, rooms } = v.kind { Some((zone, rooms)) } else { None }).collect()
}

/// Exit areas: boxes listed by a `ZLIST` named `...ExitAreas!00_ExitList`, boxes carrying the `escape` action label,
/// and plain boxes whose node path says so (`Exits!ExitBox`, `...!ExitArea`), never the in-game map legend icons.
pub fn exit_areas(g: &SceneGraph) -> Vec<VolumeDef> {
    let idx = PathIndex::new(g);
    let mut listed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for n in g.nodes.iter().filter(|n| n.type_id == TYPE_ZLIST && n.name.to_ascii_lowercase().contains("exitlist")) {
        for s in zlist_items(n) {
            if let Some(i) = idx.resolve(&s) {
                listed.insert(i);
            }
        }
    }
    let mut out: Vec<VolumeDef> = volumes(g)
        .into_iter()
        .filter(|v| match &v.kind {
            VolumeKind::Action { label, .. } => listed.contains(&v.node) || label.to_ascii_lowercase().contains("/escape"),
            _ => false,
        })
        .collect();
    let have: std::collections::HashSet<usize> = out.iter().map(|v| v.node).collect();
    for n in g.nodes.iter().filter(|n| n.type_id == TYPE_BOX && !have.contains(&n.index)) {
        let lname = n.name.to_ascii_lowercase();
        let path = g.path(n.index).to_ascii_lowercase();
        let named = lname.contains("exitbox") || lname.contains("exitarea") || lname.starts_with("exits!") || lname.contains("!exits!");
        if !named || path.contains("alllevels/") || path.contains("mapsetup") || path.contains("maplegend") {
            continue;
        }
        if let (Some(x), Some(y), Some(z)) = (f32_of(n.props.get(6)), f32_of(n.props.get(7)), f32_of(n.props.get(8))) {
            out.push(VolumeDef { node: n.index, size: [x, y, z], kind: VolumeKind::Action { label: String::new(), token: "AT_EXITBOX".into(), event: String::new(), target: String::new() } });
        }
    }
    out
}

/// `eClimbBox` / `ObstacleGuide` nodes (`ZHM3HitmanMovementGuide`): four corners in node space.
#[derive(Clone, Debug, PartialEq)]
pub struct ClimbBox {
    pub node: usize,
    pub kind: String,
    pub corners: Vec<[f32; 3]>,
}

pub fn climb_boxes(g: &SceneGraph) -> Vec<ClimbBox> {
    let mut out = Vec::new();
    for n in g.nodes.iter().filter(|n| n.type_id == TYPE_CLIMB_GUIDE) {
        let kind = n.props.iter().find_map(|v| token(Some(v)).filter(|t| t.starts_with('e')).map(str::to_string)).unwrap_or_default();
        // props[1] is the rotation array (9 floats), props[2] the position (3): the corner array is the 12 float one
        let corners_src = n.props.iter().filter_map(|v| if let Value::Array(a) = v { Some(a) } else { None }).find(|a| a.len() == 12);
        let corners = corners_src.map(|a| a.chunks(3).map(|c| [f32_of(c.first()).unwrap_or(0.0), f32_of(c.get(1)).unwrap_or(0.0), f32_of(c.get(2)).unwrap_or(0.0)]).collect()).unwrap_or_default();
        out.push(ClimbBox { node: n.index, kind, corners });
    }
    out
}

// ---------------------------------------------------------------------------------------------- pickups

/// An `Action` / `AT_PICKUP` node: a world item that can be picked up.
#[derive(Clone, Debug, PartialEq)]
pub struct PickupDef {
    pub node: usize,
    pub node_type: u32,
    /// Item definition node reference (props[5], e.g. `...Weapons/CloseCombat/CC_Hammer_01`); its last path segment
    /// is the item id.
    pub item: String,
    /// Count / ammo amount (last integer property: 1 for weapons, 25 for ammo boxes).
    pub amount: i32,
    /// Pickup range in cm of the action (controller field 5, 200).
    pub range: i32,
}

impl PickupDef {
    pub fn item_id(&self) -> &str {
        self.item.rsplit(['\\', '/']).next().unwrap_or(&self.item)
    }
}

pub fn pickups(g: &SceneGraph) -> Vec<PickupDef> {
    let mut out = Vec::new();
    for n in &g.nodes {
        let Some(c) = controller(n, "Action") else { continue };
        if token(c.props.get(2)) != Some("AT_PICKUP") {
            continue;
        }
        let Some(item) = n.props.iter().find_map(|v| str_of(Some(v)).filter(|s| s.starts_with("ROOT")).map(str::to_string)) else { continue };
        let amount = n.props.iter().rev().find_map(|v| v.as_i32()).unwrap_or(1);
        out.push(PickupDef { node: n.index, node_type: n.type_id, item, amount, range: c.props.get(5).and_then(|v| v.as_i32()).unwrap_or(200) });
    }
    out
}

/// A `Usepoint*` node (`ZHM3UsePoint`): actors stop here to perform a scripted action (`ScriptC` module in `module`).
#[derive(Clone, Debug, PartialEq)]
pub struct UsePointDef {
    pub node: usize,
    pub module: Option<String>,
}

pub fn use_points(g: &SceneGraph) -> Vec<UsePointDef> {
    g.nodes
        .iter()
        .filter(|n| n.type_id == TYPE_USEPOINT)
        .map(|n| UsePointDef { node: n.index, module: controller(n, "ScriptC").and_then(|c| str_of(c.props.first()).map(str::to_string)) })
        .collect()
}

// ---------------------------------------------------------------------------------------------- objectives

/// Objective kind letter of the LOC `MissionObjectives/<n>/Type` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectiveType {
    /// `T`: kill a target.
    Target,
    /// `E`: escape the level.
    Escape,
    /// `R`: retrieve an item.
    Retrieve,
    /// `P`: protect / smuggle a person.
    Protect,
    /// `O`: optional bonus objective.
    Optional,
    Other(char),
}

impl ObjectiveType {
    pub fn from_letter(c: char) -> ObjectiveType {
        match c.to_ascii_uppercase() {
            'T' => ObjectiveType::Target,
            'E' => ObjectiveType::Escape,
            'R' => ObjectiveType::Retrieve,
            'P' => ObjectiveType::Protect,
            'O' => ObjectiveType::Optional,
            o => ObjectiveType::Other(o),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectiveEntry {
    /// Key below `MissionObjectives/` (`1`, `2`, `RehabTarget1` ...).
    pub key: String,
    pub kind: ObjectiveType,
    /// Short text ("Kill Don Fernando Delgado").
    pub text: String,
    pub detail: String,
    /// Who or what the objective is about ("Don F. Delgado").
    pub who: String,
}

/// Reads `<level>/MissionObjectives/*` of a scene's LOC (`level` is `M01`, `hideout`, ...). Entries are returned in
/// numeric order for numeric keys, then the named ones alphabetically.
pub fn mission_objectives(loc: &Loc, level: &str) -> Vec<ObjectiveEntry> {
    let prefix = format!("{level}/MissionObjectives/");
    let mut keys: Vec<String> = Vec::new();
    for (path, _) in loc.leaves() {
        if let Some(rest) = path.strip_prefix(&prefix) {
            if let Some((key, _)) = rest.split_once('/') {
                if key != "Hints" && !keys.iter().any(|k| k == key) {
                    keys.push(key.to_string());
                }
            }
        }
    }
    keys.sort_by(|a, b| match (a.parse::<u32>(), b.parse::<u32>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        _ => a.cmp(b),
    });
    keys.into_iter()
        .filter_map(|key| {
            let get = |f: &str| loc.get(&format!("{prefix}{key}/{f}"));
            let text = get("Objective")?;
            let kind = get("Type").and_then(|t| t.chars().next()).map(ObjectiveType::from_letter).unwrap_or(ObjectiveType::Other('?'));
            Some(ObjectiveEntry { detail: get("ObjectiveDetail").unwrap_or_default(), who: get("WhoOrWhat").unwrap_or_default(), key, kind, text })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_references() {
        assert_eq!(normalize_ref("ROOT\\M01/M01_CHARACTERS.zip\\Controllers!Levelcontroller"), "m01/m01_characters.zip/controllers!levelcontroller");
        assert_eq!(normalize_ref("Items!Cello"), "items!cello");
    }

    #[test]
    fn objective_letters() {
        assert_eq!(ObjectiveType::from_letter('T'), ObjectiveType::Target);
        assert_eq!(ObjectiveType::from_letter('e'), ObjectiveType::Escape);
        assert_eq!(ObjectiveType::from_letter('x'), ObjectiveType::Other('X'));
    }
}

/// A disguise definition (`ZHM3HmAs`, the nodes under `Clothbundles!` and the player's `HitmanTemplate`).
///
/// Property layout (identified on all shipped levels): `[11]` dress quality token (`EDressQuality`: `eQualityBad` 0, `Good` 1,
/// `Super` 2, `Perfect` 3, `Hitman` 4), `[12]` string array of the security zones the outfit is safe in (`ESecurityZone`
/// names without the `_PATCH` suffix), `[16]` display name key (`/ClothNames/M01_TVCrew`). The child `Ground` link
/// (`ZLNKOBJ`) carries the model id in property `[4]` and the outfit value in `[6]` that the actors wearing it use.
#[derive(Clone, Debug, PartialEq)]
pub struct DisguiseDef {
    pub node: usize,
    pub name: String,
    /// `eQualityHitman` ... `eQualityBad`.
    pub quality: String,
    /// Zone names, e.g. `eZone1`, `eZone2A`.
    pub zones: Vec<String>,
    /// Model id and outfit value of the `Ground` link (the actors that wear this outfit).
    pub prim: Option<i32>,
    pub outfit: Option<i32>,
    pub cloth_name: String,
}

pub fn disguises(g: &SceneGraph) -> Vec<DisguiseDef> {
    let mut out = Vec::new();
    for n in g.nodes.iter().filter(|n| n.type_id == TYPE_HMAS) {
        let quality = match n.props.get(11) {
            Some(Value::Token(t)) => t.clone(),
            _ => String::new(),
        };
        let zones = match n.props.get(12) {
            Some(Value::StrArray(a)) => a.clone(),
            _ => Vec::new(),
        };
        let cloth_name = match n.props.get(16) {
            Some(Value::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let link = n.children.iter().map(|&c| &g.nodes[c]).find(|c| c.type_id == TYPE_LINK);
        let prim = link.and_then(|l| l.props.get(4)).and_then(|v| v.as_i32());
        let outfit = link.and_then(|l| l.props.get(6)).and_then(|v| v.as_i32());
        out.push(DisguiseDef { node: n.index, name: n.name.clone(), quality, zones, prim, outfit, cloth_name });
    }
    out
}
