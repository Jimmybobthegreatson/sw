//! Procedural sprite images for authored emitters (no game assets needed).

use crate::desc::BuiltinSprite;
use crate::rng::Rng;

/// Straight-alpha RGBA8 image.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Generates a built-in sprite (64x64, white colour, shape in alpha).
pub fn builtin_image(kind: BuiltinSprite) -> SpriteImage {
    const N: u32 = 64;
    let mut rgba = vec![255u8; (N * N * 4) as usize];
    // low frequency value noise for the puff
    let mut rng = Rng::new(0x5EED);
    let grid: Vec<f32> = (0..9 * 9).map(|_| rng.f32()).collect();
    let noise = |u: f32, v: f32| {
        let (x, y) = (u * 8.0, v * 8.0);
        let (xi, yi) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (smooth(0.0, 1.0, x.fract()), smooth(0.0, 1.0, y.fract()));
        let g = |a: usize, b: usize| grid[(b.min(8)) * 9 + a.min(8)];
        let top = g(xi, yi) * (1.0 - fx) + g(xi + 1, yi) * fx;
        let bot = g(xi, yi + 1) * (1.0 - fx) + g(xi + 1, yi + 1) * fx;
        top * (1.0 - fy) + bot * fy
    };
    for y in 0..N {
        for x in 0..N {
            let u = (x as f32 + 0.5) / N as f32;
            let v = (y as f32 + 0.5) / N as f32;
            let (dx, dy) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
            let r = (dx * dx + dy * dy).sqrt();
            let a = match kind {
                BuiltinSprite::SoftCircle => (1.0 - r).clamp(0.0, 1.0).powf(1.6),
                BuiltinSprite::Puff => {
                    let n = 0.55 + 0.9 * (noise(u, v) - 0.5);
                    smooth(1.0, 0.15, r) * n.clamp(0.0, 1.0) * 1.4
                }
                BuiltinSprite::Spark => {
                    // bright core along the long (v) axis, tapered
                    let w = (1.0 - dx.abs()).clamp(0.0, 1.0).powf(3.0);
                    let l = (1.0 - dy.abs()).clamp(0.0, 1.0).powf(0.7);
                    (w * l * 1.2).min(1.0)
                }
                BuiltinSprite::Droplet => smooth(1.0, 0.7, (dx * dx + dy * dy * 0.6).sqrt()),
            };
            let i = ((y * N + x) * 4) as usize;
            rgba[i + 3] = (a.clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    SpriteImage { width: N, height: N, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_sprites_have_soft_alpha() {
        for k in [BuiltinSprite::SoftCircle, BuiltinSprite::Puff, BuiltinSprite::Spark, BuiltinSprite::Droplet] {
            let im = builtin_image(k);
            assert_eq!(im.rgba.len(), 64 * 64 * 4);
            let centre = im.rgba[(32 * 64 + 32) * 4 + 3];
            let corner = im.rgba[3];
            assert!(centre > 100 && corner < 8, "{k:?}: centre {centre} corner {corner}");
        }
    }
}
