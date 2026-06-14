//! Hydraulic solver invariants: mass conservation, stability, convergence.

use dhce_core::{
    elevation,
    fluid::{self, LiquidField},
    mesh::Mesh,
};

fn setup() -> (Mesh, Vec<f64>, Vec<Vec<u32>>) {
    let mesh = Mesh::new(1000.0, 1000.0, 50.0, 11);
    let terrain = elevation::assign_region_elevation(&mesh, 1000.0, 1000.0, 11, 6);
    let neighbors = mesh.region_neighbors();
    (mesh, terrain, neighbors)
}

#[test]
fn sea_fill_levels_the_surface() {
    let (mesh, terrain, _) = setup();
    let mut f = LiquidField::new(mesh.num_regions());
    let level = -0.1;
    fluid::sea_fill(&mut f, &terrain, level);
    for r in 0..mesh.num_regions() {
        if terrain[r] < level {
            assert!((terrain[r] + f.depth[r] - level).abs() < 1e-9, "surface != sea level at {r}");
        } else {
            assert_eq!(f.depth[r], 0.0, "dry region {r} has water");
        }
    }
}

#[test]
fn relax_conserves_volume_without_evaporation() {
    let (mesh, terrain, nb) = setup();
    let mut f = LiquidField::new(mesh.num_regions());
    fluid::add_rain(&mut f, &terrain, -2.0, 0.05); // rain everywhere
    let v0 = f.volume();
    assert!(v0 > 0.0);
    for _ in 0..50 {
        fluid::relax_step(&mut f, &terrain, &nb, 0.3, 0.0);
    }
    let v1 = f.volume();
    assert!((v0 - v1).abs() / v0 < 1e-6, "volume drifted: {v0} -> {v1}");
    assert!(f.depth.iter().all(|d| d.is_finite() && *d >= 0.0), "depths blew up");
}

#[test]
fn relax_is_stable_and_never_raises_the_global_surface() {
    // Downhill-only flow capped at half the surface gap can never push a cell above
    // a higher neighbor, so the global maximum surface is non-increasing. This is the
    // key stability invariant (no oscillation / blow-up).
    let (mesh, terrain, nb) = setup();
    let mut f = LiquidField::new(mesh.num_regions());
    fluid::add_rain(&mut f, &terrain, -2.0, 0.1);

    let max_surface = |f: &LiquidField| {
        terrain
            .iter()
            .zip(&f.depth)
            .map(|(t, d)| t + d)
            .fold(f64::NEG_INFINITY, f64::max)
    };

    let mut prev = max_surface(&f);
    for _ in 0..200 {
        fluid::relax_step(&mut f, &terrain, &nb, 0.3, 0.0);
        let cur = max_surface(&f);
        assert!(cur <= prev + 1e-9, "global surface rose: {prev} -> {cur}");
        assert!(f.depth.iter().all(|d| d.is_finite() && *d >= 0.0), "depths blew up");
        prev = cur;
    }
}
