//! Trait substrate (ADR 0004 tier 1): the 7 shared base palettes + the `resolve_color` ramp.
//! These lock cohesion-by-shared-structure, snow-by-temperature, vegetation tinting, and
//! determinism — and that the Region presets carry sensible default trait bundles.

use dhce_core::regions::{self, base_palettes, biome_label, resolve_color, veg};

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[test]
fn biome_label_describes_the_trait_composition() {
    // climate · cover · landform on land.
    let warm_forest = biome_label(0.30, 0.2, 0.6, 0.50, 0.60, veg::FOREST);
    assert!(warm_forest.contains("forest") && warm_forest.contains("hills"), "{warm_forest}");
    assert!(warm_forest.starts_with("temperate"), "{warm_forest}");

    // cold, sharp, high → frozen jagged peaks.
    let frozen_peak = biome_label(0.80, 0.8, 0.5, 0.10, 0.50, veg::BARREN);
    assert!(frozen_peak.contains("frozen") && frozen_peak.contains("jagged peaks"), "{frozen_peak}");

    // hot + dry → arid; submerged → a water phrase (no cover/landform).
    let arid = biome_label(0.10, 0.2, 0.2, 0.85, 0.10, veg::SCRUB);
    assert!(arid.starts_with("arid"), "{arid}");
    let water = biome_label(-0.40, 0.0, 0.0, 0.50, 0.50, veg::GRASS);
    assert!(water.contains("water") && !water.contains("grass"), "{water}");
}

#[test]
fn there_are_seven_distinct_base_palettes() {
    let bp = base_palettes();
    assert_eq!(bp.len(), regions::fam::COUNT as usize);
    assert_eq!(bp.len(), 7);
    // Verdant and Ashen lows must read very differently (green vs charcoal).
    let verdant = bp[regions::fam::VERDANT as usize].low;
    let ashen = bp[regions::fam::ASHEN as usize].low;
    assert!(verdant != ashen, "base palettes are distinct");
    assert!(verdant[1] > ashen[1], "verdant low is greener than ashen");
}

#[test]
fn ramp_is_water_below_sea_and_cover_above() {
    let base = base_palettes()[regions::fam::VERDANT as usize];
    let sea = resolve_color(&base, regions::veg::GRASS, -0.5, 0.5, 0.5);
    let land = resolve_color(&base, regions::veg::GRASS, 0.1, 0.5, 0.5);
    assert!(sea[2] > sea[0], "below sea is blue-dominant: {sea:?}");
    assert!(land[1] >= land[2], "low land is green-leaning: {land:?}");
}

#[test]
fn snow_follows_temperature_not_just_altitude() {
    let base = base_palettes()[regions::fam::VERDANT as usize];
    // High ground: cold caps to snow (bright), hot caps to bare stone (darker).
    let high_cold = resolve_color(&base, regions::veg::BARREN, 1.0, 0.0, 0.5);
    let high_hot = resolve_color(&base, regions::veg::BARREN, 1.0, 1.0, 0.5);
    assert!(luma(high_cold) > luma(high_hot) + 0.2, "cold peak is snowier: {high_cold:?} vs {high_hot:?}");
    // Cold *lowland* frosts over too (temperature decoupled from elevation).
    let low_cold = resolve_color(&base, regions::veg::GRASS, 0.15, 0.0, 0.5);
    let low_warm = resolve_color(&base, regions::veg::GRASS, 0.15, 0.8, 0.5);
    assert!(luma(low_cold) > luma(low_warm), "cold lowland frosts lighter: {low_cold:?} vs {low_warm:?}");
}

#[test]
fn vegetation_tints_the_cover() {
    let base = base_palettes()[regions::fam::VERDANT as usize];
    let barren = resolve_color(&base, regions::veg::BARREN, 0.2, 0.5, 0.5);
    let forest = resolve_color(&base, regions::veg::FOREST, 0.2, 0.5, 0.5);
    assert!(barren != forest, "vegetation changes the cover colour");
}

#[test]
fn moisture_shifts_the_cover_visibly() {
    let base = base_palettes()[regions::fam::VERDANT as usize];
    let dry = resolve_color(&base, regions::veg::GRASS, 0.2, 0.5, 0.0);
    let wet = resolve_color(&base, regions::veg::GRASS, 0.2, 0.5, 1.0);
    // Parched cover reads clearly lighter than lush cover — a legible swing, not the old ~10%.
    assert!(luma(dry) > luma(wet) + 0.05, "dry cover is visibly lighter than wet: {dry:?} vs {wet:?}");
    // …and warmer (more red- vs blue-leaning) when dry.
    assert!(dry[0] - dry[2] > wet[0] - wet[2], "dry cover tilts warmer than wet: {dry:?} vs {wet:?}");
    // Mid moisture is the neutral midpoint (no shift), so existing maps keep their overall look.
    let mid = resolve_color(&base, regions::veg::GRASS, 0.2, 0.5, 0.5);
    assert!(luma(mid) < luma(dry) && luma(mid) > luma(wet), "0.5 sits between dry and wet");
}

#[test]
fn resolve_is_deterministic() {
    let base = base_palettes()[regions::fam::STONE as usize];
    let a = resolve_color(&base, regions::veg::SCRUB, 0.3, 0.4, 0.6);
    let b = resolve_color(&base, regions::veg::SCRUB, 0.3, 0.4, 0.6);
    assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "same inputs → bit-identical colour");
}

#[test]
fn lake_threshold_tracks_region_moisture() {
    use dhce_core::regions::default_lake_min_depth;
    // Wet regions pond with a shallower basin than dry ones (lower threshold). Great Lake (3, very
    // wet) and Marsh Bog (14, wettest) sit well below Volcanic Scape (11, arid) and Open Plains (5).
    let lake = default_lake_min_depth(3);
    let marsh = default_lake_min_depth(14);
    let volcanic = default_lake_min_depth(11);
    let plains = default_lake_min_depth(5);
    assert!(lake < plains, "the wet Great Lake ponds easier than the plains ({lake} < {plains})");
    assert!(marsh < plains, "the marsh ponds easier than the plains ({marsh} < {plains})");
    assert!(plains < volcanic, "arid Volcanic Scape resists ponding more than the plains ({plains} < {volcanic})");
    assert!(lake > 0.0 && volcanic > 0.0, "thresholds are positive");
}

#[test]
fn region_presets_carry_sensible_defaults() {
    let r = regions::region_presets();
    assert_eq!(r.len(), regions::REGION_COUNT);
    // Volcanic Scape (id 11) is Ashen + hot + barren; Frozen Reaches (id 8) is Frost + cold.
    let volcanic = r[10].traits;
    assert_eq!(volcanic.palette_family, regions::fam::ASHEN);
    assert!(volcanic.temperature > 0.8, "volcanic is hot");
    let frozen = r[7].traits;
    assert_eq!(frozen.palette_family, regions::fam::FROST);
    assert!(frozen.temperature < 0.2, "frozen is cold");
    // default_traits_for round-trips the roster bundle.
    assert_eq!(regions::default_traits_for(11), volcanic);
}
