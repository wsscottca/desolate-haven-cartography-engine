//! Elevation + surface geometry invariants.

use dhce_core::{elevation, geometry, mesh::Mesh};

#[test]
fn elevation_is_bounded_and_deterministic() {
    let mesh = Mesh::new(800.0, 800.0, 50.0, 3);
    let a = elevation::assign_region_elevation(&mesh, 800.0, 800.0, 3, 5);
    let b = elevation::assign_region_elevation(&mesh, 800.0, 800.0, 3, 5);
    assert_eq!(a.len(), mesh.num_regions());
    assert_eq!(a.len(), b.len());
    for r in 0..a.len() {
        assert_eq!(a[r].to_bits(), b[r].to_bits(), "elevation not reproducible at {r}");
        assert!((-1.0..=1.0).contains(&a[r]), "elevation out of range: {}", a[r]);
    }
}

#[test]
fn surface_counts_and_normals_are_consistent() {
    let mesh = Mesh::new(1000.0, 1000.0, 40.0, 7);
    let elev = elevation::assign_region_elevation(&mesh, 1000.0, 1000.0, 7, 6);
    let s = geometry::build_surface(&mesh, &elev, 120.0);

    let nr = mesh.num_regions();
    assert_eq!(s.positions.len(), nr * 3);
    assert_eq!(s.normals.len(), nr * 3);
    assert_eq!(s.heights.len(), nr);
    assert_eq!(s.indices.len(), mesh.num_triangles() * 3);

    assert!(s.positions.iter().all(|v| v.is_finite()), "non-finite position");
    assert!(s.indices.iter().all(|&i| (i as usize) < nr), "index out of range");

    for r in 0..nr {
        let (x, y, z) = (s.normals[3 * r], s.normals[3 * r + 1], s.normals[3 * r + 2]);
        let len = (x * x + y * y + z * z).sqrt();
        assert!((len - 1.0).abs() < 1e-4, "normal not unit at {r}: {len}");
        assert!(z >= 0.0, "normal should point upward at {r}");
    }
}
