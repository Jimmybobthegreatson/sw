//! Numeric checks for rendered music: sample-step statistics used to prove that transitions and loop
//! wraps do not click.
//!
//! The *step* of a signal is `max(|dL|, |dR|)` between consecutive frames. A click (hard cut, buffer
//! seam, phase jump) shows up as a step far above what the programme material itself produces, so a
//! boundary is "continuous" when the largest step inside a small window around it does not exceed a
//! multiple of a high percentile of the steps elsewhere.

/// Steps of interleaved stereo `x` (length = frames - 1).
pub fn steps(x: &[f32]) -> Vec<f32> {
    let n = x.len() / 2;
    (1..n).map(|i| (x[i * 2] - x[(i - 1) * 2]).abs().max((x[i * 2 + 1] - x[(i - 1) * 2 + 1]).abs())).collect()
}

/// `q`-quantile (0..1) of `v` (copy, partial sort).
pub fn quantile(v: &[f32], q: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s[((s.len() - 1) as f32 * q.clamp(0.0, 1.0)) as usize]
}

#[derive(Clone, Debug)]
pub struct BoundaryReport {
    /// Frame index of the boundary.
    pub frame: usize,
    /// Largest step within the window.
    pub max_step: f32,
    /// Reference step level (quantile of the steps outside all windows).
    pub reference: f32,
    pub ratio: f32,
    pub ok: bool,
}

/// Checks every boundary in `frames` (positions in frames) with a window of `±half_window` frames.
/// `reference` = `q` quantile of all steps outside the windows; passes when `max_step <= limit *
/// reference` (and the absolute step stays below 1.0, i.e. no full-scale jump).
pub fn check_boundaries(x: &[f32], frames: &[usize], half_window: usize, q: f32, limit: f32) -> Vec<BoundaryReport> {
    let st = steps(x);
    let mut inside = vec![false; st.len()];
    for &f in frames {
        for i in f.saturating_sub(half_window)..(f + half_window).min(st.len()) {
            inside[i] = true;
        }
    }
    let outside: Vec<f32> = st.iter().zip(&inside).filter(|(_, i)| !**i).map(|(s, _)| *s).collect();
    let reference = quantile(&outside, q).max(1e-6);
    frames
        .iter()
        .map(|&f| {
            let a = f.saturating_sub(half_window);
            let b = (f + half_window).min(st.len());
            let m = st[a.min(st.len())..b].iter().cloned().fold(0.0f32, f32::max);
            BoundaryReport { frame: f, max_step: m, reference, ratio: m / reference, ok: m <= limit * reference && m < 1.0 }
        })
        .collect()
}

/// Peak and DC offset of interleaved stereo `x`, plus whether any sample is NaN/inf.
pub fn basic_stats(x: &[f32]) -> (f32, f32, bool) {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let dc = x.iter().sum::<f32>() / x.len().max(1) as f32;
    (peak, dc, x.iter().any(|v| !v.is_finite()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize) -> Vec<f32> {
        let mut v = Vec::new();
        for i in 0..n {
            let s = (i as f32 * 0.05).sin() * 0.5;
            v.push(s);
            v.push(s);
        }
        v
    }

    #[test]
    fn clean_signal_passes_and_a_jump_is_caught() {
        let mut x = sine(20_000);
        let r = check_boundaries(&x, &[5000, 15000], 100, 0.999, 3.0);
        assert!(r.iter().all(|b| b.ok), "{r:?}");
        // hard phase jump at 15000
        for i in 15000..20_000 {
            x[i * 2] = -x[i * 2];
            x[i * 2 + 1] = -x[i * 2 + 1];
        }
        let r = check_boundaries(&x, &[5000, 15000], 100, 0.999, 3.0);
        assert!(r[0].ok);
        assert!(!r[1].ok, "{:?}", r[1]);
    }

    #[test]
    fn stats() {
        let (p, dc, bad) = basic_stats(&[0.5, -0.5, 0.25, -0.25]);
        assert_eq!(p, 0.5);
        assert!(dc.abs() < 1e-6 && !bad);
        assert!(basic_stats(&[f32::NAN]).2);
    }
}
