//! Hydraulic liquid simulation.
//!
//! A height-field model over the dual mesh: each region carries a liquid column
//! (`depth`) of some [`LiquidType`]. Liquid flows toward lower-surface neighbors
//! (surface = terrain + depth) so it runs downhill and pools in basins with a level
//! surface. All units are normalized elevation (the same space as the terrain), and
//! the solver is a simple, stable relaxation that converges to equilibrium.
//!
//! Phase 3 seeds liquid two ways: a `sea_fill` to a global level (instant oceans +
//! lakes) and `add_rain` over land for the watchable settle. The Course/Flood tools
//! (Phase 4) add interactive per-type sources on top.

use crate::liquids::LiquidType;
use crate::mesh::Mesh;

const WET: f64 = 1.0e-4; // a region with at least this much liquid renders as wet

/// Per-region liquid state.
pub struct LiquidField {
    /// Liquid column height per region (normalized elevation units).
    pub depth: Vec<f64>,
    /// Liquid type id per region (see [`LiquidType`]).
    pub kind: Vec<u8>,
}

impl LiquidField {
    pub fn new(n: usize) -> Self {
        LiquidField {
            depth: vec![0.0; n],
            kind: vec![0; n],
        }
    }

    pub fn clear(&mut self) {
        for d in &mut self.depth {
            *d = 0.0;
        }
        for k in &mut self.kind {
            *k = 0;
        }
    }

    /// Total liquid volume (sum of depths) — used by conservation checks.
    pub fn volume(&self) -> f64 {
        self.depth.iter().sum()
    }
}

/// Fill every region whose terrain is below `level` with water up to `level`
/// (instant sea + lakes, with shorelines following the terrain).
pub fn sea_fill(field: &mut LiquidField, terrain: &[f64], level: f64) {
    for r in 0..terrain.len() {
        let d = level - terrain[r];
        if d > 0.0 {
            field.depth[r] = d;
            field.kind[r] = LiquidType::Water as u8;
        } else {
            field.depth[r] = 0.0;
            field.kind[r] = 0;
        }
    }
}

/// Add a uniform layer of `amount` water to land at or above `level` (rainfall).
pub fn add_rain(field: &mut LiquidField, terrain: &[f64], level: f64, amount: f64) {
    for r in 0..terrain.len() {
        if terrain[r] >= level {
            field.depth[r] += amount;
            if field.kind[r] == 0 {
                field.kind[r] = LiquidType::Water as u8;
            }
        }
    }
}

/// One relaxation step: each wet region sends a fraction of its column toward
/// lower-surface neighbors, capped so it never overshoots a level surface (which
/// keeps the solver stable). `flow_rate` ∈ (0, 0.5]; `evaporation` ∈ [0, 1).
pub fn relax_step(
    field: &mut LiquidField,
    terrain: &[f64],
    neighbors: &[Vec<u32>],
    flow_rate: f64,
    evaporation: f64,
) {
    let n = terrain.len();
    let mut delta = vec![0.0f64; n];

    for r in 0..n {
        let w = field.depth[r];
        if w <= WET {
            continue;
        }
        let surf_r = terrain[r] + w;

        // Total downhill surface drop to neighbors (for proportional sharing).
        let mut total_drop = 0.0;
        for &nb in &neighbors[r] {
            let surf_n = terrain[nb as usize] + field.depth[nb as usize];
            if surf_n < surf_r {
                total_drop += surf_r - surf_n;
            }
        }
        if total_drop <= 0.0 {
            continue; // local minimum — liquid stays (a lake)
        }

        let movable = w * flow_rate;
        let src_kind = field.kind[r];
        for &nb in &neighbors[r] {
            let nb = nb as usize;
            let surf_n = terrain[nb] + field.depth[nb];
            if surf_n < surf_r {
                let drop = surf_r - surf_n;
                // Share by drop, but never move more than half the gap (anti-overshoot).
                let amt = (movable * (drop / total_drop)).min(drop * 0.5);
                delta[r] -= amt;
                delta[nb] += amt;
                // Liquid carries its type into dry cells it flows into (lava stays lava).
                if field.depth[nb] <= WET && field.kind[nb] == 0 {
                    field.kind[nb] = src_kind;
                }
            }
        }
    }

    for r in 0..n {
        let d = (field.depth[r] + delta[r]) * (1.0 - evaporation);
        field.depth[r] = if d > 0.0 { d } else { 0.0 };
        if field.depth[r] <= WET {
            field.kind[r] = 0;
        } else if field.kind[r] == 0 {
            field.kind[r] = LiquidType::Water as u8;
        }
    }
}

/// Render-ready liquid surface: one vertex per region at its liquid-surface height,
/// indexed by the triangles whose three corners are all wet.
pub struct LiquidSurface {
    pub positions: Vec<f32>, // 3 per region: x, y, (terrain+depth) × exaggeration
    pub normals: Vec<f32>,   // 3 per region
    pub types: Vec<f32>,     // 1 per region: liquid type id
    pub indices: Vec<u32>,   // 3 per wet triangle
}

/// Build the liquid surface mesh at a vertical `exaggeration`.
pub fn liquid_surface(
    mesh: &Mesh,
    terrain: &[f64],
    field: &LiquidField,
    exaggeration: f64,
) -> LiquidSurface {
    let nr = mesh.num_regions();
    let mut positions = vec![0.0f32; nr * 3];
    let mut types = vec![0.0f32; nr];
    for r in 0..nr {
        let p = mesh.pos_of_r(r);
        let surf = terrain[r] + field.depth[r];
        positions[3 * r] = p[0] as f32;
        positions[3 * r + 1] = p[1] as f32;
        positions[3 * r + 2] = (surf * exaggeration) as f32;
        types[r] = field.kind[r] as f32;
    }

    let nt = mesh.num_triangles();
    let mut indices = Vec::new();
    let mut accum = vec![0.0f64; nr * 3];

    for t in 0..nt {
        let a = mesh.r_begin_s(3 * t);
        let b = mesh.r_begin_s(3 * t + 1);
        let c = mesh.r_begin_s(3 * t + 2);
        if field.depth[a] <= WET || field.depth[b] <= WET || field.depth[c] <= WET {
            continue;
        }

        let (pa, pb, pc) = (mesh.pos_of_r(a), mesh.pos_of_r(b), mesh.pos_of_r(c));
        let za = (terrain[a] + field.depth[a]) * exaggeration;
        let zb = (terrain[b] + field.depth[b]) * exaggeration;
        let zc = (terrain[c] + field.depth[c]) * exaggeration;
        let (ux, uy, uz) = (pb[0] - pa[0], pb[1] - pa[1], zb - za);
        let (vx, vy, vz) = (pc[0] - pa[0], pc[1] - pa[1], zc - za);
        let mut nx = uy * vz - uz * vy;
        let mut ny = uz * vx - ux * vz;
        let mut nz = ux * vy - uy * vx;
        if nz < 0.0 {
            nx = -nx;
            ny = -ny;
            nz = -nz;
        }
        for &r in &[a, b, c] {
            accum[3 * r] += nx;
            accum[3 * r + 1] += ny;
            accum[3 * r + 2] += nz;
        }
        indices.push(a as u32);
        indices.push(b as u32);
        indices.push(c as u32);
    }

    let mut normals = vec![0.0f32; nr * 3];
    for r in 0..nr {
        let (nx, ny, nz) = (accum[3 * r], accum[3 * r + 1], accum[3 * r + 2]);
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        if len > 1e-12 {
            normals[3 * r] = (nx / len) as f32;
            normals[3 * r + 1] = (ny / len) as f32;
            normals[3 * r + 2] = (nz / len) as f32;
        } else {
            normals[3 * r + 2] = 1.0;
        }
    }

    LiquidSurface {
        positions,
        normals,
        types,
        indices,
    }
}
