//! Volumetric isosurface extraction (N5 — authored caves / overhangs / tunnels).
//!
//! **Naive Surface Nets** over a sampled density grid: `density > 0` = solid (rock), `< 0` = air; the
//! `density == 0` isosurface is the carved wall. Chosen over marching cubes because it's **table-free**
//! (no 256×16 triangle table to mis-transcribe), gives smooth cave walls, and stays within the
//! determinism contract (lerp / average / compare only — no transcendentals; `sqrt` is permitted and
//! used by the caller's sphere SDF). One vertex per straddling cell (averaged edge crossings); quads
//! connect the four cells around each sign-changing grid edge.

/// A compact triangle mesh in **Godot space** (positions `x, y(up), z`). Self-contained so the caller
/// can pack it straight into a level scene.
pub struct VolumeMesh {
    pub positions: Vec<f32>, // 3 per vertex (Godot x, y, z)
    pub normals: Vec<f32>,   // 3 per vertex
    pub indices: Vec<u32>,   // 3 per triangle
}

/// Extract the `density == 0` surface over a grid of `dims` sample points spaced `cell` apart from
/// `min` (world/Godot space). `density(x, y, z)` returns signed density (> 0 solid). Empty if the
/// grid doesn't straddle the surface. Total samples are the caller's responsibility to bound.
pub fn surface_nets<F: Fn(f64, f64, f64) -> f64>(
    min: [f64; 3],
    dims: [usize; 3],
    cell: f64,
    density: &F,
) -> VolumeMesh {
    let (nx, ny, nz) = (dims[0], dims[1], dims[2]);
    let empty = VolumeMesh { positions: Vec::new(), normals: Vec::new(), indices: Vec::new() };
    if nx < 2 || ny < 2 || nz < 2 {
        return empty;
    }

    // Sample the density grid.
    let sidx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;
    let mut d = vec![0.0f64; nx * ny * nz];
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                d[sidx(i, j, k)] = density(
                    min[0] + i as f64 * cell,
                    min[1] + j as f64 * cell,
                    min[2] + k as f64 * cell,
                );
            }
        }
    }

    // One vertex per straddling cell (cells span sample [i..i+1] × [j..j+1] × [k..k+1]).
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let cidx = |i: usize, j: usize, k: usize| (k * cy + j) * cx + i;
    let mut cell_vert = vec![u32::MAX; cx * cy * cz];
    let mut positions: Vec<f32> = Vec::new();
    let mut normals: Vec<f32> = Vec::new();

    // Cube corner offsets + the 12 edges (corner-index pairs).
    const CORNERS: [[usize; 3]; 8] =
        [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]];
    const EDGES: [(usize, usize); 12] =
        [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)];

    for k in 0..cz {
        for j in 0..cy {
            for i in 0..cx {
                let mut cv = [0.0f64; 8];
                for (n, off) in CORNERS.iter().enumerate() {
                    cv[n] = d[sidx(i + off[0], j + off[1], k + off[2])];
                }
                let mut has_pos = false;
                let mut has_neg = false;
                for &v in &cv {
                    if v > 0.0 {
                        has_pos = true;
                    } else {
                        has_neg = true;
                    }
                }
                if !(has_pos && has_neg) {
                    continue; // wholly inside or outside — no surface here
                }
                // Vertex = average of the zero-crossings on the cell's 12 edges (local 0..1 coords).
                let (mut sx, mut sy, mut sz, mut cnt) = (0.0f64, 0.0f64, 0.0f64, 0u32);
                for &(a, b) in EDGES.iter() {
                    let (va, vb) = (cv[a], cv[b]);
                    if (va > 0.0) != (vb > 0.0) {
                        let t = va / (va - vb); // crossing parameter along a→b
                        let (pa, pb) = (CORNERS[a], CORNERS[b]);
                        sx += pa[0] as f64 + (pb[0] as f64 - pa[0] as f64) * t;
                        sy += pa[1] as f64 + (pb[1] as f64 - pa[1] as f64) * t;
                        sz += pa[2] as f64 + (pb[2] as f64 - pa[2] as f64) * t;
                        cnt += 1;
                    }
                }
                if cnt == 0 {
                    continue;
                }
                let inv = 1.0 / cnt as f64;
                cell_vert[cidx(i, j, k)] = (positions.len() / 3) as u32;
                positions.push((min[0] + (i as f64 + sx * inv) * cell) as f32);
                positions.push((min[1] + (j as f64 + sy * inv) * cell) as f32);
                positions.push((min[2] + (k as f64 + sz * inv) * cell) as f32);
                normals.extend_from_slice(&[0.0, 0.0, 0.0]); // accumulated below
            }
        }
    }

    // Quad per sign-changing grid edge, connecting the 4 cells around it. `flip` orients winding by
    // the sign direction so all faces wind consistently.
    let cellv = |i: usize, j: usize, k: usize| cell_vert[cidx(i, j, k)];
    let mut indices: Vec<u32> = Vec::new();
    let quad = |indices: &mut Vec<u32>, a: u32, b: u32, c: u32, e: u32, flip: bool| {
        if a == u32::MAX || b == u32::MAX || c == u32::MAX || e == u32::MAX {
            return;
        }
        if flip {
            indices.extend_from_slice(&[a, c, b, a, e, c]);
        } else {
            indices.extend_from_slice(&[a, b, c, a, c, e]);
        }
    };

    // x-edges: sample (i,j,k)→(i+1,j,k); 4 cells vary in (j,k).
    for k in 1..cz {
        for j in 1..cy {
            for i in 0..cx {
                let (v0, v1) = (d[sidx(i, j, k)], d[sidx(i + 1, j, k)]);
                if (v0 > 0.0) == (v1 > 0.0) {
                    continue;
                }
                quad(&mut indices, cellv(i, j - 1, k - 1), cellv(i, j, k - 1), cellv(i, j, k), cellv(i, j - 1, k), v0 > 0.0);
            }
        }
    }
    // y-edges: (i,j,k)→(i,j+1,k); 4 cells vary in (i,k).
    for k in 1..cz {
        for j in 0..cy {
            for i in 1..cx {
                let (v0, v1) = (d[sidx(i, j, k)], d[sidx(i, j + 1, k)]);
                if (v0 > 0.0) == (v1 > 0.0) {
                    continue;
                }
                quad(&mut indices, cellv(i - 1, j, k - 1), cellv(i, j, k - 1), cellv(i, j, k), cellv(i - 1, j, k), v0 <= 0.0);
            }
        }
    }
    // z-edges: (i,j,k)→(i,j,k+1); 4 cells vary in (i,j).
    for k in 0..cz {
        for j in 1..cy {
            for i in 1..cx {
                let (v0, v1) = (d[sidx(i, j, k)], d[sidx(i, j, k + 1)]);
                if (v0 > 0.0) == (v1 > 0.0) {
                    continue;
                }
                quad(&mut indices, cellv(i - 1, j - 1, k), cellv(i, j - 1, k), cellv(i, j, k), cellv(i - 1, j, k), v0 > 0.0);
            }
        }
    }

    // Smooth normals from accumulated face normals.
    let mut accum = vec![0.0f64; positions.len()];
    for tri in indices.chunks_exact(3) {
        let (ia, ib, ic) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let p = |idx: usize| [positions[3 * idx] as f64, positions[3 * idx + 1] as f64, positions[3 * idx + 2] as f64];
        let (a, b, c) = (p(ia), p(ib), p(ic));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for &idx in &[ia, ib, ic] {
            accum[3 * idx] += n[0];
            accum[3 * idx + 1] += n[1];
            accum[3 * idx + 2] += n[2];
        }
    }
    for i in 0..positions.len() / 3 {
        let (nx_, ny_, nz_) = (accum[3 * i], accum[3 * i + 1], accum[3 * i + 2]);
        let len = (nx_ * nx_ + ny_ * ny_ + nz_ * nz_).sqrt();
        if len > 1e-12 {
            normals[3 * i] = (nx_ / len) as f32;
            normals[3 * i + 1] = (ny_ / len) as f32;
            normals[3 * i + 2] = (nz_ / len) as f32;
        } else {
            normals[3 * i + 1] = 1.0;
        }
    }

    VolumeMesh { positions, normals, indices }
}
