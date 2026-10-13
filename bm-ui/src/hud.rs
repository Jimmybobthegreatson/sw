//! Data-driven HUD: indicators (health / ammo / notoriety / suspicion style bars), subtitles, on-screen prompts and
//! a compass, described by a [`HudLayout`] (RON or JSON) and driven by a [`HudState`] of named values.
//!
//! The original in-game HUD ("OSD") is a scene group `AllLevels/osd.zip` (`Compass`, `WeaponDisplay`, `InfoDisplay`,
//! `ActionMenu`, `PickupMenu`, `SubTitles`, ...) whose logic lives in native controllers, so the numbers
//! (health, ammo, notoriety) have to come from the game layer. Here every widget reads a *named variable* from
//! [`HudState`]; `bm-game` sets `health`, `ammo_clip`, `notoriety`, ... and the layout decides how they look.
//! The built-in layout is [`default_layout`]; a mod replaces it with `UI/hud.ron` or `UI/hud.json` in any mod dir.
//!
//! Visibility expressions (`visible`): `name`, `!name`, `name>0.5`, `name<=3`, `name==1`, `name!=0` (missing variables
//! read as 0).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::context::UiContext;
use crate::draw::{Blend, DrawList};
use crate::input::Action;
use crate::text::TextOptions;
use crate::types::{Align, Color, Rect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HudAnchor {
    #[default]
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl HudAnchor {
    pub fn fraction(self) -> [f32; 2] {
        match self {
            HudAnchor::TopLeft => [0.0, 0.0],
            HudAnchor::Top => [0.5, 0.0],
            HudAnchor::TopRight => [1.0, 0.0],
            HudAnchor::Left => [0.0, 0.5],
            HudAnchor::Center => [0.5, 0.5],
            HudAnchor::Right => [1.0, 0.5],
            HudAnchor::BottomLeft => [0.0, 1.0],
            HudAnchor::Bottom => [0.5, 1.0],
            HudAnchor::BottomRight => [1.0, 1.0],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarDir {
    #[default]
    LeftToRight,
    RightToLeft,
    BottomToTop,
    TopToBottom,
}

fn one() -> f32 {
    1.0
}
fn white() -> Color {
    Color::WHITE
}
fn font_menu() -> String {
    "Menu".into()
}
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WidgetKind {
    /// Filled rectangle with optional border.
    Panel {
        color: Color,
        #[serde(default)]
        border: Option<(Color, f32)>,
    },
    /// Value bar (`value` / `max` are variable names or numbers, `max` defaults to 1).
    Bar {
        value: String,
        #[serde(default)]
        max: Option<String>,
        #[serde(default)]
        dir: BarDir,
        fill: Color,
        #[serde(default)]
        back: Option<Color>,
        #[serde(default)]
        border: Option<(Color, f32)>,
        /// Number of visible segments (0 = continuous).
        #[serde(default)]
        segments: u32,
        /// Switch to `low_fill` and pulse when the fraction drops below this.
        #[serde(default)]
        low_threshold: Option<f32>,
        #[serde(default)]
        low_fill: Option<Color>,
        /// Optional label (LOC key or `%literal`) drawn above the bar, left aligned.
        #[serde(default)]
        label: Option<String>,
    },
    /// Text with `{var}` interpolation; `text` is markup (`%`-prefixed literals and LOC keys both work).
    Text {
        text: String,
        #[serde(default = "font_menu")]
        font: String,
        #[serde(default = "white")]
        color: Color,
        #[serde(default)]
        align: Align,
        #[serde(default = "yes")]
        shadow: bool,
        #[serde(default)]
        outline: bool,
        #[serde(default)]
        wrap: Option<f32>,
        #[serde(default = "one")]
        scale: f32,
    },
    /// Sprite from the UI context (`ctx.add_sprite`) tinted by `tint`; `alpha_var` modulates its alpha.
    Icon {
        sprite: String,
        #[serde(default = "white")]
        tint: Color,
        #[serde(default)]
        alpha_var: Option<String>,
    },
    /// Subtitle block fed by [`HudState::subtitles`].
    Subtitles {
        #[serde(default = "font_menu")]
        font: String,
        #[serde(default = "white")]
        color: Color,
        #[serde(default = "default_lines")]
        max_lines: usize,
    },
    /// Contextual prompts (`[E] Open door`) fed by [`HudState::prompts`], one per row.
    Prompts {
        #[serde(default = "font_menu")]
        font: String,
        #[serde(default = "white")]
        color: Color,
        #[serde(default)]
        align: Align,
    },
    /// Heading strip: `heading_var` in degrees (0 = north), `fov` degrees visible across the widget width.
    Compass {
        heading_var: String,
        #[serde(default = "default_fov")]
        fov: f32,
        #[serde(default = "white")]
        color: Color,
    },
}

fn default_lines() -> usize {
    2
}
fn default_fov() -> f32 {
    120.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HudWidget {
    pub id: String,
    #[serde(default)]
    pub anchor: HudAnchor,
    #[serde(default)]
    pub offset: [f32; 2],
    #[serde(default)]
    pub size: [f32; 2],
    #[serde(default)]
    pub visible: Option<String>,
    pub kind: WidgetKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HudLayout {
    #[serde(default)]
    pub name: String,
    /// The pixel height the offsets/sizes are authored for; everything scales with `screen_h / reference_height`.
    #[serde(default = "default_ref_h")]
    pub reference_height: f32,
    /// Safe-area margin in reference pixels.
    #[serde(default = "default_margin")]
    pub margin: f32,
    pub widgets: Vec<HudWidget>,
}

fn default_ref_h() -> f32 {
    720.0
}
fn default_margin() -> f32 {
    24.0
}

impl HudLayout {
    /// Parses RON; `Option` fields may be written without `Some(..)` (RON `implicit_some`).
    pub fn from_ron(s: &str) -> Result<HudLayout, String> {
        crate::types::ron_options().from_str(s).map_err(|e| e.to_string())
    }
    pub fn from_json(s: &str) -> Result<HudLayout, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).unwrap_or_default()
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
    pub fn widget(&self, id: &str) -> Option<&HudWidget> {
        self.widgets.iter().find(|w| w.id == id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subtitle {
    pub speaker: Option<String>,
    /// Text or LOC key (resolved by the HUD).
    pub text: String,
    pub start: f32,
    pub duration: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub action: Action,
    /// LOC key (relative to `AllLevels/Interface/` or `AllLevels/`) or `%literal`.
    pub text: String,
}

/// Runtime values the HUD displays.
#[derive(Clone, Debug, Default)]
pub struct HudState {
    pub time: f32,
    vars: HashMap<String, f32>,
    texts: HashMap<String, String>,
    pub subtitles: Vec<Subtitle>,
    pub prompts: Vec<Prompt>,
}

impl HudState {
    pub fn new() -> HudState {
        HudState::default()
    }

    pub fn set(&mut self, name: &str, v: f32) -> &mut Self {
        self.vars.insert(name.to_string(), v);
        self
    }

    pub fn get(&self, name: &str) -> f32 {
        self.vars.get(name).copied().unwrap_or(0.0)
    }

    pub fn set_text(&mut self, name: &str, v: impl Into<String>) -> &mut Self {
        self.texts.insert(name.to_string(), v.into());
        self
    }

    /// Queues a subtitle starting now.
    pub fn say(&mut self, speaker: Option<&str>, text: &str, duration: f32) {
        self.subtitles.push(Subtitle { speaker: speaker.map(str::to_string), text: text.to_string(), start: self.time, duration });
    }

    pub fn update(&mut self, dt: f32) {
        self.time += dt;
        let t = self.time;
        self.subtitles.retain(|s| t < s.start + s.duration);
    }

    /// Subtitles active right now, oldest first.
    pub fn active_subtitles(&self) -> impl Iterator<Item = &Subtitle> {
        let t = self.time;
        self.subtitles.iter().filter(move |s| t >= s.start && t < s.start + s.duration)
    }

    fn number(&self, tok: &str) -> f32 {
        tok.trim().parse().unwrap_or_else(|_| self.get(tok.trim()))
    }

    /// Evaluates a visibility expression (see module docs).
    pub fn eval(&self, expr: &str) -> bool {
        let e = expr.trim();
        if e.is_empty() {
            return true;
        }
        for op in ["==", "!=", ">=", "<=", ">", "<"] {
            if let Some((l, r)) = e.split_once(op) {
                let (a, b) = (self.number(l), self.number(r));
                return match op {
                    "==" => (a - b).abs() < 1e-6,
                    "!=" => (a - b).abs() >= 1e-6,
                    ">=" => a >= b,
                    "<=" => a <= b,
                    ">" => a > b,
                    _ => a < b,
                };
            }
        }
        if let Some(n) = e.strip_prefix('!') {
            return self.number(n) == 0.0 && !self.texts.contains_key(n.trim());
        }
        self.number(e) != 0.0 || self.texts.get(e).is_some_and(|t| !t.is_empty())
    }

    /// Replaces `{var}` with the variable (integers without decimals) or text; unknown names stay untouched
    /// (so `{ZW_SELECT}` button tokens survive for the text layout).
    pub fn interpolate(&self, template: &str) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(o) = rest.find('{') {
            out.push_str(&rest[..o]);
            let Some(c) = rest[o..].find('}') else { break };
            let name = &rest[o + 1..o + c];
            if let Some(t) = self.texts.get(name) {
                out.push_str(t);
            } else if let Some(v) = self.vars.get(name) {
                if v.fract().abs() < 1e-4 { out.push_str(&format!("{}", v.round() as i64)) } else { out.push_str(&format!("{v:.1}")) }
            } else {
                out.push_str(&rest[o..o + c + 1]);
            }
            rest = &rest[o + c + 1..];
        }
        out.push_str(rest);
        out
    }
}

fn frac(state: &HudState, value: &str, max: Option<&str>) -> f32 {
    let v = state.number(value);
    let m = max.map(|m| state.number(m)).unwrap_or(1.0);
    if m <= 0.0 { 0.0 } else { (v / m).clamp(0.0, 1.0) }
}

/// Draws a HUD.
pub struct Hud;

impl Hud {
    /// Draws all widgets of `layout` on a `screen` of the given pixel size.
    pub fn draw(layout: &HudLayout, state: &HudState, ctx: &UiContext, dl: &mut DrawList, screen: [f32; 2]) {
        let s = screen[1] / layout.reference_height.max(1.0);
        let m = layout.margin * s;
        let area = Rect::new(m, m, screen[0] - 2.0 * m, screen[1] - 2.0 * m);
        for w in &layout.widgets {
            if let Some(v) = &w.visible {
                if !state.eval(v) {
                    continue;
                }
            }
            let size = [w.size[0] * s, w.size[1] * s];
            let f = w.anchor.fraction();
            let px = area.x + f[0] * area.w + w.offset[0] * s - f[0] * size[0];
            let py = area.y + f[1] * area.h + w.offset[1] * s - f[1] * size[1];
            let r = Rect::new(px, py, size[0], size[1]);
            Self::draw_widget(&w.kind, r, s, state, ctx, dl);
        }
    }

    fn draw_widget(kind: &WidgetKind, r: Rect, s: f32, state: &HudState, ctx: &UiContext, dl: &mut DrawList) {
        match kind {
            WidgetKind::Panel { color, border } => {
                dl.rect(r, *color);
                if let Some((c, t)) = border {
                    dl.border(r, (t * s).max(1.0), *c);
                }
            }
            WidgetKind::Bar { value, max, dir, fill, back, border, segments, low_threshold, low_fill, label } => {
                let k = frac(state, value, max.as_deref());
                if let Some(b) = back {
                    dl.rect(r, *b);
                }
                let mut color = *fill;
                if let (Some(th), Some(lf)) = (low_threshold, low_fill) {
                    if k < *th {
                        let pulse = 0.65 + 0.35 * (state.time * 6.0).sin().abs();
                        color = lf.with_alpha(pulse);
                    }
                }
                let inner = Rect::new(r.x + s, r.y + s, (r.w - 2.0 * s).max(0.0), (r.h - 2.0 * s).max(0.0));
                let fill_rect = |frac: f32| match dir {
                    BarDir::LeftToRight => Rect::new(inner.x, inner.y, inner.w * frac, inner.h),
                    BarDir::RightToLeft => Rect::new(inner.right() - inner.w * frac, inner.y, inner.w * frac, inner.h),
                    BarDir::BottomToTop => Rect::new(inner.x, inner.bottom() - inner.h * frac, inner.w, inner.h * frac),
                    BarDir::TopToBottom => Rect::new(inner.x, inner.y, inner.w, inner.h * frac),
                };
                if *segments > 1 {
                    let n = *segments as f32;
                    let lit = (k * n).ceil() as u32;
                    let gap = 2.0 * s;
                    for i in 0..*segments {
                        let (a, b) = (i as f32 / n, (i + 1) as f32 / n);
                        let rr = match dir {
                            BarDir::LeftToRight | BarDir::RightToLeft => {
                                let sx = if matches!(dir, BarDir::LeftToRight) { inner.x + inner.w * a } else { inner.right() - inner.w * b };
                                Rect::new(sx, inner.y, (inner.w / n - gap).max(1.0), inner.h)
                            }
                            _ => {
                                let sy = if matches!(dir, BarDir::BottomToTop) { inner.bottom() - inner.h * b } else { inner.y + inner.h * a };
                                Rect::new(inner.x, sy, inner.w, (inner.h / n - gap).max(1.0))
                            }
                        };
                        let c = if i < lit { color } else { color.with_alpha(0.18) };
                        dl.rect(rr, c);
                    }
                } else {
                    dl.rect(fill_rect(k), color);
                }
                if let Some((c, t)) = border {
                    dl.border(r, (t * s).max(1.0), *c);
                }
                if let Some(l) = label {
                    let lay = ctx.layout_text(&ctx.strings.resolve(&state.interpolate(l)), &TextOptions::new("Menu").color(Color::WHITE));
                    dl.text_shadow(&lay, [r.x, r.y - lay.height - 3.0 * s], Color::WHITE, [1.0, 1.0], Color([0, 0, 0, 220]));
                }
            }
            WidgetKind::Text { text, font, color, align, shadow, outline, wrap, scale } => {
                let expanded = ctx.strings.resolve(&state.interpolate(text));
                let mut opts = TextOptions::new(font).color(*color).align(*align);
                opts.scale = scale * s;
                opts.max_width = wrap.map(|w| w * s).or(if r.w > 0.0 { Some(r.w) } else { None });
                let lay = ctx.layout_text(&expanded, &opts);
                let origin = [r.x, r.y];
                if *outline {
                    dl.text_outline(&lay, origin, Color::WHITE, Color([0, 0, 0, 255]), 1.0);
                } else if *shadow {
                    dl.text_shadow(&lay, origin, Color::WHITE, [s.max(1.0), s.max(1.0)], Color([0, 0, 0, 220]));
                } else {
                    dl.text(&lay, origin, Color::WHITE);
                }
            }
            WidgetKind::Icon { sprite, tint, alpha_var } => {
                if let Some(sp) = ctx.sprite(sprite) {
                    let a = alpha_var.as_deref().map(|v| state.get(v).clamp(0.0, 1.0)).unwrap_or(1.0);
                    let size = if r.w > 0.0 && r.h > 0.0 { Some([r.w, r.h]) } else { Some([sp.size[0] * s, sp.size[1] * s]) };
                    sp.draw(dl, [r.x, r.y], size, tint.with_alpha(a));
                }
            }
            WidgetKind::Subtitles { font, color, max_lines } => {
                let mut y = r.y;
                let active: Vec<&Subtitle> = state.active_subtitles().collect();
                let skip = active.len().saturating_sub(*max_lines);
                for sub in active.into_iter().skip(skip) {
                    let age = state.time - sub.start;
                    let remain = sub.start + sub.duration - state.time;
                    let a = (age / 0.15).min(1.0).min(remain / 0.25).clamp(0.0, 1.0);
                    let body = ctx.strings.resolve(&sub.text);
                    let text = match &sub.speaker {
                        Some(sp) => format!("{sp}: {body}"),
                        None => body,
                    };
                    let mut opts = TextOptions::new(font).color(*color).align(Align::Center);
                    opts.scale = s;
                    opts.max_width = Some(r.w);
                    let lay = ctx.layout_text(&text, &opts);
                    dl.text_outline(&lay, [r.x, y], Color::WHITE.with_alpha(a), Color([0, 0, 0, 255]), 1.0);
                    y += lay.height + 4.0 * s;
                }
            }
            WidgetKind::Prompts { font, color, align } => {
                let mut y = r.y;
                for p in &state.prompts {
                    let key = ctx.input_map.label(p.action);
                    let text = format!("[{key}] {}", ctx.strings.resolve(&p.text));
                    let mut opts = TextOptions::new(font).color(*color).align(*align);
                    opts.scale = s;
                    opts.max_width = if r.w > 0.0 { Some(r.w) } else { None };
                    let lay = ctx.layout_text(&text, &opts);
                    dl.text_shadow(&lay, [r.x, y], Color::WHITE, [s.max(1.0), s.max(1.0)], Color([0, 0, 0, 220]));
                    y += lay.height + 3.0 * s;
                }
            }
            WidgetKind::Compass { heading_var, fov, color } => {
                let heading = state.get(heading_var);
                dl.rect(Rect::new(r.x, r.bottom() - s, r.w, s), color.with_alpha(0.35));
                let mut a = 0.0f32;
                while a < 360.0 {
                    let mut d = a - heading;
                    while d > 180.0 {
                        d -= 360.0;
                    }
                    while d < -180.0 {
                        d += 360.0;
                    }
                    if d.abs() <= fov * 0.5 {
                        let x = r.x + r.w * (0.5 + d / fov);
                        let cardinal = (a as i32) % 90 == 0;
                        let fade = (1.0 - (d.abs() / (fov * 0.5)).powi(3)).clamp(0.0, 1.0);
                        if cardinal {
                            let ch = match a as i32 { 0 => "N", 90 => "E", 180 => "S", _ => "W" };
                            let mut o = TextOptions::new("Menu").align(Align::Center);
                            o.scale = s;
                            let lay = ctx.layout_text(ch, &o);
                            dl.text(&lay, [x - lay.width * 0.5, r.y], color.with_alpha(fade));
                        } else {
                            dl.rect(Rect::new(x - s * 0.5, r.bottom() - 5.0 * s, s, 4.0 * s), color.with_alpha(fade * 0.7));
                        }
                    }
                    a += 15.0;
                }
                // centre marker
                dl.rect_blend(Rect::new(r.x + r.w * 0.5 - s, r.bottom() - 7.0 * s, 2.0 * s, 7.0 * s), Color([255, 70, 40, 255]), Blend::Alpha);
            }
        }
    }
}

/// Built-in HUD layout (RON text, also the template for mods: copy it to `UI/hud.ron`).
pub const DEFAULT_LAYOUT_RON: &str = r##"(
    name: "default",
    reference_height: 720.0,
    margin: 24.0,
    widgets: [
        // top centre: compass strip
        (id: "compass", anchor: Top, offset: (0.0, 0.0), size: (300.0, 22.0), visible: "show_compass",
         kind: Compass(heading_var: "heading", fov: 120.0, color: "#e8e8e8")),
        // top left: health
        (id: "health", anchor: TopLeft, offset: (0.0, 24.0), size: (220.0, 12.0),
         kind: Bar(value: "health", max: "health_max", fill: "#c8c8c8", back: "#00000099", border: ("#ffffff55", 1.0),
                   low_threshold: Some(0.3), low_fill: Some("#d8281e"), label: Some("%HEALTH"))),
        // top right: notoriety
        (id: "notoriety", anchor: TopRight, offset: (0.0, 24.0), size: (220.0, 12.0),
         kind: Bar(value: "notoriety", max: "notoriety_max", fill: "#e6c864", back: "#00000099", border: ("#ffffff55", 1.0),
                   segments: 10, label: Some("%NOTORIETY"))),
        // suspicion indicator under the notoriety bar, shown while somebody is suspicious
        (id: "suspicion", anchor: TopRight, offset: (0.0, 64.0), size: (220.0, 8.0), visible: "suspicion>0",
         kind: Bar(value: "suspicion", max: "suspicion_max", fill: "#e8e8e8", back: "#00000099",
                   low_fill: Some("#d8281e"), low_threshold: Some(0.0), label: Some("%SUSPICION"))),
        // bottom right: weapon + ammo
        (id: "weapon_panel", anchor: BottomRight, offset: (0.0, 0.0), size: (200.0, 64.0), visible: "has_weapon",
         kind: Panel(color: "#00000099", border: Some(("#ffffff44", 1.0)))),
        (id: "weapon_name", anchor: BottomRight, offset: (-10.0, -38.0), size: (180.0, 20.0), visible: "has_weapon",
         kind: Text(text: "{weapon_name}", font: "Menu", align: Right)),
        (id: "ammo", anchor: BottomRight, offset: (-10.0, -10.0), size: (180.0, 20.0), visible: "has_weapon",
         kind: Text(text: "{ammo_clip} / {ammo_reserve}", font: "Header", align: Right)),
        // bottom centre: prompts and subtitles
        (id: "prompts", anchor: Bottom, offset: (0.0, -120.0), size: (400.0, 60.0),
         kind: Prompts(align: Center)),
        (id: "subtitles", anchor: Bottom, offset: (0.0, -40.0), size: (760.0, 60.0),
         kind: Subtitles(max_lines: 2)),
    ],
)"##;

/// The built-in layout. Panics only if the embedded RON is broken (guarded by a unit test).
pub fn default_layout() -> HudLayout {
    HudLayout::from_ron(DEFAULT_LAYOUT_RON).expect("builtin HUD layout parses")
}

/// Loads a mod layout from the VFS (`UI/hud.ron`, then `UI/hud.json`); `None` if neither exists.
/// Errors are logged and the caller keeps its current layout.
pub fn load_layout(vfs: &bm_vfs::Vfs) -> Option<HudLayout> {
    for (path, ron) in [("UI/hud.ron", true), ("UI/hud.json", false)] {
        let Ok(bytes) = vfs.open(path) else { continue };
        let text = crate::xml::decode_text(&bytes);
        let r = if ron { HudLayout::from_ron(&text) } else { HudLayout::from_json(&text) };
        match r {
            Ok(l) => return Some(l),
            Err(e) => log::error!("{path}: {e}"),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::test_font;

    fn ctx() -> UiContext {
        let mut c = UiContext::new();
        for n in ["Menu", "Header"] {
            let mut f = test_font();
            f.name = n.into();
            c.fonts.insert(f);
        }
        c
    }

    #[test]
    fn builtin_layout_parses_and_round_trips() {
        let l = default_layout();
        assert!(l.widget("health").is_some() && l.widget("subtitles").is_some());
        let back = HudLayout::from_ron(&l.to_ron()).unwrap();
        assert_eq!(back, l);
        let j = HudLayout::from_json(&l.to_json()).unwrap();
        assert_eq!(j, l);
    }

    #[test]
    fn expressions_and_interpolation() {
        let mut st = HudState::new();
        st.set("health", 75.0).set("ammo", 12.5).set_text("weapon", "Silverballers");
        assert!(st.eval("health>50") && !st.eval("health>=80") && st.eval("health<=75"));
        assert!(st.eval("health") && !st.eval("missing") && st.eval("!missing") && st.eval("missing==0"));
        assert!(st.eval("weapon") && !st.eval("!weapon"));
        assert_eq!(st.interpolate("{health} hp, {ammo}, {weapon}, {ZW_SELECT}"), "75 hp, 12.5, Silverballers, {ZW_SELECT}");
    }

    #[test]
    fn subtitles_expire() {
        let mut st = HudState::new();
        st.say(Some("47"), "Hello", 1.0);
        st.update(0.5);
        assert_eq!(st.active_subtitles().count(), 1);
        st.update(0.6);
        assert_eq!(st.active_subtitles().count(), 0);
        assert!(st.subtitles.is_empty());
    }

    #[test]
    fn draw_respects_visibility_and_anchors() {
        let c = ctx();
        let layout = default_layout();
        let mut st = HudState::new();
        st.set("health", 40.0).set("health_max", 100.0).set("notoriety", 5.0).set("notoriety_max", 10.0);
        let mut dl = DrawList::new();
        Hud::draw(&layout, &st, &c, &mut dl, [1280.0, 720.0]);
        let without = dl.quad_count();
        assert!(without > 0);
        // weapon widgets appear when has_weapon
        st.set("has_weapon", 1.0).set("ammo_clip", 7.0).set("ammo_reserve", 28.0).set_text("weapon_name", "Silverballer");
        let mut dl2 = DrawList::new();
        Hud::draw(&layout, &st, &c, &mut dl2, [1280.0, 720.0]);
        assert!(dl2.quad_count() > without + 10);
        // the health bar's back panel is the first quad of the health widget: anchored TopLeft + margin 24 + offset
        let first = dl.vertices[0].pos;
        assert!(first[0] >= 24.0 && first[1] >= 24.0);
    }

    #[test]
    fn segmented_bar_lights_ceil_fraction() {
        let c = ctx();
        let layout = HudLayout {
            name: "t".into(),
            reference_height: 100.0,
            margin: 0.0,
            widgets: vec![HudWidget {
                id: "b".into(),
                anchor: HudAnchor::TopLeft,
                offset: [0.0, 0.0],
                size: [100.0, 10.0],
                visible: None,
                kind: WidgetKind::Bar { value: "v".into(), max: None, dir: BarDir::LeftToRight, fill: Color::WHITE, back: None, border: None, segments: 4, low_threshold: None, low_fill: None, label: None },
            }],
        };
        let mut st = HudState::new();
        st.set("v", 0.5);
        let mut dl = DrawList::new();
        Hud::draw(&layout, &st, &c, &mut dl, [100.0, 100.0]);
        assert_eq!(dl.quad_count(), 4);
        let lit = dl.vertices.chunks(4).filter(|q| q[0].color[3] > 0.9).count();
        assert_eq!(lit, 2);
    }
}
