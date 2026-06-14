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
Target envelope **≈20 km × 20 km × up to 10 km**. Extent `W × H` is arbitrary; vertical range is
normalized elevation `[-1.5, 1.5]` × a configurable **vertical scale** (exposed as world-height in
km), so 10 km of relief is just a setting — f32 precision is fine at this size (~mm at 20 km). No
structural limit.

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

### 5. Volumetric features (caves, overhangs, tunnels) — hybrid, authored
The heightfield base can't represent geometry that folds over itself, and some features genuinely
require it (a tunnel *through* a mountain can't be faked by a hidden wall). These are **occasional,
authored** features, so rather than make the whole world volumetric (a voxel/SDF engine that would
reset the core), the template stays heightfield and gains an **authored volumetric-feature tool**:
mark a cave/overhang/tunnel and the tool produces real volumetric geometry there (local SDF /
marching-cubes or CSG carve) merged into the world mesh on export. Volumetric only where authored;
the ~99% normal terrain stays the efficient heightfield.

Biome **variety** (round vs jagged vs volcanic rock, hill types, flora, water) is **not** a
volumetric concern — it comes from per-biome **landform profiles** shaping the heightfield
(`Landform`: peak_shape / hill_amp / valley_floor / roughness) + noise/erosion, scatter (flora),
and the liquid overlay (water). Heightfield gen is also far lighter than volumetric, so it wins on
both variety and performance. (Biome-driven terrain shaping lands in the biome pass, N3.)

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
  (scatter-rule + model selection) and **biome-driven terrain shaping** (apply the per-biome
  landform profiles so jagged / rolling / volcanic biomes actually differ).
- **N4** — export pipeline: terrain `ArrayMesh`(+collision), `MultiMesh` scatter, bake-to-instances,
  save `.tscn`/`.res`; plus save/load of the authoring project.
- **N5 — Volumetric Features** — authored caves / overhangs / tunnels carved into the heightfield
  base (hybrid; local volumetric meshing merged on export). See §5.
- **Later** — generation-streaming / LOD only if a world outgrows RAM; Atmospheric Fog.

## Open items
- Export granularity (per-chunk scenes vs one merged mesh) + collision strategy.
- MultiMesh-vs-instances threshold and the "bake to instances" UX.
- Threaded-gen mechanism (C# `Task` + `Callable.CallDeferred` for main-thread tessellation) —
  validate live.
- Render-distance default + tile unload policy.
