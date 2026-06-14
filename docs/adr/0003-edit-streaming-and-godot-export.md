---
status: active
date: 2026-06-14
owner: wsscottca
---

# ADR 0003 — Edit-time render streaming + Godot-native export

## Status
Accepted.

## Context
N0 validated the native stack (see [ADR 0002](0002-native-godot-stack.md)): the Godot 4 + C#
tool drives the `dhce-godot` GDExtension, and chunked terrain made **editing** fast (~36 ms/dab
at 4.28M regions; a dab re-tessellates only the ~2–3 tiles it touches). Two things remain to
define the *tool* (not just the spike):

1. **Scale.** The canon world is large — target **~20 km × 20 km** with full vertical range.
   Generating *and* meshing all of it up front freezes the window (~5 s gen at ~1.7M regions,
   worse at 20 km).
2. **Output.** The tool authors the *base* landscape — terrain + biomes + procedural trees/rocks.
   The finished world must land in the game's Godot editor as **editable** content so the owner
   can hand-place characters and make final adjustments before shipping.

## Decision

### 1. Edit-time render streaming (not load-all, not generation streaming)
Generate the full deterministic world **once, off the main thread** (so the window opens
immediately and never freezes), but **tessellate/render only the chunks within an adjustable
render distance** of the camera, loading/unloading tiles as the camera moves.

- Chunks are **sized by region count** (~12k regions/tile) so edit + tile cost stays flat at any
  world size.
- **Render distance** is a user setting (how many tiles around the camera are meshed).
- The world *data* (per-region elevation/biome/liquid in `World`) lives in Rust RAM; only the
  *meshes* stream. At 20 km this is tens–low-hundreds of MB of data — acceptable.
- **Rejected — generation streaming** (generate only near the camera): needs tileable per-chunk
  generation with halo points to avoid border seams between independently triangulated chunks —
  significant complexity, and export needs the whole world generated anyway, so it mostly just
  saves editing RAM. Deferred to a true LOD phase, only if a world is too big to hold in memory.
- **Rejected — load all + render all**: freezes on gen + tessellate and caps practical world size.

### 2. Scale + height
World extent `W × H` is arbitrary (20 km target). Vertical range is the normalized elevation
`[-1.5, 1.5]` × a configurable **exaggeration**, so "enough height" is just a vertical-scale
setting — no structural limit.

### 3. Godot-native export pipeline
Because the tool **is** a Godot project, "export" = saving **native Godot resources** the game
editor opens directly (no import/translation step):

- **Terrain →** `ArrayMesh` resources (the chunk meshes) as a `MeshInstance3D` tree (or merged),
  with optional `StaticBody3D` + collision for gameplay.
- **Scatter (trees/rocks) →** `MultiMeshInstance3D` per species for the dense base (cheap), plus a
  **"bake to individual instances"** path so chosen areas become selectable/movable `Node3D`s for
  hand-editing.
- **Biomes →** data + per-biome material/scatter config.
- Saved as a `.tscn` (+ `.res` meshes) opened in the game editor to place characters and finish by
  hand. Determinism (shared `DhceEngine`) means the export reproduces exactly and round-trips.

### 4. Biome-driven scatter
Per biome, define **scatter rules** — which tree/rock *model options* (owner drops in `.glb`/
scenes), density, slope/elevation limits — via the existing `ScatterRule` / `BiomeDecor` schema,
surfaced in the biome editor. The tool places procedurally + deterministically; export bakes them
editable. The tool references owner-supplied models; it bundles **no art** (keeps the
procedural/clean-room stance).

## Consequences
- One-time, non-blocking world gen + a render-distance setting → responsive 20 km editing.
- Export is a "bake the whole world" build step; slowness there is acceptable.
- The determinism / clean-room contract (ADR 0001/0002) is unchanged.
- Main implementation risk: calling the heavy Rust `build()` off the main thread across the gdext
  boundary — mitigated with a `ThreadedGen` fallback toggle.
- Proprietary tool → its output is purely the owner's game content; the tool's deps (gdext MPL-2.0,
  Godot MIT) impose nothing on the exported world.

## Phasing
- **N2** — tool shell + render streaming (threaded gen, progressive tessellation, render-distance,
  region-sized chunks) on a maximized window.
- **N3** — sculpt + course/flood + biome tools, brush size/intensity UI, the per-biome editor
  incl. scatter-rule + model selection.
- **N4** — export pipeline: terrain `ArrayMesh`(+collision), `MultiMesh` scatter, bake-to-instances,
  save `.tscn`/`.res`; plus save/load of the authoring project.
- **Later** — generation-streaming / LOD only if a world outgrows RAM; Atmospheric Fog.

## Open items
- Export granularity (per-chunk scenes vs one merged mesh) + collision strategy.
- MultiMesh-vs-instances threshold and the "bake to instances" UX.
- Threaded-gen mechanism (C# `Task` + `Callable.CallDeferred` for main-thread tessellation) —
  validate live.
- Render-distance default + tile unload policy.
