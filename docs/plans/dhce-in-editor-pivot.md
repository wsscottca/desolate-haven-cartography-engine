---
status: draft
date: 2026-06-15
owner: wsscottca
---

# DHCE in-editor pivot + phased build (roadmap)

**Goal:** Per [ADR 0005](../adr/0005-in-editor-authoring-and-phased-physics.md): turn DHCE into an
in-editor authoring **addon** (Rust core unchanged), keep terraform **live throughout** development,
with **phased physics** (heightfield water now → rapier for gameplay).

**Architecture:** three tracks. **Track 1 (editor-plugin reshell)** is the headline. **Track 2 (water
perf)** is shell-independent and makes live terraform responsive. **Track 3 (rapier)** is the gameplay
phase. Determinism contract unchanged ([ADR 0001](../adr/0001-shared-rust-core.md)).

**Carries over unchanged:** all of `dhce-core`, the `dhce-godot` GDExtension API, and every authoring
system (traits / biomes / shaping / data-views / hydrology). Only the C# shell changes.

---

## Track 1 — Editor-plugin reshell *(headline)*

> **Detailed staged plan: [dhce-editor-reshell.md](dhce-editor-reshell.md)** (R1–R6, persistence model,
> the §7.6 directional-brush fix, gates). Persistence = a compact `DhceWorldState` resource the tool +
> game both regenerate from. The summary below is superseded by that doc.

**Files:** `addons/dhce/` (the shipped addon: `plugin.cfg`, EditorPlugin + scripts moved from
`tool/scripts/`), `tool/` (becomes the dev/test host project with the addon enabled).

- **1.1 Addon scaffold.** `plugin.cfg` + a C# `EditorPlugin` (`[Tool]`) registering a dock (or 3D
  toolbar). Enable it in `tool/` (the dev host). The GDExtension stays at `addons/dhce/dhce_godot.dll`.
- **1.2 World node + gen-on-demand.** A `DhceWorld` `[Tool] Node3D` owning the `DhceEngine`, with a
  dock **Generate** button (no auto `_Ready` gen). Gen runs on click; result chunk meshes are children
  of the node in the edited scene. Params (seed / size / spacing / height) persist on the node.
- **1.3 Viewport authoring.** Move brush/sculpt/trait/region interaction from runtime
  `_UnhandledInput` to `EditorPlugin.forward_3d_gui_input`. Reuse `ToolState.Apply`. The dock hosts the
  panels currently in `ToolUi` (tabs, brush size, cover brush, VIEW + contextual paint, REGION
  LANDFORM, SHAPING, TRANSITIONS, PHYSICS, SUN, PALETTE, MAP).
- **1.4 Editor perf / density.** Default to a coarser authoring density (larger spacing) for
  interactive editing; add a high-density **bake** for final. Keep chunk streaming on the editor
  process tick; gen stays on the main thread (gdext can't be called off-thread — unchanged).
- **1.5 Persist / restore.** Save the authored world (elevation / biome / trait fields / liquid +
  params) so reopening the project restores it; regenerate deterministically from params + stored
  edits (reuses the existing save/load exports).
- **1.6 Package as a drop-in addon.** Document "enable the DHCE plugin" so it installs into the game
  project and future projects unchanged.

**Gate per stage:** addon enables with no errors; Generate works; viewport brush edits land; editor
stays responsive; `dotnet build` + headless smoke where applicable.

## Track 2 — Water performance *(Rust core + C#; shell-independent — do anytime)*

Makes live terraform water responsive. Today `RebuildLiquid` re-tessellates + re-uploads the **whole**
liquid surface every tick/edit (liquid is one un-chunked mesh).

- **2.1 Chunked liquid tessellation** *(biggest win)* — ✅ done (2026-06-15). `liquid_chunk_surface`
  packs only a chunk's wet triangles from a lazily-recomputed smoothed-surface cache; liquid edits
  flag liquid-dirty chunks (`mark_liquid_changed` / `mark_all_liquid_changed`); C# streams per-chunk
  liquid meshes alongside terrain and re-tessellates only dirty + in-range ones. Whole-surface
  `liquid_surface` retained for export only.
- **2.2 Active-set (sleeping) relaxation** — ✅ done (2026-06-15). `fluid::relax_step_active` iterates
  only the active set (wet + not settled); `step_fluid` rebuilds it each step from the regions that
  moved (> `SETTLE_EPS`) ∪ their neighbours, flags only those chunks, and stops when it drains. Edits
  wake their footprint (`mark_liquid_changed`); sea/rain/clear/streams wake all wet
  (`mark_all_liquid_changed`). Mass conserved (same anti-overshoot math as `relax_step`, kept for the
  fluid tests). Note: full convergence to the sleep threshold is solver-bound (slow spatial modes);
  the win is that *level/idle* water sleeps and the frontier shrinks.
- **2.3 Incremental liquid cache** — ⏸ deferred (decision below). 2.1+2.2+2.4 delivered the perf; the
  residual whole-map smooth is a secondary, Simulate-only cost, and both viable optimizations
  (incremental stencil, cfg-gated rayon) carry risk disproportionate to it right before the reshell
  re-baselines performance.
- **2.4 Idle skip** — ✅ done (2026-06-15). `liquid_active_count()` exposed; the Simulate tick skips
  entirely when it's 0 (settled). Per-edit/tick uploads were already dirty-driven (2.1).

> **Decision (review):** 2.3 re-scoped from "deterministic rayon parallelism" to **incremental liquid
> cache**. Rayon in `dhce-core` would break the frozen `dhce-wasm` cross-check (ADR 0001/0002 — wasm32
> has no threads) without cfg-gating, and after 2.1+2.2 the residual hot cost is the whole-map
> `ensure_liquid_cache` smooth, which an incremental (changed-region halo) recompute targets directly
> and with no new deps. Rayon stays in the bank if a native-only parallel pass is later wanted.

**Gate:** `cargo test -p dhce-core` green (incl. a determinism check: parallel == serial); settling no
longer hitches at 1.7 M cells.

### Later — water rendering tiers *(idea, not scheduled)*
Decouple water *rendering* fidelity from the *sim*: by default show **faux water** (a cheap shader
plane / 2-D trick) for authoring, and reserve a **high-fidelity "fly-around" render mode** (the full
simulated/animated surface) for showcase passes. Keeps the editor light while still allowing a
beauty pass. Revisit after the sim is chunked + fast (above).

## Track 3 — Rapier (gameplay phase) *(game project; when gameplay starts)*

**Files:** `..\desolate-haven` (greenfield) + a collider export from `dhce-core`.

- **3.1** Install `godot-rapier-physics` in the game; select the rapier `PhysicsServer3D` (server-level
  swap — works with C#; standard physics nodes unchanged). Pin the version.
- **3.2** Export the authored terrain as a **Rapier heightfield collider** (grid of heights + cell
  size + origin) so characters/props collide with the canon terrain; round-trip test vs `height_at`.
- **3.3** *(optional, later)* Salva 3D for **local** in-game water effects only (waterfall / pond) —
  never the terrain hydrology.

**Gate:** game runs with rapier as the physics server; a body rests on the authored terrain collider.

---

## Sequencing
Environment-first. **Track 2** can start immediately (shell-independent, fixes the live pain).
**Track 1** is the larger effort and the headline. **Track 3** waits for the gameplay phase.

## Out of scope
- Replacing the heightfield hydrology with SPH (rejected — ADR 0005).
- Rapier inside `dhce-core` (it stays at the Godot server layer in the game).
- The old standalone-app export pipeline (removed by the pivot).
