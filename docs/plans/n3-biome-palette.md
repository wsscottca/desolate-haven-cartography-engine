---
status: superseded
date: 2026-06-15
owner: wsscottca
---

> **Superseded 2026-06-15** by [`n3-biome-traits.md`](n3-biome-traits.md) (ADR 0004 rewritten to
> the trait-composition model). The shipped pieces — the `ground_color` elevation ramp, the
> per-cell colour path, and neighbour smoothing — carry forward as the trait substrate; the 14
> distinct palettes collapse to 7 shared bases and `biome_r` becomes trait fields + a classifier.
> Kept for history.

# N3 — Biome-archetype palette (tier 1 of ADR 0004)

**Goal:** Replace the flat one-color-per-biome model with the canon **7-slot shared palette** +
a context ramp, so ground color resolves by elevation/moisture from each biome's tokens (cohesive
hues + per-biome identity), with soft borders from the existing neighbour smoothing.

**Architecture:** Rust core only (`dhce-core`) + DLL rebuild. No C# changes — the recolor flows
through the existing `region_color()` → `color_cache` → `chunk_surface()`/`minimap()` path.
See [ADR 0004](../adr/0004-biome-region-territory-model.md) and `n3-tooling-design.md` §8.

**Source of truth:** `desolate-haven-guide/src/styles/tokens.css` (mirrored into `biomes.rs`).

---

## Task 1 — `Palette` + per-biome data in `biomes.rs`

**Files:** Modify `crates/dhce-core/src/biomes.rs`; Test `crates/dhce-core/tests/biomes.rs` (or
the existing biome test file).

- Add `Palette { water_deep, water_shallow, ground, ground2, rock, cap, glow, accent }` (each
  `[f32;3]`), a `c(0xRRGGBB) -> [f32;3]` helper, and `lerp3` / `clamp01`.
- Extend `BiomeDef` with `palette: Palette` and `seed: [f32;2]` (base_elevation, relief). Drop the
  flat `color`; add `BiomeDef::representative()` → `palette.ground` for swatch back-compat.
- Recolor all 14 roster entries from `tokens.css` per `biome-features.md` Base+Material+Accent
  (mapping table in ADR 0004 §3). Keep existing `Landform`/`WaterProfile` values. Seeds from the
  mapgen4 `BIOME_SEED`.
- Add `pub fn ground_color(p: &Palette, e: f32, m: f32) -> [f32;3]` — the shared ramp:
  `e<0` → `lerp(water_deep, water_shallow, (e+1)²)`; else `cover = lerp(ground, ground2, m)`,
  then low (`e<0.45`, gentle darken) → treeline (`0.45..0.75`, `lerp(cover, rock)`) → high
  (`lerp(rock, cap)`). Determinism-safe (lerp/clamp/multiply only).

**Tests:** `ground_color` below-sea is bluer than the same biome's land; high `e` for a snow biome
trends light; identical inputs → identical output. Roster palettes are populated (not all grey).

## Task 2 — Resolve color via the palette in `world.rs`

**Files:** Modify `crates/dhce-core/src/world.rs`; Test `crates/dhce-core/tests/world.rs`.

- Add fields `biome_palette: Vec<Palette>` (idx 0 = neutral grey; 1..=14 from roster) and
  `moisture_r: Vec<f64>`. Seed `biome_palette` in `new()`; keep `biome_color` = `palette.ground`
  (representative) so `biome_color_of`/swatches keep working.
- In `build()`, store the per-region moisture already computed in the classify loop into
  `moisture_r` (no extra fbm2 cost).
- `region_color()`: per region, `ground_color(&biome_palette[id], elevation_r[r] as f32,
  moisture_r[r] as f32)` as the base, then the existing smoothing + jitter unchanged.
- `after_edit()`: recompute `color_cache[r]` from `ground_color(...)` + jitter (no smoothing, as
  today). Reads `moisture_r[r]` (fall back to 0.5 if unsized).

**Tests:** after `build()`, `color_cache` is non-empty and not uniform; a forced high-elevation
snow-biome cell is lighter than a low-elevation cell of the same biome.

## Task 3 — Build, test, DLL swap, smoke

- `cargo test -p dhce-core` green.
- Build the DLL from `crates/dhce-godot` (`cargo build --release`); **user closes Godot**, copy
  `~/.dhce-build/release/dhce_godot.dll` → `tool/addons/dhce/`.
- Headless smoke: `godot --headless --path tool --quit-after 250` clean; user F5 to confirm the
  recolor (cohesive hues, soft borders, snow caps, water).

## Out of scope (later tiers)
- Region tier (named places, accent/seed overrides, seed-terrain-from-regions) — ADR 0004 tier 2.
- Territory paint-override layer + Brush/Territory/Select tools + borders overlay — tier 3.
- `glow`/`accent` consumption (borders, scatter, particles); per-biome editor GDExtension setters.
