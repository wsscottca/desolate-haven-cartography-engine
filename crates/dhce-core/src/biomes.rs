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
