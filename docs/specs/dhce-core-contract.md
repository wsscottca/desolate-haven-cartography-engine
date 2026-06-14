---
status: active
date: 2026-06-14
owner: wsscottca
---

# dhce-core contract (self-contained)

The shared generation + simulation core of the Desolate Haven Cartography Engine.
One Rust crate, two adapters: `dhce-wasm` (browser tool) and `dhce-godot`
(GDExtension for the Godot `desolate-haven` game). **Same seed + inputs ⇒ identical
output on every target** — this is what keeps the authoring tool and the game in sync.

This document is self-contained: all types and call sequences are inlined so it can
be handed to a Godot/C# (or DeepSeek-in-Rider) consumer without reading the source.

## Determinism guarantees

- PRNG: our own SplitMix64 (`prng::Rng`). Same seed ⇒ same stream on wasm32 + native.
- Noise: our own 2D simplex (`noise::simplex2`/`fbm2`), integer-hash gradients.
- **No transcendentals** (`sin`/`cos`/`exp`) in shared generation math — only
  `floor`/multiply/add, which are IEEE-deterministic across platforms. (Rendering may
  use trig; that is per-front-end and not part of the shared contract.)
- Float math is `f64`, narrowed to `f32` only at storage/output boundaries.
- The fluid solver is fixed-step and mass-conserving.
- Order-sensitive passes (the stream priority-flood + flow accumulation) use a **total
  order** keyed on `(f64::total_cmp, region index)`, so ties resolve identically on
  wasm32 and native.

## Core modules (Rust API)

```rust
// prng.rs
prng::Rng::new(seed: u64) -> Rng
Rng::next_u64(&mut self) -> u64
Rng::next_f64(&mut self) -> f64      // [0,1)

// noise.rs
noise::simplex2(x: f64, y: f64, seed: u64) -> f64           // [-1,1]
noise::fbm2(x: f64, y: f64, seed: u64, octaves: u32) -> f64 // [-1,1]

// mesh.rs — dual half-edge mesh over a Delaunay triangulation (delaunator, ISC)
mesh::Mesh::new(width: f64, height: f64, spacing: f64, seed: u64) -> Mesh
Mesh::num_regions(&self) -> usize        // Voronoi cells = surface vertices
Mesh::num_triangles(&self) -> usize
Mesh::pos_of_r(&self, r: usize) -> [f64; 2]
Mesh::region_neighbors(&self) -> Vec<Vec<u32>>   // adjacency for the fluid solver
// half-edge navigation: t_of_s, s_next_s, s_opposite_s -> Option<usize>,
//   r_begin_s, r_end_s, is_boundary_r

// elevation.rs
elevation::assign_region_elevation(&Mesh, width, height, seed: u64, octaves: u32) -> Vec<f64>
//   per-region normalized elevation [-1,1], island-shaped (fbm minus radial falloff)

// biomes.rs
biomes::BIOME_COUNT: usize = 14
biomes::roster() -> [BiomeDef; 14]       // label, color [f32;3], landform, water
biomes::moisture_at(x, y, width, height, seed: u64) -> f64  // [0,1] proxy
biomes::classify(elevation: f64, moisture: f64, dist_center: f64) -> u8  // 1..=14

// fluid.rs — height-field hydraulic sim (units = normalized elevation)
fluid::LiquidField { depth: Vec<f64>, kind: Vec<u8> }   // kind: 0 water, 1 lava
fluid::sea_fill(&mut LiquidField, terrain: &[f64], level: f64)
fluid::add_rain(&mut LiquidField, terrain: &[f64], level: f64, amount: f64)
fluid::relax_step(&mut LiquidField, terrain: &[f64], neighbors: &[Vec<u32>],
                  flow_rate: f64 /*≤0.5*/, evaporation: f64)
fluid::liquid_surface(&Mesh, terrain, &LiquidField, neighbors: &[Vec<u32>],
                      exaggeration) -> LiquidSurface
//   `neighbors` drives a render-only Laplacian smoothing of the wet surface (the sim
//   is untouched / still mass-conserving) so settling water reads level, not spiky.

// streams.rs — procedural tributaries via flow accumulation on the dual mesh
streams::accumulate(terrain: &[f64], neighbors: &[Vec<u32>], num_boundary: usize,
                    seed_mask: &[bool], threshold: f64, depth_gain: f64) -> StreamResult
//   StreamResult { flow: Vec<f64>, is_stream: Vec<bool>, carve_delta: Vec<f64> }
//   Priority-flood pit fill → steepest-descent receivers → flow accumulation, gated to
//   the catchment that drains into seed_mask (the painted main rivers). Carves channels
//   ∝ sqrt(flow); seed cells always stream.

// geometry.rs — render-ready surface (a TIN over the dual mesh)
geometry::build_surface(&Mesh, elevation: &[f64], exaggeration: f64,
                        region_color: &[f32]) -> Surface
//   Surface { positions: Vec<f32> /*3/vtx*/, normals: Vec<f32> /*3/vtx*/,
//             heights: Vec<f32>, colors: Vec<f32> /*3/vtx*/, indices: Vec<u32> /*3/tri*/ }

// scatter.rs — deterministic decoration placement (rocks/trees)
scatter::scatter(seed: u64, &Mesh, elevation, biome, exaggeration, density) -> Vec<Instance>
//   Instance { x, y, z, scale, species /*0 tree, 1 rock*/ : f32 }
```

## Typical call sequence (any front-end)

1. `Mesh::new(width, height, spacing, seed)` — once per seed/density change.
2. `elevation::assign_region_elevation(...)` → `terrain: Vec<f64>`.
3. (optional) classify each region via `biomes::classify(...)` → `biome: Vec<u8>`.
4. `geometry::build_surface(mesh, terrain, exaggeration, region_color)` → upload mesh.
5. Liquids: `sea_fill` / `add_rain`, then loop `relax_step` to settle; `liquid_surface`
   for the render mesh.
6. Decoration: `scatter(...)` → instance buffer.

Brushes (interactive edits) mutate `terrain`/`biome`/`LiquidField` in place over a
radial falloff, then re-run steps 4–6 for the affected output.

## Authoring operations (front-end engine)

The browser tool (`dhce-wasm`) layers these authored-edit operations over the core. A
native front-end must reproduce them to stay map-compatible; all are deterministic.

- **Sculpt** `paint_terrain(cx, cy, radius, strength, mode)` — mode 0 raise / 1 carve /
  2 level / 3 crest, smoothstep radial falloff. Auto-biome cells under the brush are
  re-classified from their new elevation (manual paint is locked, so color tracks edits).
- **Course (rivers)** `paint_course(cx, cy, radius, intensity, kind)` — carves a
  U-channel *toward* a bed (idempotent under a dragged stroke), lays thin water, and
  tags the cells in a persistent course (main-river) seed mask.
- **Streams** `generate_streams(threshold, depth_gain)` — restores any prior stream
  carving, runs `streams::accumulate`, then carves the returned channels + thin water.
  Idempotent (restore-then-recarve); no-op until a Course stroke is painted.
- **Flood** `paint_liquid(cx, cy, radius, amount, kind)` — pour liquid (kind 0/1) into a basin.
- **Biome paint** `paint_biome(cx, cy, radius, id)` — assign + lock a biome over the brush.
- **Select** `region_at(x, y) -> i32`, `biome_at(region)`, `select_contiguous(region) ->
  Vec<u32>` (flood same-biome), `set_biome_of(region, id)` (reassign + lock),
  `selection_indices(regions) -> Vec<u32>` (highlight fill geometry).
- **Boundary / territory** `regions_in_polygon(xs, ys) -> Vec<u32>` (ray-cast PIP over
  region centroids, frame excluded).
- **Biome profiles** `biome_color_of/biome_landform_of/biome_water_of(id)` +
  `set_biome_color/set_biome_landform(id, idx, v)/set_biome_water(id, idx, v)` — the
  per-biome editable characteristics (color; landform peak/hill/valley/roughness; water
  raininess/rain_shadow/evaporation/flow/ocean_depth), seeded from `roster()`.
- **Save/load** exports: elevation, biome, liquid depth+kind, biome colors, course mask.

## GDExtension adapter (`dhce-godot`) — Godot/C# surface

Class `DhceEngine` (extends `RefCounted`). Methods callable from GDScript/C#:

```
build(width: float, height: float, spacing: float, seed: float, octaves: int) -> void
region_count() -> int
version() -> String
surface_positions(exaggeration: float) -> PackedFloat32Array   // x,y,z per vertex
surface_normals(exaggeration: float)   -> PackedFloat32Array   // 3 per vertex
surface_indices(exaggeration: float)   -> PackedInt32Array     // 3 per triangle
biome_at(region: int) -> int                                   // 1..=14, 0 = none
```

Build a Godot mesh by feeding `surface_positions`/`surface_normals` into an
`ArrayMesh` (Mesh.ARRAY_VERTEX / ARRAY_NORMAL) with `surface_indices` as
`ARRAY_INDEX`. Because the core is deterministic, a `(seed, spacing, exaggeration)`
that looks right in the browser tool produces the same mesh here.

**Toolchain note:** `dhce-godot` depends on `godot` (godot-rust/gdext, MIT-elected).
gdext is pinned to a Godot 4.x API; align the `godot` crate version with the project's
Godot build (or set `GODOT4_BIN`). The crate is excluded from the default Cargo
workspace and built on its own; the resulting `.dll`/`.so` + `dhce.gdextension`
manifest are copied into the Godot project's `addons/`.
