//! Menu runtime: layout of a [`Window`], focus navigation / page stack, option values and drawing.
//!
//! Layout rules (derived from the XML, confidence medium - see `docs/ui.md`):
//!
//! * the window rectangle is `W x H` centred on the design canvas (PC: 1024x768) when `AlignH/AlignV = Center`;
//! * an item with `X`/`Y` is placed at that offset from the window's top-left; `Alignment` says whether `X` is the
//!   left edge, the centre or the right edge of the item;
//! * an item without `Y` flows below the previous one (`previous bottom + LineSpace`);
//!   members of a horizontal `<Navigation>` group share the cursor;
//! * toggles draw `Text` right-aligned at `X` and the current choice at `X + OptionOffset`; sliders draw `Text`
//!   right-aligned at `X + TextOffsetX` and a track of `SliderSize` pixels at `X + SliderOffset`; check boxes draw
//!   their box left of the label.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::context::UiContext;
use crate::draw::{Blend, DrawList};
use crate::menu::{Action, ColorSet, Item, ItemKind, MenuDoc, Navigation, Window};
use crate::text::{TextLayout, TextOptions};
use crate::types::{Align, Color, Rect};

pub const DEFAULT_CANVAS: [f32; 2] = [1024.0, 768.0];

#[derive(Clone, Debug)]
pub enum LaidKind {
    Text,
    Header,
    Button(Action),
    Slider { option: String, lower: f32, upper: f32, step: f32, size: f32, track: Rect },
    Toggle { option: String, choices: Vec<String>, value_pos: [f32; 2] },
    CheckBox { option: String, box_rect: Rect },
    Graphic { name: String },
    List { interface: String },
    Other,
}

#[derive(Clone, Debug)]
pub struct LaidItem {
    /// Index in [`Window::walk`] order.
    pub index: usize,
    pub kind: LaidKind,
    pub name: Option<String>,
    /// Resolved display text (localised, markup expanded).
    pub text: String,
    pub layout: TextLayout,
    /// Top-left of the text block in canvas pixels.
    pub text_pos: [f32; 2],
    /// Hit/focus rectangle in canvas pixels.
    pub rect: Rect,
    pub focusable: bool,
    pub color_set: Option<String>,
    pub activate_button: Option<String>,
    pub receive_all_input: bool,
}

#[derive(Clone, Debug)]
pub struct WindowLayout {
    pub name: String,
    pub rect: Rect,
    pub items: Vec<LaidItem>,
    pub background: Option<String>,
    pub frame: bool,
    pub dialog: bool,
    /// Indices into `items` of focusable items in navigation order.
    pub focus_order: Vec<usize>,
    pub navigation: Navigation,
}

fn default_font(item: &Item) -> &str {
    match (&item.font, &item.kind) {
        (Some(f), _) => f,
        (None, ItemKind::Header) => "Header",
        _ => "Menu",
    }
}

/// Lays `win` out on a canvas of `canvas` pixels.
pub fn layout_window(doc: &MenuDoc, win: &Window, ctx: &UiContext, canvas: [f32; 2]) -> WindowLayout {
    let (ww, wh) = (if win.w > 0.0 { win.w } else { canvas[0] }, if win.h > 0.0 { win.h } else { canvas[1] });
    let wx = win.align_h.origin(match win.align_h { Align::Left => 0.0, Align::Center => canvas[0] * 0.5, Align::Right => canvas[0] }, ww) + win.x;
    let wy = win.align_v.origin(match win.align_v { Align::Left => 0.0, Align::Center => canvas[1] * 0.5, Align::Right => canvas[1] }, wh) + win.y;
    let rect = Rect::new(wx, wy, ww, wh);
    let mut items = Vec::new();
    let mut cursor_y = 0.0f32;
    let mut index = 0usize;
    fn rec(
        doc: &MenuDoc,
        win: &Window,
        ctx: &UiContext,
        rect: Rect,
        list: &[Item],
        horizontal: bool,
        cursor_y: &mut f32,
        index: &mut usize,
        out: &mut Vec<LaidItem>,
    ) {
        let group_start_y = *cursor_y;
        let mut group_bottom = *cursor_y;
        for it in list {
            let my_index = *index;
            *index += 1;
            if let ItemKind::Group(nav) = &it.kind {
                rec(doc, win, ctx, rect, &it.children, *nav == Navigation::Horizontal, cursor_y, index, out);
                continue;
            }
            let y_rel = it.y.unwrap_or(if horizontal { group_start_y } else { *cursor_y });
            let li = lay_item(doc, ctx, rect, it, my_index, y_rel);
            let bottom = y_rel + li.layout.height.max(li.rect.h);
            group_bottom = group_bottom.max(bottom);
            if !horizontal && !matches!(it.kind, ItemKind::Other(_)) {
                *cursor_y = bottom + win.line_space;
            }
            out.push(li);
        }
        if horizontal {
            *cursor_y = group_bottom + win.line_space;
        }
    }
    rec(doc, win, ctx, rect, &win.items, false, &mut cursor_y, &mut index, &mut items);
    let focus_order = items.iter().enumerate().filter(|(_, i)| i.focusable).map(|(k, _)| k).collect();
    WindowLayout {
        name: win.name.clone(),
        rect,
        items,
        background: win.background.clone(),
        frame: win.frame,
        dialog: win.dialog,
        focus_order,
        navigation: win.navigation,
    }
}

fn lay_item(doc: &MenuDoc, ctx: &UiContext, win: Rect, it: &Item, index: usize, y_rel: f32) -> LaidItem {
    let text = it.text.as_deref().map(|t| ctx.strings.expand_key(t)).unwrap_or_default();
    let font = default_font(it).to_string();
    let a = &it.attrs;
    let x_rel = it.x.unwrap_or(match it.alignment {
        Align::Center => win.w * 0.5,
        Align::Right => win.w,
        Align::Left => 0.0,
    });
    // slider / toggle / checkbox label alignment comes from the ButtonGraphic (TextAlignment/TextOffset) or template
    let bg = a.get("ButtonGraphic").and_then(|n| doc.button_graphics.get(&n.to_ascii_lowercase()));
    let mut align = it.alignment;
    let mut label_dx = 0.0;
    if let (ItemKind::Slider { .. }, Some(bg)) = (&it.kind, bg) {
        if let Some(ta) = bg.text_alignment {
            align = ta;
        }
        label_dx = bg.text_offset[0];
    }
    let mut opts = TextOptions::new(&font).align(align);
    if let Some(w) = it.width {
        opts.max_width = Some(w);
    }
    let layout = ctx.layout_text(&text, &opts);
    let w = if it.width.is_some() { layout.width } else { layout.width };
    let ax = win.x + x_rel + label_dx;
    // with a wrap width the block starts at X (left) / is centred on X / ends at X
    let left = match (it.width.is_some(), align) {
        (true, Align::Left) => ax,
        (true, Align::Center) => ax - w * 0.5,
        (true, Align::Right) => ax - w,
        (false, Align::Left) => ax,
        (false, Align::Center) => ax - w * 0.5,
        (false, Align::Right) => ax - w,
    };
    let top = win.y + y_rel;
    let text_pos = [left, top];
    let mut rect = Rect::new(left, top, w.max(1.0), layout.height.max(1.0));
    let kind = match &it.kind {
        ItemKind::Text => LaidKind::Text,
        ItemKind::Header => LaidKind::Header,
        ItemKind::Button(a) => LaidKind::Button(a.clone()),
        ItemKind::Slider { option, lower, upper, step, size } => {
            let off = a.f32("SliderOffset").unwrap_or(0.0);
            let h = bg.and_then(|b| b.height).unwrap_or(18.0);
            let track = Rect::new(win.x + x_rel + off, top + (layout.height - h.min(layout.height)) * 0.5, *size, h.min(layout.height.max(8.0)));
            rect = rect.union(&track);
            LaidKind::Slider { option: option.clone(), lower: *lower, upper: *upper, step: *step, size: *size, track }
        }
        ItemKind::Toggle { option } => {
            let off = a.f32("OptionOffset").unwrap_or(25.0);
            let choices: Vec<String> = it.choices.iter().map(|c| ctx.strings.expand_key(c)).collect();
            let vp = [win.x + x_rel + off, top];
            let max_w = choices
                .iter()
                .map(|c| ctx.measure(c, &TextOptions::new(&font))[0])
                .fold(0.0f32, f32::max);
            rect = rect.union(&Rect::new(vp[0], top, max_w, layout.height));
            LaidKind::Toggle { option: option.clone(), choices, value_pos: vp }
        }
        ItemKind::CheckBox { option } => {
            let (dx, dy) = (a.f32("CheckBoxOffsetX").unwrap_or(23.0), a.f32("CheckBoxOffsetY").unwrap_or(-16.0));
            let size = 12.0;
            let box_rect = Rect::new(win.x + x_rel + dx, top + layout.height * 0.5 - size * 0.5 + (dy + 16.0) * 0.0, size, size);
            rect = rect.union(&box_rect);
            LaidKind::CheckBox { option: option.clone(), box_rect }
        }
        ItemKind::Graphic { name } => LaidKind::Graphic { name: name.clone() },
        ItemKind::List { interface } => LaidKind::List { interface: interface.clone() },
        ItemKind::Group(_) | ItemKind::Other(_) => LaidKind::Other,
    };
    LaidItem {
        index,
        kind,
        name: it.name.clone(),
        text,
        layout,
        text_pos,
        rect,
        focusable: it.focusable,
        color_set: it.color_set.clone(),
        activate_button: it.activate_button.clone(),
        receive_all_input: it.receive_all_input,
    }
}

/// Navigation / pointer input for a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    /// Name of a directly-activating button (`ZW_SELECT4`): triggers items with that `ActivateButton`.
    Button(String),
    /// Canvas-space pointer position (hover focus).
    PointerMove([f32; 2]),
    /// Canvas-space click (focus + select).
    PointerClick([f32; 2]),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEvent {
    None,
    /// A page was pushed.
    PageOpened(String),
    /// A page was popped (name of the page that closed).
    PageClosed(String),
    /// A game-logic action was triggered (`LoadScene`, `Quit`, `Custom`, ...). `item` is the item name if any.
    Action { action: Action, item: Option<String> },
    /// Slider / toggle / checkbox changed.
    OptionChanged { option: String, value: f32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageState {
    pub window: String,
    /// Index into `focus_order` of the focused item.
    pub focus: usize,
}

#[derive(Clone, Debug)]
pub struct Transition {
    pub from: Option<String>,
    pub to: Option<String>,
    pub t: f32,
    pub duration: f32,
}

impl Transition {
    /// Alpha of the incoming page (0..1) and the outgoing page.
    pub fn alphas(&self) -> (f32, f32) {
        let k = (self.t / self.duration.max(1e-3)).clamp(0.0, 1.0);
        (k, 1.0 - k)
    }
}

/// The page stack of a menu document.
pub struct MenuNav {
    pub doc: Arc<MenuDoc>,
    pub stack: Vec<PageState>,
    /// Option values (sliders: raw value; toggles: choice index; checkboxes: 0/1).
    pub options: HashMap<String, f32>,
    pub canvas: [f32; 2],
    pub transition: Option<Transition>,
    pub transition_time: f32,
    layout: Option<WindowLayout>,
}

impl MenuNav {
    pub fn new(doc: Arc<MenuDoc>, root: &str) -> MenuNav {
        MenuNav {
            doc,
            stack: vec![PageState { window: root.to_string(), focus: 0 }],
            options: HashMap::new(),
            canvas: DEFAULT_CANVAS,
            transition: None,
            transition_time: 0.15,
            layout: None,
        }
    }

    pub fn current(&self) -> Option<&PageState> {
        self.stack.last()
    }

    pub fn current_window(&self) -> Option<&Window> {
        self.doc.window(&self.current()?.window)
    }

    /// Layout of the current page (cached until the page or the strings change - call `invalidate`).
    pub fn layout(&mut self, ctx: &UiContext) -> Option<&WindowLayout> {
        if self.layout.is_none() {
            let win = self.current_window()?.clone();
            self.layout = Some(layout_window(&self.doc, &win, ctx, self.canvas));
        }
        self.layout.as_ref()
    }

    pub fn invalidate(&mut self) {
        self.layout = None;
    }

    /// Replaces the document (hot reload) keeping the stack where the pages still exist.
    pub fn set_doc(&mut self, doc: Arc<MenuDoc>) {
        self.doc = doc;
        let doc = self.doc.clone();
        self.stack.retain(|p| doc.window(&p.window).is_some());
        self.invalidate();
    }

    pub fn option(&self, name: &str) -> f32 {
        self.options.get(name).copied().unwrap_or(0.0)
    }

    pub fn set_option(&mut self, name: &str, v: f32) {
        self.options.insert(name.to_string(), v);
    }

    fn push(&mut self, window: &str, reset: bool) -> MenuEvent {
        if self.doc.window(window).is_none() {
            log::warn!("menu: window '{window}' not found");
            return MenuEvent::None;
        }
        let from = self.current().map(|p| p.window.clone());
        if reset {
            self.stack.clear();
        }
        self.stack.push(PageState { window: window.to_string(), focus: 0 });
        self.transition = Some(Transition { from, to: Some(window.to_string()), t: 0.0, duration: self.transition_time });
        self.invalidate();
        MenuEvent::PageOpened(window.to_string())
    }

    fn pop(&mut self) -> MenuEvent {
        if self.stack.len() <= 1 {
            return MenuEvent::None;
        }
        let from = self.stack.pop().map(|p| p.window);
        self.transition = Some(Transition { from: from.clone(), to: self.current().map(|p| p.window.clone()), t: 0.0, duration: self.transition_time });
        self.invalidate();
        MenuEvent::PageClosed(from.unwrap_or_default())
    }

    /// Advances page transitions.
    pub fn update(&mut self, dt: f32) {
        if let Some(t) = &mut self.transition {
            t.t += dt;
            if t.t >= t.duration {
                self.transition = None;
            }
        }
    }

    fn activate(&mut self, item_idx: usize, ctx: &UiContext) -> MenuEvent {
        let Some(l) = self.layout(ctx).cloned() else { return MenuEvent::None };
        let Some(it) = l.items.get(item_idx) else { return MenuEvent::None };
        match &it.kind {
            LaidKind::Button(a) => match a {
                Action::OpenWindow { window, reset_stack } => self.push(window, *reset_stack),
                Action::CloseWindow { .. } => self.pop(),
                Action::SetDifficulty { level, window } => {
                    self.set_option("DifficultyLevel", *level as f32);
                    match window {
                        Some(w) => self.push(w, false),
                        None => MenuEvent::Action { action: a.clone(), item: it.name.clone() },
                    }
                }
                other => MenuEvent::Action { action: other.clone(), item: it.name.clone() },
            },
            LaidKind::Toggle { option, choices, .. } => {
                let n = choices.len().max(1) as f32;
                let v = (self.option(option) + 1.0) % n;
                self.set_option(option, v);
                MenuEvent::OptionChanged { option: option.clone(), value: v }
            }
            LaidKind::CheckBox { option, .. } => {
                let v = if self.option(option) > 0.5 { 0.0 } else { 1.0 };
                self.set_option(option, v);
                MenuEvent::OptionChanged { option: option.clone(), value: v }
            }
            _ => MenuEvent::None,
        }
    }

    fn adjust(&mut self, item_idx: usize, dir: f32, ctx: &UiContext) -> MenuEvent {
        let Some(l) = self.layout(ctx).cloned() else { return MenuEvent::None };
        let Some(it) = l.items.get(item_idx) else { return MenuEvent::None };
        match &it.kind {
            LaidKind::Slider { option, lower, upper, step, .. } => {
                let v = (self.option(option) + dir * step).clamp(*lower, *upper);
                self.set_option(option, v);
                MenuEvent::OptionChanged { option: option.clone(), value: v }
            }
            LaidKind::Toggle { option, choices, .. } => {
                let n = choices.len().max(1) as f32;
                let v = (self.option(option) + dir + n) % n;
                self.set_option(option, v);
                MenuEvent::OptionChanged { option: option.clone(), value: v }
            }
            _ => MenuEvent::None,
        }
    }

    /// Feeds one navigation input.
    pub fn handle(&mut self, nav: &Nav, ctx: &UiContext) -> MenuEvent {
        let Some(l) = self.layout(ctx).cloned() else { return MenuEvent::None };
        let n = l.focus_order.len();
        let focus = self.current().map(|p| p.focus).unwrap_or(0).min(n.saturating_sub(1));
        let set_focus = |s: &mut MenuNav, f: usize| {
            if let Some(p) = s.stack.last_mut() {
                p.focus = f;
            }
        };
        match nav {
            Nav::Up | Nav::Down if n > 0 => {
                let f = if *nav == Nav::Down { (focus + 1) % n } else { (focus + n - 1) % n };
                set_focus(self, f);
                MenuEvent::None
            }
            Nav::Left | Nav::Right if n > 0 => {
                let dir = if *nav == Nav::Right { 1.0 } else { -1.0 };
                self.adjust(l.focus_order[focus], dir, ctx)
            }
            Nav::Select if n > 0 => self.activate(l.focus_order[focus], ctx),
            Nav::Back => {
                // a CloseWindow item with Cancel wins over a plain pop
                self.pop()
            }
            Nav::Button(name) => {
                if let Some(k) = l.items.iter().position(|i| i.activate_button.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(name))) {
                    self.activate(k, ctx)
                } else {
                    MenuEvent::None
                }
            }
            Nav::PointerMove(p) => {
                if let Some(pos) = l.focus_order.iter().position(|&k| l.items[k].rect.contains(*p)) {
                    set_focus(self, pos);
                }
                MenuEvent::None
            }
            Nav::PointerClick(p) => {
                if let Some(pos) = l.focus_order.iter().position(|&k| l.items[k].rect.contains(*p)) {
                    set_focus(self, pos);
                    let k = l.focus_order[pos];
                    // click on a slider track sets the value directly
                    if let LaidKind::Slider { option, lower, upper, track, .. } = &l.items[k].kind {
                        let t = ((p[0] - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
                        let v = lower + (upper - lower) * t;
                        self.set_option(option, v);
                        return MenuEvent::OptionChanged { option: option.clone(), value: v };
                    }
                    self.activate(k, ctx)
                } else {
                    MenuEvent::None
                }
            }
            _ => MenuEvent::None,
        }
    }

    /// Draws the current page (and cross-fades during transitions).
    pub fn draw(&mut self, ctx: &UiContext, dl: &mut DrawList) {
        let doc = self.doc.clone();
        let mut pages: Vec<(WindowLayout, usize, f32)> = Vec::new();
        let (a_in, a_out) = self.transition.as_ref().map(Transition::alphas).unwrap_or((1.0, 0.0));
        if let Some(t) = &self.transition {
            let cur_name = self.stack.last().map(|p| p.window.as_str());
            if let Some(from) = t.from.as_deref().filter(|f| Some(*f) != cur_name).and_then(|n| doc.window(n)) {
                if a_out > 0.0 {
                    pages.push((layout_window(&doc, from, ctx, self.canvas), 0, a_out));
                }
            }
        }
        if let Some(cur) = self.layout(ctx).cloned() {
            let focus = self.current().map(|p| p.focus).unwrap_or(0);
            pages.push((cur, focus, a_in));
        }
        let options = self.options.clone();
        for (l, focus, alpha) in pages {
            draw_window(&doc, &l, focus, &options, ctx, dl, alpha);
        }
    }
}

fn color_for<'a>(doc: &'a MenuDoc, name: Option<&str>) -> ColorSet {
    name.and_then(|n| doc.color_set(n)).or_else(|| doc.color_set("default")).cloned().unwrap_or_default()
}

/// Draws one laid out page. `focus` indexes `layout.focus_order`.
pub fn draw_window(doc: &MenuDoc, l: &WindowLayout, focus: usize, options: &HashMap<String, f32>, ctx: &UiContext, dl: &mut DrawList, alpha: f32) {
    let tint = Color::WHITE.with_alpha(alpha);
    if let Some(bg) = l.background.as_deref().and_then(|n| ctx.sprite(n)) {
        bg.draw(dl, [0.0, 0.0], None, tint);
    }
    if l.dialog || l.frame && l.background.is_none() {
        dl.rect(l.rect, Color([10, 10, 10, 200]).with_alpha(alpha));
        dl.border(l.rect, 1.0, Color([200, 200, 200, 200]).with_alpha(alpha));
    }
    let focused_item = l.focus_order.get(focus).copied();
    for (k, it) in l.items.iter().enumerate() {
        let is_focus = Some(k) == focused_item;
        let cs = color_for(doc, it.color_set.as_deref());
        let color = if is_focus { cs.focus } else { cs.normal }.mul(tint);
        match &it.kind {
            LaidKind::Graphic { name } => {
                if let Some(s) = ctx.sprite(name) {
                    s.draw(dl, [l.rect.x, l.rect.y], None, tint);
                }
                continue;
            }
            LaidKind::Other | LaidKind::List { .. } => continue,
            _ => {}
        }
        if is_focus {
            // focus highlight: soft additive bar behind the entry
            let r = Rect::new(it.rect.x - 10.0, it.rect.y - 2.0, it.rect.w + 20.0, it.rect.h + 4.0);
            dl.rect_blend(r, Color([255, 255, 255, 28]).with_alpha(alpha), Blend::Additive);
            dl.rect(Rect::new(r.x, r.y + 1.0, 2.0, r.h - 2.0), Color([230, 40, 30, 255]).with_alpha(alpha));
        }
        let shadow = Color([0, 0, 0, 200]).with_alpha(alpha);
        dl.text_shadow(&it.layout, it.text_pos, color, [1.0, 1.0], shadow);
        match &it.kind {
            LaidKind::Toggle { option, choices, value_pos } => {
                let i = (options.get(option).copied().unwrap_or(0.0) as usize).min(choices.len().saturating_sub(1));
                if let Some(c) = choices.get(i) {
                    let lay = ctx.layout_text(c, &TextOptions::new("Menu"));
                    dl.text_shadow(&lay, *value_pos, color, [1.0, 1.0], shadow);
                }
            }
            LaidKind::Slider { option, lower, upper, track, .. } => {
                let v = options.get(option).copied().unwrap_or(*lower);
                let t = if upper > lower { ((v - lower) / (upper - lower)).clamp(0.0, 1.0) } else { 0.0 };
                let bar = Rect::new(track.x, track.y + track.h * 0.5 - 1.0, track.w, 2.0);
                dl.rect(bar, Color([90, 90, 90, 255]).with_alpha(alpha));
                dl.rect(Rect::new(bar.x, bar.y, bar.w * t, bar.h), color);
                dl.rect(Rect::new(track.x + track.w * t - 3.0, track.y + 2.0, 6.0, track.h - 4.0), color);
            }
            LaidKind::CheckBox { option, box_rect } => {
                dl.border(*box_rect, 1.0, color);
                if options.get(option).copied().unwrap_or(0.0) > 0.5 {
                    dl.rect(Rect::new(box_rect.x + 3.0, box_rect.y + 3.0, box_rect.w - 6.0, box_rect.h - 6.0), color);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::test_font;
    use crate::menu::MenuContext;

    const XML: &str = r#"
<Windows>
  <Platform PC="True">
    <Template Name="Window" W="1024" H="768" AlignV="Center" AlignH="Center" LineSpace="11" />
    <Template Name="LeftPosX" X="376" Alignment="Right" />
    <Template Name="RightPosX" X="391" />
    <Template Name="TopPos" Type="TextBlock" Text="" Y="196" />
    <Template Name="ToggleOption" OptionOffset="25" Template="LeftPosX" />
  </Platform>
  <Window Name="Main" Template="Window" PopOnBack="False">
    <GuiElement Template="TopPos" />
    <GuiElement Type="OpenWindow" Text="%Start" Window="Opts" Template="LeftPosX" />
    <GuiElement Type="OpenWindow" Text="%Options" Window="Opts" Template="LeftPosX" />
    <GuiElement Type="LoadScene" Text="%Go" Scene="m01" Template="LeftPosX" />
  </Window>
  <Window Name="Opts" Template="Window">
    <GuiElement Type="Slider" Option="Vol" Text="%Vol" Template="RightPosX" LowerBound="0" UpperBound="100" Step="5" SliderSize="100" />
    <GuiElement Type="HM3ToggleOption" Option="Eax" Text="%Eax" Template="ToggleOption"><ToggleTextItem Text="%No" /><ToggleTextItem Text="%Yes" /></GuiElement>
    <Navigation Navigation="Horizontal">
      <GuiElement Type="CloseWindow" Text="%Accept" X="100" Y="629" />
      <GuiElement Type="CloseWindow" Text="%Cancel" X="300" Y="629" Cancel="True" />
    </Navigation>
  </Window>
</Windows>"#;

    fn ctx() -> UiContext {
        let mut c = UiContext::new();
        let mut f = test_font();
        f.name = "Menu".into();
        c.fonts.insert(f);
        c
    }

    fn nav() -> MenuNav {
        let doc = MenuDoc::parse_str(XML, "t", &MenuContext::default());
        MenuNav::new(Arc::new(doc), "Main")
    }

    #[test]
    fn flow_layout_and_alignment() {
        let c = ctx();
        let mut n = nav();
        let l = n.layout(&c).unwrap().clone();
        assert_eq!(l.items.len(), 4);
        // window is 1024x768 at the canvas origin
        assert_eq!(l.rect, Rect::new(0.0, 0.0, 1024.0, 768.0));
        // TopPos is empty text at Y=196 with height 10; next item flows at 196 + 10 + 11
        assert_eq!(l.items[0].rect.y, 196.0);
        assert_eq!(l.items[1].rect.y, 196.0 + 10.0 + 11.0);
        assert_eq!(l.items[2].rect.y, 196.0 + 2.0 * 21.0);
        // right aligned at X=376: "Start" is 5 glyphs of 7px
        let w = l.items[1].layout.width;
        assert_eq!(w, 35.0);
        assert_eq!(l.items[1].text_pos[0], 376.0 - 35.0);
        assert_eq!(l.focus_order, vec![1, 2, 3]);
    }

    #[test]
    fn horizontal_group_shares_cursor_and_slider_toggle_geometry() {
        let c = ctx();
        let mut n = nav();
        n.handle(&Nav::Select, &c); // Start -> Opts
        let l = n.layout(&c).unwrap().clone();
        assert_eq!(n.current().unwrap().window, "Opts");
        assert_eq!(l.items.len(), 4);
        match &l.items[0].kind {
            LaidKind::Slider { track, .. } => assert_eq!((track.x, track.w), (391.0, 100.0)),
            k => panic!("{k:?}"),
        }
        match &l.items[1].kind {
            LaidKind::Toggle { value_pos, choices, .. } => {
                assert_eq!(value_pos[0], 376.0 + 25.0);
                assert_eq!(choices, &vec!["No".to_string(), "Yes".to_string()]);
            }
            k => panic!("{k:?}"),
        }
        assert_eq!(l.items[2].rect.y, 629.0);
        assert_eq!(l.items[3].rect.y, 629.0);
    }

    #[test]
    fn navigation_actions_and_options() {
        let c = ctx();
        let mut n = nav();
        assert_eq!(n.handle(&Nav::Down, &c), MenuEvent::None);
        assert_eq!(n.current().unwrap().focus, 1);
        n.handle(&Nav::Up, &c);
        n.handle(&Nav::Up, &c); // wraps to last
        assert_eq!(n.current().unwrap().focus, 2);
        match n.handle(&Nav::Select, &c) {
            MenuEvent::Action { action: Action::LoadScene { scene, .. }, .. } => assert_eq!(scene, "m01"),
            e => panic!("{e:?}"),
        }
        n.handle(&Nav::Down, &c);
        assert_eq!(n.handle(&Nav::Select, &c), MenuEvent::PageOpened("Opts".into()));
        assert_eq!(n.handle(&Nav::Right, &c), MenuEvent::OptionChanged { option: "Vol".into(), value: 5.0 });
        n.handle(&Nav::Left, &c);
        assert_eq!(n.handle(&Nav::Left, &c), MenuEvent::OptionChanged { option: "Vol".into(), value: 0.0 });
        n.handle(&Nav::Down, &c);
        assert_eq!(n.handle(&Nav::Select, &c), MenuEvent::OptionChanged { option: "Eax".into(), value: 1.0 });
        assert_eq!(n.handle(&Nav::Select, &c), MenuEvent::OptionChanged { option: "Eax".into(), value: 0.0 });
        assert_eq!(n.handle(&Nav::Back, &c), MenuEvent::PageClosed("Opts".into()));
        assert_eq!(n.handle(&Nav::Back, &c), MenuEvent::None); // root stays
        assert!(n.transition.is_some());
        n.update(1.0);
        assert!(n.transition.is_none());
    }

    #[test]
    fn pointer_hover_and_click() {
        let c = ctx();
        let mut n = nav();
        let l = n.layout(&c).unwrap().clone();
        let r = l.items[3].rect;
        n.handle(&Nav::PointerMove([r.x + 1.0, r.y + 1.0]), &c);
        assert_eq!(n.current().unwrap().focus, 2);
        n.handle(&Nav::PointerMove([5.0, 5.0]), &c);
        assert_eq!(n.current().unwrap().focus, 2); // unchanged outside items
        let r = l.items[1].rect;
        assert_eq!(n.handle(&Nav::PointerClick([r.x + 1.0, r.y + 1.0]), &c), MenuEvent::PageOpened("Opts".into()));
    }

    #[test]
    fn draw_produces_geometry() {
        let c = ctx();
        let mut n = nav();
        let mut dl = DrawList::new();
        n.draw(&c, &mut dl);
        // three buttons' glyphs + the focus bar
        assert!(dl.quad_count() > 10);
    }
}
