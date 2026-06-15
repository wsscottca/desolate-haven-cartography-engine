//! Trait-composition world model (ADR 0004): the shared base palettes, the vegetation set,
//! the per-cell trait bundle, the colour resolver, and the per-region default trait presets.
//!
//! A cell's appearance is the *culmination* of independent trait fields (held in `world::World`):
//! a `palette_family` picks one of [`base_palettes`]; `vegetation` tints the cover; `temperature`
//! drives snow/frost; `elevation` + `moisture` ramp the rest. The named "biomes" here are really
//! **Region presets** — a default trait bundle + accent + water profile per canon place.
//! Determinism: every function is lerp / averaging / threshold — no transcendentals.

/// Per-biome water/physics profile (drives rainfall + liquids; consumed by the fluid stage).
#[derive(Clone, Copy, Debug)]
pub struct WaterProfile {
    pub raininess: f32,
    pub rain_shadow: f32,
    pub evaporation: f32,
    pub flow: f32,
    pub ocean_depth: f32,
}

/// Decoration seam — thin now, filled during the scatter pass. All optional so the schema can
/// grow without breaking authored data.
#[derive(Clone, Debug, Default)]
pub struct BiomeDecor {
    pub scatter: Vec<ScatterRule>,
    pub icon: Option<u32>,
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

/// Number of named Region presets (ids `1..=BIOME_COUNT`; `0` = auto/unclassified).
pub const BIOME_COUNT: usize = 14;

// --- traits -------------------------------------------------------------------------------------

/// Vegetation cover (the `vegetation` trait enum, stored as `u8`).
pub mod veg {
    pub const BARREN: u8 = 0;
    pub const GRASS: u8 = 1;
    pub const SCRUB: u8 = 2;
    pub const FOREST: u8 = 3;
    pub const EVERGREEN: u8 = 4;
    pub const MARSH: u8 = 5;
    pub const THORN: u8 = 6;
    pub const COUNT: u8 = 7;
}

/// Shared base palette family (the `palette_family` trait enum, stored as `u8`).
pub mod fam {
    pub const VERDANT: u8 = 0;
    pub const ARID: u8 = 1;
    pub const STONE: u8 = 2;
    pub const ASHEN: u8 = 3;
    pub const FROST: u8 = 4;
    pub const WETLAND: u8 = 5;
    pub const EXOTIC: u8 = 6;
    pub const COUNT: u8 = 7;
}

/// The per-cell trait bundle (the *adjectives*). `elevation` lives in `World` (sculpt-owned).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellTraits {
    pub jaggedness: f32,       // peak sharpness
    pub relief: f32,           // foothills / hilliness amplitude
    pub foothill_falloff: f32, // width of the mountain→plain skirt
    pub erosion: f32,          // crisp ↔ eroded
    pub temperature: f32,      // 0 cold (snow/frost) … 1 hot (arid)
    pub moisture: f32,         // 0 dry … 1 wet
    pub vegetation: u8,        // see `veg`
    pub palette_family: u8,    // see `fam`
}

/// One shared light→dark terrain ramp (no vegetation — that tints on top). Six stops from the
/// canon `tokens.css`: submerged deep→shallow, then low/rock terrain, then warm/cold high caps.
#[derive(Clone, Copy, Debug)]
pub struct BasePalette {
    pub water_deep: [f32; 3],
    pub water_shallow: [f32; 3],
    pub low: [f32; 3],
    pub rock: [f32; 3],
    pub cap_warm: [f32; 3], // bare high ground (hot)
    pub cap_cold: [f32; 3], // snow / frost (cold)
}

/// 0xRRGGBB → `[r, g, b]` in 0..1 (mirrors the canon hex tokens).
fn c(hex: u32) -> [f32; 3] {
    [((hex >> 16) & 0xFF) as f32 / 255.0, ((hex >> 8) & 0xFF) as f32 / 255.0, (hex & 0xFF) as f32 / 255.0]
}
fn clamp01(t: f32) -> f32 {
    if t < 0.0 { 0.0 } else if t > 1.0 { 1.0 } else { t }
}
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// The 7 shared base palettes, indexed by `fam::*`. Tokens from `tokens.css`.
pub fn base_palettes() -> [BasePalette; fam::COUNT as usize] {
    let bp = |wd, ws, lo, rk, cw, cc| BasePalette {
        water_deep: c(wd), water_shallow: c(ws), low: c(lo), rock: c(rk), cap_warm: c(cw), cap_cold: c(cc),
    };
    [
        // Verdant — greens (temperate / forest lowlands)
        bp(0x0C1C36, 0x32708C, 0x6E9256, 0x8C8174, 0x84807E, 0xF2F6FC),
        // Arid — golds / tans (plains, dry)
        bp(0x1E486E, 0x2C688A, 0xB6AA6C, 0x8C8174, 0x84807E, 0xF2F6FC),
        // Stone — cool greys (mountains, rock)
        bp(0x0C1C36, 0x32708C, 0x6F665A, 0x84807E, 0x8C8174, 0xF2F6FC),
        // Ashen — volcanic reds / charcoals
        bp(0x7A2A1C, 0xD2542A, 0x3A3338, 0xC2462A, 0x565250, 0x84807E),
        // Frost — pale blue-whites
        bp(0x0C1C36, 0x32708C, 0x8FB9C6, 0x84807E, 0xBFE3EA, 0xF2F6FC),
        // Wetland — muddy greens / browns
        bp(0x0C1C36, 0x32708C, 0x4F5B38, 0x6E7A4B, 0x6F665A, 0xF2F6FC),
        // Exotic — fey / otherworldly (deep wood, sacred)
        bp(0x0C1C36, 0x32708C, 0x494337, 0x6F665A, 0x84807E, 0xF2F6FC),
    ]
}

/// Cover colour + tint strength for a vegetation id (Barren = no tint).
pub fn vegetation_tint(v: u8) -> ([f32; 3], f32) {
    match v {
        veg::GRASS => (c(0x7E9A50), 0.55),
        veg::SCRUB => (c(0x7A7A4A), 0.40),
        veg::FOREST => (c(0x3B6E41), 0.70),
        veg::EVERGREEN => (c(0x35623A), 0.80),
        veg::MARSH => (c(0x4F5B38), 0.70),
        veg::THORN => (c(0x8A3A30), 0.60),
        _ => ([0.0, 0.0, 0.0], 0.0), // Barren
    }
}

/// Treeline / rockline thresholds (normalized elevation) for the ground ramp.
const TREELINE: f32 = 0.45;
const ROCKLINE: f32 = 0.75;

/// Resolve a cell's ground colour: `base_palette[family]` ramped by elevation, tinted by
/// vegetation, with snow/frost driven by `temperature` (so cold *lowlands* frost over and warm
/// *peaks* stay bare). `e` normalized elevation (≈[-1.5,1.5]); `temperature`/`moisture` in 0..1.
/// Determinism-safe (lerp / clamp / multiply only).
pub fn resolve_color(base: &BasePalette, vegetation: u8, e: f32, temperature: f32, moisture: f32) -> [f32; 3] {
    if e < 0.0 {
        let mut t = clamp01(e + 1.0);
        t = t * t; // hold the dark depths
        return lerp3(base.water_deep, base.water_shallow, t);
    }
    // Cover = low terrain tinted by vegetation, slightly darkened when wet.
    let (tint, strength) = vegetation_tint(vegetation);
    let mut cover = lerp3(base.low, tint, strength);
    let wet = 1.0 - 0.10 * clamp01(moisture);
    cover = [cover[0] * wet, cover[1] * wet, cover[2] * wet];
    // Cold lowlands frost over regardless of altitude (temperature decoupled from elevation).
    let frost = clamp01((0.25 - temperature) / 0.25);
    cover = lerp3(cover, base.cap_cold, frost * 0.6);

    if e < TREELINE {
        let shade = 1.0 - 0.12 * clamp01(e / TREELINE);
        [cover[0] * shade, cover[1] * shade, cover[2] * shade]
    } else if e < ROCKLINE {
        let t = (e - TREELINE) / (ROCKLINE - TREELINE);
        lerp3(cover, base.rock, clamp01(t))
    } else {
        // Cap: warm/bare when hot, snow when cold.
        let cap = lerp3(base.cap_warm, base.cap_cold, clamp01(1.0 - temperature));
        let t = clamp01((e - ROCKLINE) / (1.0 - ROCKLINE));
        lerp3(base.rock, cap, t)
    }
}

// --- region presets (the 14 canon places) -------------------------------------------------------

/// A named canon place: its accent, default trait bundle (stamped by the Region preset), and
/// water profile. The trait bundle is a *starting point* the author paints/edits over.
pub struct BiomeDef {
    pub label: &'static str,
    pub accent: [f32; 3],
    pub traits: CellTraits,
    pub water: WaterProfile,
}

impl BiomeDef {
    /// A representative swatch colour for this region (its `--mk-*` accent).
    pub fn representative(&self) -> [f32; 3] {
        self.accent
    }
}

/// The 14 canon Region presets. Accents mirror `lore/biome-features.md`; trait bundles are
/// modest starting points (tuned live in the editor). Order: index `i` is id `i + 1`.
pub fn roster() -> [BiomeDef; BIOME_COUNT] {
    let t = |jaggedness, relief, foothill_falloff, erosion, temperature, moisture, vegetation, palette_family| CellTraits {
        jaggedness, relief, foothill_falloff, erosion, temperature, moisture, vegetation, palette_family,
    };
    let w = |raininess, rain_shadow, evaporation, flow, ocean_depth| WaterProfile {
        raininess, rain_shadow, evaporation, flow, ocean_depth,
    };
    // Magik accents (tokens.css).
    const MK_GOLD: u32 = 0xC7A24B;
    const MK_SILVER: u32 = 0xC2C8D2;
    const MK_RED: u32 = 0xB23A2E;
    const MK_BLUE: u32 = 0x2E5C9E;
    const MK_PURPLE: u32 = 0x6E5A82;
    const MK_LIGHTBLUE: u32 = 0x6FA9CE;
    const MK_ORANGE: u32 = 0xD6883A;
    const MK_BLACK: u32 = 0x100D14;
    const MK_WHITE: u32 = 0xF2F0FA;
    const MK_TEAL: u32 = 0x2E8C8C;
    const MK_GREEN: u32 = 0x5A9A4A;
    [
        BiomeDef { label: "Jagged Mountains",     accent: c(MK_GOLD),      traits: t(0.90, 0.70, 0.50, 0.40, 0.30, 0.40, veg::EVERGREEN, fam::STONE),   water: w(1.0, 1.3, 0.4, 0.30, 1.4) },
        BiomeDef { label: "Sacred Woods Plateau", accent: c(MK_BLUE),      traits: t(0.30, 0.50, 0.60, 0.30, 0.50, 0.60, veg::FOREST,    fam::VERDANT), water: w(1.1, 0.9, 0.5, 0.25, 1.2) },
        BiomeDef { label: "Great Lake",           accent: c(MK_BLUE),      traits: t(0.10, 0.20, 0.50, 0.20, 0.50, 0.90, veg::GRASS,     fam::VERDANT), water: w(1.2, 0.7, 0.8, 0.4, 1.8) },
        BiomeDef { label: "Temperate Forest",     accent: c(MK_WHITE),     traits: t(0.30, 0.50, 0.50, 0.30, 0.55, 0.60, veg::FOREST,    fam::VERDANT), water: w(1.1, 0.9, 0.6, 0.3, 1.2) },
        BiomeDef { label: "Open Plains",          accent: c(MK_WHITE),     traits: t(0.10, 0.30, 0.70, 0.30, 0.60, 0.40, veg::GRASS,     fam::ARID),    water: w(0.9, 1.0, 0.7, 0.2, 1.0) },
        BiomeDef { label: "Underdeep",            accent: c(MK_SILVER),    traits: t(0.40, 0.50, 0.50, 0.50, 0.45, 0.40, veg::SCRUB,     fam::STONE),   water: w(0.6, 1.2, 0.3, 0.2, 1.6) },
        BiomeDef { label: "Deep Wood",            accent: c(MK_PURPLE),    traits: t(0.30, 0.50, 0.50, 0.30, 0.50, 0.70, veg::EVERGREEN, fam::EXOTIC),  water: w(1.2, 0.8, 0.5, 0.3, 1.2) },
        BiomeDef { label: "Frozen Reaches",       accent: c(MK_LIGHTBLUE), traits: t(0.50, 0.40, 0.50, 0.40, 0.05, 0.50, veg::BARREN,    fam::FROST),   water: w(0.8, 1.1, 0.2, 0.2, 1.4) },
        BiomeDef { label: "Lost Isles",           accent: c(MK_TEAL),      traits: t(0.70, 0.30, 0.40, 0.60, 0.40, 0.50, veg::SCRUB,     fam::STONE),   water: w(1.2, 0.7, 0.8, 0.3, 1.6) },
        BiomeDef { label: "Blisterwood",          accent: c(MK_RED),       traits: t(0.50, 0.50, 0.50, 0.50, 0.70, 0.30, veg::THORN,     fam::ASHEN),   water: w(0.9, 1.0, 0.5, 0.25, 1.2) },
        BiomeDef { label: "Volcanic Scape",       accent: c(MK_ORANGE),    traits: t(0.80, 0.50, 0.40, 0.60, 0.95, 0.20, veg::BARREN,    fam::ASHEN),   water: w(0.5, 1.4, 0.3, 0.2, 1.4) },
        BiomeDef { label: "Blight Ruins",         accent: c(MK_BLACK),     traits: t(0.40, 0.40, 0.50, 0.60, 0.50, 0.40, veg::BARREN,    fam::ASHEN),   water: w(0.7, 1.1, 0.4, 0.2, 1.2) },
        BiomeDef { label: "Scattered Isles",      accent: c(MK_SILVER),    traits: t(0.30, 0.30, 0.50, 0.40, 0.55, 0.60, veg::GRASS,     fam::ARID),    water: w(1.2, 0.7, 0.9, 0.35, 1.7) },
        BiomeDef { label: "Marsh Bog",            accent: c(MK_GREEN),     traits: t(0.10, 0.30, 0.60, 0.30, 0.55, 0.95, veg::MARSH,     fam::WETLAND), water: w(1.3, 0.6, 0.7, 0.3, 1.3) },
    ]
}

/// Default trait bundle a Region preset stamps (id 1..=14); id 0/unknown → a neutral pass-through.
pub fn default_traits_for(biome_id: u8) -> CellTraits {
    let id = biome_id as usize;
    if id >= 1 && id <= BIOME_COUNT {
        roster()[id - 1].traits
    } else {
        CellTraits { jaggedness: 0.3, relief: 0.4, foothill_falloff: 0.5, erosion: 0.3, temperature: 0.5, moisture: 0.5, vegetation: veg::GRASS, palette_family: fam::VERDANT }
    }
}

// --- classification -----------------------------------------------------------------------------

/// A moisture proxy in `[0, 1]` from a dedicated noise channel.
pub fn moisture_at(x: f64, y: f64, width: f64, height: f64, seed: u64) -> f64 {
    let m = crate::noise::fbm2((x / width) * 3.0, (y / height) * 3.0, seed ^ 0x4D4F_4953_5455_5245, 4);
    (m * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// Auto-classify a region into a Region-preset id (`1..=BIOME_COUNT`) from elevation, moisture,
/// and normalized distance from the map center — the seed layer the author paints over.
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
