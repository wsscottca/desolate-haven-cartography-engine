//! Stream accumulation: determinism, the catchment gate, and basic sanity.

use dhce_core::{elevation, mesh::Mesh, streams};

fn setup() -> (Mesh, Vec<f64>, Vec<Vec<u32>>, usize) {
    let mesh = Mesh::new(1000.0, 1000.0, 50.0, 7);
    let terrain = elevation::assign_region_elevation(&mesh, 1000.0, 1000.0, 7, 6);
    let neighbors = mesh.region_neighbors();
    let num_b = mesh.num_boundary_regions();
    (mesh, terrain, neighbors, num_b)
}

/// Lowest-elevation interior region — a believable place to anchor a painted river.
fn lowest_interior(terrain: &[f64], num_b: usize) -> usize {
    let mut best = num_b;
    for r in num_b..terrain.len() {
        if terrain[r] < terrain[best] {
            best = r;
        }
    }
    best
}

#[test]
fn is_deterministic() {
    let (_m, terrain, nb, num_b) = setup();
    let mut seed = vec![false; terrain.len()];
    seed[lowest_interior(&terrain, num_b)] = true;

    let a = streams::accumulate(&terrain, &nb, num_b, &seed, 0.08, 0.3);
    let b = streams::accumulate(&terrain, &nb, num_b, &seed, 0.08, 0.3);
    assert_eq!(a.carve_delta.len(), b.carve_delta.len());
    for i in 0..a.carve_delta.len() {
        assert_eq!(
            a.carve_delta[i].to_bits(),
            b.carve_delta[i].to_bits(),
            "carve must be reproducible at {i}"
        );
        assert_eq!(a.flow[i].to_bits(), b.flow[i].to_bits(), "flow must be reproducible at {i}");
    }
}

#[test]
fn no_seed_means_no_streams() {
    let (_m, terrain, nb, num_b) = setup();
    let seed = vec![false; terrain.len()];
    let r = streams::accumulate(&terrain, &nb, num_b, &seed, 0.08, 0.3);
    assert!(r.is_stream.iter().all(|&s| !s), "no painted river ⇒ no tributaries");
    assert!(r.carve_delta.iter().all(|&d| d == 0.0), "no streams ⇒ no carving");
}

#[test]
fn seeded_river_grows_a_catchment() {
    let (_m, terrain, nb, num_b) = setup();
    let mut seed = vec![false; terrain.len()];
    let outlet = lowest_interior(&terrain, num_b);
    seed[outlet] = true;

    let r = streams::accumulate(&terrain, &nb, num_b, &seed, 0.05, 0.3);
    let count = r.is_stream.iter().filter(|&&s| s).count();
    assert!(count > 0, "a painted river at a low point should grow tributaries");

    // Every stream cell carves a positive channel; non-streams never carve.
    for i in 0..terrain.len() {
        if r.is_stream[i] {
            assert!(r.carve_delta[i] > 0.0, "stream {i} must carve");
        } else {
            assert_eq!(r.carve_delta[i], 0.0, "non-stream {i} must not carve");
        }
        assert!(r.flow[i].is_finite() && r.flow[i] >= 1.0, "flow sane at {i}");
    }
}
