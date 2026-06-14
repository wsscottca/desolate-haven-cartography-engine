//! Deterministic, cross-target PRNG (SplitMix64).
//!
//! SplitMix64 is a tiny, fast, public-domain generator (Vigna). We use our own
//! implementation — not `@redblobgames/prng` (Apache-2.0) — so the stream is ours
//! and is byte-identical on wasm32 and native targets.

/// A small deterministic PRNG. Same seed ⇒ same sequence on every platform.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed the generator. Any seed is valid, including `0`.
    #[inline]
    pub fn new(seed: u64) -> Self {
        Rng { state: seed }
    }

    /// Next raw 64-bit value (SplitMix64).
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform `f64` in `[0, 1)` with full 53-bit mantissa precision.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
    }

    /// Uniform `f32` in `[0, 1)`. Computed in `f64` then narrowed (determinism rule).
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        self.next_f64() as f32
    }

    /// Uniform integer in `[0, n)`. `n` must be `> 0`.
    ///
    /// Lemire multiply-shift — unbiased enough for procedural placement.
    #[inline]
    pub fn next_below(&mut self, n: u32) -> u32 {
        debug_assert!(n > 0, "next_below requires n > 0");
        (((self.next_u64() >> 32) * n as u64) >> 32) as u32
    }
}
