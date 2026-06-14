# Third-party licenses

DHCE is a clean-room, procedurally-driven tool. We avoid bundled art and prefer
generating visuals mathematically (SVG / SDF / shaders), so the only third-party
surface is **code dependencies** and the **fonts**. Everything below is permissive.

The license gate (`scripts/check-licenses.mjs`, run from `web/build.sh`) fails the
build if any dependency uses a non-permissive license.

## Policy

- Allowed: MIT, ISC, BSD-2/3-Clause, Apache-2.0, Zlib, Unlicense, CC0, 0BSD, MPL-2.0, OFL-1.1.
- Rust crates licensed **"MIT OR Apache-2.0"** are used under the **MIT** election → no Apache
  obligations. Apache-2.0-*only* crates are permitted but flagged (we minimize Apache lineage).
- **No** GPL/LGPL/AGPL or unlicensed dependencies.
- **No** code derived from mapgen4 / `@redblobgames/dual-mesh` / `@redblobgames/prng` (Apache-2.0).

## Runtime — Rust core (`crates/dhce-core`)

| Crate | License | Notes |
|---|---|---|
| *(none yet)* | — | Phase 1 adds a permissive Delaunay crate (MIT-elect). PRNG + noise are our own. |

## Runtime — WASM adapter (`crates/dhce-wasm`)

| Crate | License | Notes |
|---|---|---|
| `wasm-bindgen` | MIT OR Apache-2.0 | Used under MIT election. |
| `console_error_panic_hook` (dev) | MIT OR Apache-2.0 | Dev diagnostics only. |

## Runtime — Web front-end (`web/`)

| Package | License | Notes |
|---|---|---|
| *(none)* | — | Math (`mat4`/`vec3`) and rendering are our own; zero runtime npm deps. |

## Build tooling (not shipped)

| Tool | License | Notes |
|---|---|---|
| esbuild | MIT | Bundler. |
| wasm-pack | MIT OR Apache-2.0 | WASM build (MIT election). |

## Fonts (the only external art)

| Font | License | Notes |
|---|---|---|
| IM Fell English SC | OFL-1.1 | Display. |
| Alegreya | OFL-1.1 | Body. |
| Metamorphous | OFL-1.1 | Labels. |

Self-hosted copies (when added) live in `assets/fonts/` with their `OFL.txt`.
