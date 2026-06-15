---
status: active
date: 2026-06-15
owner: wsscottca
---

# N3 — Trait-composition world model (implementation plan)

**Goal:** Build the trait-field world model from
[ADR 0004](../adr/0004-biome-region-territory-model.md): 9 paintable trait fields → an emergent
biome classifier → named Regions → Territory overrides, with natural per-trait border transitions.

**Architecture:** Bottom-up in 5 stages, each shippable and visible on its own. Rust core
(`dhce-core`) owns the fields + blend + classifier; C# (`tool/`) owns the brush/border/editor UI.
Determinism-safe throughout (lerp / averaging / threshold + existing `fbm2`).

**Carries forward from the shipped palette slice:** the `ground_color` elevation ramp, the per-cell
colour path (`region_color`/`after_edit`/`color_cache`), and neighbour smoothing.

---

## Stage 1 — Trait substrate *(Rust core + DLL rebuild)* — ✅ done (2026-06-14)

**Files:** `crates/dhce-core/src/biomes.rs`, `crates/dhce-core/src/world.rs`,
`crates/dhce-core/tests/biomes.rs`, `crates/dhce-core/tests/world.rs`.

1. **Base palettes (7) + vegetation in `biomes.rs`.**
   - `enum Vegetation { Barren, Grass, Scrub, Forest, Evergreen, Marsh, Thorn }` (+ `u8` round-trip).
   - `enum PaletteFamily { Verdant, Arid, Stone, Ashen, Frost, Wetland, Exotic }` (7).
   - `struct BasePalette { water_deep, water_shallow, low, rock, cap_warm, cap_cold }` (each `[f32;3]`,
     from `tokens.css`), and `fn base_palettes() -> [BasePalette; 7]`.
   - `fn vegetation_tint(v: Vegetation) -> ([f32;3], f32)` — cover colour + tint strength (Barren = 0).
   - Replace `ground_color` with
     `fn resolve_color(base: &BasePalette, veg: Vegetation, e: f32, temperature: f32, moisture: f32) -> [f32;3]`:
     water ramp below sea; on land, `cover = lerp(base.low, veg_tint, strength)`; lift `→ rock` at
     treeline; high band `→ lerp(cap_warm, cap_cold, temperature)` (so snow follows *temperature*,
     not just altitude). Lerp/clamp only.

2. **Trait fields in `world.rs`.** Add resident `Vec`s beside `elevation_r`/`moisture_r`:
   `jaggedness_r, relief_r, foothill_falloff_r, erosion_r, temperature_r` (`Vec<f64>`),
   `vegetation_r` (`Vec<u8>`), `palette_family_r` (`Vec<u8>`). Seed all at `build()` from the
   existing auto-classification (map each classified biome → a default trait preset, incl. base
   palette + vegetation + temperature) so a fresh world looks sensible. Keep `biomes::classify`
   for that seeding.

3. **Render via traits.** `cell_color(r)` reads the trait fields → `resolve_color(...)`. `region_color`
   + `after_edit` unchanged otherwise (smoothing + jitter stay).

**Tests:** 7 base palettes populated + distinct; `resolve_color` puts snow at high *cold* and not at
high *hot*; vegetation tints the cover; deterministic; `world` colours vary and lift with elevation
(adapt the two shipped colour tests to the trait path).

**Gate:** `cargo test -p dhce-core` green; DLL rebuilt + swapped (user closes Godot); headless
smoke clean; user F5.

## Stage 2 — Biome classifier *(Rust core + DLL)* — ✅ done (2026-06-15, in-editor)

**Files:** `biomes.rs` (+ test), `world.rs`, `crates/dhce-godot/src/lib.rs`, `addons/dhce/DhceDock.cs`,
`addons/dhce/DhcePlugin.cs`.

- `biomes::biome_label(elevation, jaggedness, relief, temperature, moisture, vegetation) -> String`
  composes a descriptor from coarse buckets — **climate** (temperature × moisture) · **cover**
  (vegetation) · **landform** (elevation + jaggedness + relief), with a water phrase when submerged
  (e.g. "temperate forest hills", "frozen jagged peaks", "arid scrub flats", "frozen deep water").
  Threshold-only (determinism-safe), off the render path. It is mechanical *description* generated
  from the dials — **not** canon place lore (the 14 named places are Regions; lore lives in the guide).
- `World::biome_label_at(x, y)` + GDExtension `biome_label_at(x, y) -> GString`.
- In-editor surface (adapted from "HUD"): the dock shows a live **"Biome: …"** readout under the
  cursor, fed by the plugin's existing hover raycast (the brush-gizmo cast) — no extra picking.

> **Deferred (review):** locked-label pinning + per-biome counts weren't needed for the readout; the
> emergent label is recomputed from live traits each hover. Colouring `VIEW_BIOME` by the classified
> label (vs the current Region-accent view) can follow if wanted.

**Tests:** `biome_label_describes_the_trait_composition` (representative bundles → expected words).

## Stage 3 — Brush + transition buffer + blend pass *(Rust core + C#)* — ✅ done (2026-06-14)

**Files:** `world.rs` (+ test), `lib.rs`, `tool/scripts/ToolUi.cs`.

- `paint_trait` / `paint_region_traits` now keep a **painted-base** snapshot (`*_base`) of the six
  scalar fields in step with the live `*_r` fields.
- `blend_traits(transition_width_m)` — `width → k` passes of Laplacian (`diffuse_field`) over
  `neighbors`, always **from the painted base**, so it's idempotent (re-run never compounds) and a
  uniform region is a fixed point. Whole-field diffusion makes the transition buffer *emergent*
  (interiors don't drift) — no explicit discontinuity band to track. Enums (vegetation,
  palette_family) keep their painted values; only the six scalars grade. Recolours + flags all
  chunks dirty (same refresh path as the palette editor).
- C#: Biomes-tab **TRANSITIONS** section — a Width (m) slider + a **Blend borders** button calling
  `blend_traits`, then re-tessellating via `RepaintDirtyTerrain` + minimap refresh.

**Tests (world.rs):** `blend_traits_is_idempotent_on_rerun`, `blend_traits_grades_a_painted_border`
(temperature cold→hot across the seam), `blend_traits_leaves_a_uniform_field_unchanged`.

> **Visible scope:** the blend grades *every* scalar (including the landform dials), but only
> temperature/moisture currently change anything on screen (colour). Jaggedness/relief/foothill/
> erosion now grade in the field, yet stay invisible until a **shaping pass** consumes them to
> perturb elevation — that's the natural next slice (no plan stage owns it yet).

## Stage 3b — Data-layer view modes *(Rust core + C#)* — ✅ done (2026-06-14)

Toggleable **views** that recolour the same meshes (and the minimap) by a single field, so a data
layer can be read and painted directly — decoupling authoring legibility from the composed Natural
look. (Added in response to "can't really see moisture": a dedicated Moisture view carries the
legibility, so the Natural view stays realistic.)

**Files:** `biomes.rs`, `world.rs` (+ test), `lib.rs`, `CartographerSpike.cs`, `ToolUi.cs`.

- Core: `view_mode` (`VIEW_NATURAL|TEMPERATURE|MOISTURE|ELEVATION|BIOME`) + `set_view_mode`.
  `cell_color` branches to a heat / wet / hypsometric ramp (`biomes::heat_ramp` / `wet_ramp` /
  `elevation_ramp`) or flat biome accents; data views skip neighbour smoothing + jitter (exact
  readout) and the minimap drops its liquid overlay. Same recolour + flag-all-dirty refresh path.
- C#: an unshaded `_dataMat` (raw field colours, no sun shading) + `SetViewMode`; a **VIEW** selector
  in the right panel. Picking Temperature/Moisture arms the matching trait brush so you paint in-view.
- Natural-view moisture tint softened to subtle (`MOIST_VALUE_SWING` 0.32 → 0.14) now that the
  Moisture view carries precise legibility.

**Test (world.rs):** `temperature_view_maps_the_field_to_a_heat_ramp` (hot = red / cold = blue, and
differs from Natural).

## Stage 3c — Shaping pass (landform dials → terrain height) *(Rust core + C#)* — ✅ done (2026-06-14)

Closes the gap flagged in Stage 3: the landform dials now reshape the terrain.

**Files:** `world.rs` (+ tests), `lib.rs`, `tool/scripts/ToolUi.cs`.

- `shape_terrain(strength)` layers a deterministic elevation delta on the sculpted base from the
  live (blended) dial fields: **jaggedness** → high-frequency roughness gated by altitude (peaks
  jag, lowlands stay smooth); **relief** → mid-frequency rolling hills on land; **foothill_falloff**
  → widens the down-slope skirt the detail reaches; **erosion** → damps roughness + neighbour-mean
  smooths the added delta. fbm2 + lerp/averaging only (cross-target safe).
- Idempotent via a stored `shape_delta` (restore-then-reapply, mirroring `stream_carve`), so
  re-running or changing strength/dials never compounds. Risen land sheds the water it rose through.
  Recolours (the ramp reads elevation) + flags all chunks dirty; auto-biome reclassify is skipped
  (colour is trait-driven, so it'd only affect the Biome view) to keep Apply snappy.
- C#: a **SHAPING** section in the Biomes tab (Strength slider + Apply shaping) that re-tessellates
  + rebuilds liquid.

**Tests (world.rs):** `shaping_roughens_with_jaggedness_and_is_idempotent`,
`shaping_strength_zero_is_a_no_op`.

### UI layout
- The Temperature/Moisture brushes moved out of the Biomes-tab trait dropdown to a **contextual
  paint slider under the VIEW select** (pick the view → paint that field in place, no tab hop).
- **Blend borders** (+ Width) moved to **under the map** in the right panel.
- The landform dials (jaggedness / relief / foothill / erosion) are now **set per-Region** via a
  **REGION LANDFORM** editor (numeric SpinBox inputs, 0..1) — `set_region_landform(region, idx,
  value)` stamps onto that region's cells; Apply shaping bakes. They're no longer a per-cell brush,
  so the trait brush keeps only Vegetation / Palette family. (True free-form "sections" arrive with
  the Stage 4 border/territory tooling; until then the 14 Regions are the grouping.)
- Vertical relief doubled: `TerrainHeightKm` default 1.2 → 2.4 km (live via the Height slider).

## Stage 4 — Border tool (scopes) + Region tier *(Rust core + C#)*

**Files:** `world.rs` (+ test), `lib.rs`, `tool/scripts/ToolState.cs`, `addons/dhce/{DhceDock,DhcePlugin,DhceWorld,DhceWorldState}.cs`.

- **Stage 4a — ✅ done (2026-06-15, in-editor).** The named-Region **tier** itself: a per-cell
  `region_r: Vec<u8>` (0 = unassigned, 1..=BIOME_COUNT — the 14 canon places, distinct from `biome_r`;
  a Region may span biomes). Core: `assign_region(cells, id)` (pairs with the existing
  `regions_in_polygon`/`select_contiguous`), `region_of(cell)`, `region_id_at(x,y)`,
  `paint_region(cx,cy,r,id)` (footprint brush, 3D-sphere falloff), `region_export`/`set_region`, and a
  new **`VIEW_REGION`** that colours cells by their Region's accent (`biome_color_of`, grey if
  unassigned). C#: a **Region** tool in `ToolState`/the brush path; an **ASSIGN REGION (place)** dock
  picker (the 14 names); **VIEW → Region**; a live **"Region: …"** cursor readout; and Region
  membership added to `DhceWorldState` Save/Load. Tests: `region_tier_assigns_via_polygon_and_persists`.
- **Stage 4b — ✅ done (2026-06-15, in-editor; C#-only, no DLL change — all core primitives already
  existed).** Two more in-viewport Region-assignment modes feed the same core `assign_region`:
  - **Territory (polygon):** `DhcePlugin` intercepts viewport input in Territory mode — left-click
    places ground vertices (via `raycast_terrain`), a cyan no-depth-test `ImmediateMesh` outline
    rubber-bands to the cursor, **right-click** closes → `regions_in_polygon(xs, ys)` →
    `assign_region(cells, RegionId)`; **Esc** cancels. Motion isn't consumed, so camera nav still works
    while outlining.
  - **Select (flood):** left-click → `region_at` → `select_contiguous` (same-biome flood) →
    `assign_region`.
  - Dock **ASSIGN REGION** section gains **Brush / Polygon / Select** mode buttons; `DhceDock.SetStatus`
    made public for live polygon/flood feedback. Result is visible immediately in **VIEW → Region**
    (the assigned cells take the Region accent).
  - **Deferred (review):** a dedicated per-edge **border-line overlay** (drawing Region boundaries in
    accent) — `VIEW_REGION` already visualizes membership, so it wasn't needed for usability; a
    selection-fill overlay would need a new `cell_positions` core accessor.

**Tests:** polygon assignment ✅; region membership ✅; persistence ✅ (Stage 4a's
`region_tier_assigns_via_polygon_and_persists` exercises `regions_in_polygon`+`assign_region`). 4b is
viewport-interaction C# over those tested primitives — `dotnet build` + editor smoke are its gates.

## Stage 5 — Per-trait editor panel + presets *(C#)*

**Files:** `tool/scripts/` (editor panel, preset row).

- Brush panel: the 9 dials (Elevation, Jaggedness, Relief, Foothill falloff, Erosion, Temperature,
  Moisture, Vegetation dropdown, Palette family) + biome-preset row (Forest/Plains/Rocky/Marsh…) +
  Region selector. Editing a preset/Region writes back via the Stage 2–4 setters.

**Acceptance (whole model):** paint distinct areas (jagged rocky highland, rolling forest, plains,
marsh); borders ease naturally; the HUD names the emergent biome; Regions name + accent the places;
re-applying blend is idempotent; `cargo test -p dhce-core` + `dotnet build` green at each stage.

## Out of scope (later)
- Ordered vegetation laddering through the buffer (evergreen→forest→grass intermediate types).
- Splat-shader material blending (vertex colours suffice for now).
- Scatter/particles driven by vegetation + `--mk-*` glow (N3d).
- The `region → cell` core rename (mechanical, separate pass).
