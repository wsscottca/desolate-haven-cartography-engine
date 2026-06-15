//! GDExtension adapter exposing `dhce-core` to Godot (the cartography tool + the game).
//!
//! `DhceEngine` (a `RefCounted`) wraps the shared [`dhce_core::world::World`]: all state
//! and compute live in the core, so this adapter only marshals types across the
//! GDExtension boundary and caches the packed render surfaces. Determinism is guaranteed
//! by the core (same seed/inputs/edits ⇒ identical output on wasm32 and native), so a
//! world authored in the tool reproduces exactly in the game.
//!
//! See `docs/specs/dhce-core-contract.md` for the full contract. Pin the `godot` crate
//! version to the project's Godot 4.x build.

use dhce_core::fluid::LiquidSurface;
use dhce_core::geometry::Surface;
use dhce_core::world::World;
use godot::prelude::*;

struct DhceExtension;

#[gdextension]
unsafe impl ExtensionLibrary for DhceExtension {}

#[derive(GodotClass)]
#[class(base = RefCounted)]
struct DhceEngine {
    world: World,
    surface: Option<Surface>,
    chunk_cache: Option<Surface>,
    liquid: Option<LiquidSurface>,
    liquid_chunk_cache: Option<LiquidSurface>,
    scatter_buf: Vec<f32>,
    scatter_n: usize,
    base: Base<RefCounted>,
}

#[godot_api]
impl IRefCounted for DhceEngine {
    fn init(base: Base<RefCounted>) -> Self {
        DhceEngine {
            world: World::new(),
            surface: None,
            chunk_cache: None,
            liquid: None,
            liquid_chunk_cache: None,
            scatter_buf: Vec::new(),
            scatter_n: 0,
            base,
        }
    }
}

#[godot_api]
impl DhceEngine {
    // --- build / metadata ---

    /// Build mesh + per-region elevation + auto biome classification.
    #[func]
    fn build(&mut self, width: f64, height: f64, spacing: f64, seed: f64, octaves: i64) {
        self.world.build(width, height, spacing, seed as u64, octaves.max(1) as u32);
        self.surface = None;
        self.liquid = None;
    }

    #[func]
    fn region_count(&self) -> i64 {
        self.world.region_count() as i64
    }
    #[func]
    fn triangle_count(&self) -> i64 {
        self.world.triangle_count() as i64
    }
    #[func]
    fn version(&self) -> GString {
        GString::from(dhce_core::VERSION)
    }

    // --- terrain render surface (pack once, read each array) ---

    /// Pack the terrain render surface at a vertical `exaggeration` into the cache.
    #[func]
    fn tessellate(&mut self, exaggeration: f64) {
        self.surface = self.world.surface(exaggeration);
    }
    /// Vertex positions as Godot `Vector3`, remapped to Y-up (core is z-up): the terrain
    /// lies in Godot's XZ plane with elevation on +Y. Feed straight into `ARRAY_VERTEX`.
    #[func]
    fn surface_positions(&self) -> PackedVector3Array {
        self.surface.as_ref().map(|s| to_vec3_yup(&s.positions)).unwrap_or_default()
    }
    #[func]
    fn surface_normals(&self) -> PackedVector3Array {
        self.surface.as_ref().map(|s| to_vec3_yup(&s.normals)).unwrap_or_default()
    }
    #[func]
    fn surface_colors(&self) -> PackedColorArray {
        self.surface.as_ref().map(|s| to_colors(&s.colors)).unwrap_or_default()
    }
    #[func]
    fn surface_heights(&self) -> PackedFloat32Array {
        self.surface.as_ref().map(|s| PackedFloat32Array::from(s.heights.as_slice())).unwrap_or_default()
    }
    #[func]
    fn surface_indices(&self) -> PackedInt32Array {
        self.surface.as_ref().map(|s| u32_to_packed(&s.indices)).unwrap_or_default()
    }

    // --- terrain chunks (incremental re-tessellation: an edit re-packs only dirty tiles) ---

    #[func]
    fn chunk_count(&self) -> i64 {
        self.world.chunk_count() as i64
    }
    /// `(cols, rows)` of the chunk grid as a `Vector2i` (square: x == y); `(0, 0)` until
    /// `build`. A chunk's grid cell is `(id % cols, id / cols)`.
    #[func]
    fn chunk_grid(&self) -> Vector2i {
        let (cols, rows) = self.world.chunk_grid();
        Vector2i::new(cols as i32, rows as i32)
    }
    /// World-space center of every chunk tile, indexed by chunk id. Core `(x, y)` is the
    /// Godot ground plane (Y is up), so each `Vector2`'s `.x`/`.y` are the tile's Godot
    /// X/Z — feed straight into render-distance streaming. Empty until `build`.
    #[func]
    fn chunk_centers(&self) -> PackedVector2Array {
        let flat = self.world.chunk_centers();
        let mut v = Vec::with_capacity(flat.len() / 2);
        for c in flat.chunks_exact(2) {
            v.push(Vector2::new(c[0] as f32, c[1] as f32));
        }
        PackedVector2Array::from(v.as_slice())
    }
    /// Pack chunk `chunk` at `exaggeration` into the chunk cache, then read the getters.
    #[func]
    fn tessellate_chunk(&mut self, chunk: i64, exaggeration: f64) {
        self.chunk_cache = self.world.chunk_surface(chunk.max(0) as usize, exaggeration);
    }
    #[func]
    fn chunk_positions(&self) -> PackedVector3Array {
        self.chunk_cache.as_ref().map(|s| to_vec3_yup(&s.positions)).unwrap_or_default()
    }
    #[func]
    fn chunk_normals(&self) -> PackedVector3Array {
        self.chunk_cache.as_ref().map(|s| to_vec3_yup(&s.normals)).unwrap_or_default()
    }
    #[func]
    fn chunk_colors(&self) -> PackedColorArray {
        self.chunk_cache.as_ref().map(|s| to_colors(&s.colors)).unwrap_or_default()
    }
    #[func]
    fn chunk_indices(&self) -> PackedInt32Array {
        self.chunk_cache.as_ref().map(|s| u32_to_packed(&s.indices)).unwrap_or_default()
    }
    /// Chunk ids changed by the last edit (cleared by this call); re-tessellate exactly these.
    #[func]
    fn take_dirty_chunks(&mut self) -> PackedInt32Array {
        u32_to_packed(&self.world.take_dirty_chunks())
    }

    // --- liquid simulation + render surface ---

    #[func]
    fn set_sea_level(&mut self, level: f64) {
        self.world.set_sea_level(level);
    }
    #[func]
    fn rain(&mut self, amount: f64) {
        self.world.rain(amount);
    }
    #[func]
    fn step_fluid(&mut self, flow_rate: f64, evaporation: f64, substeps: i64) {
        self.world.step_fluid(flow_rate, evaporation, substeps.max(0) as u32);
    }
    /// Regions the fluid solver is still iterating (0 ⇒ settled; the UI can skip the sim tick).
    #[func]
    fn liquid_active_count(&self) -> i64 {
        self.world.liquid_active_count() as i64
    }
    #[func]
    fn clear_liquid(&mut self) {
        self.world.clear_liquid();
    }
    #[func]
    fn tessellate_liquid(&mut self, exaggeration: f64) {
        self.liquid = self.world.liquid_surface(exaggeration);
    }
    #[func]
    fn liquid_positions(&self) -> PackedVector3Array {
        self.liquid.as_ref().map(|s| to_vec3_yup(&s.positions)).unwrap_or_default()
    }
    #[func]
    fn liquid_normals(&self) -> PackedVector3Array {
        self.liquid.as_ref().map(|s| to_vec3_yup(&s.normals)).unwrap_or_default()
    }
    #[func]
    fn liquid_types(&self) -> PackedFloat32Array {
        self.liquid.as_ref().map(|s| PackedFloat32Array::from(s.types.as_slice())).unwrap_or_default()
    }
    #[func]
    fn liquid_indices(&self) -> PackedInt32Array {
        self.liquid.as_ref().map(|s| u32_to_packed(&s.indices)).unwrap_or_default()
    }

    // --- liquid chunks (mirror the terrain chunk path: stream + re-tessellate only in-range tiles) ---

    /// Pack liquid chunk `chunk` at `exaggeration` into the liquid chunk cache, then read the getters.
    #[func]
    fn tessellate_liquid_chunk(&mut self, chunk: i64, exaggeration: f64) {
        self.liquid_chunk_cache = self.world.liquid_chunk_surface(chunk.max(0) as usize, exaggeration);
    }
    #[func]
    fn liquid_chunk_positions(&self) -> PackedVector3Array {
        self.liquid_chunk_cache.as_ref().map(|s| to_vec3_yup(&s.positions)).unwrap_or_default()
    }
    #[func]
    fn liquid_chunk_normals(&self) -> PackedVector3Array {
        self.liquid_chunk_cache.as_ref().map(|s| to_vec3_yup(&s.normals)).unwrap_or_default()
    }
    #[func]
    fn liquid_chunk_types(&self) -> PackedFloat32Array {
        self.liquid_chunk_cache.as_ref().map(|s| PackedFloat32Array::from(s.types.as_slice())).unwrap_or_default()
    }
    #[func]
    fn liquid_chunk_indices(&self) -> PackedInt32Array {
        self.liquid_chunk_cache.as_ref().map(|s| u32_to_packed(&s.indices)).unwrap_or_default()
    }
    /// Liquid chunk ids changed by the last edit/sim (cleared by this call); re-tessellate exactly
    /// these that are in render range.
    #[func]
    fn take_dirty_liquid_chunks(&mut self) -> PackedInt32Array {
        u32_to_packed(&self.world.take_dirty_liquid_chunks())
    }

    // --- brush tools (return touched region ids for partial mesh updates) ---

    #[func]
    fn paint_terrain(&mut self, cx: f64, cy: f64, radius: f64, strength: f64, mode: i64) -> PackedInt32Array {
        u32_to_packed(&self.world.paint_terrain(cx, cy, radius, strength, mode.max(0) as u32))
    }
    #[func]
    fn paint_liquid(&mut self, cx: f64, cy: f64, radius: f64, amount: f64, kind: i64) {
        self.world.paint_liquid(cx, cy, radius, amount, kind.max(0) as u8);
    }
    #[func]
    fn paint_course(&mut self, cx: f64, cy: f64, radius: f64, intensity: f64, kind: i64) -> PackedInt32Array {
        u32_to_packed(&self.world.paint_course(cx, cy, radius, intensity, kind.max(0) as u8))
    }
    #[func]
    fn generate_streams(&mut self, threshold: f64, depth_gain: f64) {
        self.world.generate_streams(threshold, depth_gain);
    }

    // --- biomes ---

    #[func]
    fn paint_biome(&mut self, cx: f64, cy: f64, radius: f64, biome_id: i64) -> PackedInt32Array {
        u32_to_packed(&self.world.paint_biome(cx, cy, radius, biome_id.max(0) as u8))
    }
    #[func]
    fn set_biome_color(&mut self, id: i64, r: f64, g: f64, b: f64) {
        self.world.set_biome_color(id.max(0) as usize, r as f32, g as f32, b as f32);
    }
    #[func]
    fn biome_color_of(&self, id: i64) -> PackedFloat32Array {
        PackedFloat32Array::from(self.world.biome_color_of(id.max(0) as usize).as_slice())
    }
    #[func]
    fn biome_landform_of(&self, id: i64) -> PackedFloat32Array {
        PackedFloat32Array::from(self.world.biome_landform_of(id.max(0) as usize).as_slice())
    }
    #[func]
    fn biome_water_of(&self, id: i64) -> PackedFloat32Array {
        PackedFloat32Array::from(self.world.biome_water_of(id.max(0) as usize).as_slice())
    }
    #[func]
    fn set_biome_landform(&mut self, id: i64, idx: i64, v: f64) {
        self.world.set_biome_landform(id.max(0) as usize, idx.max(0) as usize, v as f32);
    }
    #[func]
    fn set_biome_water(&mut self, id: i64, idx: i64, v: f64) {
        self.world.set_biome_water(id.max(0) as usize, idx.max(0) as usize, v as f32);
    }
    /// Set a region's landform component (`idx`: 0 jaggedness, 1 relief, 2 foothill_falloff,
    /// 3 erosion) and stamp it onto that region's cells, so the next `shape_terrain` reshapes the
    /// whole region. Ties the landform dials to the Regions instead of a per-cell brush.
    #[func]
    fn set_region_landform(&mut self, region_id: i64, idx: i64, value: f64) {
        self.world.set_region_landform(region_id.max(0) as u8, idx.max(0) as usize, value);
    }

    // --- traits (trait-composition model; ADR 0004) ---

    /// Paint one trait into the brush footprint. `trait_id`: 0 jaggedness, 1 relief,
    /// 2 foothill_falloff, 3 erosion, 4 temperature, 5 moisture, 6 vegetation (value = enum idx),
    /// 7 palette_family (value = enum idx). Returns the cells touched (for partial mesh updates).
    #[func]
    fn paint_trait(&mut self, cx: f64, cy: f64, radius: f64, trait_id: i64, value: f64) -> PackedInt32Array {
        u32_to_packed(&self.world.paint_trait(cx, cy, radius, trait_id.max(0) as u32, value))
    }
    /// Stamp a Region preset's full trait bundle into the footprint ("this area is X").
    #[func]
    fn paint_region_traits(&mut self, cx: f64, cy: f64, radius: f64, biome_id: i64) -> PackedInt32Array {
        u32_to_packed(&self.world.paint_region_traits(cx, cy, radius, biome_id.max(0) as u8))
    }
    /// Per-cell trait at world `(x, y)`; `trait_id` as in `paint_trait` (enums → index). NaN off-map.
    #[func]
    fn trait_at(&self, x: f64, y: f64, trait_id: i64) -> f64 {
        self.world.trait_at(x, y, trait_id.max(0) as u32).unwrap_or(f64::NAN)
    }
    /// One slot of base palette `family` as `[r,g,b]`. `slot`: 0 water_deep, 1 water_shallow,
    /// 2 low, 3 rock, 4 cap_warm, 5 cap_cold.
    #[func]
    fn base_palette_color(&self, family: i64, slot: i64) -> PackedFloat32Array {
        PackedFloat32Array::from(self.world.base_palette_color(family.max(0) as usize, slot.max(0) as usize).as_slice())
    }
    /// Set a base-palette slot and recolour the world (flags all chunks dirty).
    #[func]
    fn set_base_palette_color(&mut self, family: i64, slot: i64, r: f64, g: f64, b: f64) {
        self.world.set_base_palette_color(family.max(0) as usize, slot.max(0) as usize, r as f32, g as f32, b as f32);
    }
    /// Diffuse the scalar trait fields across painted-region borders so they ease into natural
    /// skirts (the transition buffer). `transition_width_m` sets the band width. Idempotent on
    /// re-run; recolours the world (flags all chunks dirty — re-tessellate via `take_dirty_chunks`).
    #[func]
    fn blend_traits(&mut self, transition_width_m: f64) {
        self.world.blend_traits(transition_width_m);
    }
    /// Switch the colour view: 0 Natural, 1 Temperature, 2 Moisture, 3 Elevation, 4 Biome. Data
    /// views recolour the same meshes by one field (a heatmap) so it can be read/painted directly.
    /// Recolours the world (flags all chunks dirty); the minimap follows the same mode.
    #[func]
    fn set_view_mode(&mut self, mode: i64) {
        self.world.set_view_mode(mode.max(0) as u8);
    }
    /// The active colour view (see `set_view_mode`).
    #[func]
    fn view_mode(&self) -> i64 {
        self.world.view_mode() as i64
    }
    /// Bake the landform trait dials (jaggedness/relief/foothill_falloff/erosion) into the terrain
    /// height. `strength` is a global gain. Idempotent on re-run; recolours + flags all chunks
    /// dirty (geometry changed — re-tessellate via `take_dirty_chunks` + rebuild the liquid).
    #[func]
    fn shape_terrain(&mut self, strength: f64) {
        self.world.shape_terrain(strength);
    }

    // --- selection / boundary tools ---

    /// Region nearest to world `(x, y)`, excluding the boundary frame; `-1` if none.
    #[func]
    fn region_at(&self, x: f64, y: f64) -> i64 {
        self.world.region_at(x, y).map(|r| r as i64).unwrap_or(-1)
    }
    /// Surface hit point (Godot Y-up) of a ray, or an empty array on a miss. C#: read
    /// `.As<Vector3[]>()` — length 1 = hit, length 0 = no terrain under the cursor. Lets the
    /// brush land on the actual surface under the cursor from any view angle.
    #[func]
    fn raycast_terrain(&self, origin: Vector3, dir: Vector3, exaggeration: f64) -> PackedVector3Array {
        match self.world.raycast_terrain(
            origin.x as f64, origin.y as f64, origin.z as f64,
            dir.x as f64, dir.y as f64, dir.z as f64,
            exaggeration,
        ) {
            Some(h) => {
                let v = Vector3::new(h[0] as f32, h[1] as f32, h[2] as f32);
                PackedVector3Array::from(&[v][..])
            }
            None => PackedVector3Array::new(),
        }
    }
    /// Normalized terrain elevation at world ground `(x, y)` (Godot XZ); NaN outside the map.
    #[func]
    fn height_at(&self, x: f64, y: f64) -> f64 {
        self.world.height_at(x, y).unwrap_or(f64::NAN)
    }
    /// Top-down minimap as an `n×n` RGBA byte buffer (biome colour + hill-shade + contours +
    /// liquid). C#: `Image.CreateFromData(n, n, false, Image.Format.Rgba8, bytes)`.
    #[func]
    fn minimap(&self, n: i64, light_x: f64, light_y: f64) -> PackedByteArray {
        PackedByteArray::from(self.world.minimap(n.max(1) as usize, light_x, light_y).as_slice())
    }
    #[func]
    fn biome_at(&self, region: i64) -> i64 {
        self.world.biome_at(region.max(0) as usize) as i64
    }
    #[func]
    fn set_biome_of(&mut self, region: i64, id: i64) {
        self.world.set_biome_of(region.max(0) as usize, id.max(0) as u8);
    }
    #[func]
    fn select_contiguous(&self, region: i64) -> PackedInt32Array {
        u32_to_packed(&self.world.select_contiguous(region.max(0) as usize))
    }
    #[func]
    fn selection_indices(&self, regions: PackedInt32Array) -> PackedInt32Array {
        let regs: Vec<u32> = regions.to_vec().iter().map(|&i| i as u32).collect();
        u32_to_packed(&self.world.selection_indices(&regs))
    }
    #[func]
    fn regions_in_polygon(&self, xs: PackedFloat32Array, ys: PackedFloat32Array) -> PackedInt32Array {
        let xs: Vec<f64> = xs.to_vec().iter().map(|&v| v as f64).collect();
        let ys: Vec<f64> = ys.to_vec().iter().map(|&v| v as f64).collect();
        u32_to_packed(&self.world.regions_in_polygon(&xs, &ys))
    }

    // --- save / load (authored state) ---

    #[func]
    fn course_mask_export(&self) -> PackedByteArray {
        PackedByteArray::from(self.world.course_mask_export().as_slice())
    }
    #[func]
    fn set_course_mask(&mut self, m: PackedByteArray) {
        self.world.set_course_mask(&m.to_vec());
    }
    #[func]
    fn elevation_export(&self) -> PackedFloat32Array {
        PackedFloat32Array::from(self.world.elevation_export().as_slice())
    }
    #[func]
    fn biome_export(&self) -> PackedByteArray {
        PackedByteArray::from(self.world.biome_export().as_slice())
    }
    #[func]
    fn liquid_depth_export(&self) -> PackedFloat32Array {
        PackedFloat32Array::from(self.world.liquid_depth_export().as_slice())
    }
    #[func]
    fn liquid_kind_export(&self) -> PackedByteArray {
        PackedByteArray::from(self.world.liquid_kind_export().as_slice())
    }
    #[func]
    fn set_elevation(&mut self, e: PackedFloat32Array) {
        self.world.set_elevation(&e.to_vec());
    }
    #[func]
    fn set_biome(&mut self, b: PackedByteArray) {
        self.world.set_biome(&b.to_vec());
    }
    #[func]
    fn set_liquid(&mut self, depth: PackedFloat32Array, kind: PackedByteArray) {
        self.world.set_liquid(&depth.to_vec(), &kind.to_vec());
    }

    // --- decoration scatter ---

    #[func]
    fn tessellate_scatter(&mut self, exaggeration: f64, density: f64, seed: f64) {
        let inst = self.world.scatter_instances(exaggeration, density, seed as u64);
        let mut data = Vec::with_capacity(inst.len() * 5);
        for i in &inst {
            data.extend_from_slice(&[i.x, i.y, i.z, i.scale, i.species]);
        }
        self.scatter_n = inst.len();
        self.scatter_buf = data;
    }
    #[func]
    fn scatter_data(&self) -> PackedFloat32Array {
        PackedFloat32Array::from(self.scatter_buf.as_slice())
    }
    #[func]
    fn scatter_count(&self) -> i64 {
        self.scatter_n as i64
    }
}

/// Pack `u32` region/triangle indices into a Godot `PackedInt32Array`.
fn u32_to_packed(v: &[u32]) -> PackedInt32Array {
    let s: Vec<i32> = v.iter().map(|&i| i as i32).collect();
    PackedInt32Array::from(s.as_slice())
}

/// Pack a flat `[x, y, z, ...]` f32 buffer (core z-up) into Godot `Vector3`s, remapped
/// to Y-up: Godot `(X, Y, Z)` = core `(x, z=height, y)`. The remap runs here (Rust), so
/// C# never iterates the vertex array.
fn to_vec3_yup(flat: &[f32]) -> PackedVector3Array {
    let n = flat.len() / 3;
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        v.push(Vector3::new(flat[3 * i], flat[3 * i + 2], flat[3 * i + 1]));
    }
    PackedVector3Array::from(v.as_slice())
}

/// Pack a flat `[r, g, b, ...]` f32 buffer into opaque Godot `Color`s.
fn to_colors(flat: &[f32]) -> PackedColorArray {
    let n = flat.len() / 3;
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        v.push(Color::from_rgba(flat[3 * i], flat[3 * i + 1], flat[3 * i + 2], 1.0));
    }
    PackedColorArray::from(v.as_slice())
}
