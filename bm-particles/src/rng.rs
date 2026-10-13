//! Small deterministic PRNG (SplitMix64 seeding + xoshiro128++-style mixing is overkill here; a plain
//! SplitMix64 stream is fast, has no bad seeds and gives identical output on every platform).

/// Deterministic random number generator. Same seed -> same sequence on every platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng { state: seed }
    }

    /// Derives an independent stream (used per emitter so spawn order between emitters does not matter).
    pub fn fork(&mut self, salt: u64) -> Rng {
        let a = self.next_u64();
        Rng::new(a ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Uniform in `[lo, hi]`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u64() >> 32) as u32 as u64 * n as u64 >> 32) as u32
    }

    /// Uniformly distributed unit vector.
    pub fn unit_vec3(&mut self) -> [f32; 3] {
        let z = self.range(-1.0, 1.0);
        let a = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        [r * a.cos(), z, r * a.sin()]
    }
}
