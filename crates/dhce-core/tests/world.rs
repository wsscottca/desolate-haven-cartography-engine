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
    // The canon two-tier water model: every region appears, the CORNERS are OCEAN, and the Great Lake
    // is a PERCHED lake in the central valley — its floor sits above the ocean, filled to its pour
    // point (not a flooded sea-level basin). Built at a realistic scale (8 km) so territories are large
    // relative to the border-grading width (a 1 km map would wash the lake basin out). The full-frame
    // canon bake fills the 4:3 frame and reaches the N/S edges, so only the four corners are open sea.
    let mut w = World::new();
    w.build(8000.0, 8000.0, 40.0, 7, 5);
    let sea = 0.0; // canon waterline: land is positive, water (ocean + the Great-Lake bowl) is below 0
    w.set_sea_level(sea);
    w.fill_lakes();

    // All 14 canon regions are present in the territory map.
    let region_ids = w.region_export();
    for id in 1..=14u8 {
        assert!(region_ids.iter().any(|&r| r == id), "canon region {id} is present");
    }

    // Corners are ocean: terrain below the sea level (the N/S edge midpoints can now be land).
    for &(x, y) in &[(160.0, 160.0), (7840.0, 160.0), (160.0, 7840.0), (7840.0, 7840.0)] {
        let h = w.height_at(x, y).expect("corner sample resolves a region");
        assert!(h < sea, "corner ({x}, {y}) is ocean, got h={h}");
    }

    // The Great Lake is a bowl: open water at the centre (below the 0 waterline), shore rising to its
    // rim — so it holds standing water.
    let elev = w.elevation_export();
    let depths = w.liquid_depth_export();
    assert!(
        (0..region_ids.len()).any(|r| region_ids[r] == 3 && depths[r] as f64 > 1e-3),
        "the Great Lake bowl holds water at the 0 waterline"
    );

    // Jagged Mountains stand as dry highland above the ocean somewhere in the region.
    assert!(
        (0..region_ids.len()).any(|r| region_ids[r] == 1 && elev[r] as f64 > sea),
        "Jagged Mountains stand above the ocean"
    );
}

#[test]
fn climate_lapse_cools_high_ground() {
    // #1 Elevation→Temperature: a stronger lapse rate cools elevated ground. Sample the Volcanic
    // Scape (a warm region with a high base elevation) so its temperature stays in range — a cold
    // peak (e.g. Jagged) would already be clamped to 0 at the default lapse.
    let mut w = World::new();
    // 8 km, full pipeline — mountain HEIGHT now comes from the shaped relief (reshape_and_reflow),
    // not the moderated base trunk, so the volcano only stands tall after shaping.
    w.build(8000.0, 8000.0, 40.0, 7, 5);
    let sea = w.min_elevation() + 0.6;
    w.set_sea_level(sea);
    w.reshape_and_reflow(1.0, 0.015);
    // Find an elevated Volcanic Scape (id 11) sample — the canon raster places the volcano
    // south-center, so scan for it rather than hard-coding a point.
    let mut sample = (0.0, 0.0);
    let mut best_h = f64::NEG_INFINITY;
    for gy in 0..80 {
        for gx in 0..80 {
            let (x, y) = ((gx as f64 + 0.5) / 80.0 * 8000.0, (gy as f64 + 0.5) / 80.0 * 8000.0);
            if w.region_id_at(x, y) == 11 {
                if let Some(h) = w.height_at(x, y) {
                    if h > best_h {
                        best_h = h;
                        sample = (x, y);
                    }
                }
            }
        }
    }
    let (vx, vy) = sample;
    assert!(best_h > 0.1, "the Volcanic Scape has elevated land, got max height {best_h}");
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
    assert!((w.erosion_of(2) - 1.0).abs() < 1e-9, "a neutral region defaults to 1.0");
    // Mountains + the Great Lake get a boosted default (see regions::default_erosion).
    assert!((w.erosion_of(1) - 1.3).abs() < 1e-9, "Jagged Mountains default to a boosted 1.3");
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
    let sea = 0.0; // canon waterline (land positive, water below 0; ocean floor is −1 km = −0.214)
    w.set_sea_level(sea);
    w.fill_lakes(); // pond the Great Lake so its basin wall is underwater (not a "visible" cliff)
    let elev = w.elevation_export();
    let depth = w.liquid_depth_export();
    let exag = 14000.0 / 3.0; // TerrainHeightKm 14 → metres per normalized unit (DhceWorld.cs)
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
    // Regression guard. NOTE: the canon now uses **intentionally steep per-region gradients** (e.g.
    // Jagged/Frozen rise 2 km across the region, Deep Wood 1.75 km) at a 14 km vertical exaggeration, so
    // the base trunk legitimately carries a lot of 45°+ dry-land slope (~21%) — gentleness is no longer
    // the goal; the grade only has to avoid a *total* cliff regime / near-vertical border artifacts (the
    // pre-grade Jacobi bug hit max≈11.5 with everything stepped). grade_shorelines + shaping + author
    // sculpting do the final polish. Bounds kept loose to track gross regressions, not the dramatic relief.
    assert!(steep_frac < 0.40, "dry-land must not be a total cliff regime, got {steep_frac:.4} > 45°");
    assert!(max_slope < 45.0, "no near-vertical (>89°) border artifacts, got max slope {max_slope:.2}");
}

#[test]
fn watershed_connects_lakes_rivers_and_ocean() {
    // After the unified pass all three water tiers coexist: an ocean below sea level, plus perched
    // water (lakes + their feeder/outlet rivers) standing on the land above it.
    let mut w = built_canon();
    w.set_sea_level(0.0); // canon waterline
    w.reshape_and_reflow(1.0, 0.05);
    let depth = w.liquid_depth_export();
    let elev = w.elevation_export();
    let ocean = (0..elev.len()).filter(|&r| elev[r] <= 0.0 && depth[r] > 0.0).count();
    let perched = (0..elev.len()).filter(|&r| elev[r] > 0.0 && depth[r] > 0.0).count();
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

#[test]
fn liquid_layout_pools_painted_cells_selectively() {
    // The liquid-paint import overlay: a kind grid (0 = none, else kind+1) painted into the region map
    // pools a shallow static body of that kind on the cells it covers — and ONLY those cells. The grid
    // here paints the north half marsh (kind 2 → value 3) and leaves the south half unpainted (0).
    let mut w = built(); // 1000×1000 canon (mostly dry land)
    let pool = 0.01f64;
    // 1 col × 2 rows: north row marsh, south row none (row 0 = north).
    w.set_liquid_layout(&[3u8, 0u8], 1, 2);
    w.apply_liquid_layout(pool);

    let depth = w.liquid_depth_export();
    let kind = w.liquid_kind_export();
    // North dry-land cells now pool marsh at exactly the pool depth (max() leaves no prior water there).
    let pooled = (0..depth.len())
        .filter(|&r| (depth[r] - pool as f32).abs() < 1e-4 && kind[r] == 2)
        .count();
    assert!(pooled > 0, "painted marsh pools on dry land at the pool depth");
    // The south half is untouched: some cells stay completely dry (depth 0). (Proves it's selective,
    // not a flood-everything.)
    let dry = depth.iter().filter(|&&d| d == 0.0).count();
    assert!(dry > 0, "unpainted cells stay dry");

    // Idempotent: re-applying with the same overlay doesn't deepen the pools (max(), not add).
    w.apply_liquid_layout(pool);
    let depth2 = w.liquid_depth_export();
    assert_eq!(depth, depth2, "re-applying the overlay is a no-op (depth held by max)");

    // Clearing the overlay makes apply a no-op (the existing pools are left as-is, nothing new stamped).
    w.clear_liquid_layout();
    let before = w.liquid_depth_export();
    w.apply_liquid_layout(pool);
    assert_eq!(before, w.liquid_depth_export(), "no overlay → apply does nothing");
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

// --- Zone Edit tool: smart selection + whole-zone operations ---------------------------------

fn sel_variance(w: &World, sel: &[u32]) -> f64 {
    let e = w.elevation_export();
    let vals: Vec<f64> = sel.iter().map(|&r| e[r as usize] as f64).collect();
    let n = vals.len().max(1) as f64;
    let mean = vals.iter().sum::<f64>() / n;
    vals.iter().map(|&x| (x - mean) * (x - mean)).sum::<f64>() / n
}

#[test]
fn select_by_elevation_grows_on_smooth_and_stops_at_contrast() {
    let mut w = built();
    let nr = w.region_count();
    w.set_elevation(&vec![0.0f32; nr]); // perfectly flat
    let start = w.region_at(500.0, 500.0).expect("centre cell");
    // On smooth ground a tight tolerance still grows across the whole interior.
    let flat_sel = w.select_by_elevation(start, 0.01);
    assert!(flat_sel.len() > nr / 2, "smart-select grows across smooth terrain ({} of {nr})", flat_sel.len());

    // Punch a lone tall spike; with a tight tolerance it is a contrast wall the select stops at.
    let spike = w.region_at(250.0, 250.0).expect("spike cell");
    assert_ne!(spike, start, "distinct seed and spike cells");
    let mut elev = w.elevation_export();
    elev[spike] = 1.0;
    w.set_elevation(&elev);
    let sel = w.select_by_elevation(start, 0.01);
    assert!(!sel.contains(&(spike as u32)), "the smart-select stops at the sharp spike");
    // Seeding on the spike: every neighbour is a cliff away → only the spike selects.
    assert_eq!(w.select_by_elevation(spike, 0.01), vec![spike as u32], "an isolated spike selects only itself");
}

#[test]
fn zone_offset_shifts_selected_only() {
    let mut w = built();
    let nr = w.region_count();
    w.set_elevation(&vec![0.0f32; nr]);
    let spike = w.region_at(250.0, 250.0).expect("spike");
    let mut elev = w.elevation_export();
    elev[spike] = 1.0;
    w.set_elevation(&elev);
    let start = w.region_at(500.0, 500.0).expect("centre");
    let sel = w.select_by_elevation(start, 0.01); // excludes the spike
    let before = w.elevation_export();
    let touched = w.zone_offset(&sel, -0.2);
    assert_eq!(touched.len(), sel.len(), "offset touches every selected cell");
    let after = w.elevation_export();
    for &r in &sel {
        let r = r as usize;
        assert!((after[r] - (before[r] - 0.2)).abs() < 1e-5, "selected cell dropped by 0.2");
    }
    assert!((after[spike] - 1.0).abs() < 1e-6, "the unselected spike is unchanged");
}

#[test]
fn zone_level_eases_to_target() {
    let target = 0.3f64;
    // weight 1.0 → every selected cell lands exactly on the target plane (level a plateau).
    let mut w = built();
    let start = w.region_at(500.0, 500.0).expect("centre");
    let sel = w.select_contiguous(start);
    w.zone_level(&sel, target, 1.0);
    let after = w.elevation_export();
    for &r in &sel {
        assert!((after[r as usize] as f64 - target).abs() < 1e-5, "weight 1 flattens to target");
    }
    // weight 0.5 → each cell eases halfway from its start toward the target.
    let mut w2 = built();
    let sel2 = w2.select_contiguous(start);
    let before = w2.elevation_export();
    w2.zone_level(&sel2, target, 0.5);
    let after2 = w2.elevation_export();
    for &r in &sel2 {
        let r = r as usize;
        let expect = before[r] as f64 + (target - before[r] as f64) * 0.5;
        assert!((after2[r] as f64 - expect).abs() < 1e-5, "weight 0.5 eases halfway");
    }
}

#[test]
fn zone_mean_matches_manual_average() {
    let w = built();
    let start = w.region_at(500.0, 500.0).expect("centre");
    let sel = w.select_contiguous(start);
    let elev = w.elevation_export();
    let manual: f64 = sel.iter().map(|&r| elev[r as usize] as f64).sum::<f64>() / sel.len() as f64;
    let got = w.zone_mean_elevation(&sel);
    assert!((got - manual).abs() < 1e-5, "zone mean matches manual average: {got} vs {manual}");
}

#[test]
fn zone_grade_ramps_along_the_axis() {
    let mut w = built();
    let nr = w.region_count();
    w.set_elevation(&vec![0.0f32; nr]);
    let start = w.region_at(500.0, 500.0).expect("centre");
    let sel = w.select_by_elevation(start, 0.01); // the whole flat interior
    // Ramp 0 at x=0 to 1 at x=1000 (west→east).
    let touched = w.zone_grade(&sel, 0.0, 500.0, 0.0, 1000.0, 500.0, 1.0);
    assert_eq!(touched.len(), sel.len());
    let west = w.height_at(120.0, 500.0).expect("west sample");
    let east = w.height_at(880.0, 500.0).expect("east sample");
    assert!(east > west, "grade rises west→east: {west} -> {east}");
    assert!((0.0..=1.0).contains(&west) && (0.0..=1.0).contains(&east), "within the ramp range");
}

#[test]
fn zone_fill_to_rim_sets_depth() {
    let mut w = built();
    let nr = w.region_count();
    w.set_elevation(&vec![0.0f32; nr]); // flat plate at 0
    let start = w.region_at(500.0, 500.0).expect("centre");
    let basin = vec![start as u32]; // a one-cell basin: its rim is the surrounding plate (0.0)
    w.zone_offset(&basin, -0.3); // dig it to -0.3
    let rim = w.zone_rim_level(&basin);
    assert!((rim - 0.0).abs() < 1e-6, "rim is the surrounding plate level: {rim}");
    let touched = w.zone_fill_liquid(&basin, rim, 0);
    assert_eq!(touched.len(), 1);
    let depth = w.liquid_depth_export();
    assert!((depth[start] as f64 - 0.3).abs() < 1e-5, "filled to rim → depth = rim - floor = 0.3");
}

#[test]
fn select_water_body_floods_only_wet_cells() {
    let mut w = built_canon();
    let sea = -0.4f64;
    w.set_sea_level(sea);
    w.fill_lakes();
    let region = w.region_export();
    let depth = w.liquid_depth_export();
    let elev = w.elevation_export();
    // Seed on a perched Great-Lake (region 3) cell that holds water.
    let seed = (0..region.len())
        .find(|&r| region[r] == 3 && depth[r] as f64 > 1e-3)
        .expect("a wet Great-Lake cell");
    let body = w.select_water_body(seed);
    assert!(body.contains(&(seed as u32)), "the seed is part of its own water body");
    for &r in &body {
        let r = r as usize;
        let wet = elev[r] as f64 <= sea || depth[r] as f64 > 0.0;
        assert!(wet, "every cell in the body is standing water");
    }
}

#[test]
fn zone_smooth_reduces_variance() {
    let mut w = built_canon(); // 8 km → regions have many cells, so the noise variance is meaningful
    let nr = w.region_count();
    // Put a deterministic noise field around one level across the WHOLE map, so smoothing's only job is
    // to relax that noise — the neighbour anchors are the same distribution, not wildly-different region
    // heights (which would legitimately *raise* a zone's variance by pulling its rim toward a tall
    // neighbour). This isolates the relaxation behaviour the test means to check.
    let mut e = vec![0.0f32; nr];
    for (r, v) in e.iter_mut().enumerate() {
        let h = (r as u32).wrapping_mul(2_654_435_761) >> 8 & 0xff; // stable per-cell hash
        *v = h as f32 / 255.0 * 0.4 - 0.2; // ~[-0.2, 0.2]
    }
    w.set_elevation(&e);
    let g = pick_land_region(&w);
    let reg = w.region_export();
    let start = (0..reg.len()).find(|&r| reg[r] as usize == g).expect("a land region cell");
    let sel = w.select_contiguous(start);
    let v0 = sel_variance(&w, &sel);
    w.zone_smooth(&sel, 8, 0.5);
    let v1 = sel_variance(&w, &sel);
    assert!(v1 < v0, "smoothing relaxes in-zone noise: {v0} -> {v1}");
}

#[test]
fn zone_feather_edits_edges_less_than_a_full_smooth() {
    // A large canon region has genuine interior cells (not all on the rim), so a narrow feather —
    // which only weights cells near the border — must move strictly less material than a full smooth.
    let mut a = built_canon();
    let sa = a.select_contiguous(a.region_at(4000.0, 4000.0).unwrap());
    assert!(sa.len() > 50, "a large region with real interior ({} cells)", sa.len());
    let before_a = a.elevation_export();
    a.zone_smooth(&sa, 3, 0.5);
    let after_a = a.elevation_export();
    let full: f64 = sa.iter().map(|&r| (after_a[r as usize] - before_a[r as usize]).abs() as f64).sum();

    let mut b = built_canon();
    let sb = b.select_contiguous(b.region_at(4000.0, 4000.0).unwrap());
    let before_b = b.elevation_export();
    b.zone_feather_edges(&sb, 1.0, 0.5, 3); // tiny width → only the rim moves
    let after_b = b.elevation_export();
    let feath: f64 = sb.iter().map(|&r| (after_b[r as usize] - before_b[r as usize]).abs() as f64).sum();

    assert!(feath > 0.0, "feather moves the edge");
    assert!(feath < full, "a narrow feather touches less than a full smooth: {feath} vs {full}");
}

// --- per-Region base height + gradient + rotation (Generate-time terrain trunk) ---------------

fn region_mean_elev(w: &World, g: usize) -> f64 {
    let reg = w.region_export();
    let el = w.elevation_export();
    let (mut s, mut n) = (0.0f64, 0.0f64);
    for r in 0..reg.len() {
        if reg[r] as usize == g {
            s += el[r] as f64;
            n += 1.0;
        }
    }
    if n > 0.0 { s / n } else { 0.0 }
}

fn region_elev_spread(w: &World, g: usize) -> f64 {
    let reg = w.region_export();
    let el = w.elevation_export();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for r in 0..reg.len() {
        if reg[r] as usize == g {
            let e = el[r] as f64;
            lo = lo.min(e);
            hi = hi.max(e);
        }
    }
    if hi >= lo { hi - lo } else { 0.0 }
}

/// The largest land region (excludes ocean id 0 and the Great Lake id 3) — a stable, sizable target.
fn pick_land_region(w: &World) -> usize {
    let reg = w.region_export();
    let mut counts = [0usize; 16];
    for &id in &reg {
        let id = id as usize;
        if id >= 1 && id < 16 {
            counts[id] += 1;
        }
    }
    let (mut best, mut bestc) = (0usize, 0usize);
    for id in 1..16 {
        if id != 3 && counts[id] > bestc {
            bestc = counts[id];
            best = id;
        }
    }
    best
}

#[test]
fn region_base_height_lifts_its_cells_on_generate() {
    // Raising a region's base height should lift that region's terrain on the next Generate.
    let mut w0 = World::new();
    w0.set_base_blend_m(300.0); // sharp base trunk so the change isn't washed by the wide grade
    w0.build(8000.0, 8000.0, 40.0, 7, 5);
    let g = pick_land_region(&w0);
    assert!(g >= 1, "found a land region");
    let m0 = region_mean_elev(&w0, g);

    let mut w1 = World::new();
    w1.set_base_blend_m(300.0);
    let cur = w1.region_base_height_of(g);
    w1.set_region_base_height(g, cur + 0.6);
    w1.build(8000.0, 8000.0, 40.0, 7, 5);
    let m1 = region_mean_elev(&w1, g);
    assert!(m1 > m0 + 0.3, "base height raises the region's mean elevation: {m0} -> {m1}");
}

#[test]
fn region_gradient_widens_the_elevation_spread() {
    // A gradient tilts the region's base plane, so its cells span a wider elevation range than flat.
    // Flatten ALL region base heights (and zero their gradients) first, so the only spread comes from
    // the gradient under test — not the inter-region grading of the (now widely varied) canon defaults.
    let g = {
        let mut w = World::new();
        w.set_base_blend_m(300.0);
        w.build(8000.0, 8000.0, 40.0, 7, 5);
        pick_land_region(&w)
    };
    let spread_with = |grad: f64| {
        let mut w = World::new();
        w.set_base_blend_m(300.0);
        for id in 0..=14usize {
            w.set_region_base_height(id, 0.3);
            w.set_region_gradient(id, 0.0);
        }
        w.set_region_gradient(g, grad);
        w.set_region_gradient_rotation(g, 90.0); // east
        w.build(8000.0, 8000.0, 40.0, 7, 5);
        region_elev_spread(&w, g)
    };
    let s0 = spread_with(0.0);
    let s1 = spread_with(0.8); // 0.8 total rise across the region (normalized)
    assert!(s1 > s0 + 0.3, "the gradient widens the region's elevation spread: {s0} -> {s1}");
}

#[test]
fn gradient_anchor_pins_the_level_independent_of_base_height() {
    let g = {
        let mut w = World::new();
        w.set_base_blend_m(300.0);
        w.build(8000.0, 8000.0, 40.0, 7, 5);
        pick_land_region(&w)
    };
    let build_mean = |base: f64, anchor: f64| {
        let mut w = World::new();
        w.set_base_blend_m(300.0);
        w.set_region_base_height(g, base);
        w.set_region_gradient_anchor(g, anchor);
        w.build(8000.0, 8000.0, 40.0, 7, 5);
        region_mean_elev(&w, g)
    };
    // Anchor set → base height no longer moves the region's level.
    let pinned_lo = build_mean(0.1, 0.5);
    let pinned_hi = build_mean(0.9, 0.5);
    assert!((pinned_lo - pinned_hi).abs() < 0.1, "anchor pins the level regardless of base: {pinned_lo} vs {pinned_hi}");
    // Sanity: with NO anchor, base height DOES move it.
    let free_lo = build_mean(0.1, f64::NAN);
    let free_hi = build_mean(0.9, f64::NAN);
    assert!(free_hi > free_lo + 0.3, "without an anchor, base height moves the level: {free_lo} -> {free_hi}");
}

#[test]
fn selection_surface_packs_the_selected_cells() {
    let mut w = World::new();
    w.build(3000.0, 3000.0, 12.0, 7, 5); // multi-chunk, dense
    let start = w.region_at(1500.0, 1500.0).expect("centre");
    let sel = w.select_contiguous(start);
    assert!(sel.len() > 3, "a real region cluster");
    let s = w.selection_surface(&sel, 100.0).expect("a surface");
    assert!(!s.positions.is_empty() && !s.indices.is_empty(), "selected cells produce overlay geometry");
    let e = w.selection_surface(&[], 100.0).expect("a (degenerate) surface");
    assert!(e.positions.is_empty() && e.indices.is_empty(), "no selection → no overlay");
}
