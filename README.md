# Desolate Haven Cartography Engine (DHCE)

A clean-room, true-3D world-authoring tool with real liquid physics. The generation
and simulation core is a shared Rust crate compiled to **WASM** (this browser tool)
and **native** (the Godot `desolate-haven` game, via GDExtension) — so a map authored
here and the world in the game come from one engine.

Successor to the `mapgen4` Sundered Vale fork. **Not** derived from it: DHCE is a
from-scratch reimplementation with zero Apache-2.0 lineage. See `LICENSE`.

## Layout

```
crates/dhce-core    pure compute + simulation (PRNG, noise, mesh, elevation,
                    rainfall, fluid sim, liquids, geometry, biomes, scatter)
crates/dhce-wasm    wasm-bindgen adapter (browser)
crates/dhce-godot   GDExtension adapter (Phase 8)
web/                3D WebGL2 front-end (own shaders, own math, own tool icons)
docs/               ADRs + the self-contained core contract spec
```

## Prerequisites

- **Node** (bundling) and **esbuild** (`npm i -g esbuild`) — already present.
- **Rust** toolchain (engine crates). Install once:
  - `winget install Rustlang.Rustup`  (or https://rustup.rs)
  - `rustup target add wasm32-unknown-unknown`
  - `cargo install wasm-pack`

The web shell runs **without** Rust using a TypeScript placeholder heightfield;
the Rust/WASM engine takes over from Phase 1 onward.

## Build & run (web)

```sh
cd web
sh build.sh          # license gate → (wasm if available) → esbuild bundle
python -m http.server 8000
# open http://localhost:8000/index.html  — orbit with drag, zoom with wheel, pan with shift-drag
```

## Test (core, needs Rust)

```sh
cargo test           # determinism + (later) fluid-conservation + canon round-trip
```

## Status

Phase 0 — repo + toolchain + clean-room 3D shell. See `docs/adr/` and the plan.
