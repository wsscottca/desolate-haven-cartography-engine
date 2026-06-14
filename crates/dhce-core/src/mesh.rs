//! Dual half-edge mesh over a Delaunay triangulation.
//!
//! Clean-room replacement for the Apache-2.0 `dual-mesh`. We use the permissive
//! `delaunator` crate for the triangulation only, and own the dual wrapper:
//!
//! - **regions** are the Delaunay vertices (Voronoi cell centers),
//! - **triangles** are the Delaunay triangles, whose circumcenters are the Voronoi
//!   vertices,
//! - **sides** are half-edges; navigation is pure index arithmetic over the
//!   `triangles`/`halfedges` arrays.
//!
//! Deterministic in `seed` (the Poisson interior + `delaunator` are both deterministic).

use crate::points;
use delaunator::{triangulate, Point, EMPTY};

/// A Delaunay/Voronoi dual mesh. See the module docs for the region/triangle/side model.
pub struct Mesh {
    region_xy: Vec<[f64; 2]>,
    triangles: Vec<usize>,
    halfedges: Vec<usize>,
    triangle_xy: Vec<[f64; 2]>,
    num_boundary_regions: usize,
}

impl Mesh {
    /// Build a mesh over `[0, width] × [0, height]`: a boundary frame (so the hull
    /// covers the rectangle) plus a Poisson-disk interior at `spacing`, triangulated.
    pub fn new(width: f64, height: f64, spacing: f64, seed: u64) -> Mesh {
        let mut region_xy: Vec<[f64; 2]> = Vec::new();

        // Boundary frame first → indices `0..num_boundary_regions`.
        let n = ((width.max(height) / spacing).ceil() as usize).max(1);
        for k in 0..n {
            let t = k as f64 / n as f64;
            region_xy.push([t * width, 0.0]); // bottom
            region_xy.push([width, t * height]); // right
            region_xy.push([(1.0 - t) * width, height]); // top
            region_xy.push([0.0, (1.0 - t) * height]); // left
        }
        let num_boundary_regions = region_xy.len();

        // Interior blue-noise, inset so it never collides with the frame.
        let inset = spacing;
        for p in points::poisson_disk(width - 2.0 * inset, height - 2.0 * inset, spacing, seed) {
            region_xy.push([p[0] + inset, p[1] + inset]);
        }

        let dpts: Vec<Point> = region_xy.iter().map(|p| Point { x: p[0], y: p[1] }).collect();
        let tri = triangulate(&dpts);

        let triangle_xy = (0..tri.triangles.len() / 3)
            .map(|t| {
                circumcenter(
                    region_xy[tri.triangles[3 * t]],
                    region_xy[tri.triangles[3 * t + 1]],
                    region_xy[tri.triangles[3 * t + 2]],
                )
            })
            .collect();

        Mesh {
            region_xy,
            triangles: tri.triangles,
            halfedges: tri.halfedges,
            triangle_xy,
            num_boundary_regions,
        }
    }

    // --- counts ---
    pub fn num_regions(&self) -> usize {
        self.region_xy.len()
    }
    pub fn num_triangles(&self) -> usize {
        self.triangles.len() / 3
    }
    pub fn num_sides(&self) -> usize {
        self.triangles.len()
    }
    pub fn num_boundary_regions(&self) -> usize {
        self.num_boundary_regions
    }

    // --- coordinates ---
    pub fn x_of_r(&self, r: usize) -> f64 {
        self.region_xy[r][0]
    }
    pub fn y_of_r(&self, r: usize) -> f64 {
        self.region_xy[r][1]
    }
    pub fn pos_of_r(&self, r: usize) -> [f64; 2] {
        self.region_xy[r]
    }
    pub fn x_of_t(&self, t: usize) -> f64 {
        self.triangle_xy[t][0]
    }
    pub fn y_of_t(&self, t: usize) -> f64 {
        self.triangle_xy[t][1]
    }
    pub fn pos_of_t(&self, t: usize) -> [f64; 2] {
        self.triangle_xy[t]
    }

    // --- half-edge navigation (pure index arithmetic) ---
    /// Triangle that side `s` belongs to.
    #[inline]
    pub fn t_of_s(&self, s: usize) -> usize {
        s / 3
    }
    /// Next side around the same triangle.
    #[inline]
    pub fn s_next_s(&self, s: usize) -> usize {
        if s % 3 == 2 {
            s - 2
        } else {
            s + 1
        }
    }
    /// Previous side around the same triangle.
    #[inline]
    pub fn s_prev_s(&self, s: usize) -> usize {
        if s % 3 == 0 {
            s + 2
        } else {
            s - 1
        }
    }
    /// Opposite (twin) side, or `None` on the hull boundary.
    #[inline]
    pub fn s_opposite_s(&self, s: usize) -> Option<usize> {
        let h = self.halfedges[s];
        if h == EMPTY {
            None
        } else {
            Some(h)
        }
    }
    /// Region at the start of side `s`.
    #[inline]
    pub fn r_begin_s(&self, s: usize) -> usize {
        self.triangles[s]
    }
    /// Region at the end of side `s`.
    #[inline]
    pub fn r_end_s(&self, s: usize) -> usize {
        self.triangles[self.s_next_s(s)]
    }
    /// Is region `r` part of the boundary frame?
    #[inline]
    pub fn is_boundary_r(&self, r: usize) -> bool {
        r < self.num_boundary_regions
    }
}

/// Circumcenter of a triangle (a Voronoi vertex). Falls back to the centroid if the
/// three points are degenerate/collinear.
fn circumcenter(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> [f64; 2] {
    let ad = a[0] * a[0] + a[1] * a[1];
    let bd = b[0] * b[0] + b[1] * b[1];
    let cd = c[0] * c[0] + c[1] * c[1];
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
    }
    let ux = (ad * (b[1] - c[1]) + bd * (c[1] - a[1]) + cd * (a[1] - b[1])) / d;
    let uy = (ad * (c[0] - b[0]) + bd * (a[0] - c[0]) + cd * (b[0] - a[0])) / d;
    [ux, uy]
}
