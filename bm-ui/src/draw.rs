//! Renderer-agnostic 2D sprite/quad batcher.
//!
//! A [`DrawList`] collects textured, tinted triangles in *screen pixels* (y down) and groups consecutive
//! quads that share texture/blend/clip into one [`DrawCmd`]. The wgpu back end ([`crate::gpu::UiRenderer`])
//! uploads it as one vertex/index buffer pair and issues one draw per command; tests inspect it directly.
//!
//! A uniform transform (scale + offset) can be pushed so that UI authored for a virtual canvas (1024x768 for
//! the original menus) is mapped to the real screen with letter/pillar-boxing - see [`Viewport`].

use std::ops::Range;

use bytemuck::{Pod, Zeroable};

use crate::font::BitmapFont;
use crate::text::TextLayout;
use crate::texture::TextureId;
use crate::types::{Color, Rect};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    /// Straight-alpha tint, 0..1 (sRGB-authored values).
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    /// `src.a * src + (1 - src.a) * dst` (`DRAWMODE_OPACITY` / normal UI).
    Alpha,
    /// `src.a * src + dst` (`DRAWMODE_ADD`, glows).
    Additive,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrawCmd {
    pub texture: TextureId,
    pub blend: Blend,
    pub indices: Range<u32>,
    /// Scissor rectangle in physical pixels `(x, y, w, h)`.
    pub clip: Option<[u32; 4]>,
}

/// Uniform 2D transform: `p' = p * scale + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub scale: f32,
    pub offset: [f32; 2],
}

impl Transform {
    pub const IDENTITY: Transform = Transform { scale: 1.0, offset: [0.0, 0.0] };
    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] * self.scale + self.offset[0], p[1] * self.scale + self.offset[1]]
    }
    pub fn apply_rect(&self, r: Rect) -> Rect {
        Rect::new(r.x * self.scale + self.offset[0], r.y * self.scale + self.offset[1], r.w * self.scale, r.h * self.scale)
    }
    /// `self` followed by `inner` applied first: `p' = self(inner(p))`.
    pub fn compose(&self, inner: &Transform) -> Transform {
        Transform {
            scale: self.scale * inner.scale,
            offset: [inner.offset[0] * self.scale + self.offset[0], inner.offset[1] * self.scale + self.offset[1]],
        }
    }
}

/// Maps a virtual design canvas onto the screen, preserving its aspect ratio (centred, bars on the rest).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub screen: [f32; 2],
    pub canvas: [f32; 2],
}

impl Viewport {
    pub fn new(screen_w: f32, screen_h: f32, canvas_w: f32, canvas_h: f32) -> Viewport {
        Viewport { screen: [screen_w, screen_h], canvas: [canvas_w, canvas_h] }
    }

    pub fn transform(&self) -> Transform {
        let s = (self.screen[0] / self.canvas[0]).min(self.screen[1] / self.canvas[1]);
        Transform {
            scale: s,
            offset: [(self.screen[0] - self.canvas[0] * s) * 0.5, (self.screen[1] - self.canvas[1] * s) * 0.5],
        }
    }

    /// Screen position -> canvas position (for mouse hit testing).
    pub fn to_canvas(&self, p: [f32; 2]) -> [f32; 2] {
        let t = self.transform();
        [(p[0] - t.offset[0]) / t.scale, (p[1] - t.offset[1]) / t.scale]
    }
}

#[derive(Clone, Debug, Default)]
pub struct DrawList {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub cmds: Vec<DrawCmd>,
    transform: Option<Transform>,
    transform_stack: Vec<Option<Transform>>,
    clip: Option<[u32; 4]>,
    clip_stack: Vec<Option<[u32; 4]>>,
}

impl DrawList {
    pub fn new() -> DrawList {
        DrawList::default()
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.cmds.clear();
        self.transform = None;
        self.transform_stack.clear();
        self.clip = None;
        self.clip_stack.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    pub fn transform(&self) -> Transform {
        self.transform.unwrap_or(Transform::IDENTITY)
    }

    /// Pushes `t` *inside* the current transform (so nested pushes compose).
    pub fn push_transform(&mut self, t: Transform) {
        self.transform_stack.push(self.transform);
        self.transform = Some(match self.transform {
            Some(cur) => cur.compose(&t),
            None => t,
        });
    }

    pub fn pop_transform(&mut self) {
        self.transform = self.transform_stack.pop().unwrap_or(None);
    }

    /// Restricts drawing to `r` (in the current transform's space); nested clips intersect.
    pub fn push_clip(&mut self, r: Rect) {
        let pr = self.transform().apply_rect(r);
        let x0 = pr.x.max(0.0).floor();
        let y0 = pr.y.max(0.0).floor();
        let x1 = pr.right().max(0.0).ceil();
        let y1 = pr.bottom().max(0.0).ceil();
        let mut c = [x0 as u32, y0 as u32, (x1 - x0).max(0.0) as u32, (y1 - y0).max(0.0) as u32];
        if let Some(o) = self.clip {
            let nx = c[0].max(o[0]);
            let ny = c[1].max(o[1]);
            let nr = (c[0] + c[2]).min(o[0] + o[2]);
            let nb = (c[1] + c[3]).min(o[1] + o[3]);
            c = [nx, ny, nr.saturating_sub(nx), nb.saturating_sub(ny)];
        }
        self.clip_stack.push(self.clip);
        self.clip = Some(c);
    }

    pub fn pop_clip(&mut self) {
        self.clip = self.clip_stack.pop().unwrap_or(None);
    }

    fn begin(&mut self, texture: TextureId, blend: Blend) {
        let start = self.indices.len() as u32;
        match self.cmds.last_mut() {
            Some(c) if c.texture == texture && c.blend == blend && c.clip == self.clip && c.indices.end == start => {}
            _ => self.cmds.push(DrawCmd { texture, blend, indices: start..start, clip: self.clip }),
        }
    }

    fn push_quad(&mut self, texture: TextureId, blend: Blend, v: [Vertex; 4]) {
        self.begin(texture, blend);
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&v);
        self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        self.cmds.last_mut().unwrap().indices.end = self.indices.len() as u32;
    }

    /// Textured axis-aligned quad. `uv = [u0, v0, u1, v1]`.
    pub fn quad(&mut self, texture: TextureId, r: Rect, uv: [f32; 4], color: Color, blend: Blend) {
        let t = self.transform();
        let c = color.to_f32();
        let (x0, y0) = (r.x * t.scale + t.offset[0], r.y * t.scale + t.offset[1]);
        let (x1, y1) = (r.right() * t.scale + t.offset[0], r.bottom() * t.scale + t.offset[1]);
        self.push_quad(
            texture,
            blend,
            [
                Vertex { pos: [x0, y0], uv: [uv[0], uv[1]], color: c },
                Vertex { pos: [x1, y0], uv: [uv[2], uv[1]], color: c },
                Vertex { pos: [x1, y1], uv: [uv[2], uv[3]], color: c },
                Vertex { pos: [x0, y1], uv: [uv[0], uv[3]], color: c },
            ],
        );
    }

    /// Quad with four arbitrary corners (clockwise from top-left), e.g. rotated compass needles.
    pub fn quad_points(&mut self, texture: TextureId, p: [[f32; 2]; 4], uv: [f32; 4], color: Color, blend: Blend) {
        let t = self.transform();
        let c = color.to_f32();
        let q = p.map(|v| t.apply(v));
        self.push_quad(
            texture,
            blend,
            [
                Vertex { pos: q[0], uv: [uv[0], uv[1]], color: c },
                Vertex { pos: q[1], uv: [uv[2], uv[1]], color: c },
                Vertex { pos: q[2], uv: [uv[2], uv[3]], color: c },
                Vertex { pos: q[3], uv: [uv[0], uv[3]], color: c },
            ],
        );
    }

    /// Solid rectangle.
    pub fn rect(&mut self, r: Rect, color: Color) {
        self.quad(TextureId::WHITE, r, [0.0, 0.0, 1.0, 1.0], color, Blend::Alpha);
    }

    pub fn rect_blend(&mut self, r: Rect, color: Color, blend: Blend) {
        self.quad(TextureId::WHITE, r, [0.0, 0.0, 1.0, 1.0], color, blend);
    }

    /// Rectangle outline of the given thickness (drawn inside `r`).
    pub fn border(&mut self, r: Rect, thickness: f32, color: Color) {
        let t = thickness.min(r.w * 0.5).min(r.h * 0.5);
        self.rect(Rect::new(r.x, r.y, r.w, t), color);
        self.rect(Rect::new(r.x, r.bottom() - t, r.w, t), color);
        self.rect(Rect::new(r.x, r.y + t, t, r.h - 2.0 * t), color);
        self.rect(Rect::new(r.right() - t, r.y + t, t, r.h - 2.0 * t), color);
    }

    /// Vertical gradient rectangle (top colour -> bottom colour).
    pub fn gradient_v(&mut self, r: Rect, top: Color, bottom: Color) {
        let t = self.transform();
        let (x0, y0) = (r.x * t.scale + t.offset[0], r.y * t.scale + t.offset[1]);
        let (x1, y1) = (r.right() * t.scale + t.offset[0], r.bottom() * t.scale + t.offset[1]);
        let (a, b) = (top.to_f32(), bottom.to_f32());
        self.push_quad(
            TextureId::WHITE,
            Blend::Alpha,
            [
                Vertex { pos: [x0, y0], uv: [0.0, 0.0], color: a },
                Vertex { pos: [x1, y0], uv: [1.0, 0.0], color: a },
                Vertex { pos: [x1, y1], uv: [1.0, 1.0], color: b },
                Vertex { pos: [x0, y1], uv: [0.0, 1.0], color: b },
            ],
        );
    }

    /// Radial-ish vignette helper: four dark edge gradients.
    pub fn vignette(&mut self, r: Rect, edge: f32, color: Color) {
        let clear = color.with_alpha(0.0);
        self.gradient_v(Rect::new(r.x, r.y, r.w, edge), color, clear);
        self.gradient_v(Rect::new(r.x, r.bottom() - edge, r.w, edge), clear, color);
    }

    /// Nine-patch: `margin` = (left, top, right, bottom) in source pixels of an `tex_size` texture region
    /// `uv_rect` (u0,v0,u1,v1). Corners keep their size, edges stretch.
    pub fn nine_slice(&mut self, texture: TextureId, dst: Rect, uv: [f32; 4], tex_size: [f32; 2], margin: [f32; 4], color: Color, blend: Blend) {
        let [ml, mt, mr, mb] = margin;
        let xs = [dst.x, dst.x + ml, dst.right() - mr, dst.right()];
        let ys = [dst.y, dst.y + mt, dst.bottom() - mb, dst.bottom()];
        let du = uv[2] - uv[0];
        let dv = uv[3] - uv[1];
        let us = [uv[0], uv[0] + du * ml / tex_size[0], uv[2] - du * mr / tex_size[0], uv[2]];
        let vs = [uv[1], uv[1] + dv * mt / tex_size[1], uv[3] - dv * mb / tex_size[1], uv[3]];
        for j in 0..3 {
            for i in 0..3 {
                let r = Rect::new(xs[i], ys[j], xs[i + 1] - xs[i], ys[j + 1] - ys[j]);
                if r.w > 0.0 && r.h > 0.0 {
                    self.quad(texture, r, [us[i], vs[j], us[i + 1], vs[j + 1]], color, blend);
                }
            }
        }
    }

    /// Draws a laid-out text block with its top-left at `origin`; `tint` multiplies the glyph colours.
    pub fn text(&mut self, layout: &TextLayout, origin: [f32; 2], tint: Color) {
        for g in &layout.glyphs {
            self.quad(g.texture, g.rect.translate(origin[0], origin[1]), g.uv, g.color.mul(tint), Blend::Alpha);
        }
    }

    /// Text with a drop shadow (drawn first, offset by `shadow_offset`).
    pub fn text_shadow(&mut self, layout: &TextLayout, origin: [f32; 2], tint: Color, shadow_offset: [f32; 2], shadow: Color) {
        self.text(layout, [origin[0] + shadow_offset[0], origin[1] + shadow_offset[1]], shadow.with_alpha(tint.0[3] as f32 / 255.0));
        self.text(layout, origin, tint);
    }

    /// Text with a 1-pixel outline (8 neighbours) - used by HUD subtitles on bright backgrounds.
    pub fn text_outline(&mut self, layout: &TextLayout, origin: [f32; 2], tint: Color, outline: Color, w: f32) {
        let o = outline.with_alpha(tint.0[3] as f32 / 255.0);
        for (dx, dy) in [(-w, 0.0), (w, 0.0), (0.0, -w), (0.0, w), (-w, -w), (w, -w), (-w, w), (w, w)] {
            let flat = TextLayout { glyphs: layout.glyphs.iter().map(|g| crate::text::PlacedGlyph { color: Color::WHITE, ..*g }).collect(), ..layout.clone() };
            self.text(&flat, [origin[0] + dx, origin[1] + dy], o);
        }
        self.text(layout, origin, tint);
    }

    /// Appends another list (its own transform/clip state is baked into its vertices already).
    pub fn append(&mut self, other: &DrawList) {
        let vbase = self.vertices.len() as u32;
        let ibase = self.indices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + vbase));
        for c in &other.cmds {
            self.cmds.push(DrawCmd {
                texture: c.texture,
                blend: c.blend,
                indices: c.indices.start + ibase..c.indices.end + ibase,
                clip: c.clip,
            });
        }
    }

    /// Convenience used by examples: measures a single-line string with `font` (no markup).
    pub fn measure(font: &BitmapFont, s: &str) -> f32 {
        font.text_width(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batching_merges_same_state() {
        let mut d = DrawList::new();
        d.rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        d.rect(Rect::new(10.0, 0.0, 10.0, 10.0), Color::BLACK);
        assert_eq!(d.cmds.len(), 1);
        assert_eq!(d.quad_count(), 2);
        d.quad(TextureId(3), Rect::new(0.0, 0.0, 1.0, 1.0), [0.0, 0.0, 1.0, 1.0], Color::WHITE, Blend::Alpha);
        d.rect_blend(Rect::new(0.0, 0.0, 1.0, 1.0), Color::WHITE, Blend::Additive);
        assert_eq!(d.cmds.len(), 3);
        assert_eq!(d.cmds[0].indices, 0..12);
        assert_eq!(d.cmds[1].indices, 12..18);
    }

    #[test]
    fn viewport_letterbox() {
        let v = Viewport::new(1280.0, 720.0, 1024.0, 768.0);
        let t = v.transform();
        assert!((t.scale - 0.9375).abs() < 1e-6);
        assert!((t.offset[0] - 160.0).abs() < 1e-3 && t.offset[1].abs() < 1e-3);
        let c = v.to_canvas([640.0, 360.0]);
        assert!((c[0] - 512.0).abs() < 1e-3 && (c[1] - 384.0).abs() < 1e-3);
        let mut d = DrawList::new();
        d.push_transform(t);
        d.rect(Rect::new(0.0, 0.0, 1024.0, 768.0), Color::WHITE);
        assert_eq!(d.vertices[0].pos, [160.0, 0.0]);
        assert!((d.vertices[2].pos[0] - 1120.0).abs() < 1e-3);
    }

    #[test]
    fn clip_nests_and_scissors() {
        let mut d = DrawList::new();
        d.push_clip(Rect::new(10.0, 10.0, 100.0, 100.0));
        d.push_clip(Rect::new(50.0, 0.0, 100.0, 60.0));
        d.rect(Rect::new(0.0, 0.0, 5.0, 5.0), Color::WHITE);
        assert_eq!(d.cmds[0].clip, Some([50, 10, 60, 50]));
        d.pop_clip();
        d.pop_clip();
        d.rect(Rect::new(0.0, 0.0, 5.0, 5.0), Color::WHITE);
        assert_eq!(d.cmds.len(), 2);
        assert_eq!(d.cmds[1].clip, None);
    }

    #[test]
    fn nine_slice_makes_nine_quads() {
        let mut d = DrawList::new();
        d.nine_slice(TextureId(1), Rect::new(0.0, 0.0, 100.0, 50.0), [0.0, 0.0, 1.0, 1.0], [16.0, 16.0], [4.0, 4.0, 4.0, 4.0], Color::WHITE, Blend::Alpha);
        assert_eq!(d.quad_count(), 9);
    }
}
