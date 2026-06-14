//! Decoration scatter is deterministic, bounded, and respects density.

use dhce_core::{elevation, mesh::Mesh, scatter};

#[test]
fn scatter_is_deterministic_and_bounded() {
    let mesh = Mesh::new(1000.0, 1000.0, 40.0, 9);
    let elev = elevation::assign_region_elevation(&mesh, 1000.0, 1000.0, 9, 6);
    let biome = vec![4u8; mesh.num_regions()]; // Temperate Forest → trees

    let a = scatter::scatter(9, &mesh, &elev, &biome, 120.0, 0.5);
    let b = scatter::scatter(9, &mesh, &elev, &biome, 120.0, 0.5);
    assert_eq!(a.len(), b.len(), "scatter count must be reproducible");
    for i in 0..a.len() {
        assert_eq!(a[i].x.to_bits(), b[i].x.to_bits(), "position must be reproducible");
        assert!(a[i].scale.is_finite() && a[i].scale > 0.0);
        assert!(a[i].species == 0.0 || a[i].species == 1.0);
    }

    assert!(!a.is_empty(), "forest at density 0.5 should place instances");
    assert!(scatter::scatter(9, &mesh, &elev, &biome, 120.0, 0.0).is_empty(), "density 0 → nothing");
}
