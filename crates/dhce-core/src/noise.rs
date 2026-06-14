//! Coherent value noise + fbm.
//!
//! This is a deterministic placeholder so terrain has shape from Phase 0. Phase 1
//! replaces it with our own simplex implementation (same public API). All math is
//! `f64` with integer hashing (wrapping ops only) → identical on wasm32 and native.

/// Integer hash of a lattice point → `[0, 1)`.
#[inline]
fn hash2(x: i32, y: i32, seed: u64) -> f64 {
    let mut h = seed
        ^ (x as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
}

/// Smoothstep easing.
#[inline]
fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// 2D value noise in roughly `[-1, 1]`.
pub fn value2(x: f64, y: f64, seed: u64) -> f64 {
    let xi = x.floor();
    let yi = y.floor();
    let (x0, y0) = (xi as i32, yi as i32);
    let (xf, yf) = (x - xi, y - yi);

    let v00 = hash2(x0, y0, seed);
    let v10 = hash2(x0 + 1, y0, seed);
    let v01 = hash2(x0, y0 + 1, seed);
    let v11 = hash2(x0 + 1, y0 + 1, seed);

    let u = smooth(xf);
    let v = smooth(yf);
    let a = v00 + (v10 - v00) * u;
    let b = v01 + (v11 - v01) * u;
    (a + (b - a) * v) * 2.0 - 1.0
}

/// Fractal Brownian motion: `octaves` layers of value noise, normalized to `[-1, 1]`.
pub fn fbm2(x: f64, y: f64, seed: u64, octaves: u32) -> f64 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for o in 0..octaves {
        let s = seed ^ (o as u64).wrapping_mul(0x1000_0001_B3);
        sum += amp * value2(x * freq, y * freq, s);
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.0
    }
}
