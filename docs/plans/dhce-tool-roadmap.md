---
status: active
date: 2026-06-14
owner: wsscottca
---

# DHCE tool — roadmap & session hand-off

Pick-up doc for a fresh session. Design is settled in the ADRs; this is **current state + how to
work + what's next**.

## What this is
The Desolate Haven Cartography Engine — a native **Godot 4.6 + C#** terrain-authoring tool driving a
shared **Rust core** through a GDExtension. It authors the canon world's base landscape (terrain +
biomes + procedural scatter) and exports an **editable Godot scene** the game editor finishes by
hand (place characters, tweak). Design:
- [ADR 0001](../adr/0001-shared-rust-core.md) — one shared Rust core, two adapters.
- [ADR 0002](../adr/0002-native-godot-stack.md) — native Godot 4 + C#, Rust powers all intensive compute.
- [ADR 0003](../adr/0003-edit-streaming-and-godot-export.md) — render-streaming + Godot-native export + biome scatter + **hybrid volumetric**.
- Core API: [docs/specs/dhce-core-contract.md](../specs/dhce-core-contract.md).

## Units & scale
**1 Godot unit = 1 m** (also Godot's own convention — physics/lighting/audio are metre-tuned). The
core's `build(width, height, spacing, …)` takes **metres**. Author-facing exports are km where the
scale warrants it (`WorldSizeKm`, `TerrainHeightKm`) and m for fine-scale tools (`SpacingM`,
`BrushRadiusM`); C# converts km→m (`×1000`) before any core call. Cost scales with **area**
(regions ∝ W·H / spacing²), so doubling `WorldSizeKm` is ~4× the regions/gen time.
**Precision ceiling:** Godot transforms are 32-bit float; at 20 km from origin the ULP is ~2 mm
(fine for terrain). Past ~40–80 km, or for precise gameplay far from origin, you'd need a
double-precision ("Large World Coordinates") engine build — a custom Godot compile, so defer until
a world actually needs it.

## Repo layout
- `crates/dhce-core` — shared Rust: gen + sim + **authoring (`world.rs` = the source of truth)** +
  chunking. Pure, deterministic, no engine deps.
- `crates/dhce-godot` — GDExtension adapter (`godot`/gdext **0.5**, pinned to **Godot 4.6**). Thin:
  every method forwards to `World`. Built standalone → `dhce_godot.dll`.
- `crates/dhce-wasm` — frozen web adapter, kept only as a native↔web determinism cross-check.
- `tool/` — the **Godot 4.6 C# project** (the tool). `tool/scripts/CartographerSpike.cs` (current
  N0/N1 driver) + `OrbitCamera.cs`; `tool/addons/dhce/` (`.gdextension` manifest + the DLL — DLL is
  gitignored, rebuild + copy). Scene root = `CartographerSpike` (`[GlobalClass]`), main scene `Main.tscn`.
- `web/` — retired TS/WebGL front-end. Do not develop.

## How to build / run / test (Windows / PowerShell)
- Prepend cargo to PATH: `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"`.
- Core tests: `cargo test -p dhce-core` (22+ tests; determinism, chunks, streams, fluid).
- **Build dir is off OneDrive** via `.cargo/config.toml` → `C:\Users\WSSco\.dhce-build`. When running
  cargo with `--manifest-path` from another CWD, also set `$env:CARGO_TARGET_DIR="C:\Users\WSSco\.dhce-build"`.
- **LNK1104 on test/exe link** = Windows Defender / OneDrive scanning the fresh `.exe`. Kill stale
  test procs by name and retry (clears on its own). A Defender exclusion for `.dhce-build` stops it.
- Build the DLL: dhce-godot is **excluded from the workspace** (`exclude` in root `Cargo.toml` — it
  needs the gdext toolchain), so `cargo build -p dhce-godot` from the repo root **fails**
  (`package ID specification did not match any packages`). Build from inside the crate dir:
  `cd crates/dhce-godot; cargo build --release`. Output → the dir set in `.cargo/config.toml`
  (currently `C:\Users\WSSco\.dhce-build\release\dhce_godot.dll` — the redirect off OneDrive applies
  to the excluded crate too, via upward config discovery; the old `crates/dhce-godot/target/release/`
  path is stale). Copy that DLL to `tool/addons/dhce/dhce_godot.dll`.
- **The DLL is locked while the Godot editor is open** — to swap it: user closes Godot → copy → reopen.
  **C#-only changes don't need a DLL swap** — just Build in the editor and F5.
- Git: a repo-scoped push allow-rule exists. Commit + push per phase. **No `Co-Authored-By` on the
  user's creative/story commits; fine on code/design commits.**

## State — DONE
- **N0 ✅** — stack + perf validated. Naive full-mesh rebuild was ~150 ms/dab @617k regions (too slow);
  **chunked terrain** fixed it → ~10–36 ms/dab @1.7–4.3M regions, ~2–4 dirty chunks/dab.
- **N1 ✅** — authoring hoisted into `dhce_core::world::World`; both adapters thin. Chunk API
  (`chunk_count`/`tessellate_chunk`/`chunk_positions|normals|colors|indices`/`take_dirty_chunks`);
  chunks **sized from region count** (~12k regions/tile → flat edit cost at any world size).
- Rendering fixed: double-sided material (the Y-up vertex remap flips winding → top-down was
  backface-culled), ambient `WorldEnvironment` + dark background, vertical scale ~300, maximized window.
- Known finding to fix in N2: **startup gen freezes** (~5 s @1.7M — the global Rust `build()` blocks
  the main thread); tessellate-all adds ~0.5 s.

## N2 — streaming tool shell ✅ DONE (verified live 2026-06-14)
Goal (ADR 0003 render-streaming): a 20 km world opens **instantly** and edits responsively.

**STATUS — VERIFIED (2026-06-14).** 20 km world (spacing 12 ⇒ **1.71M regions / 3.42M tris / 144
chunks, 12×12 grid**): window opens instantly on the "Generating…" splash, gen ~5 s behind it, terrain
fills in progressively, sculpt **3.8–5.9 ms/dab** over ~1.5–2 dirty chunks/dab (well under the ~10 ms
target). One blocker was found + fixed on the first run (see FINDING below). `cargo test -p dhce-core`
(24) + `dotnet build` green. What landed:
- Core: `World::chunk_grid() -> (cols, rows)` + `World::chunk_centers()` (square grid, `id = gy*cols+gx`),
  stored `chunk_cols/chunk_rows` set in `build_chunks`; exposed via `DhceEngine.chunk_grid()` (Vector2i)
  + `chunk_centers()` (PackedVector2Array). 2 new world tests.
- `CartographerSpike.cs`: gen is **splash-deferred on the main thread** — `_Ready` shows a `CanvasLayer`
  splash, `_Process` fires `build()` after `WarmupFrames` so the splash draws first, then `OnGenDone`
  creates an empty `MeshInstance3D` per chunk; `_Process` streams nearest-first (`ChunksPerFrame`) within
  `RenderDistance` tiles of the camera **Target**, freeing far chunks (`ClearSurfaces`). Exports:
  `RenderDistance`, `ChunksPerFrame`, `TerrainHeightKm` (vertical relief in km; `exaggeration = km*1000/3.0`,
  0.9 km ⇒ 300). `Main.tscn` → 20 km / spacing 12.
- Edits: `paint_terrain` rebuilds only dirty chunks that are currently meshed; out-of-range dirty
  chunks re-tessellate fresh (with the edit) when they next stream in.
- Note: at 12×12 with `RenderDistance=6` the whole world stays in range (progressive fill, no
  zoom-out gaps); render-distance freeing only bites on a larger grid or a lower `RenderDistance`.
- Camera (post-N2): editor-style nav — middle-drag orbit, Shift+middle pan, wheel zoom, right-drag
  freelook (mouse-look + WASD/QE fly, Shift faster, wheel = speed). Left-drag stays the sculpt tool.
  Streaming focus reads `OrbitCamera.FocusPoint` (camera→ground ray) instead of an orbit target.

**FINDING — gdext methods cannot be called from a background thread.** The original step-1 plan
(run `build()` on a C# `Task`, return via `CallDeferred`) does **not** work: Godot rejects the
cross-thread `_engine.Call("build", …)` with `ERROR: Bug, call error: #1337` (from
`ExceptionUtils.DebugCheckCallError`), the call is a no-op, and gen "completes" in ~18 ms with
`regions=0`. There is no C#-`Task`/`Thread`/`WorkerThreadPool` path — they're all non-main-thread, all
rejected. So gen runs on the main thread; the splash (rendered for a few frames first) covers the one
~5 s hitch. True off-thread gen would need an async core API (Rust-side thread inside a single gdext
call) — out of scope; revisit only if the hitch is unacceptable. `ThreadedGen` export removed.
1. ~~**Threaded gen** — run `DhceEngine.build(...)` on a C# `Task`.~~ **Not viable** — gdext rejects
   cross-thread `Call` (#1337; see FINDING above). Replaced by **splash-deferred main-thread gen**:
   show a "Generating…" splash in `_Ready`, then run `build()` from `_Process` after a few frames so
   the splash is visible during the (blocking) gen.
2. **Progressive tessellation** — create the N chunk `MeshInstance3D`s, then tessellate a few per
   frame in `_Process` instead of all in `_Ready` (removes the upload hitch; window fills in).
3. **Render distance** — `[Export] int RenderDistance`; mesh only chunks within that tile-distance of
   the camera, free far ones, re-evaluate as the camera moves. Add a core helper for chunk centers /
   `(gx,gy)` (or expose `chunk_grid()` + chunk bounds) so C# maps camera → visible tiles with no
   per-region loops.
4. Default the spike to **20 km** (Width/Height = 20000) once streaming lands; expose vertical scale
   as a world-height (km) setting.
Verify: window opens immediately on a 20 km world; tiles stream around the camera; edits stay ~10 ms;
the per-dab timing line still prints.

## Then  *(status refreshed 2026-06-15 — after the in-editor reshell, ADR 0005)*

> **Big shift since this roadmap was written:** the tool was rebuilt from a standalone F5 app into an
> **in-editor Godot plugin** ([ADR 0005](../adr/0005-in-editor-authoring-and-phased-physics.md);
> reshell plan [dhce-editor-reshell.md](dhce-editor-reshell.md)). Trait/biome/Region model is
> [ADR 0004](../adr/0004-biome-region-territory-model.md). So the N-phases below are re-cast for the
> in-editor tool.

- **N3 ✅ DONE** — the full authoring toolset, in-editor: sculpt/course/flood + trait/biome/Region
  brushes, brush + world + physics panels, per-trait editor, biome-driven shaping, natural
  transitions, data views, biome classifier, the **Region tier**, and **N3d scatter** (model-slot
  library + low-poly preview + real-mesh bake). See [n3-biome-traits.md](n3-biome-traits.md) +
  [n3-tooling-design.md](../specs/n3-tooling-design.md).
- **N4 — export pipeline — ✅ DONE.** The **per-Region level slicer**
  ([dhce-region-slicing.md](../specs/dhce-region-slicing.md)) bakes terrain `ArrayMesh` + trimesh
  collider + water + **per-slot scatter `MultiMesh`** into a `.tscn` per Region, writes the master
  `DhceWorldState` (R5 save/load) + a `regions.json` adjacency manifest. Cleanup (2026-06-15): baked
  meshes are **duplicated to embed inline** → each level `.tscn` is **self-contained** (no tool asset
  needed by the game — which also dissolves the cross-project-asset problem without touching the game
  repo); a per-slot **Bake-as-individual-instances** toggle (capped) for hand-editable cover; the
  export path defaults **tool-local** (`res://exports`), so the tool never writes into another project
  (the owner copies levels over by hand). Export **granularity** = per-Region (the slicing model is the
  answer). *(Optional later: a single merged-mesh-per-level mode — not needed now.)*
- **N5 — Volumetric features — ❌ NOT STARTED (the big forgotten one).** Authored caves / overhangs /
  tunnels carved into the heightfield (hybrid; local SDF / marching-cubes / CSG carve, merged on
  export). See [ADR 0003](../adr/0003-edit-streaming-and-godot-export.md) §5.
- **Later — ❌ outstanding bucket** — generation-streaming / LOD (only if a world outgrows RAM),
  **Atmospheric Fog**, **Scatter Tool polish**, the **scale bar** (context-aware cm→m→km readout) and
  zoomable-minimap LOD (n3-tooling-design §5). Plus the cross-repo **gates + game-side level loading**
  (gameplay phase, in the game repo) and the **reshell R6b** cleanup (retire the stale runtime app).

## Pending housekeeping
- Migrate the build-time license gate from `scripts/check-licenses.mjs` (Node) to `cargo-deny`.
- The frozen `dhce-wasm` should stay building (determinism cross-check) — don't delete it.
