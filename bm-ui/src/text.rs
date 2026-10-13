//! Text layout: LOC mini-markup, word wrapping, alignment, justification, kerning.
//!
//! Input is *expanded* text (see [`crate::strings::Strings::expand`]) which may still contain
//!
//! | markup | meaning |
//! |---|---|
//! | `\n`, `<br>` | line break |
//! | `<tab>`, `<tabsize N>` | tab to the next multiple of N pixels |
//! | `<font N>` ... `</font>` | switch to numbered font (see [`FontSet::numbered`]) |
//! | `<color rrggbb>` ... `</color>` | tint |
//! | `<caps next>`, `<caps word>`, `</caps>` | upper-case the next word / until `</caps>` |
//! | `<kerning N>`, `<linespacing N>` | extra pixels per glyph / per line |
//! | `<justify fill>` | stretch spaces so wrapped lines fill the width |
//! | `<width N>` | wrap width |
//! | `{TOKEN}` | button glyph token (`{ZW_SELECT}`), replaced by the resolver closure |
//!
//! Tags used only by the newspaper layouter (`<img>`, `<before>`, `<gutter>`, `<autocolumn>`, `<i>`) are skipped.
//! Text that merely looks like a tag (`<fix to dynamic name here>`) is rendered verbatim.

use crate::font::{BitmapFont, FontSet};
use crate::texture::TextureId;
use crate::types::{Align, Color, Rect};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedGlyph {
    pub texture: TextureId,
    /// Destination in layout space (origin = top-left of the block).
    pub rect: Rect,
    /// `[u0, v0, u1, v1]`
    pub uv: [f32; 4],
    pub color: Color,
    pub ch: char,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineInfo {
    pub y: f32,
    pub height: f32,
    pub width: f32,
    /// Glyph index range of the line in [`TextLayout::glyphs`].
    pub glyphs: std::ops::Range<usize>,
    /// The line's characters including spaces (as laid out, after caps/token processing).
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    pub glyphs: Vec<PlacedGlyph>,
    pub lines: Vec<LineInfo>,
    /// Width of the block (wrap width if given, else the widest line).
    pub width: f32,
    pub height: f32,
}

impl TextLayout {
    pub fn size(&self) -> [f32; 2] {
        [self.width, self.height]
    }
    /// The text of each line as laid out (for tests and debugging).
    pub fn line_texts(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.text.clone()).collect()
    }
}

#[derive(Clone, Debug)]
pub struct TextOptions {
    /// Default font name (a `<font N>` tag overrides it).
    pub font: String,
    pub color: Color,
    pub max_width: Option<f32>,
    pub align: Align,
    /// Extra pixels between lines.
    pub line_spacing: f32,
    pub justify: bool,
    pub scale: f32,
    pub tab_size: f32,
}

impl Default for TextOptions {
    fn default() -> Self {
        TextOptions { font: "Menu".into(), color: Color::WHITE, max_width: None, align: Align::Left, line_spacing: 0.0, justify: false, scale: 1.0, tab_size: 32.0 }
    }
}

impl TextOptions {
    pub fn new(font: &str) -> TextOptions {
        TextOptions { font: font.into(), ..Default::default() }
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }
    pub fn wrap(mut self, w: f32) -> Self {
        self.max_width = Some(w);
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Caps {
    Off,
    Next,
    Until,
}

#[derive(Clone)]
struct Style {
    font: usize,
    color: Color,
    kerning: f32,
    caps: Caps,
}

enum Atom {
    Word(Vec<(char, usize)>),
    Space(usize),
    Tab(usize),
    Break,
}

const KNOWN_TAGS: &[&str] = &[
    "br", "tab", "tabsize", "font", "/font", "color", "/color", "caps", "/caps", "kerning", "linespacing", "justify", "width", "i", "/i",
    "img", "before", "gutter", "autocolumn", "/before",
];

fn parse_tag(s: &str) -> Option<(&str, &str, usize)> {
    // s starts with '<'
    let end = s.find('>')?;
    if end > 80 {
        return None;
    }
    let inner = s[1..end].trim();
    let (name, arg) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
    let lname = name.to_ascii_lowercase();
    if KNOWN_TAGS.iter().any(|k| *k == lname) {
        Some((name, arg.trim(), end + 1))
    } else {
        None
    }
}

/// Lays `text` out. `resolve(token)` replaces `{TOKEN}`s (return `None` to drop the token).
pub fn layout(fonts: &FontSet, text: &str, opts: &TextOptions, resolve: &dyn Fn(&str) -> Option<String>) -> TextLayout {
    let scale = opts.scale;
    // fonts used, by index
    let mut used: Vec<&BitmapFont> = Vec::new();
    fn idx_of<'a>(used: &mut Vec<&'a BitmapFont>, f: &'a BitmapFont) -> usize {
        match used.iter().position(|u| std::ptr::eq(*u, f)) {
            Some(i) => i,
            None => {
                used.push(f);
                used.len() - 1
            }
        }
    }
    let Some(base_font) = fonts.get_or_default(&opts.font) else {
        return TextLayout::default();
    };
    let base_idx = idx_of(&mut used, base_font);
    let mut style_stack: Vec<Style> = vec![Style { font: base_idx, color: opts.color, kerning: 0.0, caps: Caps::Off }];
    let mut atoms: Vec<Atom> = Vec::new();
    let mut styles: Vec<Style> = Vec::new(); // style table indexed by atom char style id
    let mut line_extra = opts.line_spacing;
    let mut justify = opts.justify;
    let mut wrap = opts.max_width;
    let mut tab_size = opts.tab_size;

    let mut cur_word: Vec<(char, usize)> = Vec::new();
    let intern = |st: &Style, styles: &mut Vec<Style>| -> usize {
        if let Some(i) = styles.iter().position(|s| s.font == st.font && s.color == st.color && s.kerning == st.kerning) {
            i
        } else {
            styles.push(st.clone());
            styles.len() - 1
        }
    };
    macro_rules! flush_word {
        () => {
            if !cur_word.is_empty() {
                atoms.push(Atom::Word(std::mem::take(&mut cur_word)));
            }
        };
    }

    // resolve {TOKEN}s first
    let mut resolved = String::with_capacity(text.len());
    {
        let mut rest = text;
        while let Some(o) = rest.find('{') {
            resolved.push_str(&rest[..o]);
            match rest[o..].find('}') {
                Some(c) if c < 40 => {
                    let tok = &rest[o + 1..o + c];
                    if let Some(r) = resolve(tok) {
                        resolved.push_str(&r);
                    }
                    rest = &rest[o + c + 1..];
                }
                _ => {
                    resolved.push('{');
                    rest = &rest[o + 1..];
                }
            }
        }
        resolved.push_str(rest);
    }

    let mut s = resolved.as_str();
    while let Some(c) = s.chars().next() {
        if c == '<' {
            if let Some((name, arg, len)) = parse_tag(s) {
                let lname = name.to_ascii_lowercase();
                s = &s[len..];
                let top = style_stack.last().cloned().unwrap();
                match lname.as_str() {
                    "br" => {
                        flush_word!();
                        atoms.push(Atom::Break);
                    }
                    "tab" => {
                        flush_word!();
                        let id = intern(&top, &mut styles);
                        atoms.push(Atom::Tab(id));
                    }
                    "tabsize" => tab_size = arg.parse().unwrap_or(tab_size),
                    "font" => {
                        flush_word!();
                        let n: usize = arg.parse().unwrap_or(1);
                        let f = fonts.numbered(n).unwrap_or(base_font);
                        let i = idx_of(&mut used, f);
                        style_stack.push(Style { font: i, ..top });
                    }
                    "/font" | "/color" => {
                        flush_word!();
                        if style_stack.len() > 1 {
                            style_stack.pop();
                        }
                    }
                    "color" => {
                        flush_word!();
                        let col = crate::types::Color::parse(arg).unwrap_or(top.color);
                        style_stack.push(Style { color: col, ..top });
                    }
                    "caps" => {
                        let mode = if arg.eq_ignore_ascii_case("next") { Caps::Next } else { Caps::Until };
                        style_stack.last_mut().unwrap().caps = mode;
                    }
                    "/caps" => style_stack.last_mut().unwrap().caps = Caps::Off,
                    "kerning" => style_stack.last_mut().unwrap().kerning = arg.parse().unwrap_or(0.0),
                    "linespacing" => line_extra = opts.line_spacing + arg.parse::<f32>().unwrap_or(0.0),
                    "justify" => justify = arg.eq_ignore_ascii_case("fill"),
                    "width" => wrap = arg.parse().ok().or(wrap),
                    _ => {} // layouter-only tags: ignored
                }
                continue;
            }
        }
        s = &s[c.len_utf8()..];
        match c {
            '\n' => {
                flush_word!();
                atoms.push(Atom::Break);
            }
            '\r' => {}
            ' ' | '\u{a0}' => {
                flush_word!();
                let id = intern(style_stack.last().unwrap(), &mut styles);
                atoms.push(Atom::Space(id));
            }
            _ => {
                let st = style_stack.last_mut().unwrap();
                let ch = match st.caps {
                    Caps::Off => c,
                    Caps::Next | Caps::Until => c.to_uppercase().next().unwrap_or(c),
                };
                let st_now = st.clone();
                let id = intern(&st_now, &mut styles);
                cur_word.push((ch, id));
            }
        }
        // `caps next` ends after the first word
        if matches!(atoms.last(), Some(Atom::Space(_))) {
            for st in style_stack.iter_mut() {
                if st.caps == Caps::Next {
                    st.caps = Caps::Off;
                }
            }
        }
    }
    flush_word!();

    // measure helpers
    let font_of = |sid: usize| -> &BitmapFont { used[styles[sid].font] };
    let word_width = |w: &[(char, usize)]| -> f32 {
        let mut x = 0.0;
        let mut prev: Option<(char, usize)> = None;
        for &(c, sid) in w {
            let f = font_of(sid);
            if let Some((pc, psid)) = prev {
                if psid == sid {
                    x += f.kern(pc, c) * scale;
                }
            }
            x += (f.advance(c) + styles[sid].kerning) * scale;
            prev = Some((c, sid));
        }
        x
    };
    let space_width = |sid: usize| -> f32 { (font_of(sid).space_width() + styles[sid].kerning) * scale };

    // line breaking
    struct Line {
        atoms: Vec<usize>,
        hard_break: bool,
    }
    let mut lines: Vec<Line> = vec![Line { atoms: Vec::new(), hard_break: false }];
    let mut x = 0.0f32;
    let mut pending_space = 0.0f32;
    for (i, a) in atoms.iter().enumerate() {
        match a {
            Atom::Break => {
                lines.last_mut().unwrap().hard_break = true;
                lines.push(Line { atoms: Vec::new(), hard_break: false });
                x = 0.0;
                pending_space = 0.0;
            }
            Atom::Space(sid) => {
                if !lines.last().unwrap().atoms.is_empty() {
                    lines.last_mut().unwrap().atoms.push(i);
                    pending_space += space_width(*sid);
                }
                // spaces at a line start are dropped
            }
            Atom::Tab(sid) => {
                let _ = sid;
                lines.last_mut().unwrap().atoms.push(i);
                let tx = ((x + pending_space) / (tab_size * scale)).floor() * (tab_size * scale) + tab_size * scale;
                x = tx;
                pending_space = 0.0;
            }
            Atom::Word(w) => {
                let ww = word_width(w);
                let line_has_words = lines.last().unwrap().atoms.iter().any(|&k| matches!(atoms[k], Atom::Word(_)));
                if let Some(maxw) = wrap {
                    if line_has_words && x + pending_space + ww > maxw + 0.001 {
                        // drop the trailing spaces of the finished line, start a new one
                        let l = lines.last_mut().unwrap();
                        while matches!(l.atoms.last().map(|&k| &atoms[k]), Some(Atom::Space(_))) {
                            l.atoms.pop();
                        }
                        lines.push(Line { atoms: Vec::new(), hard_break: false });
                        x = 0.0;
                        pending_space = 0.0;
                    }
                }
                lines.last_mut().unwrap().atoms.push(i);
                x += pending_space + ww;
                pending_space = 0.0;
            }
        }
    }

    // placement
    let mut out = TextLayout::default();
    let mut y = 0.0f32;
    // per line metrics first (width needed for alignment)
    struct Metrics {
        width: f32,
        height: f32,
        ascent: f32,
        spaces: usize,
    }
    let measure_line = |l: &Line| -> Metrics {
        let mut x = 0.0f32;
        let mut spaces = 0;
        let mut asc = 0.0f32;
        let mut h = 0.0f32;
        let mut sid_last = None;
        for &k in &l.atoms {
            match &atoms[k] {
                Atom::Word(w) => {
                    x += word_width(w);
                    for &(_, sid) in w {
                        let f = font_of(sid);
                        asc = asc.max(f.ascent * scale);
                        h = h.max(f.line_height * scale);
                    }
                }
                Atom::Space(sid) => {
                    x += space_width(*sid);
                    spaces += 1;
                    sid_last = Some(*sid);
                }
                Atom::Tab(_) => {
                    let ts = tab_size * scale;
                    x = (x / ts).floor() * ts + ts;
                }
                Atom::Break => {}
            }
        }
        if h == 0.0 {
            // empty line: use the last/base font height
            let f = sid_last.map(|s| font_of(s)).unwrap_or(used[base_idx]);
            h = f.line_height * scale;
            asc = f.ascent * scale;
        }
        Metrics { width: x, height: h, ascent: asc, spaces }
    };
    let metrics: Vec<Metrics> = lines.iter().map(measure_line).collect();
    let widest = metrics.iter().map(|m| m.width).fold(0.0f32, f32::max);
    let box_w = wrap.unwrap_or(widest);
    let nlines = lines.len();
    for (li, (l, m)) in lines.iter().zip(&metrics).enumerate() {
        let start = out.glyphs.len();
        let is_last = li + 1 == nlines || l.hard_break;
        let extra_per_space = if justify && !is_last && m.spaces > 0 && wrap.is_some() { ((box_w - m.width) / m.spaces as f32).max(0.0) } else { 0.0 };
        let line_w = m.width + extra_per_space * m.spaces as f32;
        let mut px = opts.align.origin(match opts.align {
            Align::Left => 0.0,
            Align::Center => box_w * 0.5,
            Align::Right => box_w,
        }, line_w);
        let baseline = y + m.ascent;
        let mut line_text = String::new();
        for &k in &l.atoms {
            match &atoms[k] {
                Atom::Space(sid) => {
                    px += space_width(*sid) + extra_per_space;
                    line_text.push(' ');
                }
                Atom::Tab(_) => {
                    line_text.push('\t');
                    let ts = tab_size * scale;
                    let rel = px - opts.align.origin(match opts.align { Align::Left => 0.0, Align::Center => box_w * 0.5, Align::Right => box_w }, line_w);
                    px += (rel / ts).floor() * ts + ts - rel;
                }
                Atom::Word(w) => {
                    let mut prev: Option<(char, usize)> = None;
                    for &(c, sid) in w {
                        line_text.push(c);
                        let f = font_of(sid);
                        if let Some((pc, psid)) = prev {
                            if psid == sid {
                                px += f.kern(pc, c) * scale;
                            }
                        }
                        if let Some(g) = f.glyph_or_fallback(c) {
                            let page = f.pages.get(g.page as usize);
                            if let (Some(p), true) = (page, g.w > 0 && g.h > 0) {
                                if let Some(tex) = p.texture {
                                    let (pw, ph) = (p.width as f32, p.height as f32);
                                    out.glyphs.push(PlacedGlyph {
                                        texture: tex,
                                        rect: Rect::new(px + g.xoff as f32 * scale, baseline + g.yoff as f32 * scale, g.w as f32 * scale, g.h as f32 * scale),
                                        uv: [g.x as f32 / pw, g.y as f32 / ph, (g.x + g.w) as f32 / pw, (g.y + g.h) as f32 / ph],
                                        color: styles[sid].color,
                                        ch: c,
                                    });
                                }
                            }
                        }
                        px += (f.advance(c) + styles[sid].kerning) * scale;
                        prev = Some((c, sid));
                    }
                }
                Atom::Break => {}
            }
        }
        out.lines.push(LineInfo { y, height: m.height, width: line_w, glyphs: start..out.glyphs.len(), text: line_text });
        y += m.height + line_extra;
    }
    out.width = box_w;
    out.height = (y - line_extra).max(0.0);
    out
}

/// Plain-text measure (width, height) of a string with the given options.
pub fn measure(fonts: &FontSet, text: &str, opts: &TextOptions, resolve: &dyn Fn(&str) -> Option<String>) -> [f32; 2] {
    let l = layout(fonts, text, opts, resolve);
    l.size()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::test_font;

    fn fonts() -> FontSet {
        let mut f = FontSet::new();
        let mut a = test_font();
        a.name = "Menu".into();
        let mut b = test_font();
        b.name = "Text".into();
        b.line_height = 20.0;
        b.size = 16.0;
        f.insert(a);
        f.insert(b);
        f
    }
    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn single_line_size_and_glyphs() {
        let l = layout(&fonts(), "ab c", &TextOptions::new("Menu"), &none);
        // 7 + 7 + 4 + 7
        assert_eq!(l.width, 25.0);
        assert_eq!(l.height, 10.0);
        assert_eq!(l.glyphs.len(), 3); // the space has no quad
        assert_eq!(l.line_texts(), vec!["ab c"]);
        assert_eq!(l.glyphs[1].rect.x, 7.0);
        assert_eq!(l.glyphs[0].rect.y, 0.0); // baseline 8 + yoff -8
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let o = TextOptions::new("Menu").wrap(40.0);
        // words of 14 px ("ab") separated by 4 px spaces: 14+4+14 = 32, +4+14 = 50 > 40
        let l = layout(&fonts(), "ab ab ab ab", &o, &none);
        assert_eq!(l.line_texts(), vec!["ab ab", "ab ab"]);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.height, 10.0 + 10.0);
        assert_eq!(l.width, 40.0);
    }

    #[test]
    fn overlong_word_gets_own_line_and_hard_breaks() {
        let o = TextOptions::new("Menu").wrap(20.0);
        let l = layout(&fonts(), "abcdef x<br>y\nz", &o, &none);
        assert_eq!(l.line_texts(), vec!["abcdef", "x", "y", "z"]);
    }

    #[test]
    fn alignment_center_and_right() {
        let o = TextOptions::new("Menu").wrap(100.0).align(Align::Right);
        let l = layout(&fonts(), "ab", &o, &none);
        assert_eq!(l.glyphs[0].rect.x, 100.0 - 14.0);
        let o = TextOptions::new("Menu").wrap(100.0).align(Align::Center);
        let l = layout(&fonts(), "ab", &o, &none);
        assert_eq!(l.glyphs[0].rect.x, 50.0 - 7.0);
    }

    #[test]
    fn markup_font_color_tokens_and_verbatim_angle_text() {
        let l = layout(&fonts(), "a<font 1>b</font><color ff0000>c</color>{TOK}", &TextOptions::new("Menu"), &|t| {
            if t == "TOK" { Some("Z".into()) } else { None }
        });
        assert_eq!(l.line_texts(), vec!["abcZ"]);
        assert_eq!(l.glyphs[2].color, Color([255, 0, 0, 255]));
        assert_eq!(l.glyphs[3].color, Color::WHITE);
        // `<font 1>` = "Text" font (line height 20) raises the line height
        assert_eq!(l.height, 20.0);
        let l = layout(&fonts(), "<fix to dynamic>", &TextOptions::new("Menu"), &none);
        assert_eq!(l.line_texts(), vec!["<fix to dynamic>"]);
    }

    #[test]
    fn caps_and_kerning_and_justify() {
        let l = layout(&fonts(), "<caps word>ab</caps> ab", &TextOptions::new("Menu"), &none);
        assert_eq!(l.line_texts(), vec!["AB ab"]);
        let l = layout(&fonts(), "AV", &TextOptions::new("Menu"), &none);
        assert_eq!(l.glyphs[1].rect.x, 5.0); // 7 - 2 kerning
        let o = TextOptions { justify: true, ..TextOptions::new("Menu").wrap(50.0) };
        let l = layout(&fonts(), "ab ab ab ab", &o, &none);
        // first line "ab ab" (14+4+14=32) is stretched to 50: the space grows by 18
        assert!((l.lines[0].width - 50.0).abs() < 1e-3);
        assert!(l.lines[1].width < 50.0 || l.lines.len() == 2);
    }

    #[test]
    fn missing_font_is_empty() {
        let l = layout(&FontSet::new(), "x", &TextOptions::new("Nope"), &none);
        assert!(l.glyphs.is_empty());
    }
}
