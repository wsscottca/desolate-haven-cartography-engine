//! Our own 2D simplex noise + fbm.
//!
//! Clean-room implementation of the simplex algorithm (Perlin's 2002 method; the
//! patent expired in 2022). Gradient selection is by integer hash of the lattice
//! cell rather than a shuffled permutation table, so the function is stateless and
//! seed-parameterized — cheap to call and trivially deterministic.
//!
//! Determinism rule: **no transcendentals** (`sin`/`cos`/`exp`) — only `floor`,
//! multiply, add, which are IEEE-deterministic and so byte-identical on wasm32 and
//! native. (`sin`/`cos` are not guaranteed identical across platform libms.)

// Skew/unskew factors for the 2D simplex grid.
const F2: f64 = 0.366_025_403_784_438_6; // 0.5 * (sqrt(3) - 1)
const G2: f64 = 0.211_324_865_405_187_13; // (3 - sqrt(3)) / 6

// 8 gradient directions (axis + diagonal), classic 2D set; magnitude matches the
// standard scale constant below.
const GRAD2: [[f64; 2]; 8] = [
    [1.0, 1.0], [-1.0, 1.0], [1.0, -1.0], [-1.0, -1.0],
    [1.0, 0.0], [-1.0, 0.0], [0.0, 1.0], [0.0, -1.0],
];

/// Pick a gradient index in `0..8` for lattice cell `(i, j)` under `seed`.
#[inline]
fn grad_index(i: i32, j: i32, seed: u64) -> usize {
    let mut h = seed
        ^ (i as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (j as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h & 7) as usize
}

/// One simplex corner's contribution at offset `(dx, dy)` from lattice cell `(ci, cj)`.
#[inline]
fn corner(ci: i32, cj: i32, dx: f64, dy: f64, seed: u64) -> f64 {
    let t = 0.5 - dx * dx - dy * dy;
    if t <= 0.0 {
        return 0.0;
    }
    let g = GRAD2[grad_index(ci, cj, seed)];
    let t2 = t * t;
    t2 * t2 * (g[0] * dx + g[1] * dy)
}

/// 2D simplex noise in `[-1, 1]`.
pub fn simplex2(x: f64, y: f64, seed: u64) -> f64 {
    // Skew input space to determine the simplex cell.
    let s = (x + y) * F2;
    let i = (x + s).floor();
    let j = (y + s).floor();
    let t = (i + j) * G2;
    // Unskewed cell origin, and the displacement of the input from it (corner 0).
    let x0 = x - (i - t);
    let y0 = y - (j - t);

    // Which of the two triangles of the cell are we in? Pick the second corner.
    let (i1, j1) = if x0 > y0 { (1.0, 0.0) } else { (0.0, 1.0) };

    let x1 = x0 - i1 + G2;
    let y1 = y0 - j1 + G2;
    let x2 = x0 - 1.0 + 2.0 * G2;
    let y2 = y0 - 1.0 + 2.0 * G2;

    let ci = i as i32;
    let cj = j as i32;

    let n = corner(ci, cj, x0, y0, seed)
        + corner(ci + i1 as i32, cj + j1 as i32, x1, y1, seed)
        + corner(ci + 1, cj + 1, x2, y2, seed);

    // Scale to fill ~[-1, 1]; clamp guarantees the bound for downstream fbm.
    (70.0 * n).clamp(-1.0, 1.0)
}

/// Fractal Brownian motion: `octaves` layers of simplex noise, normalized to `[-1, 1]`.
pub fn fbm2(x: f64, y: f64, seed: u64, octaves: u32) -> f64 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for o in 0..octaves {
        let s = seed ^ (o as u64).wrapping_mul(0x1000_0001_B3);
        sum += amp * simplex2(x * freq, y * freq, s);
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
