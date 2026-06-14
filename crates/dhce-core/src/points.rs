//! Deterministic blue-noise point sampling (Bridson's Poisson-disk algorithm).
//!
//! Produces well-spaced points to seed the mesh. Fully deterministic and
//! cross-target safe: the candidate annulus is sampled by **rejection in a square**
//! (only multiply/compare) rather than with `sin`/`cos`, which are not bit-identical
//! across platform libms. See the determinism rule in `noise.rs`.

use crate::prng::Rng;

const SQRT2: f64 = std::f64::consts::SQRT_2;
/// Candidate attempts per active point before it is retired.
const K: u32 = 30;

/// Poisson-disk sample of `[0, width) × [0, height)` with minimum spacing `radius`.
/// Same `(width, height, radius, seed)` ⇒ identical point set on every target.
pub fn poisson_disk(width: f64, height: f64, radius: f64, seed: u64) -> Vec<[f64; 2]> {
    debug_assert!(width > 0.0 && height > 0.0 && radius > 0.0);
    let mut rng = Rng::new(seed ^ 0x504F_4953_534F_4E5F); // "POISSON_"

    let cell = radius / SQRT2;
    let gw = (width / cell).ceil() as usize + 1;
    let gh = (height / cell).ceil() as usize + 1;
    let mut grid = vec![usize::MAX; gw * gh]; // background grid → point index or MAX
    let mut pts: Vec<[f64; 2]> = Vec::new();
    let mut active: Vec<usize> = Vec::new();

    let r2 = radius * radius;

    // Seed with one point near the center (deterministic, avoids edge bias).
    let first = [width * 0.5, height * 0.5];
    insert(first, cell, gw, &mut grid, &mut pts, &mut active);

    while !active.is_empty() {
        let ai = rng.next_below(active.len() as u32) as usize;
        let center = pts[active[ai]];
        let mut placed = false;

        for _ in 0..K {
            // Annulus [radius, 2*radius] via rejection in [-2r, 2r]² — no transcendentals.
            let (dx, dy) = loop {
                let dx = (rng.next_f64() * 2.0 - 1.0) * 2.0 * radius;
                let dy = (rng.next_f64() * 2.0 - 1.0) * 2.0 * radius;
                let d2 = dx * dx + dy * dy;
                if d2 >= r2 && d2 <= 4.0 * r2 {
                    break (dx, dy);
                }
            };
            let p = [center[0] + dx, center[1] + dy];
            if p[0] < 0.0 || p[0] >= width || p[1] < 0.0 || p[1] >= height {
                continue;
            }
            if far_enough(p, cell, gw, gh, r2, &grid, &pts) {
                insert(p, cell, gw, &mut grid, &mut pts, &mut active);
                placed = true;
                break;
            }
        }
        if !placed {
            active.swap_remove(ai); // retire (standard Bridson; order is rng-driven)
        }
    }
    pts
}

#[inline]
fn insert(
    p: [f64; 2],
    cell: f64,
    gw: usize,
    grid: &mut [usize],
    pts: &mut Vec<[f64; 2]>,
    active: &mut Vec<usize>,
) {
    let gx = (p[0] / cell) as usize;
    let gy = (p[1] / cell) as usize;
    grid[gy * gw + gx] = pts.len();
    active.push(pts.len());
    pts.push(p);
}

/// True if no existing point lies within `radius` of `p` (checks the 5×5 cell block).
#[inline]
fn far_enough(p: [f64; 2], cell: f64, gw: usize, gh: usize, r2: f64, grid: &[usize], pts: &[[f64; 2]]) -> bool {
    let gx = (p[0] / cell) as usize;
    let gy = (p[1] / cell) as usize;
    let x0 = gx.saturating_sub(2);
    let y0 = gy.saturating_sub(2);
    let x1 = (gx + 2).min(gw - 1);
    let y1 = (gy + 2).min(gh - 1);
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            let gi = grid[yy * gw + xx];
            if gi != usize::MAX {
                let q = pts[gi];
                let ddx = q[0] - p[0];
                let ddy = q[1] - p[1];
                if ddx * ddx + ddy * ddy < r2 {
                    return false;
                }
            }
        }
    }
    true
}
