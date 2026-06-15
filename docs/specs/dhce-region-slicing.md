---
status: active
date: 2026-06-15
owner: wsscottca
---

# DHCE — per-Region level slicing (design of record)

Slice the authored whole-world `DhceWorldState` into **per-Region level scenes** for the (not
open-world) *Desolate Haven* game: each canon Region (the Stage 4 `region_r` tier) becomes its own
gated level. The Region tier is the seam; this is the export step that cuts on it.

## Decisions (locked with the user 2026-06-15)

1. **Artifact = baked scene + editable master state.** Each Region exports a self-contained
   `.tscn` (terrain submesh + collider + water + scatter proxies) the game loads directly — **no Rust
   core or whole-world regen at runtime.** Baking is required because the core builds the *whole*
   1.7 M-cell world from params (`build`), so there's no per-Region regenerate.
   - **"Editable state" = the single master `DhceWorldState`** (saved as part of slicing), and each
     baked level records its `region_id`. Re-editing a level = open the master in the tool, tweak,
     re-slice. *(Reviewable: per-Region state files would just duplicate the baked mesh, since the
     core can't regenerate a sub-region — so we keep one master, not 14 partial states.)*
2. **Full geometry per level:** terrain mesh + trimesh static collider + water surface + scatter
   **proxies** (from the existing core `scatter` — markers, not final art; real `.glb` is later).
3. **Export target = a selectable path, default the game repo** (`..\desolate-haven\levels\`).
4. **Adjacency manifest:** export which Regions border which (+ shared-edge cell counts) so gates are
   placed later in the gameplay phase. No gate nodes dropped now.

**Defaults (not separately asked):** slice **assigned Regions only** (unassigned cells dropped, with a
logged count); a triangle joins a Region's mesh when **all three** of its cells are in that Region
(boundary triangles between Regions are dropped — they belong to the gate seam). Level scenes are
Godot `.tscn` (native, editable). Geometry is in **Godot space** (Y-up: core `x → x`, core `y → z`,
`elev·exaggeration → y`), matching the editor preview so a level lines up with where it was authored.

## Core additions (`dhce-core` + GDExtension) — the extraction layer

A region is a set of cells (`region_r == id`). The packers mirror `geometry::build_surface` /
`fluid::liquid_surface` but **filtered to the region's triangles and re-indexed to a compact vertex
set** (so a level mesh is small and standalone):

- `region_terrain_surface(region_id, exaggeration) -> Surface` — positions/normals/colors/indices for
  triangles whose 3 cells are all in `region_id`, compacted. Empty if the Region has no full triangle.
- `region_liquid_surface(region_id, exaggeration) -> LiquidSurface` — same filter over the wet surface.
- `region_scatter_instances(region_id, exaggeration, density, seed) -> Vec<Instance>` — the existing
  `scatter` output filtered to cells in `region_id`.
- `region_adjacency() -> Vec<[u32; 3]>` — `[region_a, region_b, shared_edge_count]` for each
  unordered adjacent pair (a cell in `a` neighbouring a cell in `b`), both non-zero.
- `region_cell_count(region_id) -> usize` — for the "assigned Regions only" + dropped-count report.
  (The list of assigned ids is derived in C# from `region_export()`.)

GDExtension binds each as packed arrays (mirroring `chunk_positions`/`tessellate_chunk`).

## C# slicer (`addons/dhce/`)

- **`DhceLevelSlicer`** (static helper or a method on `DhceWorld`): for each assigned Region →
  - terrain `ArrayMesh` from `region_terrain_surface` → `MeshInstance3D` + `StaticBody3D` with a
    `ConcavePolygonShape3D` (trimesh) collider;
  - water `MeshInstance3D` from `region_liquid_surface` (transparent material), if non-empty;
  - scatter `MultiMeshInstance3D` of proxy markers from `region_scatter_instances`;
  - pack into a `PackedScene` saved as `<export>/levels/<RegionName>.tscn` (a `Node3D` root named the
    Region, `region_id` stored as metadata).
- **Master state:** `DhceWorld.Save()` writes the master `DhceWorldState` (`.res`) into the export dir.
- **Adjacency manifest:** `<export>/levels/regions.json` — `{ region, name, cells, neighbors:[{id,name,
  sharedEdges}] }[]`.
- **Dock — `SLICE LEVELS` section:** an export-path `LineEdit` (default `..\desolate-haven` resolved to
  an absolute path; editable) + a **Slice into levels** button → runs the slicer, status reports the
  per-Region cell counts + dropped (unassigned) count.

## Gates / acceptance

- **Gate:** core packers green (a test: a fully-assigned small world slices into a non-empty submesh
  whose triangles all lie in the Region; adjacency is symmetric & non-zero only for touching Regions);
  `dotnet build` + headless editor smoke clean; a headless slice writes `<n>` `.tscn` + the manifest +
  the master `.res`. Visual (open a sliced `.tscn` in the game) is the user's check.

## Out of scope (later)
- Real scatter art (`.glb`/baked MultiMesh) — N4; proxies only here.
- Gate/portal nodes + the game's level-loading + streaming-between-levels framework (gameplay phase).
- Seamless border overlap between adjacent levels (loading-seam vs shared strip) — a gameplay decision.
- The `region → cell` core rename (mechanical, separate).
