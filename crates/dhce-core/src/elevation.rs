//! Terrain elevation.
//!
//! Phase 2: a per-region height field from fbm noise with a radial island falloff,
//! so land sits in surrounding water. Per-biome landform shaping (peak/hill/valley)
//! layers on top in a later phase.

use crate::mesh::Mesh;
use crate::noise;

/// How many times the noise repeats across the map (lower = larger landmasses).
const DOMAIN: f64 = 4.0;
/// Strength of the radial coastline falloff.
const FALLOFF: f64 = 0.6;

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
