---
status: active
date: 2026-06-14
owner: wsscottca
---

# ADR 0002 — Native desktop tool on Godot 4 + C# (Rust-powered)

## Status
Accepted.

## Context
The tool shipped Phases 0–8 as a browser app (TS/WebGL2 front-end over the
`dhce-wasm` WASM build of the shared core). There is no real benefit to a web tool;
native gives performance and feature headroom. A design pass picked the native stack
and re-audited dependencies for free-for-commercial use. A hard constraint from the
owner: **Rust powers anything intensive as is reasonable** — the engine front-end
should be a thin shell, not a second compute layer.

The game `desolate-haven` is already Godot 4 + C# (.NET), and a `dhce-godot`
GDExtension adapter (godot-rust/gdext) already compiles against the shared core
(see [ADR 0001](0001-shared-rust-core.md)).

## Decision
Build the cartography tool as a **Godot 4 + C# application driving the `dhce-godot`
GDExtension**, with **all compute-intensive work in Rust** (`dhce-core`).

- **One engine, tool + game.** The same deterministic extension authors maps in the
  tool and regenerates them in the game — shared rendering conventions, no second
  stack to keep in sync, the owner's existing toolchain, least new code.
- **Compute in Rust, presentation in Godot.** Mesh build, elevation/noise, hydraulic
  sim, stream flow-accumulation, brush/course/select math, nearest-region picking, and
  save/load (de)serialization stay in `dhce-core`. Godot/C# only runs the orbit camera,
  uploads `ArrayMesh` buffers, drives shaders, and hosts UI widgets. C# runs **no**
  per-region loops; screen interactions that touch many cells call a Rust method.
- **Reuse boundary — hoist authoring into the core.** The brush spatial-hash, smoothstep
  falloff, reclassify-on-edit, course-carve, color smoothing, and stream orchestration
  were embedded in `dhce-wasm`. They now live in `dhce_core::world::World`; both adapters
  are thin marshalling shells. (Implemented in commit prior to this ADR.) All of it is
  determinism-safe — polynomials, neighbour averaging, integer hashing, threshold
  classify; no transcendentals — so the cross-target contract holds.
- **`dhce-wasm` is frozen, not deleted.** It still builds and drives the same `World`,
  serving as a living native↔web determinism cross-check.
- **Repo layout.** The Godot C# project lives in this repo under `tool/`; the compiled
  extension is consumed there and is copyable into the game for runtime regen.

### Why this maximizes Rust where it matters
The intensive work is 100% Rust on *any* native path. Godot does not move intensive work
out of Rust — it only takes over windowing/rendering/UI, which wgpu/Bevy would otherwise
force us to hand-write in Rust (none of it the hot path).

### Dependency re-audit (free-for-commercial)
| Dependency | Scope | License | Verdict |
|---|---|---|---|
| `delaunator` | core | ISC | Keep (best-in-class; `spade` MIT/Apache only if constrained Delaunay is later needed). |
| `robust` | core (transitive) | MIT/Apache | Keep. |
| `godot` (gdext) | native adapter | MPL-2.0 | Keep — file-level weak copyleft on gdext's own files; does not reach our code. |
| Godot engine | runtime | MIT | No royalties. |
| `wasm-bindgen`, wasm-pack, esbuild | web only | MIT / — | Dropped from the native build (only the frozen web crate touches them). |

The native build **removes** the JS/wasm toolchain and adds nothing new on the Rust side.
The build-time license gate migrates from the Node `scripts/check-licenses.mjs` to
**`cargo-deny`** (`deny.toml`, MIT/Apache).

## Alternatives considered
- **Native Rust (wgpu + winit + egui).** Pure Rust, no engine. Rejected: rebuilds the
  entire front-end from scratch, adds no Rust to the hot path, and creates a rendering
  stack separate from the game (drift risk).
- **Bevy (Rust ECS).** Less from-scratch than raw wgpu, but ECS is overkill for a
  forms-over-3D tool, adds a large dependency surface, and is still separate from the game.

## Consequences
- One deterministic adapter serves both tool and game; what the tool renders, the game
  renders.
- A Godot 4.x install + .NET become tool prerequisites; the `godot` crate is pinned to the
  project's Godot build (gdext API alignment).
- The web build is frozen (kept only as a cross-check); the TS/WebGL front-end is retired.
- The open risk is interactive **per-stroke mesh-update performance** at ~190k–445k regions;
  the N0 risk spike validates it (full re-pack in Rust + `ArrayMesh` rebuild, escalating to
  `RenderingServer` partial vertex updates only if needed).
