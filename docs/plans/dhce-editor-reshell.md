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

- **Directional-brush fix (§7.6) — ✅ core done (2026-06-15).** The footprint brushes gate cells by a
  **3D sphere**: distance now includes the vertical `elev·exaggeration − hit_y` term, not just
  horizontal XZ, so the brush bites the surface under the cursor from any view angle (the flat 2-D
  cylinder is why it was wrong off-top-down). Implemented as transient `World` state set per stroke via
  `set_brush_sphere(hit_y, exaggeration)` (mirrors `set_view_mode`; `(0,0)` ⇒ flat 2-D, the default —
  so every existing test/call is unchanged) rather than threading two params through six signatures.
  **Applied to all six footprint brushes** (sculpt / liquid / course / trait / biome / region-traits),
  not just the four originally listed — consistent behaviour, same mechanical edit. Determinism-safe
  (multiply/add/sqrt). Core test `brush_sphere_bounds_the_footprint_vertically` green.
- **Viewport picking — ✅ done (2026-06-15).** `DhcePlugin` overrides `_Handles`/`_Edit`/`_MakeVisible`
  + `_Forward3DGuiInput(Camera3D, InputEvent) -> int` (the 4.6 C# binding returns `int`, not the
  `AfterGuiInput` enum the docs show — cast). A left-drag on a selected `DhceWorld` builds the ray from
  the **editor camera** (`ProjectRayOrigin`/`ProjectRayNormal`) → `raycast_terrain` → `ToolState.Apply`
  (which calls `set_brush_sphere(hit.Y, exaggeration)` first, so the runtime app's angled-camera brush
  is fixed too) → `RepaintDirtyTerrain`/`RebuildLiquid`. Returns `Stop` while painting, `Pass` on a
  miss so normal selection/navigation still works. `ToolState` is reused from the shared assembly (no
  copy). A minimal tool slice landed in the dock (tool `OptionButton` + radius/strength sliders) so the
  brush is exercisable now; the full panel parity is R4.
- **⏳ deferred to R4:** the brush gizmo (overlay `MeshInstance3D` ring under the cursor) for visual
  feedback, and the rest of the dock panels.

**Gate:** core test — a 3D-sphere brush at an angled hit touches a vertically-bounded footprint (not an
infinite column) ✅. `dotnet build` clean + headless editor smoke loads the plugin ✅. Visual: sculpt
lands under the cursor from a side view — **user editor check**.

### R3.5 — Fixed-size chunk streaming *(Rust core + C#; landed with R3 core, 2026-06-15)*
User-requested mid-reshell: chunks are now a **fixed physical size** (`DEFAULT_CHUNK_SIZE_M = 256 m`,
tunable via `set_chunk_size_m` / the `DhceWorld.ChunkSizeM` export) instead of being sized from the
region count (~12 k regions/tile → a handful of huge tiles). A 20 km world becomes ~78×78 small tiles;
`DhceWorld` **snaps** the world to a whole number of tiles (so 20 km → 19.968 km at 256 m) and creates
chunk nodes **lazily** — only the in-render-distance ring is instantiated, so the live node count
tracks the visible area (a few hundred) not the ~6 000 tile *slots*. `RenderDistance`/`ChunksPerFrame`
defaults retuned (8/8) for the smaller tiles. The runtime `CartographerSpike` (pre-creates every node;
retired at R6) is pinned to coarse `set_chunk_size_m(1600)` so the new default doesn't blow it up.
Rationale: finer, lighter, smoother show/hide as the camera moves. Core tests
`chunk_size_is_settable_and_resizes_the_grid` + the updated partition test green.

### R4 — Dock UI parity *(C#)*
**Files:** `addons/dhce/DhceDock.cs` (new, ported from `tool/scripts/ToolUi.cs`), `DhcePlugin.cs`.

- **R4a — ✅ done (2026-06-15).** `DhceDock` (a `ScrollContainer`, native editor controls — not the
  runtime `CanvasLayer`/parchment theme) ports the authoring panels and wires them to the **scene's
  `DhceWorld`** + the plugin's shared `ToolState`: TOOLS (sculpt toggle grid + generate-streams), BRUSH
  (radius m + strength m + liquid kind), WORLD (seed/octaves/size/spacing/**chunk size**/height +
  Generate), VIEW + contextual paint, REGIONS (stamp swatches), TRAIT BRUSH, REGION LANDFORM, SHAPING,
  TRANSITIONS, PALETTE, PHYSICS (sea/flow/evap/substeps/rain/settle/clear + Simulate). The plugin
  `Bind`s the dock to the current world each `_Process` and drives the Simulate tick (`SimTick`) so it
  runs in-editor. Brush **radius is in metres** (a slider), not the runtime's view-fraction dots —
  correct for a world-space editor brush (no per-dab camera-distance scaling).
  - **Divergences (review):** **no SUN panel** — editor lighting is the scene's own `WorldEnvironment`
    / `DirectionalLight3D` (engine-built-ins principle), not a DHCE-managed sun. **No tab split** — one
    scrolling column suits a narrow dock. Tool-toggle highlight can lag when a swatch/trait/view arms a
    different `ToolState.Active` (cosmetic; the active tool is still correct).
- **R4b — ✅ done (2026-06-15).** **MAP:** new `DhceMinimap` (decoupled from `CartographerSpike` —
  takes the engine + bounds + a focus point) renders the core overview, zoom/pan, and a live marker at
  the editor-camera ground focus (fed by the plugin each `_Process`); refreshed on gen / view-switch /
  shaping / blend / palette + a manual **Refresh map** button. Click-to-fly dropped (no editor-camera
  reposition API) — review. Especially useful now that 3D streaming only meshes a ~4 km ring. **Brush
  gizmo:** an `ImmediateMesh` ring (unshaded, no-depth-test) laid on the surface under the cursor,
  scaled to the brush radius, updated on hover via the same `raycast_terrain` (ephemeral child of the
  world, owner-less). Both visual — **user check**.

**Gate:** every tool/panel usable from the dock against an in-editor world (visual — **user check**);
`dotnet build` clean ✅ + headless editor smoke loads the plugin + dock with no script errors ✅.

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
