//! Light / environment records.
//!
//! The light nodes of the scene tree (`ZOMNILIGHT 0x800024`, `ZSPOTLIGHT 0x800023`, `ZSPOTLIGHTSQUARE 0x800020`, `0x8000f5`
//! omni clones and the `ZENVIRONMENT 0x80000d` lighting volumes) carry the usual transform plus one `Int32` (property 4, the
//! same slot where a model node stores its prim id). That integer is a **PRM chunk index**: the chunk holds the light
//! parameters. Layout (all little endian, verified on every light of M01/M06 and plausibility-checked on all scenes):
//!
//! ```text
//! +0x00 u32  0x000c0000            record tag
//! +0x04 u32  0, +0x08 u32 0
//! +0x0c u32  low 16 bits = shape (0 spot, 1 omni, 2 square spot, 3/4 environment), high 16 bits = flags (1, 4 ...)
//! +0x10 u32  colour 0x00RRGGBB (environment: 0xAARRGGBB, A = 0xff)
//! +0x14 f32  intensity (colour multiplier, 0.5 .. 2.0)
//! +0x18 u32  projection texture id of window "projector" spots (0x364), else 0
//! +0x1c u32  secondary colour 0x00RRGGBB (shadow / sky colour, 0 = none)
//! +0x20 f32  range start  (full intensity inside, cm)          environment: second colour 0xAARRGGBB (ambient)
//! +0x24 f32  range end    (zero intensity beyond, cm)          environment: flag word (0 / 1)
//! +0x28 f32  1 / (end - start)
//! spot only (64 / 80 byte records):
//! +0x2c f32  outer cone half angle (radians)      +0x30 f32  inner cone half angle
//! +0x34 f32  cos(outer)                           +0x38 f32  cos(inner)
//! +0x3c f32  1 / (cos(inner) - cos(outer))
//! square spot only (80 bytes): +0x40 f32 aspect / projection scale (0.6 .. 1.5), +0x44.. zero
//! ```
//!
//! Colours are authored in display (gamma) space: the original engine lit in gamma space, so the renderer multiplies them with
//! the display-space albedo directly. Light direction = the node's local +Z axis in world space (verified: +Z points downwards
//! for 62/114 spots and 92/95 environments of M06, and the sun environment of M01 agrees with the backdrop sun sprite to 4 degrees).

use crate::io::{FormatError, Result};

pub const LIGHT_TAG: u32 = 0x000c_0000;

/// Node type ids that carry a light record.
pub const TYPE_OMNI: u32 = 0x800024;
pub const TYPE_SPOT: u32 = 0x800023;
pub const TYPE_SPOT_SQUARE: u32 = 0x800020;
pub const TYPE_OMNI_CLONE: u32 = 0x8000f5;
pub const TYPE_ENVIRONMENT: u32 = 0x80000d;

pub fn is_light_type(t: u32) -> bool {
    matches!(t, TYPE_OMNI | TYPE_SPOT | TYPE_SPOT_SQUARE | TYPE_OMNI_CLONE)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    /// Outer / inner half angle (radians).
    pub outer: f32,
    pub inner: f32,
    pub cos_outer: f32,
    pub cos_inner: f32,
    /// `1 / (cos_inner - cos_outer)`.
    pub inv_cos_range: f32,
}

/// One decoded light or environment record. Unknown words stay available in `raw`.
#[derive(Clone, Debug, PartialEq)]
pub struct LightRecord {
    /// `word3 & 0xffff`.
    pub shape: u16,
    /// `word3 >> 16`.
    pub flags: u16,
    /// RGB bytes of the main colour.
    pub color: [u8; 3],
    pub intensity: f32,
    /// Texture id projected by "projector" spots (0 = none).
    pub projection_texture: u32,
    /// Secondary colour bytes (`word7`), `None` when 0.
    pub color2: Option<[u8; 3]>,
    pub range_start: f32,
    pub range_end: f32,
    pub inv_range: f32,
    pub cone: Option<Cone>,
    /// Square spots: the extra float at +0x40.
    pub aspect: Option<f32>,
    /// Environment records: ambient colour (`word8`) and flag word (`word9`).
    pub ambient: Option<[u8; 3]>,
    pub env_flag: u32,
    pub raw: Vec<u32>,
}

fn rgb(w: u32) -> [u8; 3] {
    [(w >> 16) as u8, (w >> 8) as u8, w as u8]
}

impl LightRecord {
    /// Parses a PRM chunk. `environment` selects the `ZENVIRONMENT` interpretation of words 8 / 9.
    pub fn parse(chunk: &[u8], environment: bool) -> Result<LightRecord> {
        if chunk.len() < 0x30 {
            return Err(FormatError::Eof("light record"));
        }
        let w: Vec<u32> = chunk.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if w[0] != LIGHT_TAG {
            return Err(FormatError::Invalid(format!("light record tag {:#x}", w[0])));
        }
        let f = |i: usize| f32::from_bits(w[i]);
        let color2 = if w[7] & 0x00ff_ffff != 0 { Some(rgb(w[7])) } else { None };
        let mut r = LightRecord {
            shape: w[3] as u16,
            flags: (w[3] >> 16) as u16,
            color: rgb(w[4]),
            intensity: f(5),
            projection_texture: w[6],
            color2,
            range_start: 0.0,
            range_end: 0.0,
            inv_range: 0.0,
            cone: None,
            aspect: None,
            ambient: None,
            env_flag: 0,
            raw: w.clone(),
        };
        if environment {
            r.ambient = Some(rgb(w[8]));
            r.env_flag = w[9];
            return Ok(r);
        }
        r.range_start = f(8);
        r.range_end = f(9);
        r.inv_range = f(10);
        if w.len() >= 16 {
            r.cone = Some(Cone { outer: f(11), inner: f(12), cos_outer: f(13), cos_inner: f(14), inv_cos_range: f(15) });
        }
        if w.len() >= 17 {
            r.aspect = Some(f(16));
        }
        Ok(r)
    }

    pub fn is_spot(&self) -> bool {
        self.cone.is_some()
    }

    /// Distance attenuation as the record defines it: 1 up to `range_start`, falling linearly to 0 at `range_end`.
    pub fn attenuation(&self, dist: f32) -> f32 {
        ((self.range_end - dist) * self.inv_range).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn omni_record() {
        let b = rec(&[LIGHT_TAG, 0, 0, 1, 0x00ffe8a9, 1.0395f32.to_bits(), 0, 0, 54f32.to_bits(), 246f32.to_bits(), (1.0f32 / 192.0).to_bits(), 0]);
        let r = LightRecord::parse(&b, false).unwrap();
        assert_eq!(r.shape, 1);
        assert_eq!(r.color, [255, 232, 169]);
        assert!((r.attenuation(54.0) - 1.0).abs() < 1e-5);
        assert!((r.attenuation(150.0) - 0.5).abs() < 1e-3);
        assert_eq!(r.attenuation(300.0), 0.0);
        assert!(r.cone.is_none());
    }

    #[test]
    fn spot_and_environment() {
        let mut w = vec![LIGHT_TAG, 0, 0, 0, 0x00ffeca9, 1.2f32.to_bits(), 0, 0, 170f32.to_bits(), 560f32.to_bits(), (1.0f32 / 390.0).to_bits()];
        for v in [1.4835298f32, 0.87266463, 0.087155804, 0.64278764, 1.7997528] {
            w.push(v.to_bits());
        }
        let r = LightRecord::parse(&rec(&w), false).unwrap();
        let c = r.cone.unwrap();
        assert!((c.cos_outer - c.outer.cos()).abs() < 1e-4 && (c.cos_inner - c.inner.cos()).abs() < 1e-4);
        assert!((c.inv_cos_range - 1.0 / (c.cos_inner - c.cos_outer)).abs() < 1e-2);
        let e = LightRecord::parse(&rec(&[LIGHT_TAG, 0, 0, 4, 0xffffeabc, 1.1f32.to_bits(), 0, 0x83969f, 0xff596868, 1, 0, 0]), true).unwrap();
        assert_eq!(e.color, [255, 234, 188]);
        assert_eq!(e.ambient, Some([89, 104, 104]));
        assert_eq!(e.color2, Some([0x83, 0x96, 0x9f]));
    }

    #[test]
    fn rejects_other_chunks() {
        assert!(LightRecord::parse(&rec(&[0x70100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), false).is_err());
        assert!(LightRecord::parse(&[0u8; 8], false).is_err());
    }
}
