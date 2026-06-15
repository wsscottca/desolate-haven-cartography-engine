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

## Stage 1 — Trait substrate *(Rust core + DLL rebuild)*

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

## Stage 2 — Biome classifier *(Rust core + DLL)*

**Files:** `biomes.rs` (+ test), `world.rs`, `crates/dhce-godot/src/lib.rs`.

- `fn classify_biome(traits) -> u16` (or a small descriptor id) over coarse buckets of
  vegetation × landform (elevation+jaggedness+relief) × climate (temperature+moisture), plus a
  human label table. Off the render path.
- `biome_locked` carries forward: a painted biome-preset pins a label; else it's derived.
- Expose `biome_label_at(cell)` / counts via the GDExtension for the HUD/editor.

**Tests:** representative trait bundles classify to the expected label; locked cells keep theirs.

## Stage 3 — Brush + transition buffer + blend pass *(Rust core + C#)*

**Files:** `world.rs` (+ test), `lib.rs`, `tool/scripts/ToolState.cs`, `CartographerSpike.cs`, `ToolUi.cs`.

- `paint_trait(cx,cy,r, trait_id, value)` and `stamp_preset(cx,cy,r, preset)` — set targets in the
  footprint, mark a **transition buffer** band (cells within `width` of a value discontinuity).
- `blend_traits(transition_width_m)` — param-field diffusion (Laplacian average over `neighbors`)
  on each scalar field across the buffer; idempotent layered model (store painted-base vs blended).
  One **Transition width** control → `k`.
- C#: brush picks a trait/preset; Transition-width slider; Apply triggers `blend_traits`.

**Tests:** blend idempotent on re-run; diffusion converges + is symmetric; a mountain-vs-plains
paint yields a graded skirt (jaggedness + vegetation fall at independent rates).

## Stage 4 — Border tool (scopes) + Region tier *(Rust core + C#)*

**Files:** `world.rs` (+ test), `lib.rs`, `tool/scripts/` (border tool, Region panel, overlay).

- Region table: the 14 canon names (from the guide), each with `--mk-*` accent + member cells; a
  Region may span biomes. `assign_region(cells, region_id)`; `region_of(cell)`.
- **Border tool** scoped Biome / Region / Territory (polygon via existing `regions_in_polygon`);
  **Select** via existing `select_contiguous`. Border overlay drawn in the accent colour.

**Tests:** polygon assignment; region membership; selection single-scope.

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
