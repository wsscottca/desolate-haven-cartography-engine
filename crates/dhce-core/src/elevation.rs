//! Terrain elevation.
//!
//! Phase 2: a per-region height field from fbm noise with a radial island falloff,
//! so land sits in surrounding water. Per-biome landform shaping (peak/hill/valley)
//! layers on top in a later phase.

use crate::mesh::Mesh;
use crate::noise;
use crate::regions;

/// How many times the noise repeats across the map (lower = larger landmasses).
const DOMAIN: f64 = 4.0;
/// Strength of the radial coastline falloff.
const FALLOFF: f64 = 0.6;

// --- canon-map elevation tuning -----------------------------------------------------------------
/// Deep open-ocean floor the rim + ocean cells pull toward (normalized, well below sea level).
const OCEAN_FLOOR: f64 = -1.0;
/// Normalized radius (the `d = 2·|p−center|/extent` measure) where the ocean rim *starts* biting…
const RIM_INNER: f64 = 0.80;
/// …and where it reaches full open ocean. Only the outer band is pulled — interior regions near the
/// edge keep their land elevation.
const RIM_OUTER: f64 = 1.15;
/// Low-frequency texture repeats across the canon map (macro relief only; fine detail is `shape_terrain`).
const DOMAIN_CANON: f64 = 3.5;
/// Cap on base-elevation diffusion passes (grading the territory trunk across borders).
const MAX_CANON_BLEND_ITERS: usize = 256;

/// Smooth Hermite step in `[0,1]` (polynomial — determinism-safe).
fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// **Region-guided** elevation — the canon pipeline's terrain pass. Builds a per-cell base-elevation
/// trunk from each cell's region role ([`regions::base_elevation_for`]), **diffuses** it across
/// territory borders with the same certified Jacobi smoother as `blend_traits` (so borders grade —
/// no cliffs), rides low-frequency noise on top (amplitude scaled by the region's relief), and pulls
/// the outer band down with a **banded ocean rim** so the edges read as ocean without drowning
/// interior regions. The Great Lake + ocean sit below sea level, so [`crate::fluid::sea_fill`] floods
/// them. High/mid-frequency landform detail is intentionally left to `shape_terrain` (no double-count).
///
/// `neighbors` is the cell adjacency (passed in so we don't rebuild it); `transition_m` sets the
/// border-grading width. Deterministic: `par_map` (index-pure) + the Jacobi diffuser + `fbm2`.
#[allow(clippy::too_many_arguments)]
pub fn assign_region_elevation_canon(
    mesh: &Mesh,
    width: f64,
    height: f64,
    seed: u64,
    octaves: u32,
    region_ids: &[u8],
    neighbors: &[Vec<u32>],
    transition_m: f64,
) -> Vec<f64> {
    let nr = mesh.num_regions();
    // Per-cell base target from the region role (boundary frame = deep ocean).
    let base_target: Vec<f64> = crate::util::par_map(nr, |r| {
        if mesh.is_boundary_r(r) {
            return OCEAN_FLOOR;
        }
        regions::base_elevation_for(region_ids.get(r).copied().unwrap_or(0))
    });
    // Grade the trunk across territory borders (reuse the certified diffuser).
    let pitch = (width * height / nr as f64).sqrt().max(1.0);
    let iters = ((transition_m / pitch).round() as usize).clamp(0, MAX_CANON_BLEND_ITERS);
    let base = crate::world::diffuse_field(&base_target, neighbors, iters);

    // Add low-freq texture (relief-scaled) and the banded ocean rim.
    crate::util::par_map(nr, |r| {
        if mesh.is_boundary_r(r) {
            return OCEAN_FLOOR;
        }
        let p = mesh.pos_of_r(r);
        let u = p[0] / width;
        let v = p[1] / height;
        let relief = regions::default_traits_for(region_ids.get(r).copied().unwrap_or(0)).relief as f64;
        let amp = 0.05 + 0.14 * relief;
        let tex = noise::fbm2(u * DOMAIN_CANON, v * DOMAIN_CANON, seed, octaves) * amp;
        let mut h = base[r] + tex;
        // Banded radial ocean rim: pull only the outer band toward the ocean floor (never raises).
        let cx = u - 0.5;
        let cy = v - 0.5;
        let d = (cx * cx + cy * cy).sqrt() * 2.0;
        let rim = smoothstep(RIM_INNER, RIM_OUTER, d);
        let rimmed = h * (1.0 - rim) + OCEAN_FLOOR * rim;
        h = h.min(rimmed);
        h.clamp(-1.5, 1.5)
    })
}

/// Normalized per-region elevation in `[-1, 1]`, island-shaped. `width`/`height` are
/// the world extent the mesh was built over; `seed`/`octaves` drive the noise.
pub fn assign_region_elevation(
    mesh: &Mesh,
    width: f64,
    height: f64,
    seed: u64,
    octaves: u32,
) -> Vec<f64> {
    // Pure per-region map (each `e[r]` depends only on `r`'s position + the seed) → parallel-safe:
    // `par_map` preserves index order, so the result is bit-identical to the serial loop.
    crate::util::par_map(mesh.num_regions(), |r| {
        let p = mesh.pos_of_r(r);
        let u = p[0] / width;
        let v = p[1] / height;
        let mut h = noise::fbm2(u * DOMAIN, v * DOMAIN, seed, octaves);
        // Radial island falloff toward the edges.
        let cx = u - 0.5;
        let cy = v - 0.5;
        let d = (cx * cx + cy * cy).sqrt() * 2.0;
        h -= d * d * FALLOFF;
        h.clamp(-1.0, 1.0)
    })
}
