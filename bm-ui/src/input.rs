//! Input abstraction: game/menu *actions*, rebindable bindings (keyboard / mouse / gamepad), per-frame action
//! state and a user config file.
//!
//! The action names are the engine's own key-configuration ids (they are the keys of
//! `AllLevels/Interface/KeyConfiguration/*` in the LOC: `MoveUp`, `FireWeapon`, `ActionMenuOpen`, `ZW_SELECT`, ...),
//! so the original "Button Configuration" list can be rebuilt from `Action::ALL` + `Action::loc_key()`.
//!
//! Bindings are stored as plain strings so the module is independent of the window library: keys use the
//! `winit::keyboard::KeyCode` variant names (`KeyW`, `ShiftLeft`, `ArrowUp`, `Digit1`, `F5`, ...). The app translates
//! its native events into [`InputEvent`]s.
//!
//! Default bindings are *our own* sensible PC defaults - the shipped build uses baked-in bindings
//! (`HM3GPAD.cfg` is not part of the install), so the original defaults are not available as data.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

macro_rules! actions {
    ($($(#[$m:meta])* $name:ident),+ $(,)?) => {
        /// A logical input. In-game actions first, then menu navigation (`ZW_*`).
        #[allow(non_camel_case_types)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum Action { $($(#[$m])* $name),+ }
        impl Action {
            pub const ALL: &'static [Action] = &[$(Action::$name),+];
            pub fn name(self) -> &'static str { match self { $(Action::$name => stringify!($name)),+ } }
            pub fn from_name(s: &str) -> Option<Action> {
                $(if s.eq_ignore_ascii_case(stringify!($name)) { return Some(Action::$name); })+
                None
            }
        }
    };
}

actions! {
    MoveUp, MoveDown, MoveLeft, MoveRight,
    Run, CrouchSneak,
    FireWeapon, FireWeapon2,
    ReloadWeapon, UseAction, ActionMenuOpen, ActionMenuFast,
    PickupMenuOpen, PickupFast, PickupSPC, InventoryMenuOpen, DropItem, DropVideo, Holster,
    CyclePistols, CycleSmgs, CycleAssaultRifles, CycleSnipers, CycleCCs, CycleBinoculars, CycleCoins,
    ToggleCamera, CameraZoomIn, CameraZoomOut,
    Map, Briefing, IngameMenu,
    /// Menu navigation / confirmation (engine ids `ZW_*`).
    ZW_UP, ZW_DOWN, ZW_LEFT, ZW_RIGHT, ZW_SELECT, ZW_SELECT2, ZW_SELECT4, ZW_CANCEL, Accept, Enter,
}

impl Action {
    /// LOC key (relative to `AllLevels/Interface/`) of the action's display name.
    pub fn loc_key(self) -> String {
        format!("KeyConfiguration/{}", self.name())
    }

    pub fn is_menu(self) -> bool {
        matches!(
            self,
            Action::ZW_UP | Action::ZW_DOWN | Action::ZW_LEFT | Action::ZW_RIGHT | Action::ZW_SELECT | Action::ZW_SELECT2 | Action::ZW_SELECT4 | Action::ZW_CANCEL | Action::Accept | Action::Enter
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PadButton {
    South,
    East,
    West,
    North,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PadAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
    LeftTrigger,
    RightTrigger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Direction {
    Positive,
    Negative,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Binding {
    /// `winit::keyboard::KeyCode` variant name.
    Key(String),
    Mouse(MouseButton),
    WheelUp,
    WheelDown,
    Pad(PadButton),
    /// Stick/trigger direction past the dead zone.
    PadAxis(PadAxis, Direction),
}

impl Binding {
    pub fn key(name: &str) -> Binding {
        Binding::Key(name.to_string())
    }

    /// Short human readable label (`W`, `Left Shift`, `Mouse 1`, `Pad A`).
    pub fn label(&self) -> String {
        match self {
            Binding::Key(k) => {
                let k = k.as_str();
                if let Some(c) = k.strip_prefix("Key").filter(|c| c.len() == 1) {
                    return c.to_string();
                }
                if let Some(d) = k.strip_prefix("Digit") {
                    return d.to_string();
                }
                if let Some(d) = k.strip_prefix("Numpad") {
                    return format!("Num {d}");
                }
                match k {
                    "ShiftLeft" => "Left Shift".into(),
                    "ShiftRight" => "Right Shift".into(),
                    "ControlLeft" => "Left Ctrl".into(),
                    "ControlRight" => "Right Ctrl".into(),
                    "AltLeft" => "Left Alt".into(),
                    "AltRight" => "Right Alt".into(),
                    "ArrowUp" => "Up".into(),
                    "ArrowDown" => "Down".into(),
                    "ArrowLeft" => "Left".into(),
                    "ArrowRight" => "Right".into(),
                    "Escape" => "Esc".into(),
                    other => other.to_string(),
                }
            }
            Binding::Mouse(MouseButton::Left) => "Mouse 1".into(),
            Binding::Mouse(MouseButton::Right) => "Mouse 2".into(),
            Binding::Mouse(MouseButton::Middle) => "Mouse 3".into(),
            Binding::Mouse(MouseButton::Back) => "Mouse 4".into(),
            Binding::Mouse(MouseButton::Forward) => "Mouse 5".into(),
            Binding::WheelUp => "Wheel Up".into(),
            Binding::WheelDown => "Wheel Down".into(),
            Binding::Pad(b) => format!("Pad {b:?}"),
            Binding::PadAxis(a, d) => format!("Pad {a:?}{}", if *d == Direction::Positive { "+" } else { "-" }),
        }
    }

    pub fn is_pad(&self) -> bool {
        matches!(self, Binding::Pad(_) | Binding::PadAxis(..))
    }
}

/// Native input event as seen by the application layer.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    Key { code: String, pressed: bool },
    Mouse { button: MouseButton, pressed: bool },
    /// Positive = wheel up.
    Wheel(f32),
    Pad { button: PadButton, pressed: bool },
    /// Axis position in -1..1.
    PadAxis { axis: PadAxis, value: f32 },
    /// Relative mouse motion (camera look); does not drive actions.
    MouseMotion { dx: f32, dy: f32 },
    /// Absolute cursor position in window pixels.
    CursorMoved { x: f32, y: f32 },
}

/// A binding collision reported by [`InputMap::rebind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub binding: Binding,
    pub action: Action,
}

pub const SLOTS: usize = 3;

/// Action -> up to [`SLOTS`] bindings. Serialises to RON (default user file) or JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputMap {
    /// Format version of the user file.
    #[serde(default = "one")]
    pub version: u32,
    pub bindings: BTreeMap<Action, Vec<Binding>>,
    #[serde(default = "default_dead_zone")]
    pub dead_zone: f32,
    #[serde(default = "default_sens")]
    pub mouse_sensitivity: f32,
    #[serde(default)]
    pub invert_vertical: bool,
}

fn one() -> u32 {
    1
}
fn default_dead_zone() -> f32 {
    0.25
}
fn default_sens() -> f32 {
    0.4
}

impl Default for InputMap {
    fn default() -> Self {
        InputMap::defaults()
    }
}

impl InputMap {
    pub fn empty() -> InputMap {
        InputMap { version: 1, bindings: BTreeMap::new(), dead_zone: 0.25, mouse_sensitivity: 0.4, invert_vertical: false }
    }

    /// Built-in PC + gamepad defaults.
    pub fn defaults() -> InputMap {
        use Action::*;
        let k = Binding::key;
        let mut m = InputMap::empty();
        let mut set = |a: Action, b: Vec<Binding>| {
            m.bindings.insert(a, b);
        };
        set(MoveUp, vec![k("KeyW"), k("ArrowUp"), Binding::PadAxis(PadAxis::LeftY, Direction::Negative)]);
        set(MoveDown, vec![k("KeyS"), k("ArrowDown"), Binding::PadAxis(PadAxis::LeftY, Direction::Positive)]);
        set(MoveLeft, vec![k("KeyA"), k("ArrowLeft"), Binding::PadAxis(PadAxis::LeftX, Direction::Negative)]);
        set(MoveRight, vec![k("KeyD"), k("ArrowRight"), Binding::PadAxis(PadAxis::LeftX, Direction::Positive)]);
        set(Run, vec![k("ShiftLeft"), Binding::Pad(PadButton::LeftStick)]);
        set(CrouchSneak, vec![k("ControlLeft"), Binding::Pad(PadButton::RightStick)]);
        set(FireWeapon, vec![Binding::Mouse(MouseButton::Left), Binding::PadAxis(PadAxis::RightTrigger, Direction::Positive)]);
        set(FireWeapon2, vec![Binding::Mouse(MouseButton::Right), Binding::PadAxis(PadAxis::LeftTrigger, Direction::Positive)]);
        set(ReloadWeapon, vec![k("KeyR"), Binding::Pad(PadButton::West)]);
        set(UseAction, vec![k("KeyE"), Binding::Pad(PadButton::South)]);
        set(ActionMenuOpen, vec![k("KeyQ"), Binding::Pad(PadButton::North)]);
        set(ActionMenuFast, vec![Binding::Mouse(MouseButton::Middle)]);
        set(PickupMenuOpen, vec![k("KeyF"), Binding::Pad(PadButton::East)]);
        set(PickupFast, vec![k("KeyG")]);
        set(PickupSPC, vec![k("KeyV")]);
        set(InventoryMenuOpen, vec![k("Tab"), Binding::Pad(PadButton::Select)]);
        set(DropItem, vec![k("KeyX")]);
        set(DropVideo, vec![k("KeyZ")]);
        set(Holster, vec![k("KeyH"), Binding::Pad(PadButton::RightShoulder)]);
        set(CyclePistols, vec![k("Digit1")]);
        set(CycleSmgs, vec![k("Digit2")]);
        set(CycleAssaultRifles, vec![k("Digit3")]);
        set(CycleSnipers, vec![k("Digit4")]);
        set(CycleCCs, vec![k("Digit5")]);
        set(CycleBinoculars, vec![k("Digit6")]);
        set(CycleCoins, vec![k("Digit7")]);
        set(ToggleCamera, vec![k("KeyC")]);
        set(CameraZoomIn, vec![Binding::WheelUp]);
        set(CameraZoomOut, vec![Binding::WheelDown]);
        set(Map, vec![k("KeyM")]);
        set(Briefing, vec![k("KeyB")]);
        set(IngameMenu, vec![k("Escape"), Binding::Pad(PadButton::Start)]);
        set(ZW_UP, vec![k("ArrowUp"), k("KeyW"), Binding::Pad(PadButton::DPadUp)]);
        set(ZW_DOWN, vec![k("ArrowDown"), k("KeyS"), Binding::Pad(PadButton::DPadDown)]);
        set(ZW_LEFT, vec![k("ArrowLeft"), k("KeyA"), Binding::Pad(PadButton::DPadLeft)]);
        set(ZW_RIGHT, vec![k("ArrowRight"), k("KeyD"), Binding::Pad(PadButton::DPadRight)]);
        set(ZW_SELECT, vec![k("Enter"), k("NumpadEnter"), Binding::Pad(PadButton::South)]);
        set(ZW_SELECT2, vec![k("KeyE"), Binding::Pad(PadButton::West)]);
        set(ZW_SELECT4, vec![k("Space"), Binding::Pad(PadButton::North)]);
        set(ZW_CANCEL, vec![k("Escape"), k("Backspace"), Binding::Pad(PadButton::East)]);
        set(Accept, vec![k("Enter")]);
        set(Enter, vec![k("Enter")]);
        m
    }

    pub fn get(&self, a: Action) -> &[Binding] {
        self.bindings.get(&a).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Actions currently using `b` (several are fine across contexts, e.g. Escape: menu back + in-game menu).
    pub fn actions_for(&self, b: &Binding) -> Vec<Action> {
        self.bindings.iter().filter(|(_, v)| v.contains(b)).map(|(a, _)| *a).collect()
    }

    /// Sets `slot` of `action` to `b`. Returns bindings of *other actions of the same context* (menu vs game) that
    /// now collide so the UI can warn; the binding is applied regardless (`resolve_conflicts` removes them).
    pub fn rebind(&mut self, action: Action, slot: usize, b: Binding) -> Vec<Conflict> {
        let conflicts: Vec<Conflict> = self
            .actions_for(&b)
            .into_iter()
            .filter(|a| *a != action && a.is_menu() == action.is_menu())
            .map(|a| Conflict { binding: b.clone(), action: a })
            .collect();
        let v = self.bindings.entry(action).or_default();
        if slot < v.len() {
            v[slot] = b;
        } else {
            v.push(b);
            v.truncate(SLOTS);
        }
        conflicts
    }

    /// Removes `b` from every action in `conflicts`.
    pub fn resolve_conflicts(&mut self, conflicts: &[Conflict]) {
        for c in conflicts {
            if let Some(v) = self.bindings.get_mut(&c.action) {
                v.retain(|x| *x != c.binding);
            }
        }
    }

    pub fn clear_slot(&mut self, action: Action, slot: usize) {
        if let Some(v) = self.bindings.get_mut(&action) {
            if slot < v.len() {
                v.remove(slot);
            }
        }
    }

    pub fn reset_action(&mut self, action: Action) {
        let d = InputMap::defaults();
        self.bindings.insert(action, d.get(action).to_vec());
    }

    /// First keyboard/mouse label for an action (HUD prompts, `{ZW_SELECT}` tokens). Falls back to the pad label.
    pub fn label(&self, a: Action) -> String {
        let v = self.get(a);
        v.iter().find(|b| !b.is_pad()).or_else(|| v.first()).map(Binding::label).unwrap_or_else(|| "-".into())
    }

    /// Fills in actions missing from a (older / hand-edited) file with the defaults.
    pub fn fill_missing(&mut self) {
        let d = InputMap::defaults();
        for a in Action::ALL {
            self.bindings.entry(*a).or_insert_with(|| d.get(*a).to_vec());
        }
    }

    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).unwrap_or_default()
    }

    pub fn from_ron(s: &str) -> Result<InputMap, String> {
        let mut m: InputMap = ron::from_str(s).map_err(|e| e.to_string())?;
        m.fill_missing();
        Ok(m)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<InputMap, String> {
        let mut m: InputMap = serde_json::from_str(s).map_err(|e| e.to_string())?;
        m.fill_missing();
        Ok(m)
    }

    /// Loads a `.ron` or `.json` file by extension.
    pub fn load(path: &Path) -> Result<InputMap, String> {
        let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) { InputMap::from_json(&s) } else { InputMap::from_ron(&s) }
    }

    /// Loads the file, or returns the defaults when it is missing or unreadable (logged).
    pub fn load_or_default(path: &Path) -> InputMap {
        match InputMap::load(path) {
            Ok(m) => m,
            Err(e) => {
                if path.exists() {
                    log::warn!("input config {} unreadable ({e}); using defaults", path.display());
                }
                InputMap::defaults()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let text = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) { self.to_json() } else { self.to_ron() };
        // write-then-rename so a crash never leaves a truncated config
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

/// Default location of the user's binding file: `%APPDATA%/bm-rs/input.ron` (Windows),
/// `$XDG_CONFIG_HOME|~/.config/bm-rs/input.ron` elsewhere.
pub fn user_config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("bm-rs").join("input.ron")
}

/// Per-frame action state fed by [`InputEvent`]s.
#[derive(Clone, Debug, Default)]
pub struct InputState {
    held: HashSet<Binding>,
    pressed_edge: HashSet<Binding>,
    released_edge: HashSet<Binding>,
    axes: HashMap<PadAxis, f32>,
    pub cursor: [f32; 2],
    pub mouse_delta: [f32; 2],
    capture: Option<(Action, usize)>,
    captured: Option<(Action, usize, Binding)>,
}

impl InputState {
    pub fn new() -> InputState {
        InputState::default()
    }

    /// Clears edge flags and the mouse delta; call once per frame *before* feeding the frame's events.
    pub fn begin_frame(&mut self) {
        self.pressed_edge.clear();
        self.released_edge.clear();
        self.mouse_delta = [0.0, 0.0];
    }

    fn set(&mut self, b: Binding, down: bool) {
        if down {
            if self.held.insert(b.clone()) {
                self.pressed_edge.insert(b);
            }
        } else if self.held.remove(&b) {
            self.released_edge.insert(b);
        }
    }

    pub fn event(&mut self, e: &InputEvent, map: &InputMap) {
        match e {
            InputEvent::Key { code, pressed } => self.set(Binding::Key(code.clone()), *pressed),
            InputEvent::Mouse { button, pressed } => self.set(Binding::Mouse(*button), *pressed),
            InputEvent::Wheel(v) => {
                if *v > 0.0 {
                    self.pressed_edge.insert(Binding::WheelUp);
                } else if *v < 0.0 {
                    self.pressed_edge.insert(Binding::WheelDown);
                }
            }
            InputEvent::Pad { button, pressed } => self.set(Binding::Pad(*button), *pressed),
            InputEvent::PadAxis { axis, value } => {
                self.axes.insert(*axis, *value);
                for dir in [Direction::Positive, Direction::Negative] {
                    let on = match dir {
                        Direction::Positive => *value > map.dead_zone,
                        Direction::Negative => *value < -map.dead_zone,
                    };
                    self.set(Binding::PadAxis(*axis, dir), on);
                }
            }
            InputEvent::MouseMotion { dx, dy } => {
                self.mouse_delta[0] += dx;
                self.mouse_delta[1] += dy;
            }
            InputEvent::CursorMoved { x, y } => self.cursor = [*x, *y],
        }
        // rebinding capture: the next press becomes the binding
        if let Some((a, slot)) = self.capture {
            let b = match e {
                InputEvent::Key { code, pressed: true } => Some(Binding::Key(code.clone())),
                InputEvent::Mouse { button, pressed: true } => Some(Binding::Mouse(*button)),
                InputEvent::Wheel(v) if *v > 0.0 => Some(Binding::WheelUp),
                InputEvent::Wheel(v) if *v < 0.0 => Some(Binding::WheelDown),
                InputEvent::Pad { button, pressed: true } => Some(Binding::Pad(*button)),
                InputEvent::PadAxis { axis, value } if value.abs() > 0.6 => {
                    Some(Binding::PadAxis(*axis, if *value > 0.0 { Direction::Positive } else { Direction::Negative }))
                }
                _ => None,
            };
            if let Some(b) = b {
                self.capture = None;
                self.captured = Some((a, slot, b));
            }
        }
    }

    fn any(&self, set: &HashSet<Binding>, map: &InputMap, a: Action) -> bool {
        map.get(a).iter().any(|b| set.contains(b))
    }

    /// Raw position of a gamepad axis (-1..1, 0 when never seen).
    pub fn axis(&self, a: PadAxis) -> f32 {
        self.axes.get(&a).copied().unwrap_or(0.0)
    }

    /// Action is held.
    pub fn down(&self, map: &InputMap, a: Action) -> bool {
        self.any(&self.held, map, a)
    }

    /// Action went down this frame.
    pub fn pressed(&self, map: &InputMap, a: Action) -> bool {
        self.any(&self.pressed_edge, map, a)
    }

    pub fn released(&self, map: &InputMap, a: Action) -> bool {
        self.any(&self.released_edge, map, a)
    }

    /// Analogue value of an action in 0..1 (axis bindings give their magnitude, others 0/1).
    pub fn value(&self, map: &InputMap, a: Action) -> f32 {
        let mut v = 0.0f32;
        for b in map.get(a) {
            match b {
                Binding::PadAxis(ax, d) => {
                    let x = self.axes.get(ax).copied().unwrap_or(0.0);
                    let m = if *d == Direction::Positive { x } else { -x };
                    if m > map.dead_zone {
                        v = v.max(((m - map.dead_zone) / (1.0 - map.dead_zone)).clamp(0.0, 1.0));
                    }
                }
                other => {
                    if self.held.contains(other) {
                        v = 1.0;
                    }
                }
            }
        }
        v
    }

    /// Movement vector (x right, y forward) from the four move actions, clamped to the unit circle.
    pub fn move_vector(&self, map: &InputMap) -> [f32; 2] {
        let x = self.value(map, Action::MoveRight) - self.value(map, Action::MoveLeft);
        let y = self.value(map, Action::MoveUp) - self.value(map, Action::MoveDown);
        let l = (x * x + y * y).sqrt();
        if l > 1.0 { [x / l, y / l] } else { [x, y] }
    }

    /// Starts listening for the next press to bind to `(action, slot)`.
    pub fn start_capture(&mut self, action: Action, slot: usize) {
        self.capture = Some((action, slot));
        self.captured = None;
    }

    pub fn cancel_capture(&mut self) {
        self.capture = None;
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Takes the result of a finished capture and applies it to `map`; returns the conflicts to show.
    pub fn finish_capture(&mut self, map: &mut InputMap) -> Option<(Action, Vec<Conflict>)> {
        let (a, slot, b) = self.captured.take()?;
        let c = map.rebind(a, slot, b);
        Some((a, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: &str, pressed: bool) -> InputEvent {
        InputEvent::Key { code: code.into(), pressed }
    }

    #[test]
    fn defaults_cover_every_action_and_names_round_trip() {
        let m = InputMap::defaults();
        for a in Action::ALL {
            assert!(!m.get(*a).is_empty(), "{a:?} has no default binding");
            assert_eq!(Action::from_name(a.name()), Some(*a));
        }
        assert_eq!(Action::FireWeapon.loc_key(), "KeyConfiguration/FireWeapon");
    }

    #[test]
    fn ron_and_json_round_trip_with_rebinding() {
        let mut m = InputMap::defaults();
        let c = m.rebind(Action::Run, 0, Binding::key("KeyQ"));
        // KeyQ is ActionMenuOpen's default: a game-context conflict
        assert_eq!(c, vec![Conflict { binding: Binding::key("KeyQ"), action: Action::ActionMenuOpen }]);
        m.resolve_conflicts(&c);
        assert!(!m.get(Action::ActionMenuOpen).contains(&Binding::key("KeyQ")));
        m.mouse_sensitivity = 0.9;
        m.invert_vertical = true;
        let back = InputMap::from_ron(&m.to_ron()).unwrap();
        assert_eq!(back, m);
        let back = InputMap::from_json(&m.to_json()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn partial_file_gets_defaults_filled() {
        let m = InputMap::from_ron("(bindings: {Run: [Key(\"KeyT\")]})").unwrap();
        assert_eq!(m.get(Action::Run), &[Binding::key("KeyT")]);
        assert_eq!(m.get(Action::MoveUp), InputMap::defaults().get(Action::MoveUp));
        assert_eq!(m.dead_zone, 0.25);
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("bm-ui-input-{}", std::process::id()));
        let path = dir.join("input.ron");
        let mut m = InputMap::defaults();
        m.rebind(Action::Map, 0, Binding::key("KeyN"));
        m.save(&path).unwrap();
        assert_eq!(InputMap::load_or_default(&path), m);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(InputMap::load_or_default(&dir.join("missing.ron")), InputMap::defaults());
    }

    #[test]
    fn state_edges_and_axes() {
        let map = InputMap::defaults();
        let mut st = InputState::new();
        st.begin_frame();
        st.event(&key("KeyW", true), &map);
        assert!(st.pressed(&map, Action::MoveUp) && st.down(&map, Action::MoveUp));
        st.begin_frame();
        assert!(!st.pressed(&map, Action::MoveUp) && st.down(&map, Action::MoveUp));
        st.event(&key("KeyW", false), &map);
        assert!(st.released(&map, Action::MoveUp) && !st.down(&map, Action::MoveUp));
        st.event(&InputEvent::PadAxis { axis: PadAxis::LeftX, value: 0.9 }, &map);
        assert!(st.down(&map, Action::MoveRight));
        assert!((st.value(&map, Action::MoveRight) - (0.9 - 0.25) / 0.75).abs() < 1e-5);
        st.event(&InputEvent::PadAxis { axis: PadAxis::LeftX, value: 0.1 }, &map);
        assert!(!st.down(&map, Action::MoveRight));
        let mv = {
            st.event(&key("KeyW", true), &map);
            st.event(&key("KeyD", true), &map);
            st.move_vector(&map)
        };
        assert!((mv[0] * mv[0] + mv[1] * mv[1] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn capture_binds_next_press() {
        let mut map = InputMap::defaults();
        let mut st = InputState::new();
        st.start_capture(Action::Holster, 1);
        st.event(&InputEvent::MouseMotion { dx: 1.0, dy: 1.0 }, &map);
        assert!(st.is_capturing());
        st.event(&key("KeyP", true), &map);
        assert!(!st.is_capturing());
        let (a, conflicts) = st.finish_capture(&mut map).unwrap();
        assert_eq!(a, Action::Holster);
        assert!(conflicts.is_empty());
        assert_eq!(map.get(Action::Holster)[1], Binding::key("KeyP"));
        assert_eq!(map.label(Action::Holster), "H");
    }
}
