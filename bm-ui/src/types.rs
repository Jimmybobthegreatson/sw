//! Small shared value types: colours, rectangles, alignment.

use serde::{Deserialize, Serialize};

/// RON options used for all hand-written layouts: `Option` fields may omit `Some(..)`.
pub fn ron_options() -> ron::Options {
    ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
}

/// 8-bit RGBA colour (straight alpha). In RON/JSON layouts it is written as `"#rrggbb"`, `"#rrggbbaa"`,
/// `"0xaarrggbb"` or `[r, g, b, a]`; it serialises as `"#rrggbbaa"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub [u8; 4]);

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let c = self.0;
        s.serialize_str(&format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]))
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Text(String),
            Rgba([u8; 4]),
            Rgb([u8; 3]),
        }
        match Repr::deserialize(d)? {
            Repr::Text(s) => Color::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad colour '{s}'"))),
            Repr::Rgba(c) => Ok(Color(c)),
            Repr::Rgb(c) => Ok(Color([c[0], c[1], c[2], 255])),
        }
    }
}

impl Color {
    pub const WHITE: Color = Color([255, 255, 255, 255]);
    pub const BLACK: Color = Color([0, 0, 0, 255]);
    pub const TRANSPARENT: Color = Color([0, 0, 0, 0]);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color([r, g, b, a])
    }

    pub fn to_f32(self) -> [f32; 4] {
        let c = self.0;
        [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
    }

    pub fn with_alpha(self, a: f32) -> Color {
        let mut c = self.0;
        c[3] = (c[3] as f32 * a.clamp(0.0, 1.0)).round() as u8;
        Color(c)
    }

    /// Component-wise multiply (modulate).
    pub fn mul(self, o: Color) -> Color {
        let f = |a: u8, b: u8| ((a as u32 * b as u32 + 127) / 255) as u8;
        Color([f(self.0[0], o.0[0]), f(self.0[1], o.0[1]), f(self.0[2], o.0[2]), f(self.0[3], o.0[3])])
    }

    pub fn lerp(self, o: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let f = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color([f(self.0[0], o.0[0]), f(self.0[1], o.0[1]), f(self.0[2], o.0[2]), f(self.0[3], o.0[3])])
    }

    /// Parses the notations used by the game data and mods:
    /// `0xAARRGGBB` (Glacier menu XML, alpha first), `#RRGGBB`, `#RRGGBBAA`, `r,g,b[,a]` (0-255), and the
    /// LOC markup `rrggbb`.
    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim();
        if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            let v = u32::from_str_radix(h, 16).ok()?;
            return Some(if h.len() <= 6 {
                Color([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255])
            } else {
                Color([(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8])
            });
        }
        if let Some(h) = s.strip_prefix('#') {
            let v = u32::from_str_radix(h, 16).ok()?;
            return match h.len() {
                6 => Some(Color([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255])),
                8 => Some(Color([(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8])),
                3 | 4 => {
                    let n = |i: usize| (((v >> (4 * (h.len() - 1 - i))) & 0xf) as u8) * 17;
                    Some(Color([n(0), n(1), n(2), if h.len() == 4 { n(3) } else { 255 }]))
                }
                _ => None,
            };
        }
        if s.contains(',') {
            let p: Vec<u8> = s.split(',').map(|x| x.trim().parse::<u8>()).collect::<Result<_, _>>().ok()?;
            return match p.len() {
                3 => Some(Color([p[0], p[1], p[2], 255])),
                4 => Some(Color([p[0], p[1], p[2], p[3]])),
                _ => None,
            };
        }
        if s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            let v = u32::from_str_radix(s, 16).ok()?;
            return Some(Color([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]));
        }
        None
    }
}

impl Default for Color {
    fn default() -> Self {
        Color::WHITE
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, p: [f32; 2]) -> bool {
        p[0] >= self.x && p[0] < self.right() && p[1] >= self.y && p[1] < self.bottom()
    }
    pub fn center(&self) -> [f32; 2] {
        [self.x + self.w * 0.5, self.y + self.h * 0.5]
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        Rect::new(x, y, (self.right().min(o.right()) - x).max(0.0), (self.bottom().min(o.bottom()) - y).max(0.0))
    }
}

/// Horizontal text/element alignment relative to the anchor X.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Align {
    /// The element starts at X.
    #[default]
    Left,
    /// The element is centred on X (or on the window when no X is given).
    Center,
    /// The element ends at X.
    Right,
}

impl Align {
    pub fn parse(s: &str) -> Option<Align> {
        match s.trim().to_ascii_lowercase().as_str() {
            "left" => Some(Align::Left),
            "center" | "centre" | "middle" => Some(Align::Center),
            "right" => Some(Align::Right),
            _ => None,
        }
    }

    /// Left edge of an element of width `w` anchored at `x`.
    pub fn origin(self, x: f32, w: f32) -> f32 {
        match self {
            Align::Left => x,
            Align::Center => x - w * 0.5,
            Align::Right => x - w,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_formats() {
        assert_eq!(Color::parse("0xffececec"), Some(Color([0xec, 0xec, 0xec, 0xff])));
        assert_eq!(Color::parse("0xaa112233"), Some(Color([0x11, 0x22, 0x33, 0xaa])));
        assert_eq!(Color::parse("236,236,236,255"), Some(Color([236, 236, 236, 255])));
        assert_eq!(Color::parse("#ff8000"), Some(Color([255, 128, 0, 255])));
        assert_eq!(Color::parse("ffffff"), Some(Color::WHITE));
        assert_eq!(Color::parse("#f80"), Some(Color([255, 136, 0, 255])));
        assert_eq!(Color::parse("#f808"), Some(Color([255, 136, 0, 136])));
        assert_eq!(Color::parse("nope"), None);
    }

    #[test]
    fn color_serde_forms() {
        let c: Color = serde_json::from_str("\"#ff8000\"").unwrap();
        assert_eq!(c, Color([255, 128, 0, 255]));
        let c: Color = serde_json::from_str("[1,2,3,4]").unwrap();
        assert_eq!(c, Color([1, 2, 3, 4]));
        let c: Color = ron::from_str("\"0x80ff0000\"").unwrap();
        assert_eq!(c, Color([255, 0, 0, 128]));
        let back: Color = ron::from_str(&ron::to_string(&Color([9, 8, 7, 6])).unwrap()).unwrap();
        assert_eq!(back, Color([9, 8, 7, 6]));
    }

    #[test]
    fn rect_ops() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains([5.0, 5.0]) && !r.contains([10.0, 5.0]));
        assert_eq!(r.intersect(&Rect::new(5.0, 5.0, 10.0, 10.0)), Rect::new(5.0, 5.0, 5.0, 5.0));
        assert_eq!(Align::Right.origin(100.0, 30.0), 70.0);
        assert_eq!(Align::Center.origin(100.0, 30.0), 85.0);
    }
}
