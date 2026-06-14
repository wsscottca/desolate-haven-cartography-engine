//! Thin wasm-bindgen adapter exposing `dhce-core` to the browser front-end.
//!
//! Phase 0 surfaces just enough to prove the JS↔WASM boundary (version + a terrain
//! sample). Phase 2 adds the resident `Engine` (mesh + memory views + `generate`).

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// Engine version string — diagnostics + JS↔WASM boundary smoke test.
#[wasm_bindgen]
pub fn version() -> String {
    dhce_core::VERSION.to_string()
}

/// Sample the placeholder terrain field at `(x, y)` for `seed`.
/// Phase 2 replaces this with the full mesh/elevation pipeline behind an `Engine`.
#[wasm_bindgen]
pub fn sample_terrain(seed: f64, x: f64, y: f64) -> f64 {
    dhce_core::noise::fbm2(x, y, seed as u64, 6)
}
