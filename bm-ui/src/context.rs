//! [`UiContext`]: the bundle of resources every UI drawing function needs (textures, fonts, strings, sprites,
//! input labels), plus text helpers that resolve button tokens against the current bindings.

use std::collections::HashMap;

use crate::draw::DrawList;
use crate::font::FontSet;
use crate::input::{Action, InputMap};
use crate::sprite::Sprite;
use crate::strings::Strings;
use crate::text::{self, TextLayout, TextOptions};
use crate::texture::TextureStore;
use crate::types::Color;

#[derive(Default)]
pub struct UiContext {
    pub textures: TextureStore,
    pub fonts: FontSet,
    pub strings: Strings,
    pub input_map: InputMap,
    sprites: HashMap<String, Sprite>,
}

impl UiContext {
    pub fn new() -> UiContext {
        UiContext::default()
    }

    pub fn add_sprite(&mut self, name: &str, s: Sprite) {
        self.sprites.insert(name.to_ascii_lowercase(), s);
    }

    pub fn sprite(&self, name: &str) -> Option<&Sprite> {
        self.sprites.get(&name.to_ascii_lowercase())
    }

    pub fn sprite_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.sprites.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    /// Resolves `{ZW_SELECT}` / `{Start}`-style tokens: engine ids map to the label of the bound key or button
    /// (`[Enter]`); anything else is dropped.
    pub fn token(&self, tok: &str) -> Option<String> {
        let t = tok.trim();
        if let Some(a) = Action::from_name(t) {
            return Some(format!("[{}]", self.input_map.label(a)));
        }
        match t.to_ascii_lowercase().as_str() {
            "start" => Some("[Start]".into()),
            "bullet_square" => Some("\u{2022}".into()),
            _ => None,
        }
    }

    /// Localised text of a key (with markup expansion) laid out with the given options.
    pub fn layout_key(&self, key: &str, opts: &TextOptions) -> TextLayout {
        self.layout_text(&self.strings.expand_key(key), opts)
    }

    /// Lays out already-expanded markup text.
    pub fn layout_text(&self, text: &str, opts: &TextOptions) -> TextLayout {
        text::layout(&self.fonts, text, opts, &|t| self.token(t))
    }

    pub fn measure(&self, text: &str, opts: &TextOptions) -> [f32; 2] {
        self.layout_text(text, opts).size()
    }

    /// Draws a string at `pos` (top-left) in one call.
    pub fn draw_text(&self, dl: &mut DrawList, text: &str, pos: [f32; 2], opts: &TextOptions) -> TextLayout {
        let l = self.layout_text(text, opts);
        dl.text(&l, pos, Color::WHITE);
        l
    }
}
