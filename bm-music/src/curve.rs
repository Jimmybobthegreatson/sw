//! Fade curves and a sample-accurate gain ramp.

use serde::{Deserialize, Serialize};

/// Shape of a fade. [`Curve::apply`] maps progress `x` in `0..=1` to a *rising* gain `0..=1`; fading
/// out evaluates the curve at `1 - x`, which for [`Curve::EqualPower`] gives the complementary cosine
/// leg (constant power crossfade).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Curve {
    /// Straight line (constant amplitude sum for correlated material).
    Linear,
    /// `sin(x * pi/2)`: constant power for uncorrelated material. The default for crossfades.
    EqualPower,
    /// Raised cosine S-curve (slow start and end).
    SCurve,
    /// `x^2`: slow start, fast end.
    Quadratic,
    /// `sqrt(x)`: fast start, slow end.
    Sqrt,
    /// Exponential-style perceptual curve: `(exp(k x) - 1) / (exp(k) - 1)`, `k > 0` bends towards slow start.
    Exp(f32),
    /// Piecewise linear `(x, y)` points sorted by x; clamped outside the range.
    Points(Vec<(f32, f32)>),
}

impl Default for Curve {
    fn default() -> Self {
        Curve::EqualPower
    }
}

impl Curve {
    /// Rising gain for progress `x` (clamped to `0..=1`). `apply(0) == 0` and `apply(1) == 1` for all
    /// built-in shapes.
    pub fn apply(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Linear => x,
            Curve::EqualPower => (x * std::f32::consts::FRAC_PI_2).sin(),
            Curve::SCurve => 0.5 - 0.5 * (x * std::f32::consts::PI).cos(),
            Curve::Quadratic => x * x,
            Curve::Sqrt => x.sqrt(),
            Curve::Exp(k) => {
                if k.abs() < 1e-4 {
                    x
                } else {
                    ((k * x).exp() - 1.0) / (k.exp() - 1.0)
                }
            }
            Curve::Points(p) => {
                if p.is_empty() {
                    return x;
                }
                if x <= p[0].0 {
                    return p[0].1;
                }
                for w in p.windows(2) {
                    if x <= w[1].0 {
                        let d = (w[1].0 - w[0].0).max(1e-9);
                        return w[0].1 + (w[1].1 - w[0].1) * (x - w[0].0) / d;
                    }
                }
                p[p.len() - 1].1
            }
        }
    }
}

/// Fade description used by transitions: duration and curve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FadeDef {
    pub secs: f32,
    pub curve: Curve,
}

impl Default for FadeDef {
    fn default() -> Self {
        FadeDef { secs: 2.0, curve: Curve::EqualPower }
    }
}

impl FadeDef {
    pub fn new(secs: f32, curve: Curve) -> FadeDef {
        FadeDef { secs, curve }
    }
}

/// A gain ramp from `from` to `to` over `total` frames, evaluated per sample. Rising ramps follow the
/// curve, falling ramps follow its mirror (so equal-power crossfades stay equal-power).
#[derive(Clone, Debug)]
pub struct Ramp {
    from: f32,
    to: f32,
    total: u64,
    pos: u64,
    curve: Curve,
}

impl Ramp {
    pub fn constant(v: f32) -> Ramp {
        Ramp { from: v, to: v, total: 0, pos: 0, curve: Curve::Linear }
    }

    /// Starts a ramp from the current value of `self` (if any) to `to`.
    pub fn retarget(&mut self, to: f32, frames: u64, curve: Curve) {
        let now = self.value();
        *self = Ramp { from: now, to, total: frames, pos: 0, curve };
        if frames == 0 {
            self.from = to;
        }
    }

    /// Current value (does not advance).
    pub fn value(&self) -> f32 {
        if self.total == 0 || self.pos >= self.total {
            return self.to;
        }
        let x = self.pos as f32 / self.total as f32;
        if self.to >= self.from {
            self.from + (self.to - self.from) * self.curve.apply(x)
        } else {
            self.to + (self.from - self.to) * self.curve.apply(1.0 - x)
        }
    }

    /// Value, then advances by one frame.
    #[inline]
    pub fn next(&mut self) -> f32 {
        let v = self.value();
        if self.pos < self.total {
            self.pos += 1;
        }
        v
    }

    pub fn advance(&mut self, frames: u64) {
        self.pos = (self.pos + frames).min(self.total);
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn is_done(&self) -> bool {
        self.pos >= self.total
    }

    /// Frames until the ramp has reached its target.
    pub fn remaining(&self) -> u64 {
        self.total.saturating_sub(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_hit_endpoints_and_are_monotonic() {
        let cs = [Curve::Linear, Curve::EqualPower, Curve::SCurve, Curve::Quadratic, Curve::Sqrt, Curve::Exp(3.0), Curve::Exp(-2.0)];
        for c in &cs {
            assert!(c.apply(0.0).abs() < 1e-6, "{c:?}");
            assert!((c.apply(1.0) - 1.0).abs() < 1e-6, "{c:?}");
            let mut last = -1.0;
            for i in 0..=100 {
                let v = c.apply(i as f32 / 100.0);
                assert!(v >= last - 1e-6, "{c:?} not monotonic");
                last = v;
            }
        }
    }

    #[test]
    fn equal_power_crossfade_keeps_power_constant() {
        let c = Curve::EqualPower;
        for i in 0..=50 {
            let x = i as f32 / 50.0;
            let p = c.apply(x).powi(2) + c.apply(1.0 - x).powi(2);
            assert!((p - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn points_curve_interpolates() {
        let c = Curve::Points(vec![(0.0, 0.0), (0.5, 0.2), (1.0, 1.0)]);
        assert!((c.apply(0.25) - 0.1).abs() < 1e-6);
        assert!((c.apply(0.75) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn ramp_is_continuous_when_retargeted() {
        let mut r = Ramp::constant(0.0);
        r.retarget(1.0, 100, Curve::Linear);
        for _ in 0..40 {
            r.next();
        }
        let before = r.value();
        r.retarget(0.0, 100, Curve::Linear);
        assert!((r.value() - before).abs() < 1e-6);
        let mut last = r.value();
        for _ in 0..100 {
            let v = r.next();
            assert!(v <= last + 1e-6);
            last = v;
        }
        assert!(r.value().abs() < 1e-6);
        assert!(r.is_done());
    }
}
