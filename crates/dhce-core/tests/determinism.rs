//! Determinism is the contract that keeps the browser tool and the Godot game in
//! sync: same seed/inputs ⇒ identical output on every target. These tests lock the
//! Phase 0 primitives; later phases extend them (fluid conservation, canon round-trip).

use dhce_core::{noise, prng::Rng};

#[test]
fn prng_stream_is_reproducible() {
    let pull = || {
        let mut r = Rng::new(0x0DE5_01A7_E000_0001);
        (0..16).map(|_| r.next_u64()).collect::<Vec<_>>()
    };
    assert_eq!(pull(), pull(), "same seed must yield the same stream");
}

#[test]
fn prng_f64_stays_in_unit_interval() {
    let mut r = Rng::new(1);
    for _ in 0..100_000 {
        let v = r.next_f64();
        assert!((0.0..1.0).contains(&v), "next_f64 out of range: {v}");
    }
}

#[test]
fn prng_below_is_bounded() {
    let mut r = Rng::new(7);
    for _ in 0..100_000 {
        assert!(r.next_below(10) < 10);
    }
}

#[test]
fn noise_is_bit_identical_and_bounded() {
    for i in 0..1000 {
        let x = i as f64 * 0.37;
        let y = i as f64 * 0.91;
        let a = noise::fbm2(x, y, 42, 6);
        let b = noise::fbm2(x, y, 42, 6);
        // Bit-identical for the same inputs (the cross-target guarantee, checked here
        // on the native target; a wasm harness asserts equality across targets later).
        assert_eq!(a.to_bits(), b.to_bits(), "noise not reproducible at i={i}");
        assert!((-1.0001..=1.0001).contains(&a), "noise out of range: {a}");
    }
}
