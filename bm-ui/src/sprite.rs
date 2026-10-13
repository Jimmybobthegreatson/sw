//! Sprites: multi-tile textured pictures (the engine's `ZWINPIC`).
//!
//! A `ZWINPIC` is stored as a scene node (`0x200046`) whose property tail is `ALIGNMENT_*, DRAWMODE_*, TYPE_*,
//! I32 width, I32 height, I32 prm_chunk`. The PRM chunk lists the tiles the picture was cut into (images are split
//! into power-of-two tiles, each its own texture) - see `docs/ui.md`.

use crate::draw::{Blend, DrawList};
use crate::texture::TextureId;
use crate::types::{Color, Rect};

/// Which point of the sprite the draw position refers to (`ALIGNMENT_*` tokens).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor(pub [f32; 2]);

impl Anchor {
    pub const TOP_LEFT: Anchor = Anchor([0.0, 0.0]);
    pub const CENTER: Anchor = Anchor([0.5, 0.5]);

    /// `ALIGNMENT_Top_Left`, `ALIGNMENT_Left`, `ALIGNMENT_Center`, ...
    pub fn from_token(tok: &str) -> Anchor {
        let t = tok.trim_start_matches("ALIGNMENT_").to_ascii_lowercase();
        let (mut x, mut y) = (0.0, 0.0);
        let has = |s: &str| t.split('_').any(|p| p == s);
        if has("center") {
            x = 0.5;
            y = 0.5;
        }
        if has("left") {
            x = 0.0;
        }
        if has("right") {
            x = 1.0;
        }
        if has("top") {
            y = 0.0;
        }
        if has("bottom") {
            y = 1.0;
        }
        // a lone horizontal token (`Left`, `Right`) means vertically centred
        if !has("top") && !has("bottom") && !has("center") {
            y = 0.5;
        }
        if t == "none" {
            return Anchor::TOP_LEFT;
        }
        Anchor([x, y])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpritePart {
    pub texture: TextureId,
    /// Destination relative to the sprite's top-left corner, y down.
    pub rect: Rect,
    pub uv: [f32; 4],
    pub color: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sprite {
    pub name: String,
    /// Design size in pixels (the picture, not the padded tiles).
    pub size: [f32; 2],
    pub parts: Vec<SpritePart>,
    pub blend: Blend,
    pub anchor: Anchor,
    /// Draw priority byte stored in the node (higher = later); informational.
    pub layer: i32,
}

impl Sprite {
    /// A single-texture sprite covering `size`.
    pub fn from_texture(name: &str, texture: TextureId, size: [f32; 2]) -> Sprite {
        Sprite {
            name: name.to_string(),
            size,
            parts: vec![SpritePart { texture, rect: Rect::new(0.0, 0.0, size[0], size[1]), uv: [0.0, 0.0, 1.0, 1.0], color: Color::WHITE }],
            blend: Blend::Alpha,
            anchor: Anchor::TOP_LEFT,
            layer: 0,
        }
    }

    /// Draws with the anchor point at `pos`. `size` stretches the design size (tiles scale proportionally).
    pub fn draw(&self, dl: &mut DrawList, pos: [f32; 2], size: Option<[f32; 2]>, tint: Color) {
        let target = size.unwrap_or(self.size);
        let (sx, sy) = (
            if self.size[0] > 0.0 { target[0] / self.size[0] } else { 1.0 },
            if self.size[1] > 0.0 { target[1] / self.size[1] } else { 1.0 },
        );
        let ox = pos[0] - self.anchor.0[0] * target[0];
        let oy = pos[1] - self.anchor.0[1] * target[1];
        for p in &self.parts {
            let r = Rect::new(ox + p.rect.x * sx, oy + p.rect.y * sy, p.rect.w * sx, p.rect.h * sy);
            dl.quad(p.texture, r, p.uv, p.color.mul(tint), self.blend);
        }
    }

    /// Bounding box of the tiles in sprite space (may exceed `size` because tiles are padded).
    pub fn bounds(&self) -> Rect {
        let mut it = self.parts.iter().map(|p| p.rect);
        let first = it.next().unwrap_or_default();
        it.fold(first, |a, b| a.union(&b))
    }
}

/// Decodes a `ZWINPIC` PRM chunk (see module docs): `n`, `n x 10 words {x, y (tile centre, y up), u0, v0, u1,
/// v1, 0, w, h, colour}`, `n` again, `n` chunk refs of `[0x20101, tex_id, ..]` image chunks.
pub fn parse_winpic_chunk(w: &[u32], mut image_of_chunk: impl FnMut(u32) -> Option<TextureId>) -> Option<Vec<SpritePart>> {
    let n = *w.first()? as usize;
    if n == 0 || n > 64 || w.len() < 1 + n * 10 + 1 + n || w[1 + n * 10] as usize != n {
        return None;
    }
    let mut parts = Vec::with_capacity(n);
    for i in 0..n {
        let r = &w[1 + i * 10..1 + i * 10 + 10];
        let f = |k: usize| f32::from_bits(r[k]);
        let (cx, cy, tw, th) = (f(0), f(1), f(7), f(8));
        let texture = image_of_chunk(w[2 + n * 10 + i])?;
        let c = r[9];
        parts.push(SpritePart {
            texture,
            // centre-based, y up -> top-left, y down
            rect: Rect::new(cx - tw * 0.5, -cy - th * 0.5, tw, th),
            uv: [f(2), f(3), f(4), f(5)],
            color: Color([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]),
        });
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors() {
        assert_eq!(Anchor::from_token("ALIGNMENT_Top_Left"), Anchor([0.0, 0.0]));
        assert_eq!(Anchor::from_token("ALIGNMENT_Left"), Anchor([0.0, 0.5]));
        assert_eq!(Anchor::from_token("ALIGNMENT_Center"), Anchor([0.5, 0.5]));
        assert_eq!(Anchor::from_token("ALIGNMENT_Bottom_Right"), Anchor([1.0, 1.0]));
        assert_eq!(Anchor::from_token("ALIGNMENT_None"), Anchor([0.0, 0.0]));
    }

    #[test]
    fn winpic_chunk_four_tiles() {
        // ScrollDown (9x11): four tiles 8x8, 4x8, 8x3, 4x3
        let f = |x: f32| x.to_bits();
        let mut w = vec![4u32];
        for (x, y, u1, v1, tw, th) in [(4.0, -4.0, 1.0, 1.0, 8.0, 8.0), (10.0, -4.0, 1.0, 1.0, 4.0, 8.0), (4.0, -9.5, 1.0, 0.75, 8.0, 3.0), (10.0, -9.5, 1.0, 0.75, 4.0, 3.0)] {
            w.extend_from_slice(&[f(x), f(y), 0, 0, f(u1), f(v1), 0, f(tw), f(th), 0xffffffff]);
        }
        w.extend_from_slice(&[4, 0x1cc7, 0x1cc8, 0x1cc9, 0x1cca]);
        let parts = parse_winpic_chunk(&w, |c| Some(TextureId(c - 0x1cc7 + 10))).unwrap();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].rect, Rect::new(0.0, 0.0, 8.0, 8.0));
        assert_eq!(parts[1].rect, Rect::new(8.0, 0.0, 4.0, 8.0));
        assert_eq!(parts[2].rect, Rect::new(0.0, 8.0, 8.0, 3.0));
        assert_eq!(parts[3].texture, TextureId(13));
        assert_eq!(parts[2].uv, [0.0, 0.0, 1.0, 0.75]);
        assert_eq!(parts[0].color, Color::WHITE);
        // malformed
        assert!(parse_winpic_chunk(&w[..20], |_| Some(TextureId(1))).is_none());
    }

    #[test]
    fn draw_applies_anchor_and_stretch() {
        let s = Sprite { anchor: Anchor::CENTER, ..Sprite::from_texture("t", TextureId(2), [10.0, 10.0]) };
        let mut d = DrawList::new();
        s.draw(&mut d, [100.0, 100.0], Some([20.0, 20.0]), Color::WHITE);
        assert_eq!(d.vertices[0].pos, [90.0, 90.0]);
        assert_eq!(d.vertices[2].pos, [110.0, 110.0]);
    }
}
