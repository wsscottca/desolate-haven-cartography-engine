//! Terrain elevation.
//!
//! Phase 2: a per-region height field from fbm noise with a radial island falloff,
//! so land sits in surrounding water. Per-biome landform shaping (peak/hill/valley)
//! layers on top in a later phase.

use crate::mesh::Mesh;
use crate::noise;
use crate::regions;

/// How many times the noise repeats across the map (lower = larger landmasses).
const DOMAIN: f64 = 4.0;
/// Strength of the radial coastline falloff.
const FALLOFF: f64 = 0.6;

// --- canon-map elevation tuning -----------------------------------------------------------------
/// Deep open-ocean floor the rim + ocean cells pull toward — the **max water depth**: −1 km below the
/// `0` sea level (normalized −1000/(14000/3) ≈ −0.214 at the 14 km terrain scale; ±1.5 ≈ ±7 km). Water
/// spans `0 → −1 km`; land `0 → +7 km`.
const OCEAN_FLOOR: f64 = -0.2143;
/// Normalized radius (the `d = 2·|p−center|/extent` measure) where the ocean rim *starts* biting…
/// Pushed out so the rim only drowns the extreme corners: the canon region grid
/// ([`crate::regionmap::canon_region_at`]) now defines the coastline (ocean = region 0), so the old
/// aggressive radial rim is no longer wanted — it would re-round the traced silhouette into a disc.
const RIM_INNER: f64 = 1.12;
/// …and where it reaches full open ocean. Only the very corner band is pulled now.
const RIM_OUTER: f64 = 1.48;
/// Low-frequency texture repeats across the canon map (macro relief only; fine detail is `shape_terrain`).
const DOMAIN_CANON: f64 = 3.5;
/// Continental west→east tilt: the canon base map's **west** is near sea level and the land rises
/// inland (per the user's steer). The trunk of every cell west of [`TILT_PIVOT`] is lowered
/// proportionally to how far west it sits — the east is left alone, so coastal/western regions sink
/// toward the sea (the far-west Lost Isles break into isles) while tall regions keep their *relief*
/// (Frozen Reaches still spikes into icy peaks above the lowered base).
/// Kept mild: the canon map's west holds tall *ice mountains* (Frozen Reaches), so a strong
/// west-lowering tilt would fight them. A gentle coastal dip only.
const CONTINENTAL_TILT: f64 = 0.07;
/// East of this normalized x the tilt is zero; west of it the drop grows linearly to the coast.
const TILT_PIVOT: f64 = 0.45;
/// Box-blur passes over the coarse base-trunk grid (runs on the tiny coarse grid, not the mesh — cheap).
const BASE_BLUR_PASSES: usize = 3;
/// Region base-elevation below which a cell is a genuine **water basin** (ocean / Great Lake): the
/// smooth grade may *raise* land freely, but a basin is pulled back down so it still floods — its
/// submerged step is hidden by water; only the visible land rise has to be gentle. See `grade_base_trunk`.
const BASIN_CUTOFF: f64 = -0.05;
/// The Great Lake region id — shaped as a **bowl** (open water in the centre, shore rising to its base
/// height + gradient at the rim), not a tilted plane, so it reads as a lake with sloping shores.
const LAKE_REGION: usize = 3;
/// Bowl floor (normalized, below the `0` waterline) — the open-water bed (~−400 m at the 14 km scale);
/// `sea_fill` floods it to the `0` waterline.
const LAKE_FLOOR: f64 = -0.0857;
/// Inner fraction of the lake region's radius held flat at the floor (open water); beyond it the shore
/// rises to the rim (base height + gradient).
const LAKE_FLAT_FRAC: f64 = 0.45;

/// Smooth Hermite step in `[0,1]` (polynomial — determinism-safe).
fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// **Region-guided** elevation — the canon pipeline's terrain pass. Builds a per-cell base-elevation
/// trunk from each cell's region role ([`regions::base_elevation_for`]), **grades** it across territory
/// borders into a smooth macro field whose transitions span ~`transition_m` (see [`grade_base_trunk`] —
/// the cliff fix), rides low-frequency noise on top (amplitude scaled by the region's relief), and
/// pulls the outer band down with a **banded ocean rim** so the edges read as ocean without drowning
/// interior regions. The Great Lake + ocean sit below sea level (and are held there even through the
/// wide grade), so [`crate::fluid::sea_fill`]/`fill_lakes` flood them. High/mid-frequency landform
/// detail is intentionally left to `shape_terrain` (no double-count).
///
/// `transition_m` sets the border-grading width (front-end `BaseBlendM`). `neighbors` is unused now the
/// grade is a coarse-grid blur (kept in the signature for call-site stability). Deterministic: serial
/// bucket-mean + box blur + `par_map` (index-pure) bilinear + `fbm2`.
/// `base_height`/`gradient`/`rotation_deg`/`anchor` are the editable per-Region tables (indexed by region
/// id, normalized except rotation in degrees): the base trunk level, an optional linear tilt (total rise
/// across the region, low→high along `rotation_deg`, pivoting about the region centroid — at the base
/// height, or at `anchor` when finite), the tilt direction (0° = North, clockwise), and the optional
/// pivot override.
#[allow(clippy::too_many_arguments)]
pub fn assign_region_elevation_canon(
    mesh: &Mesh,
    width: f64,
    height: f64,
    seed: u64,
    octaves: u32,
    region_ids: &[u8],
    transition_m: f64,
    base_height: &[f64],
    gradient: &[f64],
    rotation_deg: &[f64],
    anchor: &[f64],
) -> Vec<f64> {
    let nr = mesh.num_regions();
    let nreg = base_height.len();
    // Per-region centroid + per-direction projection span, for the gradient tilt. Computed serially so
    // the result is deterministic regardless of thread count (the cross-target contract). `sin_cos` runs
    // only 14× (once per region) — negligible, and pure (thread-invariant).
    let mut cx = vec![0.0f64; nreg];
    let mut cy = vec![0.0f64; nreg];
    let mut cnt = vec![0.0f64; nreg];
    for r in 0..nr {
        if mesh.is_boundary_r(r) {
            continue;
        }
        let g = region_ids.get(r).copied().unwrap_or(0) as usize;
        if g < nreg {
            let p = mesh.pos_of_r(r);
            cx[g] += p[0];
            cy[g] += p[1];
            cnt[g] += 1.0;
        }
    }
    let mut dirx = vec![0.0f64; nreg];
    let mut diry = vec![0.0f64; nreg];
    for g in 0..nreg {
        if cnt[g] > 0.0 {
            cx[g] /= cnt[g];
            cy[g] /= cnt[g];
        }
        // 0° = North (−y, map-up), clockwise → d = (sin θ, −cos θ).
        let (s, c) = rotation_deg.get(g).copied().unwrap_or(0.0).to_radians().sin_cos();
        dirx[g] = s;
        diry[g] = -c;
    }
    let mut pmin = vec![f64::INFINITY; nreg];
    let mut pmax = vec![f64::NEG_INFINITY; nreg];
    let mut rmax = vec![0.0f64; nreg]; // max radial distance from centroid (for the lake bowl)
    for r in 0..nr {
        if mesh.is_boundary_r(r) {
            continue;
        }
        let g = region_ids.get(r).copied().unwrap_or(0) as usize;
        if g < nreg {
            let p = mesh.pos_of_r(r);
            let proj = (p[0] - cx[g]) * dirx[g] + (p[1] - cy[g]) * diry[g];
            if proj < pmin[g] {
                pmin[g] = proj;
            }
            if proj > pmax[g] {
                pmax[g] = proj;
            }
            let rd = ((p[0] - cx[g]).powi(2) + (p[1] - cy[g]).powi(2)).sqrt();
            if rd > rmax[g] {
                rmax[g] = rd;
            }
        }
    }
    // Per-cell base target: the region's base height (or the anchor when set), plus the gradient tilt —
    // a linear ramp pivoting about the region centroid so the total rise low→high edge equals `gradient`.
    // The Great Lake is the exception: a bowl (flat open water in the centre, shore rising to the rim).
    let base_target: Vec<f64> = crate::util::par_map(nr, |r| {
        if mesh.is_boundary_r(r) {
            return OCEAN_FLOOR;
        }
        let g = region_ids.get(r).copied().unwrap_or(0) as usize;
        if g >= nreg {
            return OCEAN_FLOOR;
        }
        let p = mesh.pos_of_r(r);
        let grad = gradient.get(g).copied().unwrap_or(0.0);
        let span = pmax[g] - pmin[g];
        let offset = if grad != 0.0 && span > 1e-6 {
            grad * ((p[0] - cx[g]) * dirx[g] + (p[1] - cy[g]) * diry[g]) / span
        } else {
            0.0
        };
        // The Great Lake's surface target is a normal region (base + tilt) so it grades smoothly into
        // its neighbours; the bowl depression is carved below the graded field in the final pass.
        let a = anchor.get(g).copied().unwrap_or(f64::NAN);
        let pivot = if a.is_finite() { a } else { base_height[g] };
        (pivot + offset).clamp(-1.5, 1.5)
    });
    // Grade the trunk into a smooth macro field whose borders span ~transition_m regardless of mesh
    // density — the cliff fix (a Jacobi diffuser's band only grows as √iters, far too narrow in practice).
    let graded = grade_base_trunk(mesh, &base_target, region_ids, width, height, transition_m);

    // Add low-freq texture (relief-scaled) and the banded ocean rim.
    crate::util::par_map(nr, |r| {
        if mesh.is_boundary_r(r) {
            return OCEAN_FLOOR;
        }
        let p = mesh.pos_of_r(r);
        let u = p[0] / width;
        let v = p[1] / height;
        let relief = regions::default_traits_for(region_ids.get(r).copied().unwrap_or(0)).relief as f64;
        let amp = 0.05 + 0.14 * relief;
        // Macro relief only — cap the octaves so the base trunk stays smooth; fine/steep detail is the
        // job of `shape_terrain` (folding high-octave noise in here re-introduced scattered steep spots).
        let tex = noise::fbm2(u * DOMAIN_CANON, v * DOMAIN_CANON, seed, octaves.min(3)) * amp;
        // Water basins (ocean, Great Lake) keep their depth so they still flood — only the smooth grade
        // could have raised them; land takes the grade so the visible rise has no cliffs.
        let g = region_ids.get(r).copied().unwrap_or(0) as usize;
        let target = base_target[r];
        // The Great Lake is a bowl carved BELOW the smooth graded land surface: the rim equals `graded`
        // (so the shore meets its neighbours with no cliff), the centre is LAKE_FLOOR (open water at the
        // 0 waterline). cx/cy/rmax are the region centroid + radius from the aggregation above (cx/cy are
        // shadowed by the rim's locals further down, so they still index the centroid vecs here). Other
        // basins are pulled to min(target, graded) so they still flood; land takes the grade.
        let trunk = if g == LAKE_REGION {
            let dist = ((p[0] - cx[g]).powi(2) + (p[1] - cy[g]).powi(2)).sqrt();
            let t = (dist / rmax[g].max(1e-6)).clamp(0.0, 1.0);
            let s = if t <= LAKE_FLAT_FRAC { 0.0 } else { (t - LAKE_FLAT_FRAC) / (1.0 - LAKE_FLAT_FRAC) };
            LAKE_FLOOR + (graded[r] - LAKE_FLOOR) * s
        } else if target < BASIN_CUTOFF {
            target.min(graded[r])
        } else {
            graded[r]
        };
        let mut h = trunk + tex;
        // Continental tilt: lower the western half toward sea level (east unchanged).
        h -= CONTINENTAL_TILT * (TILT_PIVOT - u).max(0.0);
        // Banded radial ocean rim: pull only the outer band toward the ocean floor (never raises).
        let cx = u - 0.5;
        let cy = v - 0.5;
        let d = (cx * cx + cy * cy).sqrt() * 2.0;
        let rim = smoothstep(RIM_INNER, RIM_OUTER, d);
        let rimmed = h * (1.0 - rim) + OCEAN_FLOOR * rim;
        h = h.min(rimmed);
        h.clamp(-1.5, 1.5)
    })
}

/// Grade the per-region base-elevation trunk into a smooth macro field whose transitions span roughly
/// `blend_m` **independently of the mesh density** — the recurring "borders are cliffs" fix. A Jacobi
/// diffusion's transition band only grows as √(iters), so reaching a kilometre-wide grade at a ~10 m
/// cell pitch would need tens of thousands of O(iters·cells) passes; instead we average the *interior*
/// region targets onto a coarse regular grid (cell ≈ `blend_m`/2), box-blur that tiny grid, and
/// bilinearly sample back — cost independent of the blend width. The ocean frame is excluded (left to
/// the banded rim; folding deep ocean into a wide blur would pull sea far inland).
///
/// Deterministic across targets/threads: serial bucket-mean + serial separable box blur + index-pure
/// `par_map` bilinear (no transcendentals) — the cross-target contract.
fn grade_base_trunk(mesh: &Mesh, base_target: &[f64], _region_ids: &[u8], width: f64, height: f64, blend_m: f64) -> Vec<f64> {
    let nr = mesh.num_regions();
    // Coarse cell ≈ half the blend width (so a few blur taps cover the full transition), bounded so a
    // tiny or huge blend can't make a degenerate grid.
    let cell = (blend_m * 0.5).clamp(width.max(height) / 256.0, width.max(height)).max(1.0);
    let cols = (width / cell).ceil() as usize + 1;
    let rows = (height / cell).ceil() as usize + 1;
    let mut sum = vec![0.0f64; cols * rows];
    let mut cnt = vec![0.0f64; cols * rows];
    for r in 0..nr {
        // Exclude every water basin (the boundary frame, interior sea/bays, AND the Great Lake) from the
        // land blur — anything below `BASIN_CUTOFF`. Two reasons: (1) folding the −1.0 sea into the land
        // blur would wash small coastal regions (e.g. the Volcanic Scape) below sea level; (2) including
        // the Great Lake (−0.15) would average its floor *up* toward the surrounding land AND drag the
        // neighbouring mountains *down* toward the lake, the exact "lake dry / peaks sunken" bug. Land
        // grades over land only; the coast→water drop is left to `grade_shorelines` + the basin handling.
        if mesh.is_boundary_r(r) || base_target[r] < BASIN_CUTOFF {
            continue;
        }
        let p = mesh.pos_of_r(r);
        let gx = ((p[0] / cell) as usize).min(cols - 1);
        let gy = ((p[1] / cell) as usize).min(rows - 1);
        sum[gy * cols + gx] += base_target[r];
        cnt[gy * cols + gx] += 1.0;
    }
    // Coarse-cell mean; empty cells fall back to the mean of the filled ones (so the blur has no holes).
    let (mut fsum, mut fcnt) = (0.0f64, 0.0f64);
    for i in 0..cols * rows {
        if cnt[i] > 0.0 {
            fsum += sum[i] / cnt[i];
            fcnt += 1.0;
        }
    }
    let global = if fcnt > 0.0 { fsum / fcnt } else { 0.0 };
    let mut grid: Vec<f64> = (0..cols * rows)
        .map(|i| if cnt[i] > 0.0 { sum[i] / cnt[i] } else { global })
        .collect();
    // Separable 3-tap box blur on the coarse grid (edges clamp), a few passes → smooth macro trunk.
    let mut tmp = grid.clone();
    for _ in 0..BASE_BLUR_PASSES {
        for gy in 0..rows {
            for gx in 0..cols {
                let l = grid[gy * cols + gx.saturating_sub(1)];
                let c = grid[gy * cols + gx];
                let rr = grid[gy * cols + (gx + 1).min(cols - 1)];
                tmp[gy * cols + gx] = (l + c + rr) / 3.0;
            }
        }
        for gy in 0..rows {
            for gx in 0..cols {
                let up = tmp[gy.saturating_sub(1) * cols + gx];
                let c = tmp[gy * cols + gx];
                let dn = tmp[(gy + 1).min(rows - 1) * cols + gx];
                grid[gy * cols + gx] = (up + c + dn) / 3.0;
            }
        }
    }
    // Bilinear-sample the coarse grid back to every cell (smooth per-cell base, no grid stair-steps).
    crate::util::par_map(nr, |r| {
        let p = mesh.pos_of_r(r);
        let fx = (p[0] / cell).clamp(0.0, (cols - 1) as f64);
        let fy = (p[1] / cell).clamp(0.0, (rows - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(cols - 1), (y0 + 1).min(rows - 1));
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let top = grid[y0 * cols + x0] + (grid[y0 * cols + x1] - grid[y0 * cols + x0]) * tx;
        let bot = grid[y1 * cols + x0] + (grid[y1 * cols + x1] - grid[y1 * cols + x0]) * tx;
        top + (bot - top) * ty
    })
}

/// Normalized per-region elevation in `[-1, 1]`, island-shaped. `width`/`height` are
/// the world extent the mesh was built over; `seed`/`octaves` drive the noise.
pub fn assign_region_elevation(
    mesh: &Mesh,
    width: f64,
    height: f64,
    seed: u64,
    octaves: u32,
) -> Vec<f64> {
    // Pure per-region map (each `e[r]` depends only on `r`'s position + the seed) → parallel-safe:
    // `par_map` preserves index order, so the result is bit-identical to the serial loop.
    crate::util::par_map(mesh.num_regions(), |r| {
        let p = mesh.pos_of_r(r);
        let u = p[0] / width;
        let v = p[1] / height;
        let mut h = noise::fbm2(u * DOMAIN, v * DOMAIN, seed, octaves);
        // Radial island falloff toward the edges.
        let cx = u - 0.5;
        let cy = v - 0.5;
        let d = (cx * cx + cy * cy).sqrt() * 2.0;
        h -= d * d * FALLOFF;
        h.clamp(-1.0, 1.0)
    })
}
