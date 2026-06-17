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

/// One authored scatter rule — a model slot's placement gate (the N3d scatter library). Bitmasks:
/// `veg_mask` over the 7 vegetation enum values (`1 << veg`), `biome_mask` over painted biome ids
/// 1..=14 (`1 << (biome-1)`), `region_mask` over named-Region ids 1..=14 (`1 << (region-1)`); a `0`
/// mask means "any". `density` 0..1, `scale` range in world metres, `elev` band in normalized height.
/// `species` on the produced [`Instance`] carries the `slot` index.
#[derive(Clone, Copy, Debug)]
pub struct ScatterRule {
    pub slot: u32,
    pub density: f64,
    pub scale_min: f64,
    pub scale_max: f64,
    pub elev_min: f64,
    pub elev_max: f64,
    pub veg_mask: u32,
    pub region_mask: u32,
    pub biome_mask: u32,
}

/// Rule-based deterministic scatter: at most one instance per cell (first matching rule wins). The
/// rng advances a fixed amount per cell, so placement is independent of which cells match (stable
/// under edits). Determinism-safe: integer hashing + multiply/add/compare only.
pub fn scatter_by_rules(
    seed: u64,
    mesh: &Mesh,
    elevation: &[f64],
    vegetation: &[u8],
    region: &[u8],
    biome: &[u8],
    rules: &[ScatterRule],
    exaggeration: f64,
) -> Vec<Instance> {
    let mut out = Vec::new();
    if rules.is_empty() {
        return out;
    }
    let mut rng = Rng::new(seed ^ 0x5343_4154_5F52_554C); // "SCAT_RUL"
    for r in 0..mesh.num_regions() {
        let roll = rng.next_f64();
        let jx = rng.next_f64();
        let jy = rng.next_f64();
        let sroll = rng.next_f64();
        if mesh.is_boundary_r(r) {
            continue;
        }
        let e = elevation.get(r).copied().unwrap_or(0.0);
        let veg = vegetation.get(r).copied().unwrap_or(0) as u32;
        let reg = region.get(r).copied().unwrap_or(0);
        let bio = biome.get(r).copied().unwrap_or(0);
        for rule in rules {
            if e < rule.elev_min || e > rule.elev_max {
                continue;
            }
            if rule.veg_mask != 0 && veg < 32 && rule.veg_mask & (1 << veg) == 0 {
                continue;
            }
            if rule.region_mask != 0 && (reg == 0 || rule.region_mask & (1 << (reg as u32 - 1)) == 0) {
                continue;
            }
            if rule.biome_mask != 0 && (bio == 0 || rule.biome_mask & (1 << (bio as u32 - 1)) == 0) {
                continue;
            }
            if roll >= rule.density {
                continue;
            }
            let pos = mesh.pos_of_r(r);
            let scale = rule.scale_min + (rule.scale_max - rule.scale_min) * sroll;
            out.push(Instance {
                x: (pos[0] + (jx - 0.5) * 6.0) as f32,
                y: (pos[1] + (jy - 0.5) * 6.0) as f32,
                z: (e * exaggeration) as f32,
                scale: scale as f32,
                species: rule.slot as f32,
            });
            break; // one decoration per cell
        }
    }
    out
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
