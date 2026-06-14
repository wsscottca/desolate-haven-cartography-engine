---
status: active
date: 2026-06-14
owner: wsscottca
---

# dhce-core contract (self-contained)

The shared generation + simulation core of the Desolate Haven Cartography Engine.
One Rust crate, two adapters: `dhce-godot` (GDExtension — drives the **native Godot 4
+ C# authoring tool** and the `desolate-haven` game) and `dhce-wasm` (the frozen browser
build, kept as a native↔web determinism cross-check). **Same seed + inputs + authored
edits ⇒ identical output on every target** — this keeps the tool and the game in sync.
All authoring state and compute live in `dhce_core::world::World`; the adapters are thin
marshalling shells (see ADR 0002).

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

// world.rs — resident authoring world-state: the source of truth for an edit session
world::World::new() -> World
World::build(width, height, spacing, seed: u64, octaves: u32)
//   Owns mesh + elevation + biome + liquid + per-biome profiles + spatial grid, plus
//   every authoring op (see below). surface()/liquid_surface()/scatter_instances() pack
//   the render buffers. Both adapters wrap a single World.
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

## Authoring operations (`dhce_core::world::World`)

These authored-edit operations live in the core `World`, so both adapters expose the
identical surface by forwarding to it (no per-region compute in the adapters); all are
deterministic. Brush ops return the touched region ids so a front-end can patch just
those vertices instead of re-uploading the whole mesh.

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

Class `DhceEngine` (extends `RefCounted`); every method forwards to the core `World`.
Render surfaces use a **pack-once, read-each-array** pattern: call `tessellate(exag)`
(or `tessellate_liquid`) once, then read the per-array getters.

```
build(width, height, spacing, seed: float, octaves: int) -> void
region_count() / triangle_count() -> int        version() -> String

tessellate(exaggeration: float) -> void          // pack terrain surface into cache
surface_positions() -> PackedFloat32Array         // x,y,z per vertex
surface_normals()   -> PackedFloat32Array         // 3 per vertex
surface_colors()    -> PackedFloat32Array         // rgb per vertex (smoothed biome color)
surface_heights()   -> PackedFloat32Array         // normalized elevation per vertex
surface_indices()   -> PackedInt32Array           // 3 per triangle
tessellate_liquid(exaggeration) -> void           // liquid_positions/normals/types() ->
                                                  //   PackedFloat32Array; liquid_indices() -> PackedInt32Array

// authoring (forward to World) — brush ops return PackedInt32Array of touched regions
paint_terrain(cx, cy, radius, strength, mode: int) -> PackedInt32Array  // 0 raise/1 carve/2 level/3 crest
paint_course(cx, cy, radius, intensity, kind: int) -> PackedInt32Array
paint_liquid(cx, cy, radius, amount, kind: int)    -> void
paint_biome(cx, cy, radius, biome_id: int)         -> PackedInt32Array
generate_streams(threshold, depth_gain) -> void
set_sea_level(level) / rain(amount) / step_fluid(flow, evap, substeps: int) / clear_liquid()

// selection / boundary / biome profiles
region_at(x, y) -> int (-1 none)   biome_at(region: int) -> int   set_biome_of(region, id: int)
select_contiguous(region: int) -> PackedInt32Array
selection_indices(PackedInt32Array) -> PackedInt32Array
regions_in_polygon(xs, ys: PackedFloat32Array) -> PackedInt32Array
biome_color_of / biome_landform_of / biome_water_of(id: int) -> PackedFloat32Array
set_biome_color(id, r, g, b)   set_biome_landform(id, idx, v)   set_biome_water(id, idx, v)

// save / load + decoration
elevation_export() -> PackedFloat32Array     biome_export() -> PackedByteArray
liquid_depth_export() -> PackedFloat32Array  liquid_kind_export() -> PackedByteArray
course_mask_export() -> PackedByteArray
set_elevation / set_biome / set_liquid / set_course_mask
tessellate_scatter(exaggeration, density, seed) -> void; scatter_data() -> PackedFloat32Array; scatter_count() -> int
```

Build a Godot mesh by feeding `surface_positions`/`surface_normals`/`surface_colors`
into an `ArrayMesh` (`ARRAY_VERTEX`/`ARRAY_NORMAL`/`ARRAY_COLOR`) with `surface_indices`
as `ARRAY_INDEX`. Because the core is deterministic, a `(seed, spacing, exaggeration)`
that looks right anywhere reproduces here.

**Toolchain note:** `dhce-godot` depends on `godot` (godot-rust/gdext, **MPL-2.0** —
weak copyleft confined to gdext's own files). gdext is pinned to a Godot 4.x API; align
the `godot` crate version with the project's Godot build (or set `GODOT4_BIN`). The crate
is excluded from the default Cargo workspace and built on its own; the resulting
`.dll`/`.so` + `dhce.gdextension` manifest are copied into the Godot project's `addons/`.
