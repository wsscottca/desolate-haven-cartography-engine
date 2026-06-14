//! Desolate Haven Cartography Engine — shared compute + simulation core.
//!
//! Pure, dual-target (wasm32 + native) generation and simulation. No wasm or JS
//! assumptions live here; `dhce-wasm` and `dhce-godot` are thin adapters. Given the
//! same seed and inputs, this crate produces identical results on every target —
//! that is the guarantee that keeps the authoring tool and the game in sync.
//!
//! Clean-room: nothing here is derived from mapgen4 / dual-mesh / @redblobgames code.
//!
//! Determinism rules (enforced by tests):
//! - integer math uses wrapping ops only,
//! - floating math is done in `f64` and narrowed to `f32` only at storage boundaries,
//! - no `f32::mul_add`/fast-math, no nondeterministic ordering.

pub mod prng;
pub mod noise;
pub mod mesh;
pub mod elevation;
pub mod rainfall;
pub mod fluid;
pub mod liquids;
pub mod geometry;
pub mod biomes;
pub mod scatter;

/// Engine version, surfaced to front-ends for diagnostics and the JS↔WASM smoke test.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
