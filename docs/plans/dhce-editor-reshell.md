---
status: active
date: 2026-06-15
owner: wsscottca
---

# DHCE editor reshell — meticulous plan

**Goal:** Per [ADR 0005](../adr/0005-in-editor-authoring-and-phased-physics.md): turn DHCE from a
standalone runtime app (`Main.tscn` → `CartographerSpike`, F5) into an **in-editor Godot plugin**
(`addons/dhce/`, an `EditorPlugin`) that authors the world live in the editor viewport. The Rust core
+ GDExtension and every authoring system carry over unchanged; **only the C# shell is rewritten.**

**Persistence (decided):** a compact **`DhceWorldState`** `Resource` — gen params + the authored
per-cell fields — saved to a binary `.tres`/`.res`. The tool **regenerates** preview chunks from it on
open; the game regenerates the *identical* world from the same resource via `dhce-core`
(shared-core determinism, [ADR 0001](../adr/0001-shared-rust-core.md)). No mesh bake, no export step.

**Camera/picking (decided):** use the **editor's own 3D viewport camera** via
`EditorPlugin.forward_3d_gui_input(camera, event)` — no custom `OrbitCamera`. This also fixes the
**directional-brush bug** (brush wrong off-top-down): see R3 + the §7.6 3D-sphere falloff.

**Strategy:** build the addon **alongside** the working runtime app; reach parity stage by stage;
retire `Main.tscn`/`CartographerSpike` only at R6. Commit per stage. `dotnet build` + (where it
applies) a headless editor-load smoke gate each stage; visual gates are the user opening the editor.

---

## Persistence data model (`DhceWorldState`)

A C# `Resource` (`[GlobalClass] partial class DhceWorldState : Resource`) with `[Export]` fields:

- **Params:** `int Seed; float WorldSizeKm; float SpacingM; int Octaves; float TerrainHeightKm;`
- **Authored per-cell fields** (the result of all edits, applied after a deterministic `build()`):
  elevation (`float[]`), biome (`byte[]`), liquid depth (`float[]`) + kind (`byte[]`), course mask
  (`byte[]`), the six scalar trait fields (`float[]` each), vegetation + palette_family (`byte[]`),
  the 7 base palettes (`float[]`, 7×6×3), the region-landform table (`float[]`, 15×4).

**Load = `build(params)` then restore every field** (deterministic mesh from the params, then overwrite
the per-cell state). **Save = read the fields back out.** This needs core export/import for the trait
fields, base palettes, and region landform (elevation/biome/liquid/course already have them).

> **Core additions for R5:** `*_export()` / `set_*()` for the six scalar trait fields + the two enum
> fields + base palettes + region landform — OR a single versioned `serialize()`/`deserialize()`.
> Plan uses per-field (additive, mirrors the existing export/import; lower risk).
>
> **Known v1 limitation (review):** `shape_delta` is not persisted — re-running **Apply shaping**
> after a load re-bases from the loaded (already-shaped) elevation (one-time, not destructive). Store
> `shape_delta` too if this bites.

---

## Stages

### R1 — Addon scaffold + dev host *(C# only)*
**Files:** `addons/dhce/plugin.cfg`, `addons/dhce/DhcePlugin.cs` (the `EditorPlugin`), `tool/project.godot`
(enable the plugin).

- `plugin.cfg` (name/description/script = `DhcePlugin.cs`). `DhcePlugin : EditorPlugin` with
  `[Tool]`, `_EnterTree` adds a dock (`AddControlToDock`) holding a placeholder + a **Generate** button;
  `_ExitTree` removes it. The `DhceEngine` GDExtension is already at `addons/dhce/`.
- Enable `dhce` in `tool/project.godot` `[editor_plugins]`. Keep `Main.tscn` runnable for now.

**Gate:** `dotnet build` clean; opening `tool/` in the editor loads the plugin (dock appears, no
errors). Headless check: `--editor --quit-after 5` loads with no script errors.

### R2 — `DhceWorld` node + gen-on-demand *(C#)*
**Files:** `addons/dhce/DhceWorld.cs` (`[Tool] partial class DhceWorld : Node3D`), `DhcePlugin.cs`.

- `DhceWorld` owns a `DhceEngine`, exports the gen params, and on **Generate** runs `build()` on the
  main thread (editor will block ~5 s — show an `AcceptDialog`/progress, and default to a **coarser
  authoring spacing** so the editor stays light; a high-density "bake" is a later toggle). It creates
  preview chunk `MeshInstance3D` children (reuse `tessellate_chunk` + the streaming logic, ported from
  `CartographerSpike`) under itself, streamed around the **editor camera** focus in `_Process` (the
  plugin sets `set_process(true)` and feeds the editor camera position).
- Selecting a `DhceWorld` node shows the dock; the dock drives this node.

**Gate:** add a `DhceWorld` to a scene → Generate → terrain appears in the editor viewport; flying the
editor camera streams chunks. `dotnet build` clean.

### R3 — Viewport picking + brush, incl. the 3D-sphere (directional) fix *(Rust core + C#)*
**Files:** `crates/dhce-core/src/world.rs` (+ test), `crates/dhce-godot/src/lib.rs`,
`addons/dhce/DhcePlugin.cs`, `addons/dhce/ToolState.cs` (ported).

- `forward_3d_gui_input(Camera3D camera, InputEvent e)`: build the ray from the **editor camera**
  (`camera.ProjectRayOrigin/Normal`) → `raycast_terrain` → apply the active tool at the hit; return
  `AfterGuiInput.Stop` while painting so the editor doesn't also select/move. Brush gizmo drawn via an
  overlay `MeshInstance3D` (as today).
- **Directional-brush fix (§7.6):** make the core brush falloff a **3D sphere** — distance includes the
  vertical `(elev[r] − hit_elev) · exaggeration` term, not just horizontal XY. `paint_terrain` /
  `paint_course` / `paint_liquid` / `paint_trait` take the hit elevation + exaggeration and compute
  `d3²`. The brush then bites the surface under the cursor from **any** view angle (today's 2-D
  cylinder is why it's wrong off-top-down). Determinism-safe (multiply/add/sqrt).

**Gate:** core test — a 3D-sphere brush at an angled hit touches a vertically-bounded footprint (not an
infinite column). Visual: sculpt lands under the cursor from a side view. `cargo test` + smoke green.

### R4 — Dock UI parity *(C#)*
**Files:** `addons/dhce/ToolUi*.cs` (ported from `tool/scripts/ToolUi.cs`), `DhcePlugin.cs`.

- Port the panels into the editor dock: Terrain/Biomes tabs, brush size, tools, VIEW + contextual
  paint, REGION LANDFORM, SHAPING, TRANSITIONS, PHYSICS, SUN, PALETTE, MAP (minimap). Controls live in
  the dock (a `Control`), not a runtime `CanvasLayer`. Wire to the selected `DhceWorld`.

**Gate:** every tool/panel usable from the dock against an in-editor world. `dotnet build` clean.

### R5 — Persistence (`DhceWorldState`) *(Rust core + C#)*
**Files:** `crates/dhce-core/src/world.rs` (+ test) + `lib.rs` (trait/palette/landform export+import),
`addons/dhce/DhceWorldState.cs`, `DhceWorld.cs` (Save/Load), dock buttons.

- Add the core export/import listed above (with a round-trip test: export → new World → `build` +
  import → fields bit-identical). `DhceWorld` Save writes a `DhceWorldState`; Load (or `_Ready` in the
  editor) regenerates: `build(params)` then restore. Dock **Save / Load** buttons; the node references
  its `DhceWorldState` resource so reopening the project restores the world.

**Gate:** author → Save → reopen the project → world restored identically (spot-check elevation/biome
via the views). Core round-trip test green.

### R6 — Retire the runtime app + package *(C#)*
**Files:** `tool/Main.tscn`, `tool/project.godot`, `tool/scripts/` (remove runtime-only), `addons/dhce/`.

- Point `tool/` at a host scene that uses the addon (or drop `run/main_scene`); remove
  `CartographerSpike`'s runtime-only shell once the editor path is at parity (keep the ported chunk/
  liquid streaming + gen logic in the addon). Document "enable the DHCE plugin" so the **game** installs
  `addons/dhce/` and authors in its own editor.

**Gate:** `tool/` is a dev host with the addon enabled; a clean checkout opens + authors in-editor.

---

## Risks / gotchas
- **C# `EditorPlugin` quirks:** tool scripts must be built before enabling; the editor must reload the
  assembly after a `dotnet build`. The GDExtension is already editor-loaded. The DLL hot-swap rule
  (close/reopen the editor) still applies to `dhce_godot.dll`.
- **Editor blocking gen (~5 s):** on-demand only; coarser default authoring density; progress dialog.
- **Editor-camera streaming:** the plugin reads the editor viewport camera each `_Process` to drive the
  existing render-distance streaming.
- **Resource size:** full per-cell field arrays at 1.7 M cells ≈ tens of MB binary — fine on disk,
  out of the `.tscn` text. Delta/operation-log compression is a later option (review).

## After the reshell — adapt the remaining N3 phases (in-editor)
From [`n3-tooling-design.md`](../specs/n3-tooling-design.md), re-cast for the editor: **N3d scatter**
(per-Region rules + model-slot selection, MultiMesh preview), any remaining **N3c** biome-editor polish,
and the **§7.6 brush** (done in R3). Each becomes an editor-dock workflow rather than a runtime overlay.
