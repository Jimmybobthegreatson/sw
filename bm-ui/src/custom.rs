//! Mod-defined screens: RON / JSON layouts for new UI pages (`UI/Screens/<name>.ron|json`).
//!
//! ```ron
//! (
//!   name: "mod_menu",
//!   canvas: (1280.0, 720.0),
//!   background: "#000000cc",
//!   elements: [
//!     (id: "title", anchor: Top, offset: (0.0, 80.0), size: (600.0, 40.0),
//!      kind: Text(text: "%MY MOD", font: "Header", align: Center)),
//!     (id: "play", anchor: Center, offset: (0.0, 0.0), size: (300.0, 36.0),
//!      kind: Button(text: "%Play", action: Event("mod_play"))),
//!     (id: "back", anchor: Center, offset: (0.0, 50.0), size: (300.0, 36.0),
//!      kind: Button(text: "Buttons/Back", action: Back)),
//!   ],
//! )
//! ```
//!
//! `Event(name)` is handed back to the game layer; `Open(screen)` / `Back` navigate the screen stack of a
//! [`ScreenStack`]. Text goes through the normal localisation + markup path (LOC key, `%literal`, `{var}` from the
//! screen's variables).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::context::UiContext;
use crate::draw::{Blend, DrawList};
use crate::hud::HudAnchor;
use crate::menu_view::Nav;
use crate::text::TextOptions;
use crate::types::{Align, Color, Rect};

fn menu_font() -> String {
    "Menu".into()
}
fn white() -> Color {
    Color::WHITE
}
fn canvas_default() -> [f32; 2] {
    [1280.0, 720.0]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ScreenAction {
    /// Push another screen.
    Open(String),
    /// Pop this screen.
    Back,
    /// Opaque event for the game layer (`mod_play`).
    Event(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ElementKind {
    Panel {
        color: Color,
        #[serde(default)]
        border: Option<(Color, f32)>,
    },
    Text {
        text: String,
        #[serde(default = "menu_font")]
        font: String,
        #[serde(default = "white")]
        color: Color,
        #[serde(default)]
        align: Align,
        #[serde(default)]
        wrap: bool,
    },
    /// A sprite registered in the UI context (`sprite`) or a PNG from the VFS (`texture: "UI/Images/logo.png"`).
    Image {
        #[serde(default)]
        sprite: Option<String>,
        #[serde(default)]
        texture: Option<String>,
        #[serde(default = "white")]
        tint: Color,
        #[serde(default)]
        additive: bool,
    },
    Button {
        text: String,
        action: ScreenAction,
        #[serde(default = "menu_font")]
        font: String,
        #[serde(default = "white")]
        color: Color,
        #[serde(default)]
        align: Align,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenElement {
    pub id: String,
    #[serde(default)]
    pub anchor: HudAnchor,
    #[serde(default)]
    pub offset: [f32; 2],
    #[serde(default)]
    pub size: [f32; 2],
    pub kind: ElementKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiScreen {
    pub name: String,
    #[serde(default = "canvas_default")]
    pub canvas: [f32; 2],
    #[serde(default)]
    pub background: Option<Color>,
    pub elements: Vec<ScreenElement>,
}

impl UiScreen {
    /// Parses RON; `Option` fields may be written without `Some(..)`.
    pub fn from_ron(s: &str) -> Result<UiScreen, String> {
        crate::types::ron_options().from_str(s).map_err(|e| e.to_string())
    }
    pub fn from_json(s: &str) -> Result<UiScreen, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).unwrap_or_default()
    }

    /// Element rectangle on the canvas.
    pub fn rect_of(&self, e: &ScreenElement) -> Rect {
        let f = e.anchor.fraction();
        Rect::new(f[0] * self.canvas[0] + e.offset[0] - f[0] * e.size[0], f[1] * self.canvas[1] + e.offset[1] - f[1] * e.size[1], e.size[0], e.size[1])
    }

    /// Buttons in document order.
    pub fn buttons(&self) -> Vec<&ScreenElement> {
        self.elements.iter().filter(|e| matches!(e.kind, ElementKind::Button { .. })).collect()
    }
}

/// All screens loaded from `UI/Screens/`.
#[derive(Clone, Debug, Default)]
pub struct ScreenLibrary {
    pub screens: HashMap<String, UiScreen>,
    /// Source path of each screen (for hot reload).
    pub sources: HashMap<String, String>,
}

impl ScreenLibrary {
    pub fn get(&self, name: &str) -> Option<&UiScreen> {
        self.screens.get(&name.to_ascii_lowercase())
    }

    pub fn insert(&mut self, s: UiScreen) {
        self.screens.insert(s.name.to_ascii_lowercase(), s);
    }

    /// Loads every `UI/Screens/*.ron|json`. Returns the number loaded; broken files are logged and skipped.
    pub fn load(&mut self, vfs: &bm_vfs::Vfs) -> usize {
        let mut n = 0;
        for pat in ["UI/Screens/*.ron", "UI/Screens/*.json"] {
            for e in vfs.list(pat) {
                let Ok(bytes) = vfs.open(&e.path) else { continue };
                let text = crate::xml::decode_text(&bytes);
                let r = if e.path.to_ascii_lowercase().ends_with(".ron") { UiScreen::from_ron(&text) } else { UiScreen::from_json(&text) };
                match r {
                    Ok(s) => {
                        self.sources.insert(s.name.to_ascii_lowercase(), e.path.clone());
                        self.insert(s);
                        n += 1;
                    }
                    Err(err) => log::error!("{}: {err}", e.path),
                }
            }
        }
        n
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScreenEvent {
    None,
    Opened(String),
    Closed(String),
    /// `Event(name)` of a button.
    Event(String),
}

/// Navigation state over a [`ScreenLibrary`]: screen stack + focused button.
pub struct ScreenStack {
    pub stack: Vec<(String, usize)>,
}

impl ScreenStack {
    pub fn new(root: &str) -> ScreenStack {
        ScreenStack { stack: vec![(root.to_string(), 0)] }
    }

    pub fn current<'a>(&self, lib: &'a ScreenLibrary) -> Option<&'a UiScreen> {
        lib.get(&self.stack.last()?.0)
    }

    pub fn handle(&mut self, nav: &Nav, lib: &ScreenLibrary) -> ScreenEvent {
        let Some(screen) = self.current(lib) else { return ScreenEvent::None };
        let buttons = screen.buttons();
        let n = buttons.len();
        let focus = self.stack.last().map(|s| s.1).unwrap_or(0).min(n.saturating_sub(1));
        let set = |st: &mut ScreenStack, f: usize| {
            if let Some(l) = st.stack.last_mut() {
                l.1 = f;
            }
        };
        let activate = |st: &mut ScreenStack, i: usize| -> ScreenEvent {
            let Some(ElementKind::Button { action, .. }) = buttons.get(i).map(|b| &b.kind) else { return ScreenEvent::None };
            match action {
                ScreenAction::Open(s) => {
                    if lib.get(s).is_some() {
                        st.stack.push((s.clone(), 0));
                        ScreenEvent::Opened(s.clone())
                    } else {
                        log::warn!("screen action Open({s}): no such screen");
                        ScreenEvent::None
                    }
                }
                ScreenAction::Back => st.pop(),
                ScreenAction::Event(e) => ScreenEvent::Event(e.clone()),
            }
        };
        match nav {
            Nav::Up | Nav::Left if n > 0 => {
                set(self, (focus + n - 1) % n);
                ScreenEvent::None
            }
            Nav::Down | Nav::Right if n > 0 => {
                set(self, (focus + 1) % n);
                ScreenEvent::None
            }
            Nav::Select if n > 0 => activate(self, focus),
            Nav::Back => self.pop(),
            Nav::PointerMove(p) => {
                if let Some(i) = buttons.iter().position(|b| screen.rect_of(b).contains(*p)) {
                    set(self, i);
                }
                ScreenEvent::None
            }
            Nav::PointerClick(p) => match buttons.iter().position(|b| screen.rect_of(b).contains(*p)) {
                Some(i) => {
                    set(self, i);
                    activate(self, i)
                }
                None => ScreenEvent::None,
            },
            _ => ScreenEvent::None,
        }
    }

    fn pop(&mut self) -> ScreenEvent {
        if self.stack.len() > 1 {
            ScreenEvent::Closed(self.stack.pop().unwrap().0)
        } else {
            ScreenEvent::None
        }
    }

    /// Draws the current screen on its canvas. `vars` fills `{name}` placeholders.
    pub fn draw(&self, lib: &ScreenLibrary, ctx: &UiContext, dl: &mut DrawList, images: &HashMap<String, crate::texture::TextureId>, vars: &HashMap<String, String>) {
        let Some(screen) = self.current(lib) else { return };
        let focus = self.stack.last().map(|s| s.1).unwrap_or(0);
        if let Some(bg) = screen.background {
            dl.rect(Rect::new(0.0, 0.0, screen.canvas[0], screen.canvas[1]), bg);
        }
        let mut button_idx = 0usize;
        for e in &screen.elements {
            let r = screen.rect_of(e);
            let interp = |t: &str| {
                let mut s = ctx.strings.resolve(t);
                for (k, v) in vars {
                    s = s.replace(&format!("{{{k}}}"), v);
                }
                s
            };
            match &e.kind {
                ElementKind::Panel { color, border } => {
                    dl.rect(r, *color);
                    if let Some((c, t)) = border {
                        dl.border(r, *t, *c);
                    }
                }
                ElementKind::Text { text, font, color, align, wrap } => {
                    let mut o = TextOptions::new(font).color(*color).align(*align);
                    if *wrap || r.w > 0.0 && *align != Align::Left {
                        o.max_width = Some(r.w);
                    }
                    let lay = ctx.layout_text(&interp(text), &o);
                    dl.text_shadow(&lay, [r.x, r.y], Color::WHITE, [1.0, 1.0], Color([0, 0, 0, 200]));
                }
                ElementKind::Image { sprite, texture, tint, additive } => {
                    let blend = if *additive { Blend::Additive } else { Blend::Alpha };
                    if let Some(sp) = sprite.as_deref().and_then(|n| ctx.sprite(n)) {
                        sp.draw(dl, [r.x, r.y], if r.w > 0.0 { Some([r.w, r.h]) } else { None }, *tint);
                    } else if let Some(t) = texture.as_deref().and_then(|p| images.get(&p.to_ascii_lowercase())) {
                        dl.quad(*t, r, [0.0, 0.0, 1.0, 1.0], *tint, blend);
                    }
                }
                ElementKind::Button { text, font, color, align, .. } => {
                    let focused = button_idx == focus;
                    button_idx += 1;
                    let mut o = TextOptions::new(font).color(*color).align(*align);
                    o.max_width = Some(r.w);
                    let lay = ctx.layout_text(&interp(text), &o);
                    dl.rect(r, if focused { Color([255, 255, 255, 40]) } else { Color([0, 0, 0, 90]) });
                    dl.border(r, 1.0, if focused { Color([255, 255, 255, 220]) } else { Color([255, 255, 255, 70]) });
                    let ty = r.y + (r.h - lay.height) * 0.5;
                    dl.text_shadow(&lay, [r.x, ty], if focused { Color::WHITE } else { Color([210, 210, 210, 255]) }, [1.0, 1.0], Color([0, 0, 0, 200]));
                }
            }
        }
    }

    /// All `texture:` paths of the current screen (so the owner can load them).
    pub fn image_paths(&self, lib: &ScreenLibrary) -> Vec<String> {
        self.current(lib)
            .map(|s| {
                s.elements
                    .iter()
                    .filter_map(|e| match &e.kind {
                        ElementKind::Image { texture: Some(t), .. } => Some(t.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::test_font;

    const SCREEN: &str = r##"(
        name: "mod_menu",
        canvas: (1280.0, 720.0),
        background: Some("#000000cc"),
        elements: [
            (id: "title", anchor: Top, offset: (0.0, 80.0), size: (600.0, 40.0), kind: Text(text: "%MY MOD", font: "Header", align: Center)),
            (id: "play", anchor: Center, offset: (0.0, 0.0), size: (300.0, 36.0), kind: Button(text: "%Play", action: Event("mod_play"))),
            (id: "sub", anchor: Center, offset: (0.0, 50.0), size: (300.0, 36.0), kind: Button(text: "%Sub", action: Open("sub"))),
        ],
    )"##;
    const SUB: &str = r#"{"name":"sub","elements":[{"id":"b","anchor":"Center","size":[100,30],"kind":{"Button":{"text":"%Back","action":"Back"}}}]}"#;

    fn lib() -> ScreenLibrary {
        let mut l = ScreenLibrary::default();
        l.insert(UiScreen::from_ron(SCREEN).unwrap());
        l.insert(UiScreen::from_json(SUB).unwrap());
        l
    }

    #[test]
    fn ron_and_json_parse_and_round_trip() {
        let s = UiScreen::from_ron(SCREEN).unwrap();
        assert_eq!(s.elements.len(), 3);
        assert_eq!(s.background, Some(Color([0, 0, 0, 0xcc])));
        assert_eq!(UiScreen::from_ron(&s.to_ron()).unwrap(), s);
        let j = UiScreen::from_json(SUB).unwrap();
        assert_eq!(j.canvas, [1280.0, 720.0]);
        // centre anchored button of 100x30 sits around the middle
        assert_eq!(j.rect_of(&j.elements[0]), Rect::new(590.0, 345.0, 100.0, 30.0));
    }

    #[test]
    fn navigation_events_and_stack() {
        let l = lib();
        let mut st = ScreenStack::new("mod_menu");
        assert_eq!(st.handle(&Nav::Select, &l), ScreenEvent::Event("mod_play".into()));
        st.handle(&Nav::Down, &l);
        assert_eq!(st.handle(&Nav::Select, &l), ScreenEvent::Opened("sub".into()));
        assert_eq!(st.handle(&Nav::Select, &l), ScreenEvent::Closed("sub".into())); // Back action
        assert_eq!(st.handle(&Nav::Back, &l), ScreenEvent::None); // root
        let s = l.get("mod_menu").unwrap();
        let r = s.rect_of(&s.elements[2]);
        assert_eq!(st.handle(&Nav::PointerClick([r.x + 2.0, r.y + 2.0]), &l), ScreenEvent::Opened("sub".into()));
    }

    #[test]
    fn draws_and_interpolates() {
        let l = lib();
        let mut ctx = UiContext::new();
        for n in ["Menu", "Header"] {
            let mut f = test_font();
            f.name = n.into();
            ctx.fonts.insert(f);
        }
        let st = ScreenStack::new("mod_menu");
        let mut dl = DrawList::new();
        st.draw(&l, &ctx, &mut dl, &HashMap::new(), &HashMap::new());
        assert!(dl.quad_count() > 8);
    }
}
