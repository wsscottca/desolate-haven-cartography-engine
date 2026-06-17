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

/// Number of named Region presets (ids `1..=REGION_COUNT`; `0` = auto/unclassified).
pub const REGION_COUNT: usize = 14;

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

/// Moisture colour response in the *Natural* view: parched cover reads a touch lighter + warmer
/// (tan), lush cover a touch darker + cooler. Kept subtle/realistic now that the dedicated Moisture
/// data view (see [`wet_ramp`]) carries precise legibility. Centered at moisture 0.5 (no shift).
const MOIST_VALUE_SWING: f32 = 0.14; // brightness: dry ×1.07 … wet ×0.93
const MOIST_WARM_SWING: f32 = 0.06;  // hue tilt: dry +0.03 R / −0.03 B … wet the reverse

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
    // Cover = low terrain tinted by vegetation.
    let (tint, strength) = vegetation_tint(vegetation);
    let mut cover = lerp3(base.low, tint, strength);
    // Moisture: parched cover reads lighter & warmer (tan); lush cover darker & cooler. Centered at
    // 0.5 (dryness 0) so a mid-moisture map stays neutral, and strong enough to read across a map.
    let dryness = 0.5 - clamp01(moisture);
    let val = 1.0 + dryness * MOIST_VALUE_SWING;
    let warm = dryness * MOIST_WARM_SWING;
    cover = [
        clamp01(cover[0] * val + warm),
        clamp01(cover[1] * val),
        clamp01(cover[2] * val - warm),
    ];
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

// --- data-view ramps (toggleable heatmap views of the same meshes) -----------------------------
//
// These recolour the terrain by a single field so the author can read/edit it directly, decoupled
// from the composed Natural colour. All are lerp-only (deterministic). `t`/`e` come straight from
// the per-cell trait fields; the data views skip neighbour smoothing so the readout is exact.

/// Two-stop lerp through `lo → mid → hot` at `t∈[0,1]` (mid at 0.5).
fn ramp3(lo: [f32; 3], mid: [f32; 3], hi: [f32; 3], t: f32) -> [f32; 3] {
    let t = clamp01(t);
    if t < 0.5 { lerp3(lo, mid, t * 2.0) } else { lerp3(mid, hi, (t - 0.5) * 2.0) }
}

/// Temperature view: cold → hot as blue → pale → red.
pub fn heat_ramp(t: f32) -> [f32; 3] {
    ramp3(c(0x2C5AA8), c(0xEDE6B0), c(0xC23A2A), t)
}

/// Moisture view: dry → wet as tan → green → teal-blue.
pub fn wet_ramp(t: f32) -> [f32; 3] {
    ramp3(c(0xC9A86A), c(0x6FA05A), c(0x1E6F8C), t)
}

/// Elevation view: a hypsometric ramp; `e` normalized (≈[-1.5, 1.5]) → deep water … snow peak.
pub fn elevation_ramp(e: f32) -> [f32; 3] {
    let stops = [c(0x0A1A3A), c(0x2E6E9E), c(0x4E8C50), c(0x8C7A4A), c(0xF2F2F6)];
    let x = clamp01((e + 1.5) / 3.0) * (stops.len() - 1) as f32;
    let i = (x as usize).min(stops.len() - 2);
    lerp3(stops[i], stops[i + 1], clamp01(x - i as f32))
}

// --- region presets (the 14 canon places) -------------------------------------------------------

/// A named canon place: its accent, default trait bundle (stamped by the Region preset), and
/// water profile. The trait bundle is a *starting point* the author paints/edits over.
pub struct RegionDef {
    pub label: &'static str,
    pub accent: [f32; 3],
    pub traits: CellTraits,
    pub water: WaterProfile,
}

impl RegionDef {
    /// A representative swatch colour for this region (its `--mk-*` accent).
    pub fn representative(&self) -> [f32; 3] {
        self.accent
    }
}

/// The 14 canon Region presets. Accents mirror `lore/biome-features.md`; trait bundles are
/// modest starting points (tuned live in the editor). Order: index `i` is id `i + 1`.
pub fn region_presets() -> [RegionDef; REGION_COUNT] {
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
    // traits: t(jaggedness, relief, foothill_falloff, erosion, temperature, moisture, vegetation, palette_family).
    // Tuned per the canon biome catalog (guide `lore/biome-features.md`) so each place reads distinctly while
    // a wide `foothill_falloff` lets mountains taper into hills (the "foothills" transition). Accents + labels
    // are canon and unchanged; only the landform/water knobs move. Liquids: Volcanic ponds lava, Marsh shallow
    // murky pools (see `default_liquid_kind` / `region_pond_cap`).
    [
        // Jagged peaks above a broad green hill ring (high jaggedness + wide foothill skirt, crisp/low erosion).
        RegionDef { label: "Jagged Mountains",     accent: c(MK_GOLD),      traits: t(0.92, 0.68, 0.75, 0.28, 0.28, 0.42, veg::EVERGREEN, fam::STONE),   water: w(1.0, 1.3, 0.4, 0.30, 1.4) },
        // A flat-topped elevated plateau (low jaggedness, very wide skirt → table-land, not peaks).
        RegionDef { label: "Sacred Woods Plateau", accent: c(MK_BLUE),      traits: t(0.18, 0.42, 0.82, 0.35, 0.50, 0.62, veg::FOREST,    fam::VERDANT), water: w(1.1, 0.9, 0.5, 0.25, 1.2) },
        RegionDef { label: "Great Lake",           accent: c(MK_BLUE),      traits: t(0.10, 0.20, 0.55, 0.20, 0.50, 0.90, veg::GRASS,     fam::VERDANT), water: w(1.2, 0.7, 0.8, 0.4, 1.8) },
        // Rolling forested hills.
        RegionDef { label: "Temperate Forest",     accent: c(MK_WHITE),     traits: t(0.35, 0.55, 0.60, 0.30, 0.55, 0.62, veg::FOREST,    fam::VERDANT), water: w(1.1, 0.9, 0.6, 0.3, 1.2) },
        // Big-sky flatland (very low relief, very wide skirt) — the foothill apron mountains taper into.
        RegionDef { label: "Open Plains",          accent: c(MK_WHITE),     traits: t(0.06, 0.20, 0.85, 0.35, 0.62, 0.38, veg::GRASS,     fam::ARID),    water: w(0.9, 1.0, 0.7, 0.2, 1.0) },
        // Rolling-hill surface over the cavern/mine network (low jaggedness, moderate relief = rolling, per canon).
        RegionDef { label: "Underdeep",            accent: c(MK_SILVER),    traits: t(0.22, 0.55, 0.55, 0.50, 0.45, 0.42, veg::SCRUB,     fam::STONE),   water: w(0.6, 1.2, 0.3, 0.2, 1.6) },
        // Dense old-growth on rolling ground.
        RegionDef { label: "Deep Wood",            accent: c(MK_PURPLE),    traits: t(0.30, 0.55, 0.55, 0.30, 0.48, 0.72, veg::EVERGREEN, fam::EXOTIC),  water: w(1.2, 0.8, 0.5, 0.3, 1.2) },
        // Frozen buttes.
        RegionDef { label: "Frozen Reaches",       accent: c(MK_LIGHTBLUE), traits: t(0.55, 0.45, 0.50, 0.40, 0.05, 0.50, veg::BARREN,    fam::FROST),   water: w(0.8, 1.1, 0.2, 0.2, 1.4) },
        // Tall ROUNDED rock knobs rising from the sea (high relief = tall, very low jaggedness + high erosion =
        // smooth/rounded, narrow skirt → distinct isles). Deliberately distinct from the sharp Jagged Mountains
        // (per the user's steer — guide canon updated to match: rounded Pandora-style rock, not jagged spires).
        RegionDef { label: "Lost Isles",           accent: c(MK_TEAL),      traits: t(0.18, 0.88, 0.25, 0.65, 0.42, 0.55, veg::SCRUB,     fam::STONE),   water: w(1.2, 0.7, 0.9, 0.3, 2.0) },
        RegionDef { label: "Blisterwood",          accent: c(MK_RED),       traits: t(0.55, 0.52, 0.45, 0.50, 0.70, 0.30, veg::THORN,     fam::ASHEN),   water: w(0.9, 1.0, 0.5, 0.25, 1.2) },
        // Active volcanic peaks — jagged steep cones (narrow skirt) with lava in the calderas.
        RegionDef { label: "Volcanic Scape",       accent: c(MK_ORANGE),    traits: t(0.88, 0.62, 0.32, 0.45, 0.95, 0.18, veg::BARREN,    fam::ASHEN),   water: w(0.5, 1.4, 0.3, 0.2, 1.4) },
        // Grey blighted ruins — STONE family for the canon grey `--blight`, not the volcanic-red ashen.
        RegionDef { label: "Blight Ruins",         accent: c(MK_BLACK),     traits: t(0.40, 0.42, 0.50, 0.62, 0.50, 0.38, veg::BARREN,    fam::STONE),   water: w(0.7, 1.1, 0.4, 0.2, 1.2) },
        // Flat wet lowland threaded by water channels (low relief, wide skirt, wet) — the port city.
        RegionDef { label: "Scattered Isles",      accent: c(MK_SILVER),    traits: t(0.10, 0.20, 0.80, 0.40, 0.58, 0.68, veg::GRASS,     fam::ARID),    water: w(1.2, 0.7, 0.7, 0.45, 1.7) },
        // Flat sodden ground holding shallow murky pools.
        RegionDef { label: "Marsh Bog",            accent: c(MK_GREEN),     traits: t(0.08, 0.20, 0.72, 0.30, 0.55, 0.95, veg::MARSH,     fam::WETLAND), water: w(1.3, 0.6, 0.7, 0.3, 1.3) },
    ]
}

/// Default trait bundle a Region preset stamps (id 1..=14); id 0/unknown → a neutral pass-through.
/// This is the **Region → Traits** tie: each canon Region seeds the per-cell trait primitives the
/// author then paints/blends, and those traits in turn compose the emergent [`biome_label`].
pub fn default_traits_for(region_id: u8) -> CellTraits {
    let id = region_id as usize;
    if id >= 1 && id <= REGION_COUNT {
        region_presets()[id - 1].traits
    } else {
        CellTraits { jaggedness: 0.3, relief: 0.4, foothill_falloff: 0.5, erosion: 0.3, temperature: 0.5, moisture: 0.5, vegetation: veg::GRASS, palette_family: fam::VERDANT }
    }
}

/// Per-Region **base elevation target** (normalized, the macro relief trunk the canon generator
/// diffuses + rides low-frequency noise on). Mountains sit high, the Great Lake + ocean sit below
/// sea level so [`crate::fluid::sea_fill`] floods them, coastal isles sit just under. Id `0` = ocean.
/// Starting values — tuned live in the editor. Order matches [`region_presets`] (id = index + 1).
pub fn base_elevation_for(region_id: u8) -> f64 {
    // Compressed land trunk: the highlands are pulled toward the lowland band (each value > 0.20
    // mapped `0.20 + (old − 0.20)·0.55`) so regions keep their order/identity but read as mountains &
    // hills rising from gentler ground rather than tall stepped plateaus — the within-region landform
    // relief baked by `shape_terrain` now dominates the base steps. Lowlands/coast (≤ 0.20) and the
    // water basins (Great Lake, ocean) are left below so `sea_fill` still floods them. Tuned live.
    match region_id {
        1 => 0.53,  // Jagged Mountains — jagged peaks ringed by rolling hills (was 0.80)
        2 => 0.34,  // Sacred Woods & Plateau — an elevated plateau (was 0.46)
        3 => -0.10, // Great Lake (perched basin — floor above the ocean, filled to its pour point)
        4 => 0.30,  // Temperate Forest — mountainous forest band (was 0.38)
        5 => 0.08,  // Open Plains
        6 => 0.22,  // Underdeep — rolling-hill surface (was 0.24)
        7 => 0.24,  // Deep Wood — old-growth lowland forest (was 0.28)
        8 => 0.33,  // Frozen Reaches — frozen buttes (was 0.44)
        9 => 0.05,  // Lost Isles (low island land — rim drowns the coast into spires)
        10 => 0.26, // Blisterwood (was 0.30)
        11 => 0.43, // Volcanic Scape — active volcanic mountains (was 0.62)
        12 => 0.20, // Blight Ruins
        13 => 0.02, // Scattered Isles (archipelago — noise + rim break it into isles)
        14 => 0.02, // Marsh & Bog (wet lowland)
        _ => -1.00, // 0 / unknown → open ocean
    }
}

/// Per-Region **default lake-fill threshold** (normalized basin depth) — the biome/trait tie for the
/// lake tier. Derived from the region's moisture trait: a wet place (Great Lake, Marsh Bog) ponds with
/// a shallow basin, while an arid one (Volcanic Scape, Blight Ruins) resists ponding. The editor stores
/// these per region (id `1..=14`) and exposes a slider to fine-tune each; [`crate::world::World::fill_lakes`]
/// reads the per-region value (further modulated by each cell's local moisture). Id `0` (ocean) is unused.
pub fn default_lake_min_depth(region_id: u8) -> f64 {
    // Volcanic Scape ponds **lava** (see [`default_liquid_kind`]); give it a low threshold so molten pools
    // gather in the calderas even though it's arid (the moisture rule would otherwise resist ponding).
    if region_id == 11 {
        return 0.03;
    }
    let moisture = default_traits_for(region_id).moisture as f64; // 0 dry … 1 wet
    // moisture 1 → 0.004 (ponds readily); moisture 0 → 0.18 (only deep basins hold water).
    const LO: f64 = 0.004;
    const HI: f64 = 0.18;
    HI + (LO - HI) * moisture.clamp(0.0, 1.0)
}

/// The liquid a Region's ponded basins fill with (see [`crate::liquids::LiquidType`], stored as `u8`):
/// the **Volcanic Scape** fills with lava/magma, every other Region with water. Read by
/// [`crate::world::World::fill_lakes`] so a fresh map shows molten calderas in the volcanic land.
pub fn default_liquid_kind(region_id: u8) -> u8 {
    match region_id {
        11 => crate::liquids::LiquidType::Lava as u8, // Volcanic Scape
        _ => crate::liquids::LiquidType::Water as u8,
    }
}

/// Cap on a Region's ponded depth (normalized) — `f64::INFINITY` for a normal lake/pour-point fill.
/// The **Marsh & Bog** caps shallow so its pools read as shallow murky water, not deep lakes (the
/// wetland palette already gives the murk). Read by [`crate::world::World::fill_lakes`].
pub fn region_pond_cap(region_id: u8) -> f64 {
    match region_id {
        // Marsh & Bog — mostly shallow murky water, but the cap allows the canon "deeper bog pools" too
        // (still well short of a full pour-point lake). ~0.02 ≈ 33 m at the 5 km height scale.
        14 => 0.02,
        _ => f64::INFINITY,
    }
}

/// Per-Region **default river threshold** (fraction of the basin's peak flow a cell must carry before
/// it becomes a trunk river) — the biome/trait tie for the river tier, the mirror of
/// [`default_lake_min_depth`]. Derived from the region's moisture trait: a wet place rivers readily
/// (low threshold ⇒ a denser network), an arid one only along the major valleys (high threshold).
/// The editor stores these per region (id `1..=14`) and exposes a slider per region;
/// [`crate::world::World::generate_rivers`] reads the per-region value to gate each cell. Id `0` is unused.
pub fn default_river_threshold(region_id: u8) -> f64 {
    // Scattered Isles is a port laced with waterways — drop its threshold so a dense channel network
    // (not just the trunk) carves through the lowland.
    if region_id == 13 {
        return 0.02;
    }
    let moisture = default_traits_for(region_id).moisture as f64; // 0 dry … 1 wet
    // moisture 1 → 0.02 (fine tributaries appear); moisture 0 → 0.10 (only the trunk valleys carry).
    const LO: f64 = 0.02;
    const HI: f64 = 0.10;
    HI + (LO - HI) * moisture.clamp(0.0, 1.0)
}

/// A representative colour for the **emergent biome** of a cell, derived purely from its trait
/// primitives (the *Traits → Biome* tie, the numeric companion to [`biome_label`]). Used by the
/// `VIEW_BIOME` data view so it reads the *ecological* biome (distinct from `VIEW_REGION`, which
/// shows the authored canon place). Threshold/lerp only — deterministic.
pub fn emergent_biome_color(e: f32, temperature: f32, moisture: f32, vegetation: u8) -> [f32; 3] {
    if e < -0.05 {
        return c(0x2E5C9E); // open water
    }
    if temperature < 0.30 {
        return c(0xBFE3EA); // frozen / tundra
    }
    if vegetation == veg::MARSH || moisture > 0.82 {
        return c(0x4F5B38); // marsh / wetland
    }
    if temperature > 0.72 && moisture < 0.35 {
        return c(0xC9A86A); // arid / desert
    }
    match vegetation {
        veg::EVERGREEN => c(0x35623A), // taiga / boreal
        veg::FOREST => {
            if temperature > 0.6 && moisture > 0.6 { c(0x2F6B3A) } else { c(0x3B6E41) } // jungle / temperate forest
        }
        veg::THORN => c(0x8A3A30),  // thornland
        veg::SCRUB => c(0x7A7A4A),  // shrubland
        veg::GRASS => c(0x7E9A50),  // grassland
        _ => c(0x9A8E72),           // barrens / bare ground
    }
}

// --- classification -----------------------------------------------------------------------------

/// A moisture proxy in `[0, 1]` from a dedicated noise channel.
pub fn moisture_at(x: f64, y: f64, width: f64, height: f64, seed: u64) -> f64 {
    let m = crate::noise::fbm2((x / width) * 3.0, (y / height) * 3.0, seed ^ 0x4D4F_4953_5455_5245, 4);
    (m * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// Auto-classify a region into a Region-preset id (`1..=REGION_COUNT`) from elevation, moisture,
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

/// Compose a human descriptor for the **emergent biome** from a cell's traits — the "biome
/// describes" tier of the trait model (ADR 0004), off the render path (HUD / cursor readout).
/// `climate · cover · landform` for land; a water phrase when submerged. This is mechanical
/// *description* generated from the dials — **not** canon place lore (that lives in the guide; the
/// 14 named places are Regions, not this). Threshold-only, so it's deterministic and allocation is
/// the only cost.
pub fn biome_label(
    elevation: f64,
    jaggedness: f64,
    relief: f64,
    temperature: f64,
    moisture: f64,
    vegetation: u8,
) -> String {
    let climate = if temperature < 0.30 {
        "frozen"
    } else if temperature > 0.72 {
        if moisture < 0.35 { "arid" } else { "tropical" }
    } else if moisture > 0.66 {
        "humid"
    } else if moisture < 0.30 {
        "parched"
    } else {
        "temperate"
    };

    // Submerged cells read as water (cover/landform don't apply below the surface).
    if elevation < -0.25 {
        return format!("{climate} deep water");
    }
    if elevation < -0.05 {
        return format!("{climate} shallows");
    }

    let cover = match vegetation {
        veg::GRASS => "grassland",
        veg::SCRUB => "scrub",
        veg::FOREST => "forest",
        veg::EVERGREEN => "taiga",
        veg::MARSH => "marsh",
        veg::THORN => "thornland",
        _ => "barrens", // BARREN / unknown
    };

    let landform = if elevation > 0.55 {
        if jaggedness > 0.55 { "jagged peaks" } else { "highlands" }
    } else if elevation > 0.25 {
        if relief > 0.5 { "hills" } else { "uplands" }
    } else if elevation > 0.05 {
        if relief > 0.55 { "downs" } else { "lowlands" }
    } else {
        "flats"
    };

    format!("{climate} {cover} {landform}")
}
