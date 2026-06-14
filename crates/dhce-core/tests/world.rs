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
