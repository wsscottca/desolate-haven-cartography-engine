//! Procedural tributary streams via flow accumulation on the dual mesh.
//!
//! The user paints the *main* rivers (the Course tool tags those regions in a seed
//! mask); this module grows the feeder network that drains into them. It is a single
//! near-linear pass:
//!
//! 1. **Priority-flood pit fill** (Barnes/Planchon + ε) so every interior cell drains
//!    to the boundary frame — no closed basins stall the accumulation.
//! 2. **Steepest-descent receiver** per cell (its lowest filled neighbour).
//! 3. **Flow accumulation** in descending-elevation order → upstream catchment size.
//! 4. **Catchment gate**: an ascending pass marks the cells whose flow path reaches a
//!    seeded (painted) river, so streams manifest *only* inside the catchment that
//!    feeds a hand-drawn main river.
//! 5. **Stream + carve**: cells past a flow threshold (and all seed cells) become
//!    streams, carved toward a flow-scaled bed depth.
//!
//! Determinism (the cross-target contract): only `sqrt`/`floor`/`+`/`*`/compare — no
//! `sin`/`cos`/`exp`. The priority queue and the accumulation sort both use a
//! **total order** keyed on `(f64::total_cmp, region index)`, so ties resolve
//! identically on wasm32 and native.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

/// Tiny monotone increment applied while pit-filling, so flats acquire a deterministic
/// gradient toward their spill point.
const EPS: f64 = 1.0e-7;
/// A stream cell carves at least this deep (normalized elevation units). Kept shallow (~8 m at the
/// 5 km height scale) so small streams read as gentle channels, not incised trenches.
pub(crate) const MIN_STREAM_DEPTH: f64 = 0.005;

/// Output of [`accumulate`].
pub struct StreamResult {
    /// Upstream catchment size (cell count) draining through each region.
    pub flow: Vec<f64>,
    /// Whether each region is part of the generated stream network.
    pub is_stream: Vec<bool>,
    /// Per-region channel depth to carve (≥ 0; 0 where not a stream).
    pub carve_delta: Vec<f64>,
}

/// Priority-flood pit fill (Barnes/Planchon + ε): the elevation water rises to at each cell before it
/// drains to the boundary frame (the global outlets, regions `0..num_boundary`). `filled[r] - terrain[r]`
/// is the standing-water depth that fills `r`'s closed basin to its **pour point** — the basis for both
/// stream routing (no basin stalls accumulation) and perched **lakes** ([`World::fill_lakes`]). Cells
/// unreachable from the boundary keep `f64::INFINITY`. Deterministic: total-order `(elevation, index)` heap.
pub(crate) fn fill_depressions(terrain: &[f64], neighbors: &[Vec<u32>], num_boundary: usize) -> Vec<f64> {
    let n = terrain.len();
    let mut filled = vec![f64::INFINITY; n];
    if n == 0 || neighbors.len() != n {
        return filled;
    }
    let num_boundary = num_boundary.min(n);
    let mut closed = vec![false; n];
    let mut heap: BinaryHeap<Reverse<Key>> = BinaryHeap::new();
    for r in 0..num_boundary {
        filled[r] = terrain[r];
        closed[r] = true;
        heap.push(Reverse(Key { e: filled[r], i: r }));
    }
    while let Some(Reverse(Key { e, i })) = heap.pop() {
        for &nb in &neighbors[i] {
            let nb = nb as usize;
            if closed[nb] {
                continue;
            }
            let f = terrain[nb].max(e + EPS);
            filled[nb] = f;
            closed[nb] = true;
            heap.push(Reverse(Key { e: f, i: nb }));
        }
    }
    filled
}

/// Total-order key over `(elevation, index)` for the priority queue. `f64` is not
/// `Ord`, so we wrap it and use `total_cmp` (deterministic across targets).
#[derive(Copy, Clone)]
struct Key {
    e: f64,
    i: usize,
}
impl PartialEq for Key {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Key {
    fn cmp(&self, o: &Self) -> Ordering {
        self.e.total_cmp(&o.e).then_with(|| self.i.cmp(&o.i))
    }
}

/// The drainage graph — the seed-free core shared by [`accumulate`] (painted tributaries) and
/// `World::generate_rivers` (auto trunk rivers). Follow `receiver` from any cell and you reach the
/// boundary frame, passing *through* filled basins (lakes), so this single solve ties ocean, lakes,
/// and rivers into one watershed.
pub(crate) struct Drainage {
    /// Steepest-descent receiver per cell (its lowest filled neighbour; `receiver[r] == r` = sink).
    /// Built on the pit-filled DEM, so following it routes *through* basins (lakes) to the boundary.
    pub receiver: Vec<usize>,
    /// Accumulated flow through each cell = sum of upstream per-cell `weight` (unit if `weight` is None).
    pub flow: Vec<f64>,
    /// Processing order (descending filled elevation, ascending index): a cell precedes its receiver.
    pub order: Vec<usize>,
}

/// Build the drainage graph: priority-flood pit fill → steepest-descent receivers → flow accumulation.
///
/// - `weight`: per-cell flow contribution. `None` = unit catchment (cell count, today's behaviour).
///   Pass a per-cell field (e.g. moisture / rainfall) to make flow bigger where that field is larger —
///   the tie that makes rivers grow where it rains.
///
/// Deterministic by reuse: the same total-order `(elevation, index)` heap and sort as the rest of the
/// module, so parallel == serial and wasm == native.
pub(crate) fn drainage(
    terrain: &[f64],
    neighbors: &[Vec<u32>],
    num_boundary: usize,
    weight: Option<&[f64]>,
) -> Drainage {
    let n = terrain.len();
    if n == 0 || neighbors.len() != n {
        return Drainage {
            receiver: (0..n).collect(),
            flow: vec![0.0; n],
            order: Vec::new(),
        };
    }
    let num_boundary = num_boundary.min(n);

    // 1. Priority-flood pit fill. Boundary regions are the global outlets.
    let filled = fill_depressions(terrain, neighbors, num_boundary);

    // 2. Steepest-descent receiver (lowest filled neighbour; else self = sink).
    let mut receiver: Vec<usize> = (0..n).collect();
    for r in num_boundary..n {
        if !filled[r].is_finite() {
            continue; // unreachable cell (disconnected) — stays a sink
        }
        let mut best = r;
        for &nbu in &neighbors[r] {
            let nb = nbu as usize;
            let lower = filled[nb] < filled[best];
            let tie = best != r && filled[nb] == filled[best] && nb < best;
            if lower || tie {
                best = nb;
            }
        }
        receiver[r] = best;
    }

    // Processing order: descending filled elevation, tie-break ascending index. With
    // strictly-lower receivers this guarantees a cell is processed before its receiver.
    let mut order: Vec<usize> = (0..n).filter(|&r| filled[r].is_finite()).collect();
    order.sort_by(|&a, &b| filled[b].total_cmp(&filled[a]).then_with(|| a.cmp(&b)));

    // 3. Flow accumulation (each cell contributes its `weight` — unit by default — downstream).
    let mut flow: Vec<f64> = match weight {
        Some(w) => (0..n).map(|r| w.get(r).copied().unwrap_or(1.0)).collect(),
        None => vec![1.0f64; n],
    };
    for &r in &order {
        let rcv = receiver[r];
        if rcv != r {
            flow[rcv] += flow[r];
        }
    }

    Drainage { receiver, flow, order }
}

/// The **spill / pour-point** cells of every lake: a lake cell whose steepest-descent receiver leaves
/// the lake (drains into a non-lake cell) is where that basin overflows downstream. The outlet river
/// continues from `receiver[spill]` toward the ocean. Returned in ascending index order.
pub(crate) fn lake_spill_cells(receiver: &[usize], lake_mask: &[bool]) -> Vec<usize> {
    let mut out = Vec::new();
    for r in 0..receiver.len() {
        if !lake_mask.get(r).copied().unwrap_or(false) {
            continue;
        }
        let rcv = receiver[r];
        if rcv != r && !lake_mask.get(rcv).copied().unwrap_or(false) {
            out.push(r);
        }
    }
    out
}

/// Grow tributary streams that drain into the seeded (painted) main rivers.
///
/// - `terrain`: per-region normalized elevation.
/// - `neighbors`: region adjacency (`Mesh::region_neighbors`).
/// - `num_boundary`: regions `0..num_boundary` are the boundary frame (global outlets).
/// - `seed_mask`: regions the user painted as main-river course.
/// - `threshold`: stream cutoff as a fraction `[0,1]` of the max catchment in the
///   feeding catchment (lower ⇒ more, finer tributaries).
/// - `depth_gain`: scales channel depth with `sqrt(normalized flow)`.
pub fn accumulate(
    terrain: &[f64],
    neighbors: &[Vec<u32>],
    num_boundary: usize,
    seed_mask: &[bool],
    threshold: f64,
    depth_gain: f64,
) -> StreamResult {
    let n = terrain.len();
    if n == 0 || neighbors.len() != n {
        return StreamResult {
            flow: vec![0.0; n],
            is_stream: vec![false; n],
            carve_delta: vec![0.0; n],
        };
    }
    let num_boundary = num_boundary.min(n);

    // Stages 1–3: the shared drainage solve (pit fill → receivers → unit flow accumulation).
    let Drainage { receiver, flow, order, .. } = drainage(terrain, neighbors, num_boundary, None);

    // 4. Catchment gate: ascending pass marks cells that drain into a painted river.
    let mut reaches = vec![false; n];
    for r in 0..n {
        reaches[r] = seed_mask.get(r).copied().unwrap_or(false);
    }
    for &r in order.iter().rev() {
        let rcv = receiver[r];
        if rcv != r && reaches[rcv] {
            reaches[r] = true;
        }
    }

    // 5. Streams + carve. Threshold is relative to the max catchment that reaches a
    //    main river, so it scales with whatever the user actually painted.
    let mut max_flow = 1.0f64;
    for r in num_boundary..n {
        if reaches[r] && flow[r] > max_flow {
            max_flow = flow[r];
        }
    }
    let thr_abs = threshold * max_flow;
    let mut is_stream = vec![false; n];
    let mut carve_delta = vec![0.0f64; n];
    for r in num_boundary..n {
        if !reaches[r] {
            continue;
        }
        let seeded = seed_mask.get(r).copied().unwrap_or(false);
        if seeded || flow[r] >= thr_abs {
            is_stream[r] = true;
            let nf = (flow[r] / max_flow).clamp(0.0, 1.0).sqrt();
            carve_delta[r] = MIN_STREAM_DEPTH + depth_gain * nf;
        }
    }

    StreamResult {
        flow,
        is_stream,
        carve_delta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_depressions_ponds_a_basin_to_its_pour_point() {
        // 0 = boundary outlet (the map edge / ocean), 1 = a high rim, 2 = the basin floor behind it.
        // Water in the basin can only escape over the rim, so it fills to the rim height.
        let terrain = [0.0, 0.5, 0.1];
        let neighbors: Vec<Vec<u32>> = vec![vec![1], vec![0, 2], vec![1]];
        let filled = fill_depressions(&terrain, &neighbors, 1);
        // Basin (cell 2) fills up to the rim (~0.5): a perched lake of depth ≈ 0.4.
        let lake = filled[2] - terrain[2];
        assert!((lake - 0.4).abs() < 1.0e-3, "basin fills to the pour point (depth {lake})");
        // The rim itself (cell 1) drains freely → no standing water.
        assert!((filled[1] - terrain[1]).abs() < 1.0e-3, "the rim holds no water");
    }

    #[test]
    fn drainage_accumulates_downhill_and_honours_weight() {
        // Linear chain draining to the outlet: 0 (boundary) ← 1 ← 2 ← 3.
        let terrain = [0.0, 1.0, 2.0, 3.0];
        let neighbors: Vec<Vec<u32>> = vec![vec![1], vec![0, 2], vec![1, 3], vec![2]];

        // Unit weight: the outlet-adjacent cell collects the whole chain; the headwater carries itself.
        let d = drainage(&terrain, &neighbors, 1, None);
        assert_eq!(d.receiver[3], 2, "steepest descent points downhill");
        assert_eq!(d.flow[1], 3.0, "unit flow accumulates the upstream cell count");
        assert_eq!(d.flow[3], 1.0, "headwater carries only itself");

        // Weighted: tripling the rain on the headwater makes the trunk carry proportionally more.
        let w = [0.0, 1.0, 1.0, 3.0];
        let dw = drainage(&terrain, &neighbors, 1, Some(&w));
        assert_eq!(dw.flow[1], 5.0, "weight makes a wet headwater carry proportionally more");
        assert_eq!(dw.flow[3], 3.0, "headwater carries its own weight");
    }

    #[test]
    fn lake_spill_cells_finds_the_pour_point() {
        // Same chain; pretend cells 2,3 are a lake. Cell 2's receiver (1) is outside the lake → spill.
        let terrain = [0.0, 1.0, 2.0, 3.0];
        let neighbors: Vec<Vec<u32>> = vec![vec![1], vec![0, 2], vec![1, 3], vec![2]];
        let d = drainage(&terrain, &neighbors, 1, None);
        let lake_mask = [false, false, true, true];
        let spills = lake_spill_cells(&d.receiver, &lake_mask);
        assert_eq!(spills, vec![2], "the lake cell draining to dry land is the pour point");
    }
}
