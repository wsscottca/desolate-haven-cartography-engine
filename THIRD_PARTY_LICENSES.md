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

PRNG, simplex noise, and Poisson-disk sampling are our own. The only dependency is
the Delaunay triangulation:

| Crate | License | Notes |
|---|---|---|
| `delaunator` 1.x | ISC | Delaunay triangulation. We own the dual half-edge wrapper. |
| `robust` 1.x | MIT OR Apache-2.0 | Robust geo predicates (via `delaunator`); MIT election. |

## Runtime — WASM adapter (`crates/dhce-wasm`)

| Crate | License | Notes |
|---|---|---|
| `wasm-bindgen` (+ macro/shared) | MIT OR Apache-2.0 | MIT election. |
| `console_error_panic_hook` (dev) | Apache-2.0/MIT | Dev diagnostics; MIT election. |
| build-macro tree: `proc-macro2`, `quote`, `syn`, `bumpalo`, `cfg-if`, `once_cell`, `rustversion`, `log` | MIT OR Apache-2.0 | Transitive; MIT election. |
| `unicode-ident` | (MIT OR Apache-2.0) AND Unicode-3.0 | Transitive; all parts permissive. |

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
