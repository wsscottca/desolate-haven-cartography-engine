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

    let centers = w.chunk_centers();
    assert_eq!(centers.len(), w.chunk_count() * 2, "one (x, y) per chunk id");

    // Each center sits on its tile's half-step and stays inside the world bounds — this is
    // the exact mapping the front-end relies on to turn a camera position into visible tiles.
    let sx = width / cols as f64;
    let sy = height / rows as f64;
    for id in 0..w.chunk_count() {
        let (gx, gy) = (id % cols, id / cols);
        let (cx, cy) = (centers[2 * id], centers[2 * id + 1]);
        assert!((cx - (gx as f64 + 0.5) * sx).abs() < 1e-6, "center x is the tile mid-point");
        assert!((cy - (gy as f64 + 0.5) * sy).abs() < 1e-6, "center y is the tile mid-point");
        assert!(cx > 0.0 && cx < width && cy > 0.0 && cy < height, "center is in-bounds");
    }
}
