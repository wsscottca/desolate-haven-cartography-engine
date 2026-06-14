//! GPU geometry packing.
//!
//! Phase 2: a triangulated surface (TIN) over the dual mesh — one vertex per region
//! (positioned at its elevation), indexed by the Delaunay triangles. Smooth normals
//! come from an area-weighted average of incident face normals. Per-liquid surfaces
//! and instance buffers join later phases.

use crate::mesh::Mesh;

/// Render-ready surface: flat arrays the front-end uploads directly.
pub struct Surface {
    /// 3 floats per vertex: world x, world y, z = elevation × exaggeration.
    pub positions: Vec<f32>,
    /// 3 floats per vertex: smooth surface normal.
    pub normals: Vec<f32>,
    /// 1 float per vertex: normalized elevation `[-1, 1]`.
    pub heights: Vec<f32>,
    /// 3 floats per vertex: biome base color (RGB).
    pub colors: Vec<f32>,
    /// 3 indices per triangle (region indices).
    pub indices: Vec<u32>,
}

/// Build the surface from the dual mesh + per-region elevation at a vertical
/// `exaggeration`. Cheap relative to the mesh build — recompute on exaggeration
/// change without rebuilding the triangulation.
pub fn build_surface(
    mesh: &Mesh,
    elevation_r: &[f64],
    exaggeration: f64,
    region_color: &[f32],
) -> Surface {
    let nr = mesh.num_regions();
    let mut positions = vec![0.0f32; nr * 3];
    let mut heights = vec![0.0f32; nr];
    let mut colors = vec![0.0f32; nr * 3];
    for r in 0..nr {
        let p = mesh.pos_of_r(r);
        positions[3 * r] = p[0] as f32;
        positions[3 * r + 1] = p[1] as f32;
        positions[3 * r + 2] = (elevation_r[r] * exaggeration) as f32;
        heights[r] = elevation_r[r] as f32;
        colors[3 * r] = region_color[3 * r];
        colors[3 * r + 1] = region_color[3 * r + 1];
        colors[3 * r + 2] = region_color[3 * r + 2];
    }

    let nt = mesh.num_triangles();
    let mut indices = Vec::with_capacity(nt * 3);
    let mut accum = vec![0.0f64; nr * 3]; // accumulated (area-weighted) vertex normals

    for t in 0..nt {
        let a = mesh.r_begin_s(3 * t);
        let b = mesh.r_begin_s(3 * t + 1);
        let c = mesh.r_begin_s(3 * t + 2);

        let (pa, pb, pc) = (mesh.pos_of_r(a), mesh.pos_of_r(b), mesh.pos_of_r(c));
        let za = elevation_r[a] * exaggeration;
        let zb = elevation_r[b] * exaggeration;
        let zc = elevation_r[c] * exaggeration;

        // Face normal = (pb-pa) × (pc-pa); magnitude ∝ 2·area (so it's area-weighted).
        let (ux, uy, uz) = (pb[0] - pa[0], pb[1] - pa[1], zb - za);
        let (vx, vy, vz) = (pc[0] - pa[0], pc[1] - pa[1], zc - za);
        let mut nx = uy * vz - uz * vy;
        let mut ny = uz * vx - ux * vz;
        let mut nz = ux * vy - uy * vx;
        if nz < 0.0 {
            // Orient upward regardless of triangle winding.
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
            normals[3 * r + 2] = 1.0; // flat fallback for isolated/degenerate vertices
        }
    }

    Surface {
        positions,
        normals,
        heights,
        colors,
        indices,
    }
}
