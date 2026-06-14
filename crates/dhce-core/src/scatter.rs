//! Deterministic decoration placement.
//!
//! Given a seed + mesh + elevation + biome assignment, emit decoration instances
//! (procedural rocks/trees) — identical in the browser tool and in Godot. Submerged
//! and boundary regions are skipped; per-biome weights decide species + density.
//!
//! Phase 7. The front-end synthesizes the actual sprite shape procedurally; this
//! module only decides *where*, *what*, and *how big*.

use crate::mesh::Mesh;
use crate::prng::Rng;

/// One placed decoration instance.
#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub scale: f32,
    /// 0 = tree, 1 = rock.
    pub species: f32,
}

/// Per-biome `(placement probability, species)`.
fn biome_decor(biome: u8) -> (f64, f32) {
    match biome {
        2 | 4 | 7 => (0.55, 0.0), // Sacred Woods / Temperate Forest / Deep Wood → trees
        5 => (0.12, 0.0),         // Open Plains → sparse trees
        1 | 8 => (0.40, 1.0),     // Jagged Mountains / Frozen Reaches → rocks
        11 | 12 => (0.25, 1.0),   // Volcanic / Blight → rocks
        14 => (0.10, 0.0),        // Marsh → sparse
        _ => (0.05, 1.0),         // sparse rocks elsewhere
    }
}

/// Scatter decoration instances over land regions, deterministic in `seed`.
/// `density` (0..1) globally scales placement; `exaggeration` lifts each instance to
/// its terrain height.
pub fn scatter(
    seed: u64,
    mesh: &Mesh,
    elevation: &[f64],
    biome: &[u8],
    exaggeration: f64,
    density: f64,
) -> Vec<Instance> {
    let mut out = Vec::new();
    if density <= 0.0 {
        return out;
    }
    let mut rng = Rng::new(seed ^ 0x5343_4154_5445_525F); // "SCATTER_"
    for r in 0..mesh.num_regions() {
        // Always advance the rng once per region so the stream is placement-independent.
        let roll = rng.next_f64();
        if mesh.is_boundary_r(r) {
            continue;
        }
        let e = elevation.get(r).copied().unwrap_or(0.0);
        if e <= 0.02 {
            continue; // land only
        }
        let b = biome.get(r).copied().unwrap_or(0);
        let (p, species) = biome_decor(b);
        if roll >= p * density {
            continue;
        }
        let pos = mesh.pos_of_r(r);
        let jx = (rng.next_f64() - 0.5) * 6.0;
        let jy = (rng.next_f64() - 0.5) * 6.0;
        let scale = 4.0 + rng.next_f64() * 6.0;
        out.push(Instance {
            x: (pos[0] + jx) as f32,
            y: (pos[1] + jy) as f32,
            z: (e * exaggeration) as f32,
            scale: scale as f32,
            species,
        });
    }
    out
}
