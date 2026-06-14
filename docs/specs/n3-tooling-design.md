---
status: active
date: 2026-06-14
owner: wsscottca
---

# N3 — authoring tools: umbrella design

Self-contained design for the N3 phase of the DHCE tool (see
[ADR 0003](../adr/0003-edit-streaming-and-godot-export.md) §Phasing and the
[roadmap](../plans/dhce-tool-roadmap.md)). N3 turns the N2 streaming shell into a usable
authoring tool: on-screen tools + world/physics controls, biome-driven terrain shaping with
natural transitions, a per-biome editor, and scatter-rule authoring.

This document is the **spec for all four slices**; it is decomposed into an ordered build
sequence in §6. Types and call sequences are inlined so it stands alone.

## 0. Context — where N2 left off

- The Rust core (`dhce_core::world::World`, via the `DhceEngine` GDExtension) **already exposes
  the full N3 authoring surface**: `paint_terrain` (raise/carve/level/crest), `paint_course`,
  `generate_streams`, `paint_liquid`, `paint_biome`, region selection, per-biome profile
  getters/setters (`set_biome_color/landform/water`), and `scatter`/`tessellate_scatter`. See
  [dhce-core-contract.md](dhce-core-contract.md).
- The tool (`tool/scripts/CartographerSpike.cs`) wires **exactly one**: left-drag →
  `paint_terrain(mode 0)` raise. Everything else is unreached.
- **One genuinely-new core capability is needed:** `biome_landform` is stored and editable but
  **never consumed** — no pass reads it to shape elevation. N3b builds that (plus blending).
- Units: **1 Godot unit = 1 m**; author-facing sizes in km (world/relief), m for fine tools
  (spacing/brush). C# converts to metres before any core call.

## 1. Decisions (locked in brainstorming)

- **One umbrella design**, four ordered slices, each its own plan → implement cycle.
- **UI** = on-screen `Control` overlay on a `CanvasLayer` (runtime tool via F5, not an editor dock).
- **Biome shaping** = an explicit, idempotent **Apply** pass over a layered elevation model;
  sculpts re-base on top and survive re-shaping.
- **Biome transitions** = deterministic **param-field diffusion** across mesh adjacency, one
  global *Transition width (m)* control; baked at Apply time.
- **Scatter** = author per-biome rules + model slots now, cheap proxy preview; bake `.glb`/
  MultiMesh in N4.
- Core stays **deterministic** (the cross-platform rule: shared generation math uses
  multiply/add/min/max/lerp only — no `pow`/`sin`/`exp`).

## 2. Code structure (refactor as we build)

`CartographerSpike.cs` is a ~315-line god-class (engine init + splash + streaming + input +
painting). N3 adds enough that we split by responsibility (done incrementally, starting in N3a):

- **`CartographerSpike.cs`** (root `Node3D`) — owns the `DhceEngine`, world gen/splash
  lifecycle, terrain chunk streaming + upload, and the new **liquid surface upload**. Public
  surface for the tools: `GodotObject Engine`, `RepaintChunks(int[] dirty)`, `RepaintAll()`,
  `Regenerate()`, world↔grid helpers, `Exaggeration`.
- **`ToolController.cs`** — active tool + brush params; routes left-drag → ground hit → the
  correct core brush call → `RepaintChunks(take_dirty_chunks())`.
- **`ToolUi.cs`** (+ `BiomeEditorPanel.cs`, `ScatterRulesPanel.cs`) — the `CanvasLayer` overlay.
  Controls use `MouseFilter = Stop`, so clicks on UI are consumed before `_UnhandledInput` and
  never paint terrain.
- **`OrbitCamera.cs`** — unchanged (editor-style nav already shipped in N2).

## 3. Slice specs

### N3a — tool shell + world/physics controls + liquid rendering (pure C#, no DLL rebuild)

On-screen overlay driving core ops that already exist.

**Brush tools** (left-drag applies the active tool at the ground hit; dirty-chunk repaint):

| Tool | Core call | Notes |
|---|---|---|
| Sculpt: Raise / Carve / Level / Crest | `paint_terrain(x, z, r, strength, mode 0–3)` | mode sub-buttons |
| River | `paint_course(x, z, r, intensity, kind)` | |
| Generate Streams (button) | `generate_streams(threshold, depth_gain)` | run after courses painted |
| Flood | `paint_liquid(x, z, r, amount, kind 0/1)` | needs liquid rendering (below) |
| Biome Paint | `paint_biome(x, z, r, id)` | biome picker (14); recolor via dirty chunks |

- **Brush params:** radius (m) + strength sliders; contextual extras (liquid kind, biome id).
  Shortcuts: `1–5` select tools, `[` / `]` brush size, mirroring editor muscle memory.

**World panel** — seed, octaves, world size (km), spacing (m), terrain height (km) + **Regenerate**.
- *Live* (no rebuild): terrain height → re-tessellate with new exaggeration; render distance;
  chunks-per-frame.
- *Regen-required* (rebuild via the existing splash-deferred `build()`; resets base/shaped/terrain
  — see §4): seed, octaves, world size, spacing. Regenerate warns that it discards edits.

**Physics panel** — the fluid sim, all existing core calls: **sea level** (`set_sea_level`),
**rain** (`rain`), **flow / evaporation / substeps** + **Settle** (`step_fluid`), **Clear**
(`clear_liquid`); optional **Simulate** toggle stepping each frame (liquid re-tessellate throttled).

**Liquid rendering (new):** the tool renders no water today; Flood *and* Physics both need it.
Add **one** `MeshInstance3D` for the liquid surface fed by `tessellate_liquid(exag)` +
`liquid_positions / liquid_normals / liquid_types / liquid_indices`, re-tessellated when liquid
changes or after a fluid step. Whole-surface (not chunked) for N3 — water is sparse; chunked
liquid is a deferred optimization if it bites.

**HUD:** active tool + region/triangle counts + the per-dab timing line, moved on-screen.

**Acceptance:** every tool paints; world settings + Regenerate work; flood + a Settle pass shows
moving/settling water; UI clicks never paint terrain; F5-only (no DLL rebuild).

### N3b — biome-driven shaping + natural transitions (Rust core + DLL rebuild)

The meaty slice — the only one touching the determinism contract.

**Layered elevation model.** Add two resident `Vec<f64>` fields beside the live `terrain`:

```
base            // raw generated elevation; set at build(), immutable until regen/seed change
shaped_baseline // last-applied shaped base; initialized == base at build (identity, no shaping)
terrain         // the live surface everything reads (render/streams/fluid); sculpts mutate this
```

**`apply_biome_shaping()`** (the Apply button) — idempotent, sculpt-preserving:

```
delta[r]        = terrain[r] - shaped_baseline[r]      // accumulated sculpt, as a height delta
new_shaped[r]   = shape(base[r], blended_param[r], detail_noise(r))
terrain[r]      = new_shaped[r] + delta[r]
shaped_baseline = new_shaped
→ mark all chunks dirty (whole-world re-tessellate)
```

Re-running with unchanged profiles/width is a no-op (`delta` unchanged → `new_shaped ==
shaped_baseline` → `terrain` unchanged). Biomes are the **input**: shaping never reclassifies
(that would oscillate). `build()` and Regenerate reset `base = generated`, `shaped_baseline =
base`, `terrain = base`.

**Natural transitions — param-field diffusion.** A hard per-region biome id would cut seams.
Before shaping, build per-region fields from each region's biome and smooth them over the mesh
adjacency:

```
param[r] = landform_profile[biome[r]]     // 4-vec: peak_shape, hill_amp, valley_floor, roughness
color[r] = biome_color[biome[r]]          // 3-vec
repeat k times (double-buffered):
    blended[r] = (param[r] + Σ param[neighbors(r)]) / (1 + degree(r))   // Laplacian average
k = round(transition_width_m / mean_region_spacing_m), clamped to a sane range
```

`shape(...)` reads `blended_param[r]`; tessellation reads `blended_color[r]`. `k` *is* the
transition width — one global *Transition width (m)* slider beside Apply. Diffusion is
deterministic and cheap for an explicit pass (≈ regions × avg-degree × k; sub-second at 1.7M / k≤~10).
Blending is **baked at Apply time** (and at build for the initial classification); live biome
painting shows hard color per dab until Apply, consistent with the explicit-bake model.

**`shape(e, [peak, hill, valley, rough], x, y)`** maps blended landform over normalized elevation
using only multiply/add/lerp/min/max (no `pow`/transcendentals). Intended math (constants tuned
during N3b):

```
s = e * hill                                  // 1. amplitude
if s >= 0:                                     // 2. peak_shape: sharpen↔round positive relief
    sharp = s*s*s                              //    pointier (s∈[0,1] ⇒ s³ < s)
    domed = s*(2.0 - s)                        //    rounder/plateau (s∈[0,1] ⇒ > s)
    t = peak - 1.0                             //    peak∈~[0.6,1.4] ⇒ t∈[-0.4,0.4]
    s = (t >= 0) ? lerp(s, sharp, t) : lerp(s, domed, -t)
if s < valley:                                 // 3. valley_floor: lift lows toward a target floor
    s = lerp(s, valley, VF_STRENGTH)
s += fbm2(x*DETAIL_FREQ, y*DETAIL_FREQ, seed, DETAIL_OCT) * rough * DETAIL_AMP   // 4. roughness
return s
```

`fbm2` is the core's deterministic simplex fBm (already used in gen). The wasm adapter is frozen
and does not need this; the native tool and native game share the same `dhce-godot` build, so
tool↔game output stays identical.

**New GDExtension surface:** `apply_biome_shaping() -> PackedInt32Array` (or `void` + caller
`RepaintAll`), `set_transition_width(meters: float)`, and likely `reset_shaping()`
(set `shaped_baseline = base`, re-add `delta`). N3c's editor drives the existing
`set_biome_landform/color/water`; the Apply button (in N3a's toolbar, wired here) bakes.

**Acceptance:** painting Volcanic next to Plains and hitting Apply produces a graded slope+color
transition, not a cliff; Apply is idempotent; sculpts survive Apply; `cargo test -p dhce-core`
green (new tests: idempotency, sculpt-preservation, diffusion convergence/symmetry).

### N3c — per-biome editor panel (C#, needs N3a UI + N3b core)

A panel over the 14 biomes; select one → edit:

- **Color** (`set_biome_color`) → recolor (mark that biome's chunks dirty / repaint).
- **Landform** sliders (`set_biome_landform`, idx 0–3) → take effect on **Apply shaping**.
- **Water** sliders (`set_biome_water`, idx 0–4) → feed the fluid sim.
- **Region select** (`region_at` / `select_contiguous` / `selection_indices`) → click terrain to
  target a biome, bulk-reassign via `set_biome_of`, highlight the selection.
- The **Transition width** slider + **Apply shaping** button live here (or shared with the toolbar).

**Acceptance:** editing a biome's color recolors its regions; editing landform + Apply reshapes
only that biome's territory (with N3b transitions at its borders); region-select reassigns biomes.

### N3d — scatter rules + model slots + proxy preview (C#, possibly a small core add)

- **Per-biome scatter rules** (model-slot, density, slope/elevation limits) authored in the biome
  editor and saved with the project.
  **Caveat:** the core today exposes only `scatter(seed, …, density)` with species 0 tree / 1 rock
  — the rich per-biome rule schema ADR 0003 references is not in the contract. N3d may need a
  small core `scatter` extension (per-biome density + slope/elev gating); decided at N3d's plan.
- **Model slots:** owner picks `.glb` paths per slot (tool bundles no art — records paths only).
- **Proxy preview:** one capped `MultiMesh` of small markers from the existing `scatter()`
  output, colored per species, toggleable. Real `.glb`/MultiMesh bake is N4.

**Acceptance:** per-biome rules + model paths persist; proxy markers show density and respond to
rule changes.

## 4. Determinism notes

- All N3b math (shaping + diffusion) stays within the shared-math rule (multiply/add/lerp/min/max
  + the existing deterministic `fbm2`); **no `pow`/`sin`/`exp`** in the shared path.
- The layered model keeps `base` immutable, so the same seed reproduces the same `base`; the same
  profiles + transition width reproduce the same `shaped_baseline`; sculpts are a recorded delta.
  ⇒ a saved authoring project reproduces exactly (tool↔game, ADR 0001/0002 unchanged).
- The frozen `dhce-wasm` adapter does not gain N3b; it remains the gen/sim determinism cross-check.

## 5. Open items

- `paint_biome` / `paint_liquid` dirty-chunk behaviour — confirm both mark chunks dirty (or add it)
  so the existing repaint path covers recolor + flood. *(verify at N3a)*
- Whole-surface vs chunked liquid at 20 km — start whole-surface, measure. *(N3a)*
- Exact `shape()` constants (`VF_STRENGTH`, `DETAIL_FREQ/OCT/AMP`) — tune live. *(N3b)*
- Transition-width → `k` mapping and the diffusion kernel (uniform vs degree-weighted). *(N3b)*
- Whether N3d needs the core `scatter` extension or tool-side rules suffice for proxy preview. *(N3d)*
- Authoring-project save/load of biome profiles + scatter rules formally lands in N4; N3 keeps
  them resident + exportable via the existing `*_export`/`set_*` calls.

## 6. Build sequence (ordered)

1. **N3a** — tool shell + brushes + World/Physics panels + liquid rendering + the CartographerSpike
   split. *(C#, no DLL)*
2. **N3b** — layered elevation + `apply_biome_shaping` + param-field diffusion + Transition-width;
   wire the Apply button. *(Rust + DLL rebuild)*
3. **N3c** — per-biome editor (color/landform/water/select). *(C#; needs N3a UI + N3b core)*
4. **N3d** — scatter rules + model slots + proxy preview. *(C#; needs N3c panel)*

Each slice gets its own implementation plan (`docs/plans/n3X-*.md`) when reached; N3a is planned
first. Commit + push per slice; `cargo test -p dhce-core` + `dotnet build` green at each boundary.
