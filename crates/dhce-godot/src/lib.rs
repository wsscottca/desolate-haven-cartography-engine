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
    liquid: Option<LiquidSurface>,
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
            liquid: None,
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

    // --- selection / boundary tools ---

    /// Region nearest to world `(x, y)`, excluding the boundary frame; `-1` if none.
    #[func]
    fn region_at(&self, x: f64, y: f64) -> i64 {
        self.world.region_at(x, y).map(|r| r as i64).unwrap_or(-1)
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
