> # ⚠️ DEPRECATED — archived 2026-06-19
>
> DHCE (Godot 4.6 + Rust core) is **retired and no longer maintained**. World
> generation has moved to **[Gaea](https://quadspinner.com/)** and the engine to
> **Unreal Engine 5**. This repository is kept only as a historical archive of the
> Godot/Rust authoring tool.
>
> Full history is preserved on this remote. Notable branches:
> - `n3a-tool-shell` — the N3 in-editor tool shell (minimap, world sun, water-drain).
> - `feat/dhce-cursor-and-sculpt-tools` — the last active work (surface-conforming
>   brush cursor, sculpt tools, hydraulic erosion, canon region import).

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

Bash / Git Bash:
```sh
cd web
sh build.sh
python -m http.server 8000
```

PowerShell (Windows — note: `&&` is not valid in PS 5.1, run each line separately):
```powershell
cd web
.\build.ps1                # or: powershell -ExecutionPolicy Bypass -File .\build.ps1
python -m http.server 8000
```

Then open http://localhost:8000/ — orbit (drag), zoom (wheel), pan (shift-drag),
and retune the world from the Settings panel on the right.

The build is a license gate → (Rust→WASM engine if `wasm-pack` is present) → esbuild
bundle. Without `wasm-pack` it builds the shell on the TS placeholder engine.

## Test (core, needs Rust)

```sh
cargo test           # determinism + (later) fluid-conservation + canon round-trip
```

## Status

Phase 0 — repo + toolchain + clean-room 3D shell. See `docs/adr/` and the plan.
