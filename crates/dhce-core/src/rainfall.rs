//! Climate-driven rainfall.
//!
//! Derives a per-cell rainfall amount from the per-region [`WaterProfile`](crate::regions::WaterProfile)
//! (raininess / rain-shadow / evaporation) plus the per-cell `moisture` and `temperature` traits and
//! an orographic / rain-shadow term taken from the upwind neighbour's elevation. The result feeds
//! [`fluid::add_rain_field`](crate::fluid::add_rain_field), replacing the old uniform manual-rain
//! brush. Determinism: lerp / clamp / compare only — no transcendentals, no rng.

/// Default prevailing wind, in mesh XY space (west→east). Rain falls preferentially on terrain rising
/// into it; the lee of a ridge sits in its rain shadow. Callers may pass an authored wind instead.
pub const WIND: [f64; 2] = [1.0, 0.0];
/// Scales normalized depth deposited per apply — picked so one apply lays a meaningful but
/// non-flooding layer (cf. the old manual rain brush's ~0.05 single-shot).
const RAIN_SCALE: f64 = 0.03;
/// How strongly terrain rising into the wind lifts rainfall (per unit normalized elevation gain).
const OROGRAPHIC_GAIN: f64 = 3.0;
/// How strongly a leeward descent (sheltered by upwind terrain) suppresses rainfall.
const SHADOW_GAIN: f64 = 6.0;

/// Compute per-cell rainfall (normalized water depth to deposit). One entry per region.
///
/// - `elevation` / `moisture` / `temperature`: the per-cell fields (normalized 0..1 for the traits).
/// - `biome`: per-cell biome id (`0` = unassigned, `1..=N`), indexing `biome_water`.
/// - `biome_water`: `[raininess, rain_shadow, evaporation, flow, ocean_depth]` per biome id (index 0
///   is the unassigned default), as cached on [`World`](crate::world::World).
/// - `positions`: per-region XY centroid, for the upwind look-up.
/// - `neighbors`: per-region adjacency.
/// - `sea_level`: cells at or below it are open water and accumulate no new rain.
/// - `wind`: prevailing wind vector in mesh XY (need not be normalized); rain favours terrain rising
///   into it, the lee sits in shadow. Pass [`WIND`] for the default west→east.
#[allow(clippy::too_many_arguments)]
pub fn compute_rainfall(
    elevation: &[f64],
    moisture: &[f64],
    temperature: &[f64],
    biome: &[u8],
    biome_water: &[[f32; 5]],
    positions: &[[f64; 2]],
    neighbors: &[Vec<u32>],
    sea_level: f64,
    wind: [f64; 2],
) -> Vec<f64> {
    let n = elevation.len();
    let mut out = vec![0.0; n];
    for r in 0..n {
        if elevation[r] <= sea_level {
            continue; // open water — already wet, no orographic rain
        }
        let w = biome_water.get(biome[r] as usize).copied().unwrap_or([0.0; 5]);
        let (raininess, rain_shadow, evaporation) = (w[0] as f64, w[1] as f64, w[2] as f64);

        // Base: the biome's raininess scaled by how wet the cell already is.
        let mut rain = raininess * (0.5 + moisture[r]);
        // Hot biomes evaporate falling rain (temperature 0 cold … 1 hot).
        rain *= (1.0 - evaporation * temperature[r]).clamp(0.2, 1.0);

        // Orographic / rain-shadow: compare this cell to the neighbour most directly upwind.
        let up_elev = upwind_elevation(r, elevation, positions, neighbors, wind);
        let slope = elevation[r] - up_elev;
        if slope >= 0.0 {
            rain *= 1.0 + slope * OROGRAPHIC_GAIN; // rising into the wind → wetter
        } else {
            // Descending behind upwind terrain → drier; the biome's rain_shadow deepens it.
            rain *= (1.0 + slope * rain_shadow * SHADOW_GAIN).clamp(0.1, 1.0);
        }

        out[r] = (rain * RAIN_SCALE).max(0.0);
    }
    out
}

/// Elevation of the neighbour lying most directly upwind of `r` (falls back to `r`'s own elevation
/// when it has no neighbours). Deterministic: picks the largest upwind alignment, tie-broken by the
/// first such neighbour in adjacency order.
fn upwind_elevation(r: usize, elevation: &[f64], positions: &[[f64; 2]], neighbors: &[Vec<u32>], wind: [f64; 2]) -> f64 {
    let mut best_dot = f64::NEG_INFINITY;
    let mut up = elevation[r];
    for &nb in &neighbors[r] {
        let nb = nb as usize;
        let dx = positions[nb][0] - positions[r][0];
        let dy = positions[nb][1] - positions[r][1];
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 0.0 {
            continue;
        }
        // Alignment of the neighbour direction with -wind (1.0 = directly upwind).
        let dot = -(dx * wind[0] + dy * wind[1]) / len;
        if dot > best_dot {
            best_dot = dot;
            up = elevation[nb];
        }
    }
    up
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny 3-cell chain along the wind axis (x): cell 0 upwind, 1 middle, 2 downwind.
    fn chain(elev: [f64; 3]) -> (Vec<f64>, Vec<[f64; 2]>, Vec<Vec<u32>>) {
        let positions = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]];
        let neighbors = vec![vec![1u32], vec![0u32, 2u32], vec![1u32]];
        (elev.to_vec(), positions, neighbors)
    }

    #[test]
    fn deterministic_same_input_same_output() {
        let (elev, pos, nb) = chain([0.2, 0.4, 0.3]);
        let moisture = vec![0.5; 3];
        let temperature = vec![0.5; 3];
        let biome = vec![1u8; 3];
        let water = vec![[0.0; 5], [1.0, 1.2, 0.4, 0.3, 1.4]];
        let a = compute_rainfall(&elev, &moisture, &temperature, &biome, &water, &pos, &nb, -1.0, WIND);
        let b = compute_rainfall(&elev, &moisture, &temperature, &biome, &water, &pos, &nb, -1.0, WIND);
        assert_eq!(a, b);
    }

    #[test]
    fn raininess_is_monotonic() {
        let (elev, pos, nb) = chain([0.3, 0.3, 0.3]); // flat → no orographic term
        let m = vec![0.5; 3];
        let t = vec![0.5; 3];
        let biome = vec![1u8; 3];
        let dry = compute_rainfall(&elev, &m, &t, &biome, &vec![[0.0; 5], [0.5, 1.0, 0.4, 0.3, 1.4]], &pos, &nb, -1.0, WIND);
        let wet = compute_rainfall(&elev, &m, &t, &biome, &vec![[0.0; 5], [1.5, 1.0, 0.4, 0.3, 1.4]], &pos, &nb, -1.0, WIND);
        assert!(wet[1] > dry[1], "higher raininess must yield more rain");
    }

    #[test]
    fn leeward_slope_is_drier_than_windward() {
        // A ridge at the upwind cell: middle is leeward (descends from cell 0), so it sits in shadow.
        let (elev, pos, nb) = chain([0.9, 0.2, 0.2]);
        let m = vec![0.5; 3];
        let t = vec![0.3; 3];
        let biome = vec![1u8; 3];
        let water = vec![[0.0; 5], [1.0, 1.3, 0.4, 0.3, 1.4]];
        let rain = compute_rainfall(&elev, &m, &t, &biome, &water, &pos, &nb, -1.0, WIND);
        // Windward face (cell 0, rising into the wind from nothing upwind) vs leeward (cell 1).
        assert!(rain[1] < rain[0], "leeward cell should be drier than the windward ridge");
    }

    #[test]
    fn open_water_gets_no_rain() {
        let (elev, pos, nb) = chain([-0.5, -0.2, 0.3]);
        let m = vec![0.6; 3];
        let t = vec![0.4; 3];
        let biome = vec![1u8; 3];
        let water = vec![[0.0; 5], [1.0, 1.2, 0.4, 0.3, 1.4]];
        let rain = compute_rainfall(&elev, &m, &t, &biome, &water, &pos, &nb, 0.0, WIND);
        assert_eq!(rain[0], 0.0, "submerged cell accumulates no orographic rain");
        assert!(rain[2] > 0.0, "land cell above sea level does");
    }
}
