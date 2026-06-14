//! Mesh invariants + determinism. Uses only the public `Mesh` API.

use dhce_core::mesh::Mesh;

#[test]
fn mesh_is_deterministic() {
    let a = Mesh::new(1000.0, 1000.0, 30.0, 7);
    let b = Mesh::new(1000.0, 1000.0, 30.0, 7);
    assert_eq!(a.num_regions(), b.num_regions());
    assert_eq!(a.num_triangles(), b.num_triangles());
    for r in 0..a.num_regions() {
        assert_eq!(a.x_of_r(r).to_bits(), b.x_of_r(r).to_bits(), "region {r} x differs");
        assert_eq!(a.y_of_r(r).to_bits(), b.y_of_r(r).to_bits(), "region {r} y differs");
    }
}

#[test]
fn halfedge_twins_are_consistent() {
    let m = Mesh::new(1000.0, 1000.0, 40.0, 1);
    for s in 0..m.num_sides() {
        if let Some(o) = m.s_opposite_s(s) {
            // twin-of-twin is self
            assert_eq!(m.s_opposite_s(o), Some(s), "side {s}: twin not symmetric");
            // a side and its twin connect the same two regions, opposite direction
            assert_eq!(m.r_begin_s(s), m.r_end_s(o), "side {s}: begin/end mismatch with twin");
            assert_eq!(m.r_end_s(s), m.r_begin_s(o), "side {s}: end/begin mismatch with twin");
        }
    }
}

#[test]
fn produces_a_nontrivial_bounded_mesh() {
    let m = Mesh::new(1000.0, 1000.0, 30.0, 42);
    assert!(m.num_regions() > 100, "expected many regions, got {}", m.num_regions());
    assert!(m.num_triangles() > 100, "expected many triangles, got {}", m.num_triangles());
    assert_eq!(m.num_sides(), m.num_triangles() * 3);
    assert!(
        m.num_boundary_regions() > 0 && m.num_boundary_regions() < m.num_regions(),
        "boundary frame should be a proper subset of regions"
    );
    // First region is on the boundary frame; a deep-interior index is not.
    assert!(m.is_boundary_r(0));
    assert!(!m.is_boundary_r(m.num_regions() - 1));
}
