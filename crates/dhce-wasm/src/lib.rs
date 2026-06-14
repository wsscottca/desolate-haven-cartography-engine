//! Thin wasm-bindgen adapter exposing `dhce-core` to the browser front-end.
//!
//! All state and compute live in [`dhce_core::world::World`]; this shell only marshals
//! types across the JS↔WASM boundary and caches the packed render surfaces for the
//! getter-per-array access pattern the front-end uses. The browser tool is frozen post
//! native pivot — kept as a determinism cross-check against the Godot adapter (both
//! drive the same `World`, so a given seed/edit sequence must produce identical output).

use dhce_core::fluid::LiquidSurface;
use dhce_core::geometry::Surface;
use dhce_core::world::World;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// Engine version string — diagnostics + JS↔WASM boundary smoke test.
#[wasm_bindgen]
pub fn version() -> String {
    dhce_core::VERSION.to_string()
}

/// Resident generation + simulation engine (a thin wrapper over the core `World`).
#[wasm_bindgen]
pub struct WasmEngine {
    world: World,
    surface: Option<Surface>,
    liquid: Option<LiquidSurface>,
    scatter_buf: Vec<f32>,
    scatter_n: usize,
}

impl Default for WasmEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmEngine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmEngine {
        WasmEngine {
            world: World::new(),
            surface: None,
            liquid: None,
            scatter_buf: Vec::new(),
            scatter_n: 0,
        }
    }

    /// Build mesh + per-region elevation over `[0,width] × [0,height]` at `spacing`,
    /// seeded by `seed` with `octaves` of noise.
    pub fn build(&mut self, width: f64, height: f64, spacing: f64, seed: f64, octaves: u32) {
        self.world.build(width, height, spacing, seed as u64, octaves);
        self.surface = None;
        self.liquid = None;
    }

    /// Pack the terrain render surface at a vertical `exaggeration`.
    pub fn tessellate(&mut self, exaggeration: f64) {
        self.surface = self.world.surface(exaggeration);
    }

    // --- terrain surface getters ---
    pub fn positions(&self) -> Vec<f32> {
        self.surface.as_ref().map(|s| s.positions.clone()).unwrap_or_default()
    }
    pub fn normals(&self) -> Vec<f32> {
        self.surface.as_ref().map(|s| s.normals.clone()).unwrap_or_default()
    }
    pub fn heights(&self) -> Vec<f32> {
        self.surface.as_ref().map(|s| s.heights.clone()).unwrap_or_default()
    }
    pub fn colors(&self) -> Vec<f32> {
        self.surface.as_ref().map(|s| s.colors.clone()).unwrap_or_default()
    }
    pub fn indices(&self) -> Vec<u32> {
        self.surface.as_ref().map(|s| s.indices.clone()).unwrap_or_default()
    }

    pub fn region_count(&self) -> usize {
        self.world.region_count()
    }
    pub fn triangle_count(&self) -> usize {
        self.world.triangle_count()
    }

    // --- liquid simulation ---
    pub fn set_sea_level(&mut self, level: f64) {
        self.world.set_sea_level(level);
    }
    pub fn rain(&mut self, amount: f64) {
        self.world.rain(amount);
    }
    pub fn step_fluid(&mut self, flow_rate: f64, evaporation: f64, substeps: u32) {
        self.world.step_fluid(flow_rate, evaporation, substeps);
    }
    pub fn clear_liquid(&mut self) {
        self.world.clear_liquid();
    }

    // --- brush tools ---
    pub fn paint_terrain(&mut self, cx: f64, cy: f64, radius: f64, strength: f64, mode: u32) {
        self.world.paint_terrain(cx, cy, radius, strength, mode);
    }
    pub fn paint_liquid(&mut self, cx: f64, cy: f64, radius: f64, amount: f64, kind: u32) {
        self.world.paint_liquid(cx, cy, radius, amount, kind as u8);
    }
    pub fn paint_course(&mut self, cx: f64, cy: f64, radius: f64, intensity: f64, kind: u32) {
        self.world.paint_course(cx, cy, radius, intensity, kind as u8);
    }
    pub fn generate_streams(&mut self, threshold: f64, depth_gain: f64) {
        self.world.generate_streams(threshold, depth_gain);
    }

    /// Pack the liquid render surface at a vertical `exaggeration`.
    pub fn tessellate_liquid(&mut self, exaggeration: f64) {
        self.liquid = self.world.liquid_surface(exaggeration);
    }

    // --- liquid surface getters ---
    pub fn liquid_positions(&self) -> Vec<f32> {
        self.liquid.as_ref().map(|s| s.positions.clone()).unwrap_or_default()
    }
    pub fn liquid_normals(&self) -> Vec<f32> {
        self.liquid.as_ref().map(|s| s.normals.clone()).unwrap_or_default()
    }
    pub fn liquid_types(&self) -> Vec<f32> {
        self.liquid.as_ref().map(|s| s.types.clone()).unwrap_or_default()
    }
    pub fn liquid_indices(&self) -> Vec<u32> {
        self.liquid.as_ref().map(|s| s.indices.clone()).unwrap_or_default()
    }

    // --- biomes ---
    pub fn paint_biome(&mut self, cx: f64, cy: f64, radius: f64, biome_id: u32) {
        self.world.paint_biome(cx, cy, radius, biome_id as u8);
    }
    pub fn set_biome_color(&mut self, id: u32, r: f32, g: f32, b: f32) {
        self.world.set_biome_color(id as usize, r, g, b);
    }
    pub fn biome_color_of(&self, id: u32) -> Vec<f32> {
        self.world.biome_color_of(id as usize).to_vec()
    }
    pub fn biome_landform_of(&self, id: u32) -> Vec<f32> {
        self.world.biome_landform_of(id as usize).to_vec()
    }
    pub fn biome_water_of(&self, id: u32) -> Vec<f32> {
        self.world.biome_water_of(id as usize).to_vec()
    }
    pub fn set_biome_landform(&mut self, id: u32, idx: u32, v: f32) {
        self.world.set_biome_landform(id as usize, idx as usize, v);
    }
    pub fn set_biome_water(&mut self, id: u32, idx: u32, v: f32) {
        self.world.set_biome_water(id as usize, idx as usize, v);
    }

    // --- selection / boundary tools ---
    pub fn region_at(&self, x: f64, y: f64) -> i32 {
        self.world.region_at(x, y).map(|r| r as i32).unwrap_or(-1)
    }
    pub fn biome_at(&self, region: u32) -> u8 {
        self.world.biome_at(region as usize)
    }
    pub fn set_biome_of(&mut self, region: u32, id: u32) {
        self.world.set_biome_of(region as usize, id as u8);
    }
    pub fn select_contiguous(&self, region: u32) -> Vec<u32> {
        self.world.select_contiguous(region as usize)
    }
    pub fn selection_indices(&self, regions: &[u32]) -> Vec<u32> {
        self.world.selection_indices(regions)
    }
    pub fn regions_in_polygon(&self, xs: &[f64], ys: &[f64]) -> Vec<u32> {
        self.world.regions_in_polygon(xs, ys)
    }

    // --- save / load (authored state) ---
    pub fn course_mask_export(&self) -> Vec<u8> {
        self.world.course_mask_export()
    }
    pub fn set_course_mask(&mut self, m: &[u8]) {
        self.world.set_course_mask(m);
    }
    pub fn elevation_export(&self) -> Vec<f32> {
        self.world.elevation_export()
    }
    pub fn biome_export(&self) -> Vec<u8> {
        self.world.biome_export()
    }
    pub fn liquid_depth_export(&self) -> Vec<f32> {
        self.world.liquid_depth_export()
    }
    pub fn liquid_kind_export(&self) -> Vec<u8> {
        self.world.liquid_kind_export()
    }
    pub fn set_elevation(&mut self, e: &[f32]) {
        self.world.set_elevation(e);
    }
    pub fn set_biome(&mut self, b: &[u8]) {
        self.world.set_biome(b);
    }
    pub fn set_liquid(&mut self, depth: &[f32], kind: &[u8]) {
        self.world.set_liquid(depth, kind);
    }

    // --- decoration scatter ---
    pub fn tessellate_scatter(&mut self, exaggeration: f64, density: f64, seed: f64) {
        let inst = self.world.scatter_instances(exaggeration, density, seed as u64);
        let mut data = Vec::with_capacity(inst.len() * 5);
        for i in &inst {
            data.extend_from_slice(&[i.x, i.y, i.z, i.scale, i.species]);
        }
        self.scatter_n = inst.len();
        self.scatter_buf = data;
    }
    pub fn scatter_data(&self) -> Vec<f32> {
        self.scatter_buf.clone()
    }
    pub fn scatter_count(&self) -> usize {
        self.scatter_n
    }
}
