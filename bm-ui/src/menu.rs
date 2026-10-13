//! Typed menu tree built from the Glacier `menu/*.xml` files (see `docs/ui.md` for the format).
//!
//! The XML is a small declarative language:
//!
//! * `<Template Name=".." attrs.. >` - attribute bundle (and optionally child elements) that other
//!   elements pull in with `Template=".."`; templates can reference templates.
//! * `<Window Name=".." Template="Window">` - a page; children are `<GuiElement>`s.
//! * `<GuiElement Type="TextBlock|OpenWindow|CloseWindow|Slider|HM3ToggleOption|CheckBox|Graphic|GUIList|..">`
//! * `<Navigation Navigation="Vertical|Horizontal">` - groups elements into a focus group (legends, buttons).
//! * `<Platform PC="True" XBOX="True" ..>` and `<MainIniOption Name=".." Value="..">` - conditional blocks
//!   evaluated at load time against a [`MenuContext`].
//! * `<ColorSetDefinition>`, `<ButtonGraphicDefinition>` - style tables.
//!
//! Everything is resolved at load time: templates are merged into the elements that use them, conditional
//! blocks are flattened, element types are mapped to [`ItemKind`] / [`Action`]. All original attributes are
//! kept in [`Item::attrs`] so nothing is lost for game logic that needs the raw data.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::{Align, Color};
use crate::xml::{self, Element};

/// Platform selector used by `<Platform PC="True">` blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    Pc,
    Xbox,
    Xenon,
    Ps2,
}

impl Platform {
    fn attr(self) -> &'static str {
        match self {
            Platform::Pc => "PC",
            Platform::Xbox => "XBOX",
            Platform::Xenon => "XENON",
            Platform::Ps2 => "PS2",
        }
    }
}

/// Load-time conditions: target platform and `HitmanBloodMoney.ini` style options for `<MainIniOption>`.
#[derive(Clone, Debug)]
pub struct MenuContext {
    pub platform: Platform,
    /// Option name -> value; a missing option compares as `"0"`.
    pub main_ini: HashMap<String, String>,
}

impl Default for MenuContext {
    fn default() -> Self {
        MenuContext { platform: Platform::Pc, main_ini: HashMap::new() }
    }
}

impl MenuContext {
    fn option_matches(&self, name: &str, value: &str) -> bool {
        let have = self
            .main_ini
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .unwrap_or("0");
        have.trim().eq_ignore_ascii_case(value.trim())
    }
}

/// Ordered attribute list with case-insensitive lookup.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Attrs(pub Vec<(String, String)>);

impl Attrs {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
    pub fn f32(&self, k: &str) -> Option<f32> {
        self.get(k)?.trim().parse().ok()
    }
    pub fn bool(&self, k: &str) -> Option<bool> {
        xml::parse_bool(self.get(k)?)
    }
    pub fn set(&mut self, k: &str, v: &str) {
        if let Some(e) = self.0.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(k)) {
            e.1 = v.to_string();
        } else {
            self.0.push((k.to_string(), v.to_string()));
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
    fn from_element(e: &Element, skip: &[&str]) -> Attrs {
        Attrs(e.attrs.iter().filter(|(k, _)| !skip.iter().any(|s| s.eq_ignore_ascii_case(k))).cloned().collect())
    }
    /// `self` overlaid with `over` (values in `over` win).
    fn overlay(mut self, over: &Attrs) -> Attrs {
        for (k, v) in &over.0 {
            self.set(k, v);
        }
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Navigation {
    #[default]
    Vertical,
    Horizontal,
}

impl Navigation {
    fn parse(s: &str) -> Navigation {
        if s.eq_ignore_ascii_case("horizontal") { Navigation::Horizontal } else { Navigation::Vertical }
    }
}

/// What activating (Select) an item does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Push `window` on the page stack (`reset_stack`: clear the stack first).
    OpenWindow { window: String, reset_stack: bool },
    /// Pop the current page (`save_options`: commit option edits, `cancel`: discard them).
    CloseWindow { cancel: bool, save_options: bool },
    /// `LoadScene` family: switch to another scene (`replay`: the `LoadSceneReplay` variant).
    LoadScene { scene: String, replay: bool },
    /// `QuitLevel` / quit game requests.
    Quit { kind: String },
    /// `SetDifficultyLevel`: choose a difficulty then open `window`.
    SetDifficulty { level: i32, window: Option<String> },
    /// Everything else (`SaveGame`, `StartMission`, `BuyUpgrade`, ..): game logic dispatches on `kind`;
    /// the raw attributes are in [`Item::attrs`].
    Custom { kind: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ItemKind {
    /// Static text (`TextBlock`, `ZTextBlockGeneric`, ...).
    Text,
    /// Page title (`Header`).
    Header,
    /// Focusable button with an action.
    Button(Action),
    /// `Slider`: bound option, bounds, step and track size in pixels.
    Slider { option: String, lower: f32, upper: f32, step: f32, size: f32 },
    /// `HM3ToggleOption` / `ToggleTextSimple`: cycles through `Item::choices`.
    Toggle { option: String },
    /// `CheckBox`.
    CheckBox { option: String },
    /// A named graphic (`Graphic`).
    Graphic { name: String },
    /// `GUIList` (scrolling list; rows come from game logic, `Item::children` is the row template).
    List { interface: String },
    /// `<Navigation>` group.
    Group(Navigation),
    /// Unmodelled element type, kept verbatim.
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub kind: ItemKind,
    /// Unresolved element type string from the XML (empty for groups).
    pub type_name: String,
    pub name: Option<String>,
    /// LOC key (relative, e.g. `Frontend/StartGame`) or a literal when it starts with `%`.
    pub text: Option<String>,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub width: Option<f32>,
    pub alignment: Align,
    pub font: Option<String>,
    pub color_set: Option<String>,
    /// Takes part in focus navigation (buttons, sliders, toggles, checkboxes; `Link="False"` opts out).
    pub focusable: bool,
    /// Initial focus (`TakeFocus` attribute, default true for focusable items).
    pub take_focus: bool,
    /// Button name that activates the item directly (`ActivateButton="ZW_SELECT4"`).
    pub activate_button: Option<String>,
    /// Item reacts to input even when not focused (`ReceiveAllInput`).
    pub receive_all_input: bool,
    /// Toggle choices (`<ToggleTextItem Text=..>`) as LOC keys.
    pub choices: Vec<String>,
    /// Group members / list row template.
    pub children: Vec<Item>,
    /// Row height of a `GUIList` (`<ItemLine SizeY>`).
    pub line_height: Option<f32>,
    /// Template-resolved raw attributes.
    pub attrs: Attrs,
}

impl Item {
    pub fn action(&self) -> Option<&Action> {
        if let ItemKind::Button(a) = &self.kind { Some(a) } else { None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub name: String,
    pub x: f32,
    pub y: f32,
    /// Design size (PC template: 1024x768).
    pub w: f32,
    pub h: f32,
    /// Extra pixels between auto-stacked lines.
    pub line_space: f32,
    pub align_h: Align,
    pub align_v: Align,
    pub align_mode: Option<String>,
    /// Name of a background graphic (`HitmanBackground`, `TRCDialog`, ...).
    pub background: Option<String>,
    pub frame: bool,
    pub dialog: bool,
    pub navigation: Navigation,
    pub pop_on_back: bool,
    pub roll_back_mark: bool,
    pub items: Vec<Item>,
    pub attrs: Attrs,
}

impl Window {
    /// All items in document order, descending into groups.
    pub fn walk(&self) -> Vec<&Item> {
        fn rec<'a>(items: &'a [Item], out: &mut Vec<&'a Item>) {
            for i in items {
                out.push(i);
                if matches!(i.kind, ItemKind::Group(_)) {
                    rec(&i.children, out);
                }
            }
        }
        let mut v = Vec::new();
        rec(&self.items, &mut v);
        v
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorSet {
    pub name: String,
    pub normal: Color,
    pub focus: Color,
    pub disable: Color,
}

impl Default for ColorSet {
    fn default() -> Self {
        ColorSet {
            name: "Default".into(),
            normal: Color([0xec, 0xec, 0xec, 0xff]),
            focus: Color::WHITE,
            disable: Color([0x77, 0x77, 0x77, 0xff]),
        }
    }
}

/// One `<GraphicElement>` of a button graphic: a named picture shown in a button state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphicElement {
    pub name: String,
    /// `Focus`, `Normal`, `Disabled` (from the `<Type Value>` children).
    pub states: Vec<String>,
    pub x: f32,
    pub y: f32,
    pub priority: i32,
    pub animate_alpha: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ButtonGraphic {
    pub name: String,
    pub graphics_only: bool,
    pub text_offset: [f32; 2],
    pub text_alignment: Option<Align>,
    pub height: Option<f32>,
    pub elements: Vec<GraphicElement>,
    pub attrs: Attrs,
}

#[derive(Clone, Debug, Default)]
struct TemplateDef {
    attrs: Attrs,
    children: Vec<Element>,
}

#[derive(Clone, Debug, Default)]
pub struct MenuDoc {
    pub source: String,
    pub color_sets: HashMap<String, ColorSet>,
    pub button_graphics: HashMap<String, ButtonGraphic>,
    pub windows: Vec<Window>,
    window_index: HashMap<String, usize>,
    templates: HashMap<String, TemplateDef>,
    pub warnings: Vec<String>,
}

impl MenuDoc {
    pub fn parse_str(source: &str, name: &str, ctx: &MenuContext) -> MenuDoc {
        let doc = xml::parse(source);
        let mut m = MenuDoc { source: name.to_string(), ..Default::default() };
        m.warnings.extend(doc.warnings.iter().map(|w| format!("{name}: {w}")));
        for top in &doc.root.children {
            // normally a single <Windows> element
            m.collect_defs(&top.children, ctx);
        }
        for top in &doc.root.children {
            m.collect_windows(&top.children, ctx);
        }
        m
    }

    pub fn parse_bytes(bytes: &[u8], name: &str, ctx: &MenuContext) -> MenuDoc {
        MenuDoc::parse_str(&xml::decode_text(bytes), name, ctx)
    }

    pub fn window(&self, name: &str) -> Option<&Window> {
        self.window_index.get(&name.to_ascii_lowercase()).map(|&i| &self.windows[i])
    }

    pub fn color_set(&self, name: &str) -> Option<&ColorSet> {
        self.color_sets.get(&name.to_ascii_lowercase())
    }

    /// Merges the windows/styles of `other` (later documents override same-named entries) - used to overlay a
    /// mod's partial menu file on the shipped one.
    pub fn merge(&mut self, other: MenuDoc) {
        for (k, v) in other.color_sets {
            self.color_sets.insert(k, v);
        }
        for (k, v) in other.button_graphics {
            self.button_graphics.insert(k, v);
        }
        for (k, v) in other.templates {
            self.templates.insert(k, v);
        }
        for w in other.windows {
            let key = w.name.to_ascii_lowercase();
            if let Some(&i) = self.window_index.get(&key) {
                self.windows[i] = w;
            } else {
                self.window_index.insert(key, self.windows.len());
                self.windows.push(w);
            }
        }
        self.warnings.extend(other.warnings);
    }

    fn platform_matches(e: &Element, ctx: &MenuContext) -> bool {
        e.attr_bool(ctx.platform.attr()).unwrap_or(false)
    }

    /// Flattens conditional blocks: yields the elements that are active in `ctx`.
    fn active<'a>(children: &'a [Element], ctx: &MenuContext, out: &mut Vec<&'a Element>) {
        for c in children {
            if c.is("Platform") {
                if Self::platform_matches(c, ctx) {
                    Self::active(&c.children, ctx, out);
                }
            } else if c.is("MainIniOption") {
                let name = c.attr("Name").unwrap_or("");
                let value = c.attr("Value").unwrap_or("1");
                if ctx.option_matches(name, value) {
                    Self::active(&c.children, ctx, out);
                }
            } else {
                out.push(c);
            }
        }
    }

    fn collect_defs(&mut self, children: &[Element], ctx: &MenuContext) {
        let mut act = Vec::new();
        Self::active(children, ctx, &mut act);
        for c in act {
            if c.is("ColorSetDefinition") {
                let name = c.attr("Name").unwrap_or("").to_string();
                let mut cs = ColorSet { name: name.clone(), ..ColorSet::default() };
                if let Some(v) = c.attr("NormalColor").and_then(Color::parse) {
                    cs.normal = v;
                }
                if let Some(v) = c.attr("FocusColor").and_then(Color::parse) {
                    cs.focus = v;
                }
                if let Some(v) = c.attr("DisableColor").and_then(Color::parse) {
                    cs.disable = v;
                }
                self.color_sets.insert(name.to_ascii_lowercase(), cs);
            } else if c.is("ButtonGraphicDefinition") {
                let bg = self.parse_button_graphic(c, ctx);
                self.button_graphics.insert(bg.name.to_ascii_lowercase(), bg);
            } else if c.is("Template") {
                if let Some(n) = c.attr("Name") {
                    let def = TemplateDef { attrs: Attrs::from_element(c, &["Name"]), children: c.children.clone() };
                    self.templates.insert(n.to_ascii_lowercase(), def);
                }
            }
        }
    }

    fn parse_button_graphic(&self, c: &Element, ctx: &MenuContext) -> ButtonGraphic {
        let attrs = self.resolve_attrs(&Attrs::from_element(c, &["Name"]), 0);
        let mut elements = Vec::new();
        let mut act = Vec::new();
        Self::active(&c.children, ctx, &mut act);
        for g in act.into_iter().filter(|g| g.is("GraphicElement")) {
            let a = self.resolve_attrs(&Attrs::from_element(g, &[]), 0);
            let states = g.children_named("Type").filter_map(|t| t.attr("Value")).map(str::to_string).collect();
            elements.push(GraphicElement {
                name: a.get("Name").unwrap_or("").to_string(),
                states,
                x: a.f32("X").unwrap_or(0.0),
                y: a.f32("Y").unwrap_or(0.0),
                priority: a.f32("Priority").unwrap_or(0.0) as i32,
                animate_alpha: a.bool("AnimateAlpha").unwrap_or(false),
            });
        }
        ButtonGraphic {
            name: c.attr("Name").unwrap_or("").to_string(),
            graphics_only: attrs.bool("GraphicsOnly").unwrap_or(false),
            text_offset: [attrs.f32("TextOffsetX").unwrap_or(0.0), attrs.f32("TextOffsetY").unwrap_or(0.0)],
            text_alignment: attrs.get("TextAlignment").and_then(Align::parse),
            height: attrs.f32("H"),
            elements,
            attrs,
        }
    }

    fn collect_windows(&mut self, children: &[Element], ctx: &MenuContext) {
        let mut act = Vec::new();
        Self::active(children, ctx, &mut act);
        for c in act {
            if c.is("Window") {
                let w = self.parse_window(c, ctx);
                let key = w.name.to_ascii_lowercase();
                if let Some(&i) = self.window_index.get(&key) {
                    self.windows[i] = w; // later definition wins (platform variants)
                } else {
                    self.window_index.insert(key, self.windows.len());
                    self.windows.push(w);
                }
            }
        }
    }

    /// Template chain resolution: the template's attributes, overlaid by `own`.
    fn resolve_attrs(&self, own: &Attrs, depth: u32) -> Attrs {
        let mut base = Attrs::default();
        if depth < 16 {
            if let Some(t) = own.get("Template") {
                if let Some(def) = self.templates.get(&t.to_ascii_lowercase()) {
                    base = self.resolve_attrs(&def.attrs, depth + 1);
                }
            }
        }
        let mut own_wo = own.clone();
        own_wo.0.retain(|(k, _)| !k.eq_ignore_ascii_case("Template"));
        base.overlay(&own_wo)
    }

    fn parse_window(&mut self, c: &Element, ctx: &MenuContext) -> Window {
        let name = c.attr("Name").unwrap_or("").to_string();
        let a = self.resolve_attrs(&Attrs::from_element(c, &["Name"]), 0);
        let items = self.parse_items(&c.children, ctx, 0);
        Window {
            name,
            x: a.f32("X").unwrap_or(0.0),
            y: a.f32("Y").unwrap_or(0.0),
            w: a.f32("W").unwrap_or(0.0),
            h: a.f32("H").unwrap_or(0.0),
            line_space: a.f32("LineSpace").unwrap_or(0.0),
            align_h: a.get("AlignH").and_then(Align::parse).unwrap_or(Align::Left),
            align_v: a.get("AlignV").and_then(Align::parse).unwrap_or(Align::Left),
            align_mode: a.get("AlignMode").map(str::to_string),
            background: a.get("Background").map(str::to_string),
            frame: a.bool("Frame").unwrap_or(false),
            dialog: a.bool("Dialog").unwrap_or(false),
            navigation: a.get("Navigation").map(Navigation::parse).unwrap_or_default(),
            pop_on_back: a.bool("PopOnBack").unwrap_or(true),
            roll_back_mark: a.bool("RollBackMark").unwrap_or(false),
            items,
            attrs: a,
        }
    }

    fn parse_items(&mut self, children: &[Element], ctx: &MenuContext, depth: u32) -> Vec<Item> {
        let mut act = Vec::new();
        Self::active(children, ctx, &mut act);
        let mut out = Vec::new();
        for c in act {
            if c.is("Navigation") {
                let nav = Navigation::parse(c.attr("Navigation").unwrap_or("Vertical"));
                let kids = self.parse_items(&c.children, ctx, depth + 1);
                out.push(group_item(nav, kids));
            } else if c.is("GuiElement") {
                let own = Attrs::from_element(c, &[]);
                // Pure template reference to a group template (Legend, Header, Top, ...): expand its children.
                if own.get("Type").is_none() && c.children.is_empty() {
                    if let Some(t) = own.get("Template") {
                        if let Some(def) = self.templates.get(&t.to_ascii_lowercase()).cloned() {
                            let resolved = self.resolve_attrs(&def.attrs, 1);
                            if !def.children.is_empty() && resolved.get("Type").is_none() && depth < 12 {
                                out.extend(self.parse_items(&def.children, ctx, depth + 1));
                                continue;
                            }
                        }
                    }
                }
                let attrs = self.resolve_attrs(&own, 0);
                let mut item = self.item_from(&attrs, c, ctx, depth);
                // inline children of a template-less GuiElement (toggle items, ItemLine) handled in item_from
                if item.type_name.is_empty() && own.get("Template").is_none() && c.children.is_empty() {
                    self.warnings.push(format!("{}: GuiElement without Type at line {}", self.source, c.line));
                }
                item.attrs = attrs;
                out.push(item);
            }
        }
        out
    }

    fn item_from(&mut self, a: &Attrs, c: &Element, ctx: &MenuContext, depth: u32) -> Item {
        let ty = a.get("Type").unwrap_or("").to_string();
        let tl = ty.to_ascii_lowercase();
        let s = |k: &str| a.get(k).unwrap_or("").to_string();
        let mut children = Vec::new();
        let mut line_height = None;
        let mut choices = Vec::new();
        for ch in &c.children {
            if ch.is("ToggleTextItem") {
                if let Some(t) = ch.attr("Text") {
                    choices.push(t.to_string());
                }
            } else if ch.is("ItemLine") {
                line_height = ch.attr_f32("SizeY");
                children = self.parse_items(&ch.children, ctx, depth + 1);
            }
        }
        let kind = match tl.as_str() {
            "textblock" | "ztextblockgeneric" | "generaltextblock" | "zposttextblock" | "zpostmissiontextblock"
            | "textcredits" | "profiletextblock" | "limitsavegametextblock" | "savegamecounttextblock"
            | "savegametextblock" | "maxprofiletextblock" | "savegamesizetextblock" | "showvar" => ItemKind::Text,
            "header" => ItemKind::Header,
            "openwindow" | "openlicensewindow" | "openwindowtimer" | "opendebugmainmenu" | "pressstart" => {
                ItemKind::Button(Action::OpenWindow { window: s("Window"), reset_stack: a.bool("ResetStack").unwrap_or(false) })
            }
            "closewindow" | "closewindowoptions" => ItemKind::Button(Action::CloseWindow {
                cancel: a.bool("Cancel").unwrap_or(false),
                save_options: a.bool("SaveOptions").unwrap_or(tl == "closewindowoptions"),
            }),
            "loadscene" | "loadscenereplay" | "loadsceneuncomplete" | "levelcompletedbeforeloadnextscene" => {
                ItemKind::Button(Action::LoadScene {
                    scene: a.get("Scene").or(a.get("MSL_Scene")).unwrap_or("").to_string(),
                    replay: tl == "loadscenereplay",
                })
            }
            "quitlevel" | "quitgame" => ItemKind::Button(Action::Quit { kind: ty.clone() }),
            "setdifficultylevel" => ItemKind::Button(Action::SetDifficulty {
                level: a.f32("DifficultyLevel").unwrap_or(0.0) as i32,
                window: a.get("Window").map(str::to_string),
            }),
            "slider" => ItemKind::Slider {
                option: s("Option"),
                lower: a.f32("LowerBound").unwrap_or(0.0),
                upper: a.f32("UpperBound").unwrap_or(100.0),
                step: a.f32("Step").unwrap_or(1.0),
                size: a.f32("SliderSize").unwrap_or(100.0),
            },
            "hm3toggleoption" | "toggletextsimple" => ItemKind::Toggle { option: s("Option") },
            "checkbox" => ItemKind::CheckBox { option: s("Option") },
            "graphic" => ItemKind::Graphic { name: a.get("Name").or(a.get("Graphic")).unwrap_or("").to_string() },
            "guilist" => ItemKind::List { interface: s("Interface") },
            "" => ItemKind::Other(String::new()),
            // game-logic buttons: SaveGame, LoadGame, StartMission, BuyUpgrade, ...
            _ if a.get("Text").is_some() || a.get("ActivateButton").is_some() => {
                ItemKind::Button(Action::Custom { kind: ty.clone() })
            }
            _ => ItemKind::Other(ty.clone()),
        };
        let interactive = matches!(
            kind,
            ItemKind::Button(_) | ItemKind::Slider { .. } | ItemKind::Toggle { .. } | ItemKind::CheckBox { .. } | ItemKind::List { .. }
        );
        let link = a.bool("Link").unwrap_or(true);
        Item {
            kind,
            type_name: ty,
            name: a.get("Name").map(str::to_string),
            text: a.get("Text").map(str::to_string),
            x: a.f32("X"),
            y: a.f32("Y"),
            width: a.f32("Width").or_else(|| a.f32("W")),
            alignment: a.get("Alignment").and_then(Align::parse).unwrap_or(Align::Left),
            font: a.get("Font").map(str::to_string),
            color_set: a.get("ColorSet").map(str::to_string),
            focusable: interactive && link,
            take_focus: a.bool("TakeFocus").unwrap_or(true),
            activate_button: a.get("ActivateButton").map(str::to_string),
            receive_all_input: a.bool("ReceiveAllInput").unwrap_or(false),
            choices,
            children,
            line_height,
            attrs: Attrs::default(),
        }
    }
}

fn group_item(nav: Navigation, children: Vec<Item>) -> Item {
    Item {
        kind: ItemKind::Group(nav),
        type_name: String::new(),
        name: None,
        text: None,
        x: None,
        y: None,
        width: None,
        alignment: Align::Left,
        font: None,
        color_set: None,
        focusable: false,
        take_focus: false,
        activate_button: None,
        receive_all_input: false,
        choices: Vec::new(),
        children,
        line_height: None,
        attrs: Attrs::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
<Windows>
  <ColorSetDefinition Name="Grey" NormalColor="0xffececec" FocusColor="0xaaaaaaaa" />
  <Platform PC="True">
    <Template Name="Window" X="0" Y="0" W="1024" H="768" AlignV="Center" AlignH="Center" LineSpace="11" />
    <Template Name="LeftPosX" X="376" Alignment="Right" />
    <Template Name="BottomLeftPos" Y="629" Template="LeftPosX" />
    <Template Name="Legend">
      <GuiElement Type="CloseWindow" Text="Buttons/Back" Font="Menu" Template="BottomLeftPos" />
    </Template>
  </Platform>
  <Platform XBOX="True">
    <Template Name="Window" W="640" H="448" />
  </Platform>
  <Window Name="MainMenu" Template="Window" PopOnBack="False">
    <GuiElement Type="OpenWindow" Text="Frontend/StartGame" Window="Difficulty" Template="LeftPosX" />
    <MainIniOption Name="DebugMenus" Value="1">
      <GuiElement Type="OpenWindow" Text="Debug" Window="Dbg" Template="LeftPosX" />
    </MainIniOption>
    <GuiElement Type="Slider" Option="Vol" Text="Options/Vol" LowerBound="0" UpperBound="100" Step="5" />
    <GuiElement Type="HM3ToggleOption" Option="UseEAX" Text="Options/EAX"><ToggleTextItem Text="Options/No" /><ToggleTextItem Text="Options/Yes" /></GuiElement>
    <Platform XBOX="True"><GuiElement Type="TextBlock" Text="Xbox only" /></Platform>
    <GuiElement Template="Legend" />
  </Window>
</Windows>"#;

    #[test]
    fn templates_platforms_and_kinds() {
        let doc = MenuDoc::parse_str(SAMPLE, "t", &MenuContext::default());
        let w = doc.window("mainmenu").unwrap();
        assert_eq!((w.w, w.h), (1024.0, 768.0));
        assert!(!w.pop_on_back);
        assert_eq!(w.align_h, Align::Center);
        // StartGame, Slider, Toggle, Legend(CloseWindow); no debug item, no xbox text
        assert_eq!(w.items.len(), 4);
        let b = &w.items[0];
        assert_eq!(b.x, Some(376.0));
        assert_eq!(b.alignment, Align::Right);
        assert_eq!(b.action(), Some(&Action::OpenWindow { window: "Difficulty".into(), reset_stack: false }));
        assert!(b.focusable);
        assert!(matches!(w.items[1].kind, ItemKind::Slider { upper, step, .. } if upper == 100.0 && step == 5.0));
        assert_eq!(w.items[2].choices, vec!["Options/No", "Options/Yes"]);
        // legend: nested template chain BottomLeftPos -> LeftPosX
        let l = &w.items[3];
        assert_eq!((l.x, l.y, l.alignment), (Some(376.0), Some(629.0), Align::Right));
        assert_eq!(doc.color_set("grey").unwrap().focus, Color([0xaa, 0xaa, 0xaa, 0xaa]));
    }

    #[test]
    fn main_ini_option_and_platform_switch() {
        let mut ctx = MenuContext::default();
        ctx.main_ini.insert("DebugMenus".into(), "1".into());
        let doc = MenuDoc::parse_str(SAMPLE, "t", &ctx);
        assert_eq!(doc.window("MainMenu").unwrap().items.len(), 5);
        let ctx = MenuContext { platform: Platform::Xbox, ..Default::default() };
        let doc = MenuDoc::parse_str(SAMPLE, "t", &ctx);
        let w = doc.window("MainMenu").unwrap();
        assert_eq!(w.w, 640.0);
        assert!(w.items.iter().any(|i| i.text.as_deref() == Some("Xbox only")));
    }

    #[test]
    fn merge_overrides_windows() {
        let mut a = MenuDoc::parse_str(SAMPLE, "a", &MenuContext::default());
        let b = MenuDoc::parse_str(
            r#"<Windows><Window Name="MainMenu" W="10" H="10"><GuiElement Type="TextBlock" Text="X"/></Window></Windows>"#,
            "b",
            &MenuContext::default(),
        );
        a.merge(b);
        assert_eq!(a.window("mainmenu").unwrap().items.len(), 1);
        assert_eq!(a.windows.len(), 1);
    }
}
