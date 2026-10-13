//! Piecewise-linear curves over normalised particle life (`0 = born`, `1 = dies`) and value ranges.

use serde::{Deserialize, Serialize};

use crate::rng::Rng;

/// Inclusive random range `(min, max)`. A single value is `Range(v, v)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Range(pub f32, pub f32);

impl Range {
    pub fn constant(v: f32) -> Range {
        Range(v, v)
    }
    pub fn sample(&self, rng: &mut Rng) -> f32 {
        if self.0 == self.1 { self.0 } else { rng.range(self.0, self.1) }
    }
    pub fn mid(&self) -> f32 {
        0.5 * (self.0 + self.1)
    }
}

impl Default for Range {
    fn default() -> Self {
        Range(1.0, 1.0)
    }
}

/// Scalar curve: keys `(t, value)` sorted by `t`; values are held constant outside the key range.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub keys: Vec<(f32, f32)>,
}

impl Curve {
    pub fn constant(v: f32) -> Curve {
        Curve { keys: vec![(0.0, v)] }
    }
    pub fn linear(a: f32, b: f32) -> Curve {
        Curve { keys: vec![(0.0, a), (1.0, b)] }
    }
    pub fn from_keys(mut keys: Vec<(f32, f32)>) -> Curve {
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        Curve { keys }
    }

    pub fn eval(&self, t: f32) -> f32 {
        let k = &self.keys;
        match k.len() {
            0 => 1.0,
            1 => k[0].1,
            _ => {
                if t <= k[0].0 {
                    return k[0].1;
                }
                for w in k.windows(2) {
                    if t <= w[1].0 {
                        let span = (w[1].0 - w[0].0).max(1e-9);
                        let f = (t - w[0].0) / span;
                        return w[0].1 + (w[1].1 - w[0].1) * f;
                    }
                }
                k[k.len() - 1].1
            }
        }
    }
}

impl Default for Curve {
    fn default() -> Self {
        Curve::constant(1.0)
    }
}

/// RGBA colour curve (linear-ish 0..1 values, straight alpha).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorCurve {
    pub keys: Vec<(f32, [f32; 4])>,
}

impl ColorCurve {
    pub fn constant(c: [f32; 4]) -> ColorCurve {
        ColorCurve { keys: vec![(0.0, c)] }
    }

    /// `n` evenly spaced keys over life (the original template stores 8).
    pub fn evenly_spaced(colors: &[[f32; 4]]) -> ColorCurve {
        let n = colors.len();
        let keys = colors
            .iter()
            .enumerate()
            .map(|(i, c)| (if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 }, *c))
            .collect();
        ColorCurve { keys }
    }

    pub fn eval(&self, t: f32) -> [f32; 4] {
        let k = &self.keys;
        match k.len() {
            0 => [1.0; 4],
            1 => k[0].1,
            _ => {
                if t <= k[0].0 {
                    return k[0].1;
                }
                for w in k.windows(2) {
                    if t <= w[1].0 {
                        let span = (w[1].0 - w[0].0).max(1e-9);
                        let f = (t - w[0].0) / span;
                        let mut o = [0.0; 4];
                        for i in 0..4 {
                            o[i] = w[0].1[i] + (w[1].1[i] - w[0].1[i]) * f;
                        }
                        return o;
                    }
                }
                k[k.len() - 1].1
            }
        }
    }
}

impl Default for ColorCurve {
    fn default() -> Self {
        ColorCurve::constant([1.0; 4])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_interpolates_and_clamps() {
        let c = Curve::from_keys(vec![(1.0, 0.0), (0.0, 10.0), (0.5, 4.0)]);
        assert_eq!(c.eval(-1.0), 10.0);
        assert_eq!(c.eval(0.0), 10.0);
        assert!((c.eval(0.25) - 7.0).abs() < 1e-6);
        assert_eq!(c.eval(0.5), 4.0);
        assert!((c.eval(0.75) - 2.0).abs() < 1e-6);
        assert_eq!(c.eval(2.0), 0.0);
        assert_eq!(Curve::constant(3.0).eval(0.7), 3.0);
    }

    #[test]
    fn color_curve_evenly_spaced() {
        let c = ColorCurve::evenly_spaced(&[[0.0; 4], [1.0; 4], [0.0; 4]]);
        assert_eq!(c.keys[1].0, 0.5);
        let m = c.eval(0.25);
        assert!((m[0] - 0.5).abs() < 1e-6 && (m[3] - 0.5).abs() < 1e-6);
        assert_eq!(c.eval(1.0), [0.0; 4]);
    }
}
