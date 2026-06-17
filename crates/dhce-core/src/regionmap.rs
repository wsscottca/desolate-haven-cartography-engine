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

/// Uniform ocean margin (fraction of each side) the built-in layout forces to ocean, so the world is
/// island-bound on all four edges regardless of which region anchor sits nearest. ~0.06 ≈ 1.8 km on
/// a 30 km map. (The elevation pass's radial rim then rounds the coastline.)
const OCEAN_MARGIN: f64 = 0.06;

/// The 14 canon Region anchors as `(region_id, nx, ny)` in a **north-up, normalized** frame
/// (`nx,ny ∈ [0,1]`, **north = small y** — matches the mesh frame and the minimap's row 0 = north).
/// Translated from the guide's verbal geography (`lore/geography.md`); approximate starting points,
/// overridable by an imported PNG. The Great Lake sits just **northeast of center** per the design.
pub fn canon_anchors() -> [(u8, f64, f64); REGION_COUNT] {
    [
        (1, 0.50, 0.22),  // Jagged Mountains — north, upriver of the inflow
        (2, 0.36, 0.46),  // Sacred Woods & Plateau — west, lakeside
        (3, 0.55, 0.45),  // Great Lake — center, ~1–2 km northeast
        (4, 0.66, 0.42),  // Temperate Forest — east band, arcing NE→SW
        (5, 0.68, 0.74),  // Open Plains — southeast, the Human Castle belt
        (6, 0.52, 0.70),  // Underdeep — south of the lake
        (7, 0.20, 0.44),  // Deep Wood — west, beyond Sacred Woods
        (8, 0.10, 0.40),  // Frozen Reaches — farther west, past the Deep Wood
        (9, 0.07, 0.47),  // Lost Isles — far-west coast (past the Frozen Reaches)
        (10, 0.22, 0.78), // Blisterwood — far southwest
        (11, 0.40, 0.76), // Volcanic Scape — southwest, between Blisterwood & the plains
        (12, 0.12, 0.62), // Blight Ruins — southwest, among Blister/Frozen/Lost
        (13, 0.86, 0.80), // Scattered Isles — southeast of the castle (port)
        (14, 0.82, 0.58), // Marsh & Bog — east, NE of the castle / N of the Scattered Isles
    ]
}

/// Built-in canon layout: each non-boundary cell takes the **nearest** anchor (squared distance,
/// lowest id wins ties → total order → deterministic). Boundary-frame cells are ocean (`0`), so the
/// rectangle's rim reads as open water; the elevation pass's banded rim then drowns the outer band.
pub fn layout_from_anchors(mesh: &Mesh, width: f64, height: f64) -> Vec<u8> {
    let anchors = canon_anchors();
    crate::util::par_map(mesh.num_regions(), |r| {
        if mesh.is_boundary_r(r) {
            return 0u8;
        }
        let p = mesh.pos_of_r(r);
        let (u, v) = (p[0] / width, p[1] / height);
        // Uniform ocean margin on every side → the world is island-bound.
        if u < OCEAN_MARGIN || u > 1.0 - OCEAN_MARGIN || v < OCEAN_MARGIN || v > 1.0 - OCEAN_MARGIN {
            return 0u8;
        }
        let mut best_id = 0u8;
        let mut best_d2 = f64::INFINITY;
        for &(id, ax, ay) in anchors.iter() {
            let dx = u - ax;
            let dy = v - ay;
            let d2 = dx * dx + dy * dy;
            // Strictly-less keeps the first (lowest-id) anchor on a tie → deterministic.
            if d2 < best_d2 {
                best_d2 = d2;
                best_id = id;
            }
        }
        best_id
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
        // A coarse but dense-enough mesh to land cells near every anchor.
        Mesh::new(1000.0, 1000.0, 25.0, 7)
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
    fn great_lake_anchor_is_northeast_of_center() {
        let (id, nx, ny) = canon_anchors()[2]; // index 2 → region id 3
        assert_eq!(id, 3, "the third anchor is the Great Lake");
        assert!(nx > 0.5, "the lake is east of center (nx={nx})");
        assert!(ny < 0.5, "the lake is north of center (north = small y; ny={ny})");
    }

    #[test]
    fn outer_margin_is_ocean() {
        let m = mesh();
        let lay = layout_from_anchors(&m, 1000.0, 1000.0);
        let mut checked = false;
        for r in 0..m.num_regions() {
            if m.is_boundary_r(r) {
                continue;
            }
            let p = m.pos_of_r(r);
            let (u, v) = (p[0] / 1000.0, p[1] / 1000.0);
            if u < 0.04 || u > 0.96 || v < 0.04 || v > 0.96 {
                assert_eq!(lay[r], 0, "an outer-margin cell reads as ocean");
                checked = true;
            }
        }
        assert!(checked, "the coarse mesh has some margin cells to check");
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
