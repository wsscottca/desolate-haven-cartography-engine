//! Biome roster, classification, and the per-biome decoration model.
//!
//! Carries the user's authored 14-biome data (colors, landform + water physics) and
//! the over-engineering seam for the coming biome pass: scatter / icon / region /
//! fog / particles. Phase 5 fills the roster + classifier; Phase 7 uses the decor.

/// Per-biome landform shaping profile (drives elevation shaping).
#[derive(Clone, Copy, Debug)]
pub struct Landform {
    pub peak_shape: f32,
    pub hill_amp: f32,
    pub valley_floor: f32,
    pub roughness: f32,
}

/// Per-biome water/physics profile (drives rainfall + liquids).
#[derive(Clone, Copy, Debug)]
pub struct WaterProfile {
    pub raininess: f32,
    pub rain_shadow: f32,
    pub evaporation: f32,
    pub flow: f32,
    pub ocean_depth: f32,
}

/// Decoration seam — thin now, filled during the biome pass (Phase 7). All optional
/// so the schema can grow without breaking authored data.
#[derive(Clone, Debug, Default)]
pub struct BiomeDecor {
    /// Procedural sprite scatter rules (rocks/trees): species + gating.
    pub scatter: Vec<ScatterRule>,
    /// Procedural / SVG biome icon id, if any.
    pub icon: Option<u32>,
    /// Per-biome fog: linear color, density, height.
    pub fog: Option<([f32; 3], f32, f32)>,
}

/// One procedural-scatter rule for a biome.
#[derive(Clone, Copy, Debug)]
pub struct ScatterRule {
    pub species: u32,
    pub density: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub slope_min: f32,
    pub slope_max: f32,
    pub elev_min: f32,
    pub elev_max: f32,
}

/// Number of named biomes (ids `1..=BIOME_COUNT`; `0` means "auto-classified").
pub const BIOME_COUNT: usize = 14;

/// A biome's identity + default appearance and (future) physics.
pub struct BiomeDef {
    pub label: &'static str,
    pub color: [f32; 3],
    pub landform: Landform,
    pub water: WaterProfile,
}

/// The 14-biome roster — the user's Sundered Vale palette (recolorable in the
/// editor). Order is significant: index `i` is biome id `i + 1`.
pub fn roster() -> [BiomeDef; BIOME_COUNT] {
    let l = |peak_shape, hill_amp, valley_floor, roughness| Landform {
        peak_shape,
        hill_amp,
        valley_floor,
        roughness,
    };
    let w = |raininess, rain_shadow, evaporation, flow, ocean_depth| WaterProfile {
        raininess,
        rain_shadow,
        evaporation,
        flow,
        ocean_depth,
    };
    [
        BiomeDef { label: "Jagged Mountains", color: [0.518, 0.502, 0.494], landform: l(1.25, 1.15, 0.0, 1.1), water: w(1.0, 1.3, 0.4, 0.30, 1.4) },
        BiomeDef { label: "Sacred Woods Plateau", color: [0.306, 0.420, 0.290], landform: l(0.8, 1.0, 0.1, 0.9), water: w(1.1, 0.9, 0.5, 0.25, 1.2) },
        BiomeDef { label: "Great Lake", color: [0.118, 0.282, 0.431], landform: l(0.6, 0.5, -0.3, 0.6), water: w(1.2, 0.7, 0.8, 0.4, 1.8) },
        BiomeDef { label: "Temperate Forest", color: [0.243, 0.369, 0.216], landform: l(0.9, 1.0, 0.0, 1.0), water: w(1.1, 0.9, 0.6, 0.3, 1.2) },
        BiomeDef { label: "Open Plains", color: [0.541, 0.545, 0.341], landform: l(0.8, 0.7, 0.05, 0.7), water: w(0.9, 1.0, 0.7, 0.2, 1.0) },
        BiomeDef { label: "Underdeep", color: [0.180, 0.165, 0.227], landform: l(1.1, 1.0, -0.2, 1.2), water: w(0.6, 1.2, 0.3, 0.2, 1.6) },
        BiomeDef { label: "Deep Wood", color: [0.169, 0.259, 0.149], landform: l(0.95, 1.1, 0.0, 1.1), water: w(1.2, 0.8, 0.5, 0.3, 1.2) },
        BiomeDef { label: "Frozen Reaches", color: [0.788, 0.839, 0.871], landform: l(1.1, 1.0, 0.0, 1.0), water: w(0.8, 1.1, 0.2, 0.2, 1.4) },
        BiomeDef { label: "Lost Isles", color: [0.357, 0.549, 0.478], landform: l(0.7, 0.8, -0.1, 0.8), water: w(1.2, 0.7, 0.8, 0.3, 1.6) },
        BiomeDef { label: "Blisterwood", color: [0.431, 0.227, 0.306], landform: l(1.0, 1.1, 0.0, 1.2), water: w(0.9, 1.0, 0.5, 0.25, 1.2) },
        BiomeDef { label: "Volcanic Scape", color: [0.420, 0.180, 0.133], landform: l(1.4, 1.0, 0.0, 1.3), water: w(0.5, 1.4, 0.3, 0.2, 1.4) },
        BiomeDef { label: "Blight Ruins", color: [0.353, 0.329, 0.275], landform: l(1.0, 0.9, 0.0, 1.0), water: w(0.7, 1.1, 0.4, 0.2, 1.2) },
        BiomeDef { label: "Scattered Isles", color: [0.478, 0.627, 0.659], landform: l(0.6, 0.7, -0.2, 0.7), water: w(1.2, 0.7, 0.9, 0.35, 1.7) },
        BiomeDef { label: "Marsh Bog", color: [0.290, 0.322, 0.212], landform: l(0.6, 0.7, -0.1, 0.8), water: w(1.3, 0.6, 0.7, 0.3, 1.3) },
    ]
}

/// A moisture proxy in `[0, 1]` from a dedicated noise channel.
pub fn moisture_at(x: f64, y: f64, width: f64, height: f64, seed: u64) -> f64 {
    let m = crate::noise::fbm2((x / width) * 3.0, (y / height) * 3.0, seed ^ 0x4D4F_4953_5455_5245, 4);
    (m * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// Auto-classify a region into a biome id (`1..=BIOME_COUNT`) from elevation,
/// moisture, and normalized distance from the map center. The exotic biomes
/// (Underdeep, Blisterwood, Volcanic, Blight) are reserved for manual painting.
pub fn classify(elevation: f64, moisture: f64, dist_center: f64) -> u8 {
    if elevation < -0.12 {
        return if dist_center > 0.85 { 13 } else { 3 }; // Scattered Isles rim / Great Lake
    }
    if elevation < 0.0 {
        return if moisture > 0.6 { 14 } else { 9 }; // Marsh Bog / Lost Isles (shoreline)
    }
    if elevation > 0.6 {
        return if dist_center > 0.7 || moisture < 0.35 { 8 } else { 1 }; // Frozen Reaches / Jagged
    }
    if elevation > 0.35 {
        return if moisture > 0.55 { 2 } else { 1 }; // Sacred Woods Plateau / Jagged foothills
    }
    if moisture > 0.66 {
        return 7; // Deep Wood
    }
    if moisture > 0.45 {
        return 4; // Temperate Forest
    }
    5 // Open Plains
}
