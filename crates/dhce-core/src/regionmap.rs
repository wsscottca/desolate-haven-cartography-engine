//! Canon region layout — the **first** pass of the canon-map pipeline (ADR 0004 / the canon-map
//! change). Assigns every mesh cell a region id (`0` = ocean, `1..=REGION_COUNT` = a canon place),
//! which becomes the authoritative `region_r` field and *is* the territory partition.
//!
//! Two sources, both pure and deterministic:
//! - [`layout_from_anchors`] — a built-in canonical layout from [`canon_anchors`] (nearest anchor,
//!   squared distance, low-id tiebreak), the default when nothing is imported.
//! - [`layout_from_grid`] — samples an imported region-coloured PNG that the front-end has already
//!   resolved to an id grid (row 0 = north, matching the mesh's north-small-y frame).
//!
//! Determinism: per-cell argmin over a fixed anchor set / integer grid indexing, wrapped in the
//! order-preserving [`crate::util::par_map`]. No transcendentals beyond the contract-safe `sqrt`
//! (here only squared distances are compared, so not even that).

use crate::mesh::Mesh;
use crate::regions::REGION_COUNT;

/// The 14 canon Region anchors as `(region_id, nx, ny)` in a **north-up, normalized** frame
/// (`nx,ny ∈ [0,1]`, **north = small y** — matches the mesh frame and the minimap's row 0 = north).
/// Translated from the guide's verbal geography (`lore/geography.md`); approximate starting points,
/// overridable by an imported PNG. The Great Lake sits just **northeast of center** per the design.
pub fn canon_anchors() -> [(u8, f64, f64); REGION_COUNT] {
    // Positioned to the canon base map (`assets/canon-map-base.jpg`): icy peaks NW, gray peaks N,
    // conifer forests W-of-center (Deep Wood W / Temperate E), the elven valley + small lake at
    // center, the human grass-plains belt across the whole east, marsh in the NE corner, the tan
    // savanna south-center, and the lava volcano at south-center with the blight/thorn lands SW.
    [
        (1, 0.46, 0.09),  // Jagged Mountains — gray rocky peaks, north-center
        (2, 0.49, 0.36),  // Sacred Woods & Plateau — central green valley + settlement
        (3, 0.40, 0.30),  // Great Lake — small lake just NW of the valley (river headwaters)
        (4, 0.34, 0.40),  // Temperate Forest — east half of the conifer band
        (5, 0.74, 0.46),  // Open Plains — the big eastern grass-hills (Human Castle belt)
        (6, 0.40, 0.56),  // Underdeep — tan southern savanna, south-center
        (7, 0.25, 0.33),  // Deep Wood — dense conifer forest, west half of the band
        (8, 0.18, 0.10),  // Frozen Reaches — snowy ice-peaks NW + west-coast glaciers
        (9, 0.06, 0.26),  // Lost Isles — far-NW coast / floating isles
        (10, 0.34, 0.74), // Blisterwood — red thorn strip hugging the volcano's west flank
        (11, 0.45, 0.82), // Volcanic Scape — the lava volcano, south-center
        (12, 0.17, 0.63), // Blight Ruins — blighted gray covering the SW coast
        (13, 0.87, 0.74), // Scattered Isles — SE port isles
        (14, 0.80, 0.15), // Marsh & Bog — mottled wetland, NE corner
    ]
}

/// The canon base map (`assets/canon-map-base.jpg`) classified into a region raster, baked at build
/// time. `crates/dhce-core/canon/classify_canon.py` flood-fills the ocean for the real organic
/// coastline and assigns each land pixel to a region by colour-class + nearest-centroid, then bakes
/// the artist's 4:3 map straight through (no letterbox/crop) into this `CANON_W × CANON_H` grid
/// (row 0 = north), matching the canon's aspect and the 4:3 world. Regenerate by re-running that
/// script. The grid is sampled by normalized `(u, v)`, so it is decoupled from the world's metres.
const CANON_W: usize = 1536;
const CANON_H: usize = 1152;
static CANON_MAP: &[u8] = include_bytes!("canon_region_map.bin");

/// Region id at a world point's normalized `(u, v)` (`u` west→east, `v` north→south, row 0 = north):
/// a direct sample of the baked canon raster ([`CANON_MAP`]). `0` = ocean, `1..=14` = a canon place.
/// This *is* the territory partition — the elevation + climate passes derive everything else from it,
/// so the generated world reproduces the canon silhouette + region placement, organic edges and all.
pub fn canon_region_at(u: f64, v: f64) -> u8 {
    if CANON_MAP.len() < CANON_W * CANON_H {
        return 0;
    }
    let gx = ((u.clamp(0.0, 1.0) * CANON_W as f64) as usize).min(CANON_W - 1);
    let gy = ((v.clamp(0.0, 1.0) * CANON_H as f64) as usize).min(CANON_H - 1);
    let id = CANON_MAP[gy * CANON_W + gx];
    if id as usize <= REGION_COUNT { id } else { 0 }
}

/// Built-in canon layout: each non-boundary cell reads its region straight from the traced canon map
/// ([`canon_region_at`]). Boundary-frame cells are forced ocean (`0`). This replaced the earlier
/// nearest-anchor Voronoi layout (which could only make convex blobs, never the canon's irregular
/// coastline + diagonal forest spit); [`canon_anchors`] is kept only as a coordinate reference.
pub fn layout_from_anchors(mesh: &Mesh, width: f64, height: f64) -> Vec<u8> {
    crate::util::par_map(mesh.num_regions(), |r| {
        if mesh.is_boundary_r(r) {
            return 0u8;
        }
        let p = mesh.pos_of_r(r);
        canon_region_at(p[0] / width, p[1] / height)
    })
}

/// PNG-override layout: sample a front-end-resolved id grid (`ids`, row-major, `cols × rows`,
/// **row 0 = north**) at each cell's normalized position. `ids` hold `0` (ocean / unmatched) or
/// `1..=REGION_COUNT`. Out-of-range ids and boundary cells fall back to ocean (`0`).
pub fn layout_from_grid(mesh: &Mesh, width: f64, height: f64, ids: &[u8], cols: usize, rows: usize) -> Vec<u8> {
    if cols == 0 || rows == 0 || ids.len() < cols * rows {
        return layout_from_anchors(mesh, width, height);
    }
    crate::util::par_map(mesh.num_regions(), |r| {
        if mesh.is_boundary_r(r) {
            return 0u8;
        }
        let p = mesh.pos_of_r(r);
        let u = (p[0] / width).clamp(0.0, 1.0);
        let v = (p[1] / height).clamp(0.0, 1.0);
        let gx = ((u * cols as f64) as usize).min(cols - 1);
        let gy = ((v * rows as f64) as usize).min(rows - 1);
        let id = ids[gy * cols + gx];
        if id as usize <= REGION_COUNT { id } else { 0 }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> Mesh {
        // Dense enough to land a cell inside even the small canon regions (Great Lake / Lost Isles /
        // Blisterwood are well under 1% of the raster).
        Mesh::new(1000.0, 1000.0, 10.0, 7)
    }

    #[test]
    fn anchors_cover_all_regions_once() {
        let a = canon_anchors();
        assert_eq!(a.len(), REGION_COUNT);
        let mut seen = [false; REGION_COUNT + 1];
        for &(id, nx, ny) in a.iter() {
            assert!((1..=REGION_COUNT as u8).contains(&id), "id in range");
            assert!(!seen[id as usize], "each region id appears once");
            seen[id as usize] = true;
            assert!((0.0..=1.0).contains(&nx) && (0.0..=1.0).contains(&ny), "anchor in unit square");
        }
    }

    #[test]
    fn layout_ids_in_range_and_boundary_is_ocean() {
        let m = mesh();
        let lay = layout_from_anchors(&m, 1000.0, 1000.0);
        assert_eq!(lay.len(), m.num_regions());
        for r in 0..m.num_regions() {
            assert!(lay[r] as usize <= REGION_COUNT, "id in 0..=REGION_COUNT");
            if m.is_boundary_r(r) {
                assert_eq!(lay[r], 0, "boundary frame is ocean");
            }
        }
    }

    #[test]
    fn every_region_present() {
        let m = mesh();
        let lay = layout_from_anchors(&m, 1000.0, 1000.0);
        for id in 1..=REGION_COUNT as u8 {
            assert!(lay.iter().any(|&x| x == id), "region {id} has at least one cell");
        }
    }

    #[test]
    fn great_lake_anchor_is_north_central() {
        let (id, nx, ny) = canon_anchors()[2]; // index 2 → region id 3
        assert_eq!(id, 3, "the third anchor is the Great Lake");
        // Canon map: the lake sits just NW of the central valley (river headwaters) — north of
        // center, roughly central horizontally.
        assert!(ny < 0.5, "the lake is north of center (north = small y; ny={ny})");
        assert!((0.25..=0.55).contains(&nx), "the lake is near the horizontal center (nx={nx})");
    }

    #[test]
    fn corners_are_ocean() {
        // The canon map is open sea in all four corners (the continent fills the 4:3 frame and now
        // reaches the N/S edges — the full-frame bake no longer letterboxes — but the corners stay
        // ocean), so the corner cells of the baked raster read as ocean. Edge *midpoints* may be land,
        // so only the corners are asserted here.
        let m = mesh();
        let lay = layout_from_anchors(&m, 1000.0, 1000.0);
        let mut checked = 0;
        for r in 0..m.num_regions() {
            if m.is_boundary_r(r) {
                continue;
            }
            let p = m.pos_of_r(r);
            let (u, v) = (p[0] / 1000.0, p[1] / 1000.0);
            if (u < 0.04 || u > 0.96) && (v < 0.04 || v > 0.96) {
                assert_eq!(lay[r], 0, "a corner cell reads as ocean (u={u}, v={v})");
                checked += 1;
            }
        }
        assert!(checked > 0, "the coarse mesh has some corner cells to check");
    }

    #[test]
    fn deterministic_repeat() {
        let m = mesh();
        let a = layout_from_anchors(&m, 1000.0, 1000.0);
        let b = layout_from_anchors(&m, 1000.0, 1000.0);
        assert_eq!(a, b, "layout is deterministic");
    }

    #[test]
    fn grid_override_assigns_and_falls_back() {
        let m = mesh();
        // A 2×2 grid: north row all region 1, south row all region 5.
        let ids = vec![1u8, 1, 5, 5];
        let lay = layout_from_grid(&m, 1000.0, 1000.0, &ids, 2, 2);
        // A northern interior cell should read 1; a southern one should read 5.
        let mut saw_north = false;
        let mut saw_south = false;
        for r in 0..m.num_regions() {
            if m.is_boundary_r(r) {
                continue;
            }
            let p = m.pos_of_r(r);
            if p[1] < 400.0 {
                assert_eq!(lay[r], 1, "north half = region 1");
                saw_north = true;
            } else if p[1] > 600.0 {
                assert_eq!(lay[r], 5, "south half = region 5");
                saw_south = true;
            }
        }
        assert!(saw_north && saw_south, "sampled both halves");
        // Empty grid → falls back to the anchor layout (non-empty, in range).
        let fb = layout_from_grid(&m, 1000.0, 1000.0, &[], 0, 0);
        assert_eq!(fb.len(), m.num_regions());
    }
}
