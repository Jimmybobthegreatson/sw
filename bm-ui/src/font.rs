//! Bitmap fonts: the engine's pre-rasterised TrueType atlases and mod-supplied AngelCode `.fnt` fonts.
//!
//! # Game format (`ZFONT` node 0x20004c -> PRM chunk), confidence medium-high
//!
//! The three UI fonts (`Text` = Square 721 14 px, `Menu` = Square 721 Bold Condensed 16 px, `Header` = same 20 px)
//! are baked at build time: each is a `ZFONT` scene node whose last `I32` property indexes a PRM chunk:
//!
//! ```text
//! u32 version (1), u32 8 (?), u32 pixel size, f32 scale (1.0), u32 glyph_count, u32 line_height,
//! u32 kerning_count, u32 page_count,
//! glyph_count x { u32 codepoint,
//!                 u32 page_chunk,                 // PRM chunk [0x20101, tex_id, 0, 5, ..] -> TEX image id
//!                 u8 x0, y0, x1, y1,              // atlas rectangle (128x128 pages)
//!                 u8 w, h, i8 xoff, i8 yoff,      // w/h repeat x1-x0 / y1-y0; offsets from pen / baseline
//!                 u8 advance, u8 line, u8 0, 0xcd },
//! kerning_count x { u16 second, u16 first, i32 delta }   // plus one trailing word
//! ```
//!
//! Page images are RGBA with the glyph shape in the alpha channel (white colour).
//!
//! # Mod format
//!
//! `Fonts/<Name>.fnt` in AngelCode BMFont *text* format (`info`, `common`, `page`, `char`, `kerning` lines) with the
//! page images next to it (PNG). A mod font with the same name as a game font replaces it.

use std::collections::HashMap;

use bm_vfs::Vfs;

use crate::texture::{Image, TextureId, TextureStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub page: u16,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// Offset of the bitmap's left edge from the pen position.
    pub xoff: i16,
    /// Offset of the bitmap's top edge from the baseline (negative = above).
    pub yoff: i16,
    pub advance: i16,
}

#[derive(Clone, Debug)]
pub struct FontPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub label: String,
    /// Filled by [`BitmapFont::register`].
    pub texture: Option<TextureId>,
}

#[derive(Clone, Debug)]
pub struct BitmapFont {
    pub name: String,
    pub size: f32,
    pub line_height: f32,
    /// Distance from the top of a line to the baseline.
    pub ascent: f32,
    pub glyphs: HashMap<u32, Glyph>,
    pub kerning: HashMap<(u32, u32), i16>,
    pub pages: Vec<FontPage>,
}

impl BitmapFont {
    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        self.glyphs.get(&(c as u32))
    }

    /// Glyph for `c`, falling back to `?` and then to the space.
    pub fn glyph_or_fallback(&self, c: char) -> Option<&Glyph> {
        self.glyph(c).or_else(|| self.glyph('?')).or_else(|| self.glyph(' '))
    }

    pub fn advance(&self, c: char) -> f32 {
        self.glyph_or_fallback(c).map(|g| g.advance as f32).unwrap_or(self.size * 0.5)
    }

    pub fn kern(&self, a: char, b: char) -> f32 {
        self.kerning.get(&(a as u32, b as u32)).copied().unwrap_or(0) as f32
    }

    pub fn has(&self, c: char) -> bool {
        self.glyphs.contains_key(&(c as u32))
    }

    pub fn space_width(&self) -> f32 {
        self.glyph(' ').map(|g| g.advance as f32).unwrap_or(self.size * 0.3)
    }

    /// Uploads the pages to `store` and remembers their ids.
    pub fn register(&mut self, store: &mut TextureStore) {
        for p in &mut self.pages {
            let mut img = Image::new(p.width, p.height, p.rgba.clone(), p.label.clone());
            img.nearest = false;
            p.texture = Some(match p.texture {
                Some(id) => {
                    store.replace(id, img);
                    id
                }
                None => store.add_or_replace(img),
            });
        }
    }

    /// Width of `s` on one line (no markup, kerning included).
    pub fn text_width(&self, s: &str) -> f32 {
        let mut w = 0.0;
        let mut prev: Option<char> = None;
        for c in s.chars() {
            if let Some(p) = prev {
                w += self.kern(p, c);
            }
            w += self.advance(c);
            prev = Some(c);
        }
        w
    }

    fn compute_ascent(&mut self) {
        let mut best = 0i32;
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars() {
            if let Some(g) = self.glyph(c) {
                best = best.max(-(g.yoff as i32));
            }
        }
        if best == 0 {
            best = self.glyphs.values().map(|g| -(g.yoff as i32)).max().unwrap_or(self.size as i32).max(1);
        }
        self.ascent = best as f32;
    }

    /// Decodes the engine's font chunk (see module docs). `page` maps a page chunk index to its image.
    pub fn from_chunk_words(name: &str, w: &[u32], mut page: impl FnMut(u32) -> Option<FontPage>) -> Option<BitmapFont> {
        if w.len() < 8 || w[0] != 1 {
            return None;
        }
        let (size, nglyph, line, nkern) = (w[2], w[4] as usize, w[5], w[6] as usize);
        let need = 8 + nglyph * 5 + nkern * 2;
        if w.len() < need {
            return None;
        }
        let mut page_map: Vec<(u32, u16)> = Vec::new();
        let mut pages: Vec<FontPage> = Vec::new();
        let mut glyphs = HashMap::new();
        for i in 0..nglyph {
            let r = &w[8 + i * 5..8 + i * 5 + 5];
            let pc = r[1];
            let pidx = match page_map.iter().find(|(c, _)| *c == pc) {
                Some((_, i)) => *i,
                None => {
                    let p = page(pc)?;
                    pages.push(p);
                    page_map.push((pc, pages.len() as u16 - 1));
                    pages.len() as u16 - 1
                }
            };
            let b2 = r[2].to_le_bytes();
            let b3 = r[3].to_le_bytes();
            let b4 = r[4].to_le_bytes();
            glyphs.insert(
                r[0],
                Glyph {
                    page: pidx,
                    x: b2[0] as u16,
                    y: b2[1] as u16,
                    w: b3[0] as u16,
                    h: b3[1] as u16,
                    xoff: b3[2] as i8 as i16,
                    yoff: b3[3] as i8 as i16,
                    advance: b4[0] as i16,
                },
            );
        }
        let mut kerning = HashMap::new();
        let kbase = 8 + nglyph * 5;
        for i in 0..nkern {
            let pair = w[kbase + i * 2];
            let delta = w[kbase + i * 2 + 1] as i32;
            kerning.insert((pair & 0xffff, pair >> 16), delta as i16);
        }
        let mut f = BitmapFont {
            name: name.to_string(),
            size: size as f32,
            line_height: line as f32,
            ascent: 0.0,
            glyphs,
            kerning,
            pages,
        };
        f.compute_ascent();
        Some(f)
    }

    /// Parses AngelCode BMFont *text* format. `load_page(file)` returns the decoded page image.
    pub fn from_fnt_text(name: &str, text: &str, mut load_page: impl FnMut(&str) -> Option<FontPage>) -> Option<BitmapFont> {
        let mut size = 0.0f32;
        let mut line_height = 0.0f32;
        let mut base = 0.0f32;
        let mut glyphs = HashMap::new();
        let mut kerning = HashMap::new();
        let mut pages: Vec<(usize, FontPage)> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            let (tag, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let kv = parse_kv(rest);
            let num = |k: &str| kv.get(k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
            match tag {
                "info" => size = num("size").unsigned_abs() as f32,
                "common" => {
                    line_height = num("lineHeight") as f32;
                    base = num("base") as f32;
                }
                "page" => {
                    let id = num("id") as usize;
                    let file = kv.get("file")?.clone();
                    pages.push((id, load_page(&file)?));
                }
                "char" => {
                    let id = num("id") as u32;
                    glyphs.insert(
                        id,
                        Glyph {
                            page: num("page") as u16,
                            x: num("x") as u16,
                            y: num("y") as u16,
                            w: num("width") as u16,
                            h: num("height") as u16,
                            xoff: num("xoffset") as i16,
                            // BMFont yoffset is from the line top; ours from the baseline.
                            yoff: (num("yoffset") as f32 - base) as i16,
                            advance: num("xadvance") as i16,
                        },
                    );
                }
                "kerning" => {
                    kerning.insert((num("first") as u32, num("second") as u32), num("amount") as i16);
                }
                _ => {}
            }
        }
        if glyphs.is_empty() || pages.is_empty() {
            return None;
        }
        pages.sort_by_key(|(id, _)| *id);
        Some(BitmapFont {
            name: name.to_string(),
            size: if size > 0.0 { size } else { line_height },
            line_height,
            ascent: base,
            glyphs,
            kerning,
            pages: pages.into_iter().map(|(_, p)| p).collect(),
        })
    }

    /// Serialises the glyph table as BMFont text (page files are named `<name>_<n>.png`). Used by tests and by
    /// tooling that wants to dump the game's fonts for editing.
    pub fn to_fnt_text(&self) -> String {
        let mut s = format!(
            "info face=\"{}\" size={}\ncommon lineHeight={} base={} pages={}\n",
            self.name,
            self.size,
            self.line_height,
            self.ascent,
            self.pages.len()
        );
        for (i, _) in self.pages.iter().enumerate() {
            s.push_str(&format!("page id={i} file=\"{}_{i}.png\"\n", self.name));
        }
        let mut codes: Vec<_> = self.glyphs.keys().copied().collect();
        codes.sort_unstable();
        s.push_str(&format!("chars count={}\n", codes.len()));
        for c in codes {
            let g = &self.glyphs[&c];
            s.push_str(&format!(
                "char id={c} x={} y={} width={} height={} xoffset={} yoffset={} xadvance={} page={}\n",
                g.x,
                g.y,
                g.w,
                g.h,
                g.xoff,
                g.yoff as f32 + self.ascent,
                g.advance,
                g.page
            ));
        }
        let mut k: Vec<_> = self.kerning.iter().collect();
        k.sort();
        s.push_str(&format!("kernings count={}\n", k.len()));
        for ((a, b), d) in k {
            s.push_str(&format!("kerning first={a} second={b} amount={d}\n"));
        }
        s
    }
}

fn parse_kv(s: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_string();
        let after = &rest[eq + 1..];
        let (val, tail) = if let Some(q) = after.strip_prefix('"') {
            match q.find('"') {
                Some(e) => (q[..e].to_string(), &q[e + 1..]),
                None => (q.to_string(), ""),
            }
        } else {
            match after.find(char::is_whitespace) {
                Some(e) => (after[..e].to_string(), &after[e..]),
                None => (after.to_string(), ""),
            }
        };
        m.insert(key, val);
        rest = tail.trim_start();
    }
    m
}

/// Named fonts plus the `<font N>` numbering used by LOC markup.
#[derive(Clone, Debug, Default)]
pub struct FontSet {
    fonts: HashMap<String, BitmapFont>,
    default: String,
    /// `<font N>` (1-based) -> font name. Assumption (low confidence): 1 Text, 2 Menu, 3 Header, then repeating
    /// the body font; mods can override with [`FontSet::set_numbered`].
    numbered: Vec<String>,
}

impl FontSet {
    pub fn new() -> FontSet {
        FontSet::default()
    }

    pub fn insert(&mut self, font: BitmapFont) {
        let key = font.name.to_ascii_lowercase();
        if self.default.is_empty() {
            self.default = key.clone();
        }
        self.fonts.insert(key, font);
    }

    pub fn get(&self, name: &str) -> Option<&BitmapFont> {
        self.fonts.get(&name.to_ascii_lowercase())
    }

    /// Font by name, then the default font.
    pub fn get_or_default(&self, name: &str) -> Option<&BitmapFont> {
        self.get(name).or_else(|| self.fonts.get(&self.default))
    }

    pub fn set_default(&mut self, name: &str) {
        self.default = name.to_ascii_lowercase();
    }

    pub fn default_name(&self) -> &str {
        &self.default
    }

    pub fn set_numbered(&mut self, names: &[&str]) {
        self.numbered = names.iter().map(|s| s.to_ascii_lowercase()).collect();
    }

    /// Font for `<font N>` markup (1-based).
    pub fn numbered(&self, n: usize) -> Option<&BitmapFont> {
        let name = if self.numbered.is_empty() {
            const DEFAULT: [&str; 4] = ["text", "menu", "header", "text"];
            DEFAULT.get(n.saturating_sub(1)).copied().unwrap_or("text")
        } else {
            self.numbered.get(n.saturating_sub(1)).map(String::as_str).unwrap_or("text")
        };
        self.get(name).or_else(|| self.fonts.get(&self.default))
    }

    pub fn names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.fonts.values().map(|f| f.name.as_str()).collect();
        v.sort_unstable();
        v
    }

    pub fn register(&mut self, store: &mut TextureStore) {
        for f in self.fonts.values_mut() {
            f.register(store);
        }
    }

    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// Loads loose BMFont fonts: every `Fonts/*.fnt` in the VFS, replacing same-named fonts.
    /// Returns the names that were (re)loaded.
    pub fn load_mod_fonts(&mut self, vfs: &Vfs) -> Vec<String> {
        let mut loaded = Vec::new();
        for e in vfs.list("Fonts/*.fnt") {
            let Ok(bytes) = vfs.open(&e.path) else { continue };
            let text = crate::xml::decode_text(&bytes);
            let stem = e.path.rsplit(['/', '\\']).next().unwrap_or(&e.path).trim_end_matches(".fnt").trim_end_matches(".FNT");
            let dir = e.path.rsplit_once(['/', '\\']).map(|(d, _)| d.to_string()).unwrap_or_default();
            let font = BitmapFont::from_fnt_text(stem, &text, |file| {
                let p = if dir.is_empty() { file.to_string() } else { format!("{dir}/{file}") };
                let data = vfs.open(&p).ok()?;
                let (w, h, rgba) = crate::image_io::decode_png(&data).ok()?;
                Some(FontPage { width: w, height: h, rgba, label: format!("font:{stem}:{file}"), texture: None })
            });
            match font {
                Some(f) => {
                    log::info!("mod font '{}' loaded from {}", f.name, e.path);
                    loaded.push(f.name.clone());
                    // keep the texture ids of a replaced font so existing batches stay valid
                    let old_tex: Vec<Option<TextureId>> =
                        self.get(&f.name).map(|o| o.pages.iter().map(|p| p.texture).collect()).unwrap_or_default();
                    let mut f = f;
                    for (p, t) in f.pages.iter_mut().zip(old_tex) {
                        p.texture = t;
                    }
                    self.insert(f);
                }
                None => log::warn!("mod font {} could not be parsed", e.path),
            }
        }
        loaded
    }
}

#[cfg(test)]
pub(crate) fn test_font() -> BitmapFont {
    // A tiny synthetic font: every glyph is a 6x8 cell, advance 7; 'A' 'V' kern -2.
    let mut glyphs = HashMap::new();
    for (i, c) in (32u32..127).enumerate() {
        let col = (i % 16) as u16;
        let row = (i / 16) as u16;
        glyphs.insert(c, Glyph { page: 0, x: col * 8, y: row * 10, w: 6, h: 8, xoff: 0, yoff: -8, advance: if c == 32 { 4 } else { 7 } });
    }
    let mut kerning = HashMap::new();
    kerning.insert(('A' as u32, 'V' as u32), -2);
    BitmapFont {
        name: "Test".into(),
        size: 8.0,
        line_height: 10.0,
        ascent: 8.0,
        glyphs,
        kerning,
        pages: vec![FontPage { width: 128, height: 64, rgba: vec![255; 128 * 64 * 4], label: "test".into(), texture: Some(TextureId(1)) }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_decode() {
        // header + 2 glyphs + 1 kerning pair
        let w = vec![
            1, 8, 14, 0x3f800000, 2, 15, 1, 1, //
            0x20, 0xa61, 0, 0x01ff0000, 0xcd000f04, //
            0x21, 0xa61, 0x215a1657, 0xf6010b03, 0xcd000f04, //
            0x00310044, 0xffffffff,
        ];
        let f = BitmapFont::from_chunk_words("Text", &w, |_| {
            Some(FontPage { width: 2, height: 2, rgba: vec![0; 16], label: "p".into(), texture: None })
        })
        .unwrap();
        let g = f.glyph('!').unwrap();
        assert_eq!((g.x, g.y, g.w, g.h, g.xoff, g.yoff, g.advance), (0x57, 0x16, 3, 11, 1, -10, 4));
        assert_eq!(f.glyph(' ').unwrap().advance, 4);
        assert_eq!(f.pages.len(), 1);
        assert_eq!(f.kerning.get(&(0x44, 0x31)), Some(&-1));
        assert_eq!(f.line_height, 15.0);
        assert!(BitmapFont::from_chunk_words("x", &[2, 0, 0, 0, 0, 0, 0, 0], |_| None).is_none());
    }

    #[test]
    fn fnt_round_trip() {
        let f = test_font();
        let txt = f.to_fnt_text();
        let g = BitmapFont::from_fnt_text("Test", &txt, |_| {
            Some(FontPage { width: 128, height: 64, rgba: vec![255; 128 * 64 * 4], label: "p".into(), texture: None })
        })
        .unwrap();
        assert_eq!(g.glyphs.len(), f.glyphs.len());
        assert_eq!(g.glyph('A'), f.glyph('A'));
        assert_eq!(g.kern('A', 'V'), -2.0);
        assert_eq!(g.line_height, 10.0);
    }

    #[test]
    fn widths_and_fallback() {
        let f = test_font();
        assert_eq!(f.text_width("AV"), 7.0 + 7.0 - 2.0);
        assert_eq!(f.text_width("a b"), 7.0 + 4.0 + 7.0);
        assert_eq!(f.advance('\u{4e2d}'), 7.0); // falls back to '?'
    }
}
