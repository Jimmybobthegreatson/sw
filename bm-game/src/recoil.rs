//! Firearm recoil as the original game does it (`HitmanBloodMoney.exe`, `FUN_0064be10`, verified with a live trace of the
//! shotgun and the Snub Nosed revolver; see `docs/game.md`, "Recoil").
//!
//! A shot sets an accumulator to [`START`]; every frame it is halved ([`DECAY_PER_TICK`], one simulation tick here) until it
//! drops below [`THRESHOLD`] (2^-13), where the kick is zero. With `acc` the accumulator, the item values `recoil_x` /
//! `recoil_y` and `s` uniform in [-1, 1):
//!
//! * `Y = acc * recoil_y` (exact in the trace; this module turns it into a climb, see [`Kick::up`]);
//! * `X = acc * recoil_x`, multiplied by a random sign and magnitude `s * |acc * recoil_x|` when the mode randomises X
//!   (the native `2 * (r - 0.5)` with `r` in [0, 1) is `s`).
//!
//! The item values are the weapon's record (`WeaponDef::recoil_x` / `recoil_y` / `recoil_mode`). The native units are kept
//! as they come out of the routine and applied as degrees.

use crate::data::{RecoilMode, WeaponDef};

/// Accumulator value a shot sets.
pub const START: f32 = 0.1;
/// Below this the native routine returns no kick (2^-13).
pub const THRESHOLD: f32 = 1.0 / 8192.0;
/// Halving per frame of the original; one simulation tick (60 Hz) is one frame here.
pub const DECAY_PER_TICK: f32 = 0.5;

/// Kick of one tick, already in view space and in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Kick {
    /// Turn to the right (positive) or left.
    pub right: f32,
    /// Climb (positive) or dip.
    pub up: f32,
}

/// Kick of one tick for accumulator `acc`. `s_x` / `s_y` are uniform in [-1, 1) and only used by the randomised axes.
pub fn kick(acc: f32, def: &WeaponDef, s_x: f32, s_y: f32) -> Kick {
    if acc < THRESHOLD {
        return Kick::default();
    }
    let rx = acc * def.recoil_x;
    let ry = acc * def.recoil_y;
    let x = if def.recoil_mode.randomizes_x() { s_x * rx.abs() } else { rx };
    let y = if def.recoil_mode.randomizes_y() { s_y * ry.abs() } else { ry };
    // the native Y value is negative for the shotgun and the revolver, and the muzzle climbs: a negative value is a climb
    Kick { right: x, up: -y }
}

/// Halves the accumulator for the next frame; it is cleared below the threshold (the kick is zero there anyway).
pub fn decay(acc: &mut f32) {
    *acc *= DECAY_PER_TICK;
    if *acc < THRESHOLD {
        *acc = 0.0;
    }
}

impl RecoilMode {
    /// X is randomised by the mode (native `eXRandom`, `eXYRandom`).
    pub fn randomizes_x(self) -> bool {
        matches!(self, RecoilMode::XRandom | RecoilMode::XyRandom)
    }

    /// Y is randomised by the mode (native `eYRandom`, `eXYRandom`). Not verified in the original yet.
    pub fn randomizes_y(self) -> bool {
        matches!(self, RecoilMode::YRandom | RecoilMode::XyRandom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::GameData;

    fn weapon(id: &str) -> WeaponDef {
        GameData::embedded().weapon(id).cloned().expect("weapon in table")
    }

    #[test]
    fn no_kick_below_threshold() {
        let w = weapon("remington");
        assert_eq!(kick(THRESHOLD * 0.5, &w, 0.7, -0.3), Kick::default());
        assert_eq!(kick(0.0, &w, 0.7, -0.3), Kick::default());
    }

    #[test]
    fn climb_is_the_native_y_times_the_accumulator() {
        // trace: shotgun, acc 0.1, recoil_y -5 -> native Y -0.5 -> climb 0.5 degrees
        let w = weapon("remington");
        let k = kick(0.1, &w, 0.0, 0.0);
        assert!((k.up - 0.5).abs() < 1e-6, "{k:?}");
    }

    #[test]
    fn randomised_x_stays_inside_the_native_band() {
        // trace: shotgun X in +-|acc * recoil_x| = +-0.3 at acc 0.1
        let w = weapon("remington");
        for s in [-0.999f32, -0.5, 0.0, 0.25, 0.999] {
            let k = kick(0.1, &w, s, 0.0);
            assert!(k.right.abs() <= 0.3 + 1e-6, "{k:?}");
        }
        assert!((kick(0.1, &w, 0.5, 0.0).right - 0.15).abs() < 1e-6);
    }

    #[test]
    fn non_random_mode_keeps_x_deterministic() {
        let mut w = weapon("remington");
        w.recoil_mode = RecoilMode::None;
        let a = kick(0.1, &w, -1.0, 0.0);
        let b = kick(0.1, &w, 1.0, 0.0);
        assert_eq!(a, b);
        assert!((a.right + 0.3).abs() < 1e-6, "{a:?}");
    }

    #[test]
    fn accumulator_halves_until_the_threshold() {
        let mut acc = START;
        let mut kicked = 0;
        let w = weapon("remington");
        for _ in 0..40 {
            if kick(acc, &w, 0.0, 0.0) != Kick::default() {
                kicked += 1;
            }
            decay(&mut acc);
        }
        // 0.1 / 2^k drops below 2^-13 at k = 10, so ten frames carry a kick (as in the native trace)
        assert_eq!(kicked, 10);
        assert_eq!(acc, 0.0);
    }
}
