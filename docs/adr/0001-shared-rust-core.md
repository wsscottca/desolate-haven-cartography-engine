---
status: active
date: 2026-06-13
owner: wsscottca
---

# ADR 0001 — A shared Rust core for tool + game

## Status
Accepted.

## Context
The predecessor (`mapgen4` Sundered Vale fork) ran generation in TypeScript in the
browser only; the Godot game had no shared source of truth, so tool and game would
drift. We also want zero Apache-2.0 lineage and a path to true 3D + liquid physics.

## Decision
The generation **and** simulation logic lives in one pure Rust crate, `dhce-core`,
compiled to two targets through thin adapters:

- `dhce-wasm` (wasm-bindgen) → the browser authoring tool,
- `dhce-godot` (GDExtension) → the game.

The core is clean-room (no mapgen4 / dual-mesh / @redblobgames code), uses our own
PRNG + noise, and guarantees **determinism across targets** (same seed/inputs ⇒
identical output) so a world authored in the tool reproduces exactly in the game.

## Consequences
- One engine, two front-ends; no logic duplication or drift.
- A Rust toolchain becomes a build prerequisite (documented in the README).
- Determinism is a hard constraint: `f64` math narrowed to `f32` only at storage,
  wrapping integer ops, total-order sorts, fixed-step sim — enforced by tests.
- Rendering stays per-front-end (WebGL2 here, Godot's renderer there); only the
  data (meshes, fields, instances) crosses the boundary.
