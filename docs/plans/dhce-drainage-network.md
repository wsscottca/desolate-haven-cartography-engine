---
status: draft
date: 2026-06-16
owner: wsscottca
---

# Handoff — #3: One drainage network (rivers ⇄ lakes ⇄ ocean)

> Self-contained handoff for a **fresh session**. Assume no memory of the prior work. Read the
> "Current state" section first — most of what #3 needs already exists and just needs wiring.

## Goal & "feel right" intent

Make the map read as a single **watershed**: every drop flows downhill to the sea, **rivers flow into
lakes (inlets) and out of them at their pour point (outlets)**, and lake outflow continues downstream
to the next basin or the ocean. Today the three water bodies — ocean, lakes, rivers — exist but don't
know about each other. #3 connects them into one coherent drainage system. This is the third in a
series of "tie the systems together" changes (after: canon region→traits→biome→elevation→water, and
the climate links elevation→temperature / orographic→moisture).

The key realization: **the drainage network already exists implicitly.** The priority-flood +
steepest-descent receiver graph in `streams.rs` routes every cell downhill to the ocean *through*
filled depressions (lakes). #3 is mostly about (a) generating trunk rivers from flow accumulation
automatically (not only from hand-painted seeds), and (b) making lake inlets/outlets explicit so flow
visibly passes through lakes.

## Repository facts

- **Branch:** `feat/dhce-engine-batch-5-8` (the canon-map + climate work landed here; not yet PR'd).
- **Build/test (Windows / PowerShell):**
  - Core tests: `cargo test -p dhce-core -j 1`. **Use `-j 1`** and first clear stale test binaries:
    `Remove-Item 'C:\Users\WSSco\.dhce-build\debug\deps\*.exe' -Force -ErrorAction SilentlyContinue`.
    Reason: parallel-linked test exes trip Windows Defender → `LINK : fatal error LNK1104: cannot open
    file …exe`. It's an environment lock, **not** a code error — the lib itself compiles fine.
  - GDExtension DLL: `cd crates/dhce-godot; cargo build --release -j 1`, then copy
    `C:\Users\WSSco\.dhce-build\release\dhce_godot.dll` → `tool/addons/dhce/dhce_godot.dll`.
  - **C# can't be compiled headlessly** (no `.csproj` until Godot generates one on open). Match the
    existing idioms by review: `_engine.Call("method", args)`, `.As<float[]>()`/`.As<double>()`,
    the dock `Slider(label,min,max,step,val,Action<double>)` / `Header(title, open)` / `Dim(text)` /
    `Button(parent,text,Action)` helpers, `Mathf.*`.
- Determinism contract (enforced by tests): no transcendentals (`sin`/`cos`/`exp`) in the per-cell
  hot path; integer wrapping only; parallel (`util::par_map`) output must equal serial; total-order
  sorts. `streams.rs` already honors this (total-order priority queue keyed on `(elevation, index)`).

## Current state — the water systems (with the reusable primitives)

All in `crates/dhce-core/src/`:

**Ocean** — `fluid.rs::sea_fill(field, terrain, level)`: fills every cell below `level` to `level`.
Driven by `World::set_sea_level` (world.rs). The C# `DhceWorld.OnGenDone` sets the ocean level to
`min_elevation + 1 km` (normalized) at Generate.

**Lakes (DONE, this is the model to extend)** — `World::fill_lakes()` (world.rs): perched pour-point
lakes. It calls **`streams::fill_depressions(terrain, neighbors, num_boundary) -> Vec<f64>`** (a
`pub(crate)` priority-flood / Planchon–Darboux — the elevation water rises to before draining to the
boundary frame). `filled[r] - terrain[r]` is the lake depth that fills cell `r`'s closed basin to its
**pour point**. Deposits standing water where `filled > sea_level` and the depth exceeds a per-region
threshold (`region_lake_depth[region]`, moisture-tied default + slider, modulated by local moisture).
It is a **static** fill (renders, doesn't wake the relax sim — a pour-point lake with no inflow would
otherwise drain over its spill). The C# `DhceWorld` calls `fill_lakes` after `set_sea_level`.

**Rivers/streams (the other half of what #3 needs)** — `streams.rs::accumulate(terrain, neighbors,
num_boundary, seed_mask, threshold, depth_gain) -> StreamResult { flow, is_stream, carve_delta }`:
1. `fill_depressions` (same priority-flood as lakes).
2. **Steepest-descent receiver** per cell (its lowest *filled* neighbour) — `receiver[]`. **This IS
   the drainage graph**: follow `receiver` from any cell and you reach the ocean boundary, passing
   through filled basins (lakes).
3. **Flow accumulation** (each cell contributes unit catchment downstream) — `flow[]`.
4. **Catchment gate** — currently streams only manifest in cells whose flow path reaches a *seeded*
   (hand-painted) main river (`seed_mask`). **#3 wants to drop/relax this gate** so trunk rivers form
   from flow alone.
5. Stream + carve — cells past a flow threshold get `is_stream=true` and a `carve_delta` (channel
   depth ∝ √flow).
   Exposed via `World::generate_streams(threshold, depth_gain)` (world.rs ~line 690), which carves
   into `elevation_r`, records the lowering in `stream_carve` (idempotent restore-then-recarve), lays
   thin water, and marks liquid changed. The **River/Course paint tool** (`World::paint_course`,
   `course_mask`) seeds the main rivers today.

**Climate (just landed — the tie for "bigger rivers where it rains")** — `rainfall.rs::compute_rainfall
(elevation, moisture, temperature, region, biome_water, positions, neighbors, sea_level, wind)
-> Vec<f64>` returns a per-cell rainfall field (windward-wet / lee-dry). `World::derive_climate`
already produces physical `moisture_r`/`temperature_r`. The build pipeline order (in `World::build`):
mesh → region layout → region-guided elevation → `sea_fill` → trait seed → **`derive_climate`** (Pass
3b) → grid/chunks → `blend_traits`. `fill_lakes` and any river pass run *after* build (C#-driven) or
should be added to the pipeline.

## Proposed design

### Stage 1 — Drainage solve (refactor + reuse `streams.rs`)
Extract the receiver-graph + flow-accumulation core of `accumulate` into a reusable function that does
**not** require a painted `seed_mask`, e.g.:
```
pub(crate) fn drainage(terrain, neighbors, num_boundary, weight: Option<&[f64]>)
    -> Drainage { filled, receiver: Vec<usize>, flow: Vec<f64> }
```
- `filled` / `receiver` / `flow` are exactly stages 1–3 of today's `accumulate`.
- `weight`: per-cell flow contribution. `None` = unit catchment (today's behaviour). **Pass the
  rainfall field** (`rainfall::compute_rainfall`, or a normalized `moisture_r`) to make rivers bigger
  where it rains more — the tie back to climate #1/#2. This is the recommended "feel right" default.
- Keep `accumulate` working (reimplement it on top of `drainage` + the existing catchment gate) so the
  painted-Course tool still works.

### Stage 2 — Auto trunk rivers
`World::generate_rivers(threshold, depth_gain, rainfall_weighted: bool)` (new; or generalize
`generate_streams`):
- Run `drainage` (weighted by rainfall if requested).
- River cells = land cells (`elevation > sea_level`) with `flow >= threshold·max_flow`, **plus** any
  painted `course_mask` (so hand-authored rivers always render).
- Carve channels (reuse the `carve_delta = MIN_STREAM_DEPTH + depth_gain·√(flow/max)` logic) into
  `elevation_r`, recording into `stream_carve` for idempotent re-runs.
- Lay thin water along river cells (as `generate_streams` does today).
- `threshold` is the river-density knob → expose as a slider (see UI below); consider a per-region
  variant later (some regions rivier than others), mirroring the per-region lake-depth pattern.

### Stage 3 — Lake inlets & outlets (the actual "connect" step)
Because `receiver` is built on the **filled** DEM, flow already routes *into* a lake (inlets, where
upstream catchment converges on lake cells) and *out* at the **spill cell** (the lake cell whose
receiver leaves the basin — the pour point). To make this read correctly:
- **Outlet:** ensure the spill cell and the chain below it are river cells carrying the lake's
  accumulated flow (they will be if their `flow ≥ threshold`; force the outlet to be a river so even a
  modest lake has a visible outflow). Optionally widen/deepen the outlet channel.
- **Inlet:** the highest-flow river entering the lake is the main inlet; optionally widen it / mark a
  small delta (Marsh-style wet cells) where it meets the lake.
- Identify each lake's spill cell from `fill_depressions`: a lake cell `r` whose `receiver[r]` is a
  *non-lake* cell at the pour-point elevation. (A small helper over `filled`/`receiver`/the lake mask.)

### Stage 4 — Ordering (the one real subtlety)
River carving **lowers terrain**, which changes pour points and thus lakes. Resolve by ordering:
1. `sea_fill` (ocean).
2. `derive_climate` (already in build) → rainfall field available.
3. **Drainage solve + carve rivers** (Stage 1–2) on the climate'd elevation.
4. **`fill_lakes`** on the **carved** terrain → lakes now sit behind/around the carved channels and
   drain through the carved outlet.
5. (Re-)mark the lake outlet chain as river so outflow is continuous.
Carving is idempotent (`stream_carve` restore-then-recarve), so a re-run from an edited terrain is
safe. Decide whether rivers run inside `build` by default (recommended for "out of the box"
watersheds) or stay C#-driven like `fill_lakes`; if in `build`, run them before the final
`blend_traits`/chunk bake. Note the C# Generate flow currently does `set_sea_level` → `fill_lakes`
*after* build — if rivers move into build, re-check that order so lakes fill on the carved terrain.

## API / files to touch

- `crates/dhce-core/src/streams.rs` — `drainage()` (extract receiver+flow), keep `accumulate` on top
  of it; a `lake_spill_cells` helper.
- `crates/dhce-core/src/world.rs` — `generate_rivers(...)` (or generalize `generate_streams`);
  integrate Stage 3/4; a tunable `river_threshold` field (+ getter/setter) like the climate/lake
  params; persist it. Reuse `stream_carve`, `course_mask`, `fill_lakes`, `mark_all_liquid_changed`.
- `crates/dhce-core/src/rainfall.rs` — already returns the rainfall field; reuse for flow weighting.
- `crates/dhce-godot/src/lib.rs` — `#[func]` `generate_rivers` + `set_river_threshold`/getter (mirror
  the existing `fill_lakes`, `set_lapse_rate`, etc. wrappers; null the cached liquid/chunk surfaces).
- `tool/addons/dhce/DhceWorld.cs` — call rivers in the Generate flow (after climate, with the
  carve→`fill_lakes` order); a `RiverThreshold` export; `RepaintDirtyTerrain` + `RebuildLiquid` after.
- `tool/addons/dhce/DhceDock.cs` — a **RIVERS** dock section (slider for river threshold; "Generate
  rivers" button) mirroring the CLIMATE / lake sections. Keep the existing River/Course paint tool.
- `tool/addons/dhce/DhceWorldState.cs` — persist `RiverThreshold` (+ note: carved terrain + river
  water already persist via the existing Elevation/LiquidDepth fields).

## Determinism
- Reuse `streams.rs`'s total-order priority queue + `(elevation, index)` tie-break; receiver tie-break
  by lowest index; flow accumulation in descending-filled order. All already contract-safe.
- Flow weighting by the rainfall field is deterministic (the field is). Avoid floats-as-map-keys; keep
  the existing `f64::total_cmp` ordering.
- Gate with a **build-twice / thread-invariance** equality check on the carved `elevation_r` + the
  river `is_stream` mask (cf. `tests/world.rs::streams_are_idempotent_after_a_course` and
  `build_is_invariant_to_thread_count`).

## Tests (cargo, `-j 1`)
- `drainage`: on a synthetic bowl-with-spill mesh, flow accumulates downhill, the spill cell carries
  the basin's total, and the receiver chain reaches the boundary (extend the existing
  `streams::tests::fill_depressions_ponds_a_basin_to_its_pour_point`).
- Auto rivers: a sloped map yields river cells along high-flow paths without any painted seed;
  rainfall-weighting makes a wet-region valley rivier than a dry one of equal catchment.
- Lake connectivity: build a basin below a higher basin; confirm the lower lake has an outlet river
  whose flow ≥ the lake's inflow, reaching the ocean.
- Idempotency: `generate_rivers` twice → bit-identical `elevation_r`.
- Existing tests stay green (the canon `tests/world.rs` perched-lake test, climate tests, etc.).

## Verification (in-editor, needs Godot)
Generate → rivers descend ridgelines, **flow into lakes and out the far side**, and reach the coast;
no rivers running uphill or dead-ending on land above sea level. VIEW=Moisture should correlate with
river density if rainfall-weighted. Drag the river-threshold slider → more/fewer tributaries live.
Save → reopen → carved terrain + rivers + lakes round-trip.

## Open questions for the new session (resolve with the user or pick sensible defaults)
1. **Rainfall-weighted flow** on by default? (Recommended — it's the climate tie and the main "feel
   right" gain. Default `true`, with a toggle.)
2. **Auto rivers in `build`** vs a C#-triggered pass like `fill_lakes`? (Recommended: in `build` so a
   fresh Generate has a complete watershed; keep a "Generate rivers" button for re-runs.)
3. River threshold **global vs per-region** (per-region mirrors the lake-depth slider pattern; global
   is simpler — start global, add per-region later if wanted).
4. Visual style of rivers/outlets — thin carved channels + water ribbons (reuse `generate_streams`
   rendering) vs anything richer (deltas, braided channels). Start with the existing channel render.

## Pointers / prior context
- Plan/spec history: this repo's `docs/plans/` (and the prior session's plan file at
  `C:\Users\WSSco\.claude\plans\i-want-to-change-linear-knuth.md`, which held the canon-map and climate
  plans).
- Vault raw notes from the prior session (run `/wiki-ingest` to promote): the LNK1104 toolchain gotcha
  and the canon-generation architecture note under `C:\dev\vault\_raw\2026-06-16-dhce-*.md`.
- Lore: `..\..\web\desolate-haven-guide\lore\geography.md` — the canon "northern inflow forks into the
  Great Lake, river out to the SE toward the Open Plains" is exactly the inlet/outlet behaviour #3
  should make emerge.
