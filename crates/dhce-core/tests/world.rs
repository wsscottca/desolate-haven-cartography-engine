//! `World` is the authoring source of truth shared by both adapters. These tests lock
//! the cross-target contract for it: a build + edit sequence must be reproducible
//! bit-for-bit, the stream pass must stay idempotent, and the render surface must be
//! the right shape. (The same `World` drives `dhce-wasm` and `dhce-godot`, so anything
//! that diverges here would desync the tool from the game.)

use dhce_core::world::World;

fn built() -> World {
    let mut w = World::new();
    w.build(1000.0, 1000.0, 50.0, 7, 5);
    w
}

/// A realistic-scale (8 km) canon world — territories are large relative to the border-grading width,
/// so drainage and relief read clearly (a 1 km map washes the basins out). Mirrors the canon test.
fn built_canon() -> World {
    let mut w = World::new();
    w.build(8000.0, 8000.0, 40.0, 7, 5);
    w
}

#[test]
fn build_is_deterministic() {
    let a = built();
    let b = built();

    let ea: Vec<u32> = a.elevation_export().iter().map(|v| v.to_bits()).collect();
    let eb: Vec<u32> = b.elevation_export().iter().map(|v| v.to_bits()).collect();
    assert_eq!(ea, eb, "same seed must yield bit-identical elevation");
    assert_eq!(a.biome_export(), b.biome_export(), "same seed must yield identical biomes");
    assert!(a.region_count() > 0, "build should produce regions");
}

#[test]
fn canon_build_has_ocean_edges_a_ne_lake_and_all_regions() {
    // The canon two-tier water model: every region appears, the edges are OCEAN, and the Great Lake
    // is a PERCHED lake NE of center — its floor sits above the ocean, filled to its pour point (not
    // a flooded sea-level basin). Built at a realistic scale (8 km) so territories are large relative
    // to the border-grading width (a 1 km map would wash the lake basin out).
    let mut w = World::new();
    w.build(8000.0, 8000.0, 40.0, 7, 5);
    let sea = -0.4; // global ocean level: above the ocean floor (~-1), below the lake floor (~-0.1)
    w.set_sea_level(sea);
    w.fill_lakes();

    // All 14 canon regions are present in the territory map.
    let region_ids = w.region_export();
    for id in 1..=14u8 {
        assert!(region_ids.iter().any(|&r| r == id), "canon region {id} is present");
    }

    // Edges are ocean: terrain below the sea level.
    for &(x, y) in &[(4000.0, 160.0), (4000.0, 7840.0), (160.0, 4000.0), (7840.0, 4000.0)] {
        let h = w.height_at(x, y).expect("edge sample resolves a region");
        assert!(h < sea, "edge ({x}, {y}) is ocean, got h={h}");
    }

    // The Great Lake (NE of center) is a *perched* lake: its floor is ABOVE the ocean and it holds
    // standing water — not flooded down to sea level.
    assert_eq!(w.region_id_at(4400.0, 3600.0), 3, "the NE-of-center point is the Great Lake");
    let lake_floor = w.height_at(4400.0, 3600.0).expect("lake resolves");
    assert!(lake_floor > sea, "the Great Lake floor is perched above the ocean, got {lake_floor}");
    let depths = w.liquid_depth_export();
    let elev = w.elevation_export();
    let lake_has_perched_water = (0..region_ids.len())
        .any(|r| region_ids[r] == 3 && depths[r] as f64 > 1e-3 && elev[r] as f64 > sea);
    assert!(lake_has_perched_water, "the Great Lake holds perched water above the ocean");

    // Jagged Mountains (north) is dry highland standing above the ocean.
    assert_eq!(w.region_id_at(4000.0, 1600.0), 1, "the north is Jagged Mountains");
    assert!(w.height_at(4000.0, 1600.0).unwrap() > sea, "Jagged Mountains stand above the ocean");
}

#[test]
fn climate_lapse_cools_high_ground() {
    // #1 Elevation→Temperature: a stronger lapse rate cools elevated ground. Sample the Volcanic
    // Scape (a warm region with a high base elevation) so its temperature stays in range — a cold
    // peak (e.g. Jagged) would already be clamped to 0 at the default lapse.
    let mut w = World::new();
    w.build(4000.0, 4000.0, 30.0, 7, 5);
    let (vx, vy) = (1600.0, 3040.0); // Volcanic Scape anchor (0.40, 0.76)
    assert_eq!(w.region_id_at(vx, vy), 11, "the sample sits in the Volcanic Scape");
    assert!(w.height_at(vx, vy).unwrap() > 0.1, "the sample is elevated land");
    let t0 = w.trait_at(vx, vy, 4).expect("temperature at the sample"); // trait 4 = temperature
    w.set_lapse_rate(1.4); // stronger than the 0.6 default
    w.recompute_climate();
    let t1 = w.trait_at(vx, vy, 4).expect("temperature at the sample");
    assert!(t1 < t0, "a stronger lapse rate cools elevated ground ({t1} < {t0})");
}

#[test]
fn climate_moisture_is_deterministic_and_orographic_has_effect() {
    // #2 Orographic→Moisture: the derivation is deterministic, and the orographic weight changes the
    // moisture field (windward-wet / lee-dry pull).
    let mk = || {
        let mut w = World::new();
        w.build(4000.0, 4000.0, 30.0, 7, 5);
        w
    };
    let bits = |v: Vec<f32>| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    let a = mk();
    let b = mk();
    assert_eq!(bits(a.trait_field_export(4)), bits(b.trait_field_export(4)), "temperature deterministic");
    assert_eq!(bits(a.trait_field_export(5)), bits(b.trait_field_export(5)), "moisture deterministic");

    let mut w = mk();
    w.set_orographic_strength(0.0);
    w.recompute_climate();
    let m0 = w.trait_field_export(5);
    w.set_orographic_strength(1.0);
    w.recompute_climate();
    let m1 = w.trait_field_export(5);
    assert!(
        m0.iter().zip(&m1).any(|(x, y)| (x - y).abs() > 1e-3),
        "orographic strength changes the moisture field"
    );
}

#[test]
fn surface_has_expected_shape() {
    let w = built();
    let s = w.surface(100.0).expect("surface after build");
    assert_eq!(s.positions.len(), 3 * w.region_count(), "3 floats per region vertex");
    assert_eq!(s.colors.len(), 3 * w.region_count());
    assert_eq!(s.indices.len(), 3 * w.triangle_count(), "3 indices per triangle");
}

#[test]
fn paint_terrain_reports_and_raises_its_footprint() {
    let mut w = built();
    let before = w.elevation_export();
    let touched = w.paint_terrain(500.0, 500.0, 120.0, 0.2, 0); // mode 0 = raise
    assert!(!touched.is_empty(), "a raise at the center should touch some regions");

    let after = w.elevation_export();
    let raised = touched.iter().any(|&r| after[r as usize] > before[r as usize]);
    assert!(raised, "at least one region under the brush should rise");
}

#[test]
fn brush_sphere_bounds_the_footprint_vertically() {
    // The directional-brush fix (ADR 0005 §R3): with a vertical exaggeration set, the footprint
    // brushes gate cells by 3D distance, so on varied terrain the brush is a sphere under the cursor
    // — not the infinite vertical column a flat 2D radius selects when viewed off-top-down.
    let mut w = built();
    let (cx, cy) = (500.0, 500.0);
    let radius = 300.0;
    let exag = 1.0e6; // huge: any real elevation difference becomes a large vertical distance

    let hit_e = w.height_at(cx, cy).expect("centre is on-map");
    let hit_y = hit_e * exag;

    // Flat (2D) footprint: the whole horizontal disc (the buggy off-top-down behaviour).
    w.set_brush_sphere(0.0, 0.0);
    let flat = w.paint_trait(cx, cy, radius, 4, 0.5); // trait 4 = temperature (no elevation change)
    assert!(flat.len() > 1, "the disc should cover many cells");

    // 3D sphere centred on the hit: cells whose elevation puts them > radius away vertically drop out.
    w.set_brush_sphere(hit_y, exag);
    let sphere = w.paint_trait(cx, cy, radius, 4, 0.5);

    assert!(!sphere.is_empty(), "the cell under the cursor (dv≈0) stays in the sphere");
    assert!(
        sphere.len() < flat.len(),
        "the 3D sphere must clip vertically-distant cells the flat brush keeps ({} vs {})",
        sphere.len(), flat.len()
    );

    // Everything the sphere kept is within the brush radius in 3D ⇒ within `radius` vertically too.
    let elev = w.elevation_export();
    for &r in &sphere {
        let dv = (elev[r as usize] as f64 * exag - hit_y).abs();
        assert!(dv <= radius, "kept cell {r} is {dv} m above/below the hit, beyond r={radius}");
    }
}

#[test]
fn minimap_is_rgba_and_non_empty() {
    let w = built();
    let n = 64;
    let img = w.minimap(n, -0.7, -0.7);
    assert_eq!(img.len(), n * n * 4, "RGBA, n*n*4 bytes");
    // At least some on-map cells are opaque (alpha = 255).
    let opaque = (0..n * n).filter(|&i| img[i * 4 + 3] == 255).count();
    assert!(opaque > 0, "minimap should have opaque on-map pixels");
}

#[test]
fn height_at_resolves_inside_the_map_only() {
    let w = built();
    assert!(w.height_at(500.0, 500.0).is_some(), "center should resolve a region");
    assert!(w.height_at(-1000.0, -1000.0).is_none(), "far outside the map has no region");
}

#[test]
fn raycast_terrain_hits_the_surface_under_the_ray() {
    let w = built();
    let exag = 100.0;
    let h = w.height_at(500.0, 500.0).expect("center height") * exag;
    // Straight-down ray over the center, from well above the surface.
    let hit = w
        .raycast_terrain(500.0, 10_000.0, 500.0, 0.0, -1.0, 0.0, exag)
        .expect("a downward ray over the map should hit terrain");
    assert!((hit[0] - 500.0).abs() < 1e-6 && (hit[2] - 500.0).abs() < 1e-6, "hit stays on the ray XZ");
    assert!((hit[1] - h).abs() < 1.0, "hit Y ≈ terrain height {h}, got {}", hit[1]);
    // A ray pointing up into empty sky misses.
    assert!(
        w.raycast_terrain(500.0, 10_000.0, 500.0, 0.0, 1.0, 0.0, exag).is_none(),
        "an upward ray hits nothing"
    );
}

#[test]
fn streams_are_idempotent_after_a_course() {
    let mut w = built();
    // Paint a short main-river stroke so streams have a catchment to drain into.
    for i in 0..6 {
        let x = 300.0 + i as f64 * 70.0;
        w.paint_course(x, 500.0, 80.0, 0.1, 0);
    }
    w.generate_streams(0.5, 0.05);
    let first: Vec<u32> = w.elevation_export().iter().map(|v| v.to_bits()).collect();
    w.generate_streams(0.5, 0.05);
    let second: Vec<u32> = w.elevation_export().iter().map(|v| v.to_bits()).collect();
    assert_eq!(first, second, "restore-then-recarve must be bit-identical on re-run");
}

#[test]
fn auto_rivers_form_without_a_painted_course() {
    // The watershed gap #3 closes: trunk rivers must emerge from flow accumulation alone, with no
    // hand-painted Course seed. After fixing the ocean, generating rivers should lay water on land.
    let mut w = built_canon();
    w.set_sea_level(-0.4);
    let wet0 = w.liquid_depth_export().iter().filter(|&&d| d > 0.0).count();
    w.generate_rivers(0.05); // no paint_course anywhere
    let depth = w.liquid_depth_export();
    let elev = w.elevation_export();
    let land_rivers = (0..elev.len()).filter(|&r| elev[r] > -0.4 && depth[r] > 0.0).count();
    assert!(land_rivers > 0, "trunk rivers form from flow alone (no seed): {land_rivers} land river cells");
    assert!(depth.iter().filter(|&&d| d > 0.0).count() > wet0, "rivers add water beyond the ocean");
}

#[test]
fn shorelines_are_not_cliffs() {
    // The follow-up report: with the wide base grade, *low* shores round off but a *high* region standing
    // beside the filled water reads as a sheer wall — a basin floods only to its lowest rim, so any higher
    // shore stands as a cliff above the waterline. `grade_shorelines` (run inside reshape_and_reflow) eases
    // dry land down to every waterline. Measured on the true mesh adjacency (not a coarse grid), the worst
    // shore wall must be gentle. Pre-fix this world measured ~540 m; the ~34°/cell ramp brings it under ~120 m.
    let mut w = World::new();
    w.set_base_blend_m(1800.0); // editor default
    w.build(8000.0, 8000.0, 40.0, 7, 5);
    let exag = 5000.0 / 3.0; // TerrainHeightKm 5 → metres per normalized unit
    let sea = w.min_elevation() + 1000.0 / exag; // editor's default sea level (OnGenDone)
    w.set_sea_level(sea);
    w.reshape_and_reflow(1.0, 0.015); // editor ShapeStrength + RiverDepthGain
    let wall = w.max_water_edge_drop() as f64 * exag;
    println!("max shore wall (true mesh edge): {wall:.0} m");
    assert!(wall < 120.0, "shoreline grading must ease water edges; worst wall is {wall:.0} m");
}

#[test]
fn auto_rivers_carve_valleys_not_slots() {
    // Hydraulic erosion (the river generator) carves dendritic VALLEYS along the drainage, not a
    // one-cell water slot — so the eroded (lowered) area is far larger than the watered channel itself.
    let mut w = built_canon();
    let sea = -0.4;
    w.set_sea_level(sea);
    let elev0 = w.elevation_export();
    w.generate_rivers(0.05); // hydraulic erosion from flow alone (no painted Course)
    let elev1 = w.elevation_export();
    let depth1 = w.liquid_depth_export();
    let eps = 1e-6;
    let carved = (0..elev1.len()).filter(|&r| (elev1[r] as f64) < elev0[r] as f64 - eps).count();
    // Watered channel cells standing above the ocean (the river itself, not the sea fill).
    let channels = (0..elev1.len())
        .filter(|&r| depth1[r] as f64 > eps && elev1[r] as f64 > sea)
        .count();
    assert!(carved > 0 && channels > 0, "rivers carve terrain and lay water (carved={carved} channels={channels})");
    assert!(
        carved > channels,
        "carve must reach dry banks beyond the watered channel (V-valley, not a slot): carved={carved} channels={channels}"
    );
}

fn subset_variance(elev: &[f32], ids: &[u32]) -> f64 {
    let v: Vec<f64> = ids.iter().map(|&i| elev[i as usize] as f64).collect();
    if v.len() < 2 {
        return 0.0;
    }
    let m = v.iter().sum::<f64>() / v.len() as f64;
    v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / v.len() as f64
}

#[test]
fn sculpt_brushes_move_terrain() {
    // Grab lifts the footprint; Flatten eases it toward a plane; Roughen adds variance and Smooth
    // removes it. (Interactive edits — not part of the deterministic gen contract.)
    let mut w = built();
    w.set_brush_sphere(0.0, 0.0); // flat 2D footprint (no vertical sphere term)
    let (cx, cy, r) = (500.0, 500.0, 200.0);

    let before = w.height_at(cx, cy).unwrap();
    let grabbed = w.grab_terrain(cx, cy, r, 0.1);
    assert!(!grabbed.is_empty(), "grab touches the footprint");
    assert!(w.height_at(cx, cy).unwrap() > before, "grab lifts the footprint");

    w.flatten_terrain(cx, cy, r, 1.0, 0.2); // ease toward 0.2
    let h = w.height_at(cx, cy).unwrap() as f64;
    assert!((h - 0.2).abs() < 0.05, "flatten eases the centre toward the target plane, got {h}");

    let rough = w.roughen_terrain(cx, cy, r, 0.06);
    assert!(!rough.is_empty(), "roughen touches the footprint");
    let var_rough = subset_variance(&w.elevation_export(), &rough);
    for _ in 0..4 {
        w.smooth_terrain(cx, cy, r, 1.0);
    }
    let var_smooth = subset_variance(&w.elevation_export(), &rough);
    assert!(var_smooth < var_rough, "smooth reduces footprint variance ({var_smooth} < {var_rough})");
}

#[test]
fn per_region_erosion_table_roundtrips() {
    let mut w = built();
    assert!((w.erosion_of(1) - 1.0).abs() < 1e-9, "erosion defaults to neutral 1.0");
    w.set_erosion(1, 2.5);
    w.set_erosion(5, 0.0);
    let exp = w.erosion_export();
    let mut b = built();
    b.set_erosion_table(&exp);
    assert!((b.erosion_of(1) - 2.5).abs() < 1e-6, "high erosion round-trips");
    assert!((b.erosion_of(5) - 0.0).abs() < 1e-6, "zero erosion round-trips");
}

#[test]
fn zero_region_erosion_disables_carving() {
    // The per-region erosion knob fully gates the hydraulic erosion: set every region to 0 and
    // generate_rivers must not move the terrain (incision AND valley diffusion both off).
    let mut w = built_canon();
    w.set_sea_level(-0.4);
    let before = w.elevation_export();
    for id in 0..=14 {
        w.set_erosion(id, 0.0);
    }
    w.generate_rivers(0.05);
    let after = w.elevation_export();
    let changed = (0..before.len()).filter(|&i| (before[i] - after[i]).abs() > 1e-6).count();
    assert_eq!(changed, 0, "zero erosion everywhere leaves the terrain untouched ({changed} cells moved)");
}

#[test]
fn thermal_erosion_sheds_a_sharp_peak() {
    // Thermal erosion moves material from a steep crest downslope, lowering the peak.
    let mut w = built();
    w.set_brush_sphere(0.0, 0.0);
    w.paint_terrain(500.0, 500.0, 150.0, 0.8, 3); // mode 3 = sharp crest
    let peak0 = w.height_at(500.0, 500.0).unwrap();
    let touched = w.erode_brush(500.0, 500.0, 300.0);
    assert!(!touched.is_empty(), "erosion touches cells");
    let peak1 = w.height_at(500.0, 500.0).unwrap();
    assert!(peak1 < peak0, "thermal erosion sheds the sharp peak ({peak1} < {peak0})");
}

#[test]
fn auto_shaping_gives_mountains_more_relief_than_plains() {
    // Relief fix: the per-region landform dials must reach the geometry at Generate. Jagged Mountains
    // (jaggedness 0.90) should end up with materially more elevation variance than Open Plains (0.10).
    let mut w = built_canon();
    w.set_sea_level(-0.4);
    w.reshape_and_reflow(1.0, 0.05);
    let elev = w.elevation_export();
    let region = w.region_export();
    let variance = |rid: u8| {
        let v: Vec<f64> = (0..elev.len()).filter(|&r| region[r] == rid).map(|r| elev[r] as f64).collect();
        if v.len() < 2 {
            return 0.0;
        }
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        v.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / v.len() as f64
    };
    let jagged = variance(1); // Jagged Mountains
    let plains = variance(5); // Open Plains
    assert!(jagged > plains, "shaping gives the mountains more relief than the plains ({jagged} vs {plains})");
}

#[test]
fn base_trunk_borders_are_gentle_not_cliffs() {
    // The recurring "heights too severe / cliffs at borders + around the lake" report. The macro
    // base-elevation trunk must grade across territory borders gently enough that the *visible* land
    // rise reads as hills/mountainsides, not vertical steps. We measure the steepest adjacent-sample
    // slope over **dry land** (above the ocean and not under a perched lake — so a submerged basin wall
    // doesn't count) on the base+water field, before shaping (which legitimately roughens peaks), at the
    // real vertical exaggeration the editor uses.
    let mut w = built_canon(); // 8 km
    let sea = -0.4;
    w.set_sea_level(sea);
    w.fill_lakes(); // pond the Great Lake so its basin wall is underwater (not a "visible" cliff)
    let elev = w.elevation_export();
    let depth = w.liquid_depth_export();
    let exag = 5000.0 / 3.0; // TerrainHeightKm 5 → metres per normalized unit (DhceWorld.cs)
    let step = 40.0; // ≈ the region spacing
    let n = (8000.0 / step) as usize;
    // Dry interior land at a sample: inside the coastal ocean-rim band (d < 0.78 — the rim's land→sea
    // plunge is a separate concern from the interior the user is looking at), above the ocean, and not
    // under a perched lake.
    let dry = |x: f64, y: f64| -> Option<f64> {
        let (du, dv) = ((x - 4000.0) / 8000.0, (y - 4000.0) / 8000.0);
        if (du * du + dv * dv).sqrt() * 2.0 >= 0.78 {
            return None;
        }
        let ri = w.region_at(x, y)?;
        let e = elev[ri] as f64;
        if e <= sea || depth[ri] as f64 > 1e-3 {
            return None;
        }
        Some(e)
    };
    let mut max_slope = 0.0f64;
    let mut max_at = (0.0, 0.0);
    let mut steep = 0usize; // adjacent dry-land pairs steeper than 45°
    let mut land_pairs = 0usize;
    for iy in 0..n {
        let y = iy as f64 * step + step * 0.5;
        for ix in 0..n {
            let x = ix as f64 * step + step * 0.5;
            let e = match dry(x, y) {
                Some(e) => e,
                None => continue,
            };
            for (nx, ny) in [(x + step, y), (x, y + step)] {
                if let Some(en) = dry(nx, ny) {
                    land_pairs += 1;
                    let slope = (e - en).abs() * exag / step;
                    if slope > max_slope {
                        max_slope = slope;
                        max_at = (x, y);
                    }
                    if slope > 1.0 {
                        steep += 1;
                    }
                }
            }
        }
    }
    let steep_frac = steep as f64 / land_pairs.max(1) as f64;
    let reg = w.region_id_at(max_at.0, max_at.1);
    println!(
        "dry-land base slopes: max={max_slope:.2} ({:.0}°) at ({:.0},{:.0}) region={reg}, >45° fraction={steep_frac:.4} over {land_pairs} pairs",
        max_slope.atan().to_degrees(), max_at.0, max_at.1
    );
    // Regression guard against the cliff regime (the wide coarse-grid grade replaced the too-narrow
    // Jacobi diffuser). Pre-fix this world measured max≈11.5 (85°) with 13% of interior dry-land steeper
    // than 45°; post-fix it's ~2.6 (69°) with <3%. The fraction is the systematic signal; the max bound
    // catches a return toward vertical border steps (the isolated worst spot is a perched-basin rim).
    assert!(steep_frac < 0.05, "interior dry-land must not be cliff-ridden, got {steep_frac:.4} > 45°");
    assert!(max_slope < 4.0, "no near-vertical border steps, got max slope {max_slope:.2}");
}

#[test]
fn watershed_connects_lakes_rivers_and_ocean() {
    // After the unified pass all three water tiers coexist: an ocean below sea level, plus perched
    // water (lakes + their feeder/outlet rivers) standing on the land above it.
    let mut w = built_canon();
    w.set_sea_level(-0.4);
    w.reshape_and_reflow(1.0, 0.05);
    let depth = w.liquid_depth_export();
    let elev = w.elevation_export();
    let ocean = (0..elev.len()).filter(|&r| elev[r] <= -0.4 && depth[r] > 0.0).count();
    let perched = (0..elev.len()).filter(|&r| elev[r] > -0.4 && depth[r] > 0.0).count();
    assert!(ocean > 0, "the ocean tier is present");
    assert!(perched > 0, "perched water (lakes + rivers) sits above the sea — the watershed reaches land");
}

#[test]
fn reshape_and_reflow_is_idempotent() {
    // The un-carve → shape → carve rivers → fill lakes → connect outlets stack must not compound: a
    // second pass from the same dials yields bit-identical elevation (the contract for saved worlds).
    let mut w = built_canon();
    w.set_sea_level(-0.4);
    w.reshape_and_reflow(1.0, 0.05);
    let first: Vec<u32> = w.elevation_export().iter().map(|v| v.to_bits()).collect();
    w.reshape_and_reflow(1.0, 0.05);
    let second: Vec<u32> = w.elevation_export().iter().map(|v| v.to_bits()).collect();
    assert_eq!(first, second, "reshape_and_reflow must be bit-identical on re-run");
}

#[test]
fn reshape_and_reflow_is_invariant_to_thread_count() {
    // Relief + rivers ride the same parallel index→value maps as the rest of gen, so the whole pass
    // must be bit-identical regardless of thread count (parallel == serial keeps saves from desyncing).
    let prep = || {
        let mut w = built_canon();
        w.set_sea_level(-0.4);
        w.reshape_and_reflow(1.0, 0.05);
        w
    };
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let many = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let a = one.install(prep);
    let b = many.install(prep);
    let bits = |v: Vec<f32>| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(a.elevation_export()), bits(b.elevation_export()), "carved+shaped elevation identical across threads");
    assert_eq!(bits(a.liquid_depth_export()), bits(b.liquid_depth_export()), "liquid identical across threads");
}

#[test]
fn region_at_finds_an_interior_region_and_selects_its_biome() {
    let w = built();
    let r = w.region_at(500.0, 500.0).expect("an interior region near the center");
    let sel = w.select_contiguous(r);
    assert!(sel.contains(&(r as u32)), "the seed region is part of its own selection");
    let same_biome = w.biome_at(r);
    assert!(sel.iter().all(|&s| w.biome_at(s as usize) == same_biome), "selection is single-biome");
}

#[test]
fn chunks_partition_all_triangles_and_track_edits() {
    let mut w = World::new();
    w.build(3000.0, 3000.0, 12.0, 7, 5); // dense enough to need several chunks
    assert!(w.chunk_count() > 1, "should partition into several chunks");

    // Each triangle is assigned to exactly one chunk → chunk triangle counts sum to total.
    let mut tri_total = 0usize;
    for c in 0..w.chunk_count() {
        if let Some(s) = w.chunk_surface(c, 100.0) {
            assert_eq!(s.positions.len(), s.colors.len(), "3 floats per local vertex for both");
            tri_total += s.indices.len() / 3;
        }
    }
    assert_eq!(tri_total, w.triangle_count(), "chunks partition all triangles exactly once");

    // Fresh build: nothing dirty. An edit flags the chunks under the brush, and taking
    // them clears the flags.
    assert!(w.take_dirty_chunks().is_empty(), "no dirty chunks before editing");
    let touched = w.paint_terrain(500.0, 500.0, 120.0, 0.2, 0);
    assert!(!touched.is_empty());
    assert!(!w.take_dirty_chunks().is_empty(), "an edit flags its chunks dirty");
    assert!(w.take_dirty_chunks().is_empty(), "taking dirty chunks clears them");
}

fn liquid_volume(w: &World) -> f64 {
    w.liquid_depth_export().iter().map(|&d| d as f64).sum()
}

#[test]
fn step_fluid_sleeps_when_settled_and_conserves_volume() {
    // Active-set sim: a sea fill wakes the wet cells but is already a level surface, so one step
    // finds no downhill flow → the active set drains to empty (water sleeps → idle costs nothing).
    let mut w = built();
    w.set_sea_level(0.2);
    assert!(w.liquid_active_count() > 0, "sea fill wakes the wet cells");
    let v0 = liquid_volume(&w);
    w.step_fluid(0.3, 0.0, 4);
    assert_eq!(w.liquid_active_count(), 0, "a level pool sleeps (empty active set)");
    let v1 = liquid_volume(&w);
    assert!((v0 - v1).abs() / v0.max(1e-9) < 1e-6, "settle conserves volume: {v0} -> {v1}");
}

#[test]
fn active_set_flow_conserves_volume() {
    // A poured blob flows downhill through the active set; with no evaporation, volume is conserved
    // regardless of how far it has settled (the mass-conservation invariant of the active solver).
    let mut w = built();
    w.paint_liquid(500.0, 500.0, 200.0, 0.3, 0);
    let v0 = liquid_volume(&w);
    assert!(v0 > 0.0, "the blob laid down water");
    for _ in 0..200 {
        w.step_fluid(0.3, 0.0, 1);
    }
    let v1 = liquid_volume(&w);
    assert!((v0 - v1).abs() / v0 < 1e-6, "active-set flow conserves volume: {v0} -> {v1}");
}

#[test]
fn liquid_chunks_partition_the_wet_surface() {
    // The chunked liquid surface must cover exactly the same wet triangles as the whole-surface
    // build (so streaming per chunk loses nothing) — the basis of the chunked-liquid perf win.
    let mut w = World::new();
    w.build(3000.0, 3000.0, 12.0, 7, 5); // dense enough for several chunks
    w.set_sea_level(0.5); // flood a substantial area
    let _ = w.take_dirty_liquid_chunks(); // drain the sea-fill's flag-all so the edit check is clean

    let whole = w.liquid_surface(100.0).expect("liquid surface").indices.len() / 3;
    assert!(whole > 0, "the sea fill should produce wet triangles");

    let mut chunked = 0usize;
    for c in 0..w.chunk_count() {
        if let Some(s) = w.liquid_chunk_surface(c, 100.0) {
            chunked += s.indices.len() / 3;
        }
    }
    assert_eq!(chunked, whole, "liquid chunks partition the wet surface exactly once");

    // A liquid edit flags liquid chunks (reading chunk surfaces above does not); taking clears them.
    assert!(w.take_dirty_liquid_chunks().is_empty(), "reads don't flag liquid chunks");
    w.paint_liquid(1500.0, 1500.0, 200.0, 0.2, 0);
    assert!(!w.take_dirty_liquid_chunks().is_empty(), "a liquid edit flags its chunks");
    assert!(w.take_dirty_liquid_chunks().is_empty(), "taking liquid-dirty chunks clears them");
}

fn avg_luma(colors: &[f32]) -> f32 {
    let n = colors.len() / 3;
    let mut s = 0.0f32;
    for r in 0..n {
        s += 0.2126 * colors[3 * r] + 0.7152 * colors[3 * r + 1] + 0.0722 * colors[3 * r + 2];
    }
    s / n.max(1) as f32
}

#[test]
fn build_colors_are_not_uniform() {
    // The palette + ramp must produce a varied map, not one flat fill.
    let w = built();
    let s = w.surface(100.0).expect("surface");
    let (r0, g0, b0) = (s.colors[0], s.colors[1], s.colors[2]);
    let varied = (1..w.region_count()).any(|r| {
        (s.colors[3 * r] - r0).abs() > 0.02
            || (s.colors[3 * r + 1] - g0).abs() > 0.02
            || (s.colors[3 * r + 2] - b0).abs() > 0.02
    });
    assert!(varied, "biome palette + elevation ramp should vary colour across the map");
}

#[test]
fn surface_color_lifts_with_elevation() {
    // Drive traits + elevation directly so the assertion is independent of generated terrain:
    // stamp Temperate Forest everywhere, then high (bare cap) reads brighter than forested lowland.
    let mut w = built();
    let nr = w.region_count();
    w.paint_region_traits(500.0, 500.0, 5000.0, 4); // id 4 = Temperate Forest, over the whole map

    w.set_elevation(&vec![0.1f32; nr]);
    let low = avg_luma(&w.surface(100.0).expect("surface").colors);
    w.set_elevation(&vec![1.2f32; nr]);
    let high = avg_luma(&w.surface(100.0).expect("surface").colors);

    assert!(high > low + 0.15, "high ground reads brighter than lowland cover: {high} vs {low}");
}

#[test]
fn paint_trait_sets_the_field_under_the_brush() {
    let mut w = built();
    let touched = w.paint_trait(500.0, 500.0, 200.0, 4, 0.0); // trait 4 = temperature → 0 (cold)
    assert!(!touched.is_empty(), "the trait brush touches cells");
    let t = w.trait_at(500.0, 500.0, 4).expect("a cell at the brush centre");
    assert!(t < 0.3, "temperature eased toward 0 at the brush centre, got {t}");
}

#[test]
fn blend_traits_grades_a_painted_border() {
    // Cold on the left, hot on the right, with a gap between the two stamps. After blending, the
    // seam reads as a cold→hot gradient instead of a hard step.
    let mut w = built();
    w.paint_trait(250.0, 500.0, 220.0, 4, 0.0); // left: temperature cold
    w.paint_trait(750.0, 500.0, 220.0, 4, 1.0); // right: temperature hot
    w.blend_traits(400.0);
    let left = w.trait_at(300.0, 500.0, 4).expect("left cell");
    let mid = w.trait_at(500.0, 500.0, 4).expect("seam cell");
    let right = w.trait_at(700.0, 500.0, 4).expect("right cell");
    assert!(left < mid && mid < right, "temperature grades cold→hot across the seam: {left} {mid} {right}");
}

#[test]
fn blend_traits_is_idempotent_on_rerun() {
    // Re-applying the same width must not over-smooth: the pass always grades from the painted
    // base, so the colour buffer is bit-identical on the second run.
    let mut w = built();
    w.paint_trait(300.0, 500.0, 150.0, 4, 0.0);
    w.paint_trait(700.0, 500.0, 150.0, 4, 1.0);
    w.blend_traits(250.0);
    let first: Vec<u32> = w.surface(100.0).expect("surface").colors.iter().map(|v| v.to_bits()).collect();
    w.blend_traits(250.0);
    let second: Vec<u32> = w.surface(100.0).expect("surface").colors.iter().map(|v| v.to_bits()).collect();
    assert_eq!(first, second, "re-blending from the painted base must not compound");
}

#[test]
fn blend_traits_leaves_a_uniform_field_unchanged() {
    // A locally-constant field is a fixed point of the neighbour average — a whole-map stamp must
    // survive the blend untouched (no drift from the diffusion).
    let mut w = built();
    w.paint_region_traits(500.0, 500.0, 5000.0, 4); // Temperate Forest over the whole map
    let before = w.trait_at(500.0, 500.0, 4).expect("cell");
    w.blend_traits(300.0);
    let after = w.trait_at(500.0, 500.0, 4).expect("cell");
    assert!((before - after).abs() < 1e-9, "a uniform field is a fixed point of the blend: {before} vs {after}");
}

fn variance(v: &[f32]) -> f32 {
    let n = v.len().max(1) as f32;
    let mean = v.iter().sum::<f32>() / n;
    v.iter().map(|&x| (x - mean) * (x - mean)).sum::<f32>() / n
}

#[test]
fn shaping_roughens_with_jaggedness_and_is_idempotent() {
    // From a flat plateau, cranking jaggedness everywhere should add roughness; re-running must
    // restore the base then reapply identically (idempotent), not pile detail on detail.
    let mut w = built();
    w.paint_trait(500.0, 500.0, 5000.0, 0, 1.0); // trait 0 = jaggedness → 1 over the whole map
    w.set_elevation(&vec![0.6f32; w.region_count()]); // flat high ground (so altitude-gating is on)

    let flat = variance(&w.elevation_export());
    w.shape_terrain(1.0);
    let shaped = w.elevation_export();
    assert!(variance(&shaped) > flat + 1e-4, "jaggedness adds roughness: {flat} -> {}", variance(&shaped));

    let a: Vec<u32> = shaped.iter().map(|v| v.to_bits()).collect();
    w.shape_terrain(1.0); // re-run from the restored base
    let b: Vec<u32> = w.elevation_export().iter().map(|v| v.to_bits()).collect();
    assert_eq!(a, b, "re-shaping restores the base then reapplies → bit-identical");
}

#[test]
fn region_landform_applies_to_a_regions_cells() {
    // Setting a region's jaggedness should write it onto every cell of that region (so a later
    // shape pass reshapes the whole region) — the region-tied replacement for the landform brush.
    let mut w = built();
    let r = w.region_at(500.0, 500.0).expect("centre region");
    let region_id = w.biome_at(r);
    w.set_region_landform(region_id, 0, 0.9); // idx 0 = jaggedness
    let t = w.trait_at(500.0, 500.0, 0).expect("centre cell jaggedness");
    assert!((t - 0.9).abs() < 1e-9, "region jaggedness applied to its cells: {t}");
}

#[test]
fn shaping_strength_zero_is_a_no_op() {
    let mut w = built();
    w.paint_trait(500.0, 500.0, 5000.0, 0, 1.0);
    w.set_elevation(&vec![0.6f32; w.region_count()]);
    let before: Vec<u32> = w.elevation_export().iter().map(|v| v.to_bits()).collect();
    w.shape_terrain(0.0);
    let after: Vec<u32> = w.elevation_export().iter().map(|v| v.to_bits()).collect();
    assert_eq!(before, after, "zero strength adds nothing");
}

#[test]
fn temperature_view_maps_the_field_to_a_heat_ramp() {
    // The Temperature data view recolours the same mesh by the field: hot reads red-dominant,
    // cold blue-dominant — independent of the composed Natural colour.
    let mut w = built();
    w.paint_trait(250.0, 500.0, 200.0, 4, 1.0); // hot patch (left)
    w.paint_trait(750.0, 500.0, 200.0, 4, 0.0); // cold patch (right)
    w.set_view_mode(1); // VIEW_TEMPERATURE
    let s = w.surface(100.0).expect("surface");
    let hot = w.region_at(250.0, 500.0).expect("hot region");
    let cold = w.region_at(750.0, 500.0).expect("cold region");
    assert!(s.colors[3 * hot] > s.colors[3 * hot + 2], "hot reads red-dominant");
    assert!(s.colors[3 * cold + 2] > s.colors[3 * cold], "cold reads blue-dominant");
    // Back to Natural, the same hot cell is not the heat-ramp red (composed terrain colour).
    w.set_view_mode(0);
    let s2 = w.surface(100.0).expect("surface");
    assert!(
        (s2.colors[3 * hot] - s.colors[3 * hot]).abs() > 0.05
            || (s2.colors[3 * hot + 2] - s.colors[3 * hot + 2]).abs() > 0.05,
        "Natural view differs from the heat view"
    );
}

#[test]
fn chunk_grid_is_empty_before_build() {
    let w = World::new();
    assert_eq!(w.chunk_grid(), (0, 0), "no grid before build");
    assert!(w.chunk_centers().is_empty(), "no centers before build");
}

#[test]
fn chunk_grid_and_centers_match_the_partition() {
    let mut w = World::new();
    let (width, height) = (4000.0, 4000.0);
    w.build(width, height, 12.0, 7, 5); // dense enough for a multi-tile grid

    let (cols, rows) = w.chunk_grid();
    assert!(cols > 1 && rows > 1, "a dense world spans several tiles");
    assert_eq!(cols, rows, "the chunk grid is square");
    assert_eq!(cols * rows, w.chunk_count(), "the grid covers every chunk exactly");

    // Tiles are a fixed physical size: the grid is `ceil(dim / chunk_size_m)` each side.
    let tile = w.chunk_size_m();
    assert_eq!(cols, (width / tile).ceil() as usize, "cols = ceil(width / chunk size)");

    let centers = w.chunk_centers();
    assert_eq!(centers.len(), w.chunk_count() * 2, "one (x, y) per chunk id");

    // Each center sits on its fixed tile's half-step and stays inside the world bounds — this is
    // the exact mapping the front-end relies on to turn a camera position into visible tiles.
    for id in 0..w.chunk_count() {
        let (gx, gy) = (id % cols, id / cols);
        let (cx, cy) = (centers[2 * id], centers[2 * id + 1]);
        assert!((cx - (gx as f64 + 0.5) * tile).abs() < 1e-6, "center x is the tile mid-point");
        assert!((cy - (gy as f64 + 0.5) * tile).abs() < 1e-6, "center y is the tile mid-point");
        assert!(cx > 0.0 && cx < width && cy > 0.0 && cy < height, "center is in-bounds");
    }
}

#[test]
fn region_tier_assigns_via_polygon_and_persists() {
    // Stage 4: the named-Region tier — assign a polygon's cells to a Region, read it back, persist.
    let mut w = built(); // 1000 × 1000
    let cells = w.regions_in_polygon(&[100.0, 900.0, 900.0, 100.0], &[100.0, 100.0, 900.0, 900.0]);
    assert!(!cells.is_empty(), "the polygon should enclose cells");

    w.assign_region(&cells, 3);
    assert_eq!(w.region_of(cells[0] as usize), 3, "assigned cell reports its Region");
    assert_eq!(w.region_id_at(500.0, 500.0), 3, "centre of the polygon is in Region 3");
    assert_eq!(w.region_id_at(500.0, 500.0) >= 0, true);

    // A brush dab assigns too (the Region tool).
    let painted = w.paint_region(500.0, 500.0, 100.0, 7);
    assert!(!painted.is_empty());
    assert_eq!(w.region_id_at(500.0, 500.0), 7, "brush re-assigned the centre to Region 7");

    // Persistence round-trip.
    let exp = w.region_export();
    let mut b = built();
    b.set_region(&exp);
    assert_eq!(b.region_export(), exp, "Region membership round-trips");
    assert_eq!(b.region_id_at(500.0, 500.0), 7, "the brushed centre (Region 7) survives the round-trip");
}

#[test]
fn build_is_invariant_to_thread_count() {
    // The gen path is parallelised (rayon) only on pure index→value maps, so output must be
    // *bit-identical* regardless of thread count — this is the property that keeps saved worlds from
    // desyncing (parallel == serial). Build under a 1-thread pool (≡ serial) vs a 4-thread pool.
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let many = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let a = one.install(built);
    let b = many.install(built);
    let bits = |v: Vec<f32>| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(a.elevation_export()), bits(b.elevation_export()), "elevation identical across thread counts");
    assert_eq!(a.biome_export(), b.biome_export(), "biomes identical across thread counts");
    // Colour cache (region_color, incl. the parallel smoothing) goes through the surface producer.
    let sa = a.surface(100.0).expect("surface");
    let sb = b.surface(100.0).expect("surface");
    assert_eq!(bits(sa.colors), bits(sb.colors), "colours identical across thread counts");

    // Liquid surface smoothing is parallelised too — flood some water and compare across thread counts.
    let mut la = built();
    let mut lb = built();
    la.paint_liquid(500.0, 500.0, 300.0, 0.4, 0);
    lb.paint_liquid(500.0, 500.0, 300.0, 0.4, 0);
    let qa = one.install(|| la.liquid_surface(100.0)).expect("liquid");
    let qb = many.install(|| lb.liquid_surface(100.0)).expect("liquid");
    assert_eq!(bits(qa.positions), bits(qb.positions), "liquid surface identical across thread counts");
}

#[test]
fn surface_nets_sphere_is_a_closed_manifold() {
    // N5 Layer B: Surface Nets must extract a watertight isosurface (every edge shared by exactly two
    // triangles). A solid ball density field, well inside the grid, is the canonical check — it catches
    // any quad-connectivity/winding mistake without needing visual inspection.
    use dhce_core::volumetric::surface_nets;
    use std::collections::HashMap;
    let r = 3.0;
    let density = |x: f64, y: f64, z: f64| r - (x * x + y * y + z * z).sqrt(); // > 0 inside the ball
    let m = surface_nets([-5.0, -5.0, -5.0], [21, 21, 21], 0.5, &density);

    assert!(!m.positions.is_empty() && !m.indices.is_empty(), "the ball yields a surface");
    assert_eq!(m.indices.len() % 3, 0, "triangles");
    let vtx = (m.positions.len() / 3) as u32;
    assert!(m.indices.iter().all(|&i| i < vtx), "indices reference real vertices");

    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for tri in m.indices.chunks_exact(3) {
        for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            *edges.entry(if a < b { (a, b) } else { (b, a) }).or_insert(0) += 1;
        }
    }
    let open = edges.values().filter(|&&c| c != 2).count();
    assert_eq!(open, 0, "closed manifold — {open} edges not shared by exactly two triangles");
}

#[test]
fn rule_scatter_respects_vegetation_and_elevation() {
    use dhce_core::scatter::ScatterRule;
    let mut w = built();
    w.paint_trait(500.0, 500.0, 300.0, 6, 3.0); // vegetation = Forest (enum id 6, value 3)

    let forest = [ScatterRule {
        slot: 0, density: 1.0, scale_min: 1.0, scale_max: 2.0,
        elev_min: -10.0, elev_max: 10.0, veg_mask: 1 << 3 /* Forest */, region_mask: 0, biome_mask: 0,
    }];
    let inst = w.scatter_by_rules(&forest, 100.0, 7);
    assert!(!inst.is_empty(), "the forest rule places on the painted patch");
    assert!(inst.iter().all(|i| i.species == 0.0), "all instances come from slot 0");
    assert!(inst.iter().all(|i| i.scale >= 1.0 && i.scale <= 2.0), "scale respects the rule range");

    // An elevation band above the terrain ceiling matches nothing.
    let too_high = [ScatterRule {
        slot: 0, density: 1.0, scale_min: 1.0, scale_max: 1.0,
        elev_min: 5.0, elev_max: 10.0, veg_mask: 0, region_mask: 0, biome_mask: 0,
    }];
    assert!(w.scatter_by_rules(&too_high, 100.0, 7).is_empty(), "no cell sits above elevation 5");
}

#[test]
fn rule_scatter_respects_biome_mask() {
    use dhce_core::scatter::ScatterRule;
    let mut w = built();
    w.paint_biome(500.0, 500.0, 300.0, 5); // paint biome id 5 over the centre patch

    let masked = [ScatterRule {
        slot: 0, density: 1.0, scale_min: 1.0, scale_max: 1.0,
        elev_min: -10.0, elev_max: 10.0, veg_mask: 0, region_mask: 0, biome_mask: 1 << 4, // biome 5
    }];
    let any = [ScatterRule {
        slot: 0, density: 1.0, scale_min: 1.0, scale_max: 1.0,
        elev_min: -10.0, elev_max: 10.0, veg_mask: 0, region_mask: 0, biome_mask: 0,
    }];
    let masked_n = w.scatter_by_rules(&masked, 100.0, 9).len();
    let any_n = w.scatter_by_rules(&any, 100.0, 9).len();
    assert!(masked_n > 0, "the biome-masked rule places on the painted patch");
    assert!(any_n >= masked_n, "an unmasked rule places at least as widely as the masked one");
}

#[test]
fn region_slicing_extracts_submesh_and_adjacency() {
    // Slicer core: per-Region terrain submesh + Region adjacency for the level export.
    let mut w = built(); // 1000 × 1000
    let left = w.regions_in_polygon(&[10.0, 500.0, 500.0, 10.0], &[10.0, 10.0, 990.0, 990.0]);
    assert!(!left.is_empty());
    w.assign_region(&left, 1);
    // The canon build pre-populates regions, so Region 1 also holds its canon cells elsewhere; what
    // must hold is that every assigned cell now reports Region 1 (count ≥ the assignment).
    assert!(left.iter().all(|&c| w.region_of(c as usize) == 1), "every assigned cell reports Region 1");
    assert!(w.region_cell_count(1) >= left.len(), "Region 1 contains at least the assigned cells");

    let surf = w.region_terrain_surface(1, 100.0).expect("built world");
    assert!(!surf.positions.is_empty() && !surf.indices.is_empty(), "Region 1 has interior geometry");
    assert_eq!(surf.indices.len() % 3, 0, "triangles");
    let vtx = (surf.positions.len() / 3) as u32;
    assert!(surf.indices.iter().all(|&i| i < vtx), "indices are compacted + in range");

    // Assign the abutting right half to Region 2 → the two Regions share a border ⇒ adjacency.
    let right = w.regions_in_polygon(&[500.0, 990.0, 990.0, 500.0], &[10.0, 10.0, 990.0, 990.0]);
    w.assign_region(&right, 2);
    let adj = w.region_adjacency();
    assert!(adj.iter().any(|p| p[0] == 1 && p[1] == 2 && p[2] > 0), "Regions 1 & 2 are adjacent: {adj:?}");
}

#[test]
fn save_load_round_trips_authored_fields() {
    // R5 persistence: author a world, export every field, then rebuild a fresh world from the same
    // params and restore — the restored fields must be bit-identical (regenerate-then-overwrite).
    let mut a = World::new();
    a.build(1000.0, 1000.0, 50.0, 7, 5);
    a.paint_terrain(500.0, 500.0, 200.0, 0.3, 0);  // sculpt
    a.paint_trait(300.0, 500.0, 150.0, 4, 0.2);    // temperature scalar
    a.paint_trait(700.0, 500.0, 150.0, 6, 3.0);    // vegetation = Forest (enum)
    a.paint_biome(500.0, 500.0, 100.0, 9);         // stamp + lock biome 9
    a.paint_liquid(500.0, 500.0, 150.0, 0.2, 0);   // some water
    a.set_base_palette_color(2, 3, 0.1, 0.2, 0.3); // Stone rock slot
    a.set_region_landform(4, 1, 0.8);              // region 4 relief dial

    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    let elev = a.elevation_export();
    let biome = a.biome_export();
    let locked = a.biome_locked_export();
    let pal = a.base_palettes_export();
    let land = a.region_landform_export();

    let mut b = World::new();
    b.build(1000.0, 1000.0, 50.0, 7, 5);
    b.set_elevation(&elev);
    b.set_biome(&biome);
    b.set_biome_locked(&locked);
    b.set_liquid(&a.liquid_depth_export(), &a.liquid_kind_export());
    b.set_course_mask(&a.course_mask_export());
    for tid in 0..8 { b.set_trait_field(tid, &a.trait_field_export(tid)); }
    b.set_base_palettes(&pal);
    b.set_region_landform_table(&land);
    b.refresh_colors();

    assert_eq!(bits(&b.elevation_export()), bits(&elev), "elevation round-trips");
    assert_eq!(b.biome_export(), biome, "biome round-trips");
    assert_eq!(b.biome_locked_export(), locked, "biome locks round-trip");
    for tid in 0..6 { assert_eq!(bits(&b.trait_field_export(tid)), bits(&a.trait_field_export(tid)), "scalar trait {tid} round-trips"); }
    for tid in 6..8 { assert_eq!(b.trait_field_export(tid), a.trait_field_export(tid), "enum trait {tid} round-trips"); }
    assert_eq!(b.base_palettes_export(), pal, "base palettes round-trip");
    assert_eq!(b.region_landform_export(), land, "region landform round-trips");
}

#[test]
fn chunk_size_is_settable_and_resizes_the_grid() {
    let (width, height) = (4000.0, 4000.0);
    let coarse = {
        let mut w = World::new();
        w.set_chunk_size_m(1000.0);
        w.build(width, height, 30.0, 7, 4);
        assert_eq!(w.chunk_size_m(), 1000.0);
        w.chunk_count()
    };
    let fine = {
        let mut w = World::new();
        w.set_chunk_size_m(256.0);
        w.build(width, height, 30.0, 7, 4);
        w.chunk_count()
    };
    assert!(fine > coarse, "a smaller chunk size yields more (smaller) tiles ({fine} vs {coarse})");

    // Absurd inputs clamp to the floor rather than exploding the grid.
    let mut w = World::new();
    w.set_chunk_size_m(0.0);
    assert!(w.chunk_size_m() >= 32.0, "chunk size clamps to a sane minimum");
}
