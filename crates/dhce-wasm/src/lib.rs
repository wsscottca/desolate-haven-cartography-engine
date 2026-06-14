//! Thin wasm-bindgen adapter exposing `dhce-core` to the browser front-end.
//!
//! The resident `WasmEngine` holds the mesh, elevation, and a liquid field between
//! calls. `build` constructs mesh + elevation; `tessellate`/`tessellate_liquid` pack
//! the render surfaces; the liquid API (`set_sea_level`, `rain`, `step_fluid`) runs
//! the hydraulic sim. Phase 6 moves this behind a worker + shared memory views.

use dhce_core::fluid::{self, LiquidField, LiquidSurface};
use dhce_core::mesh::Mesh;
use dhce_core::{elevation, geometry};
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

/// Resident generation + simulation engine.
#[wasm_bindgen]
pub struct WasmEngine {
    width: f64,
    height: f64,
    mesh: Option<Mesh>,
    elevation_r: Vec<f64>,
    neighbors: Vec<Vec<u32>>,
    field: LiquidField,
    sea_level: f64,
    surface: Option<geometry::Surface>,
    liquid: Option<LiquidSurface>,
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
            width: 0.0,
            height: 0.0,
            mesh: None,
            elevation_r: Vec::new(),
            neighbors: Vec::new(),
            field: LiquidField::new(0),
            sea_level: 0.0,
            surface: None,
            liquid: None,
        }
    }

    /// Build mesh + per-region elevation over `[0,width] × [0,height]` at `spacing`,
    /// seeded by `seed` with `octaves` of noise. Re-applies the stored sea level so
    /// water tracks the new terrain.
    pub fn build(&mut self, width: f64, height: f64, spacing: f64, seed: f64, octaves: u32) {
        let mesh = Mesh::new(width, height, spacing, seed as u64);
        let nr = mesh.num_regions();
        self.elevation_r = elevation::assign_region_elevation(&mesh, width, height, seed as u64, octaves);
        self.neighbors = mesh.region_neighbors();
        self.field = LiquidField::new(nr);
        fluid::sea_fill(&mut self.field, &self.elevation_r, self.sea_level);
        self.width = width;
        self.height = height;
        self.mesh = Some(mesh);
        self.surface = None;
        self.liquid = None;
    }

    /// Pack the terrain render surface at a vertical `exaggeration`.
    pub fn tessellate(&mut self, exaggeration: f64) {
        if let Some(mesh) = &self.mesh {
            self.surface = Some(geometry::build_surface(mesh, &self.elevation_r, exaggeration));
        }
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
    pub fn indices(&self) -> Vec<u32> {
        self.surface.as_ref().map(|s| s.indices.clone()).unwrap_or_default()
    }

    pub fn region_count(&self) -> usize {
        self.mesh.as_ref().map(|m| m.num_regions()).unwrap_or(0)
    }
    pub fn triangle_count(&self) -> usize {
        self.mesh.as_ref().map(|m| m.num_triangles()).unwrap_or(0)
    }

    // --- liquid simulation ---
    /// Fill every region below `level` with water (instant sea + lakes).
    pub fn set_sea_level(&mut self, level: f64) {
        self.sea_level = level;
        fluid::sea_fill(&mut self.field, &self.elevation_r, level);
    }

    /// Add a uniform `amount` of rainfall to land above the current sea level.
    pub fn rain(&mut self, amount: f64) {
        fluid::add_rain(&mut self.field, &self.elevation_r, self.sea_level, amount);
    }

    /// Advance the hydraulic solver `substeps` relaxation steps.
    pub fn step_fluid(&mut self, flow_rate: f64, evaporation: f64, substeps: u32) {
        for _ in 0..substeps {
            fluid::relax_step(&mut self.field, &self.elevation_r, &self.neighbors, flow_rate, evaporation);
        }
    }

    /// Remove all liquid.
    pub fn clear_liquid(&mut self) {
        self.field.clear();
    }

    // --- brush tools (Phase 4) ---
    /// Sculpt the terrain under `(cx, cy)` within `radius`. `mode`: 0 raise, 1 carve,
    /// 2 level (toward the height at the brush center), 3 crest (sharp peak).
    /// `strength` is the per-application elevation delta. JS re-tessellates after.
    pub fn paint_terrain(&mut self, cx: f64, cy: f64, radius: f64, strength: f64, mode: u32) {
        let mesh = match &self.mesh {
            Some(m) => m,
            None => return,
        };
        let r2 = radius * radius;

        // Center height for the level tool (nearest region to the cursor).
        let mut center_e = 0.0;
        if mode == 2 {
            let mut best = f64::INFINITY;
            for ri in 0..mesh.num_regions() {
                let p = mesh.pos_of_r(ri);
                let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2);
                if d2 < best {
                    best = d2;
                    center_e = self.elevation_r[ri];
                }
            }
        }

        for ri in 0..mesh.num_regions() {
            let p = mesh.pos_of_r(ri);
            let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2);
            if d2 >= r2 {
                continue;
            }
            let t = 1.0 - (d2 / r2).sqrt(); // 1 at center → 0 at the rim
            let w = t * t * (3.0 - 2.0 * t); // smoothstep falloff
            let e = &mut self.elevation_r[ri];
            match mode {
                0 => *e += strength * w,
                1 => *e -= strength * w,
                2 => *e += (center_e - *e) * w * 0.5,
                3 => *e += strength * (t * t * t) * 2.5, // sharper, peaked
                _ => {}
            }
            *e = e.clamp(-1.5, 1.5);
        }
    }

    /// Place `amount` of liquid `kind` (0 water, 1 lava) under `(cx, cy)` within
    /// `radius`. Used by the Course (trickle) and Flood (pour) tools.
    pub fn paint_liquid(&mut self, cx: f64, cy: f64, radius: f64, amount: f64, kind: u32) {
        let mesh = match &self.mesh {
            Some(m) => m,
            None => return,
        };
        let r2 = radius * radius;
        for ri in 0..mesh.num_regions() {
            let p = mesh.pos_of_r(ri);
            let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2);
            if d2 >= r2 {
                continue;
            }
            let t = 1.0 - (d2 / r2).sqrt();
            let add = amount * t * t * (3.0 - 2.0 * t);
            if add > 0.0 {
                self.field.depth[ri] += add;
                self.field.kind[ri] = kind as u8;
            }
        }
    }

    /// Pack the liquid render surface at a vertical `exaggeration`.
    pub fn tessellate_liquid(&mut self, exaggeration: f64) {
        if let Some(mesh) = &self.mesh {
            self.liquid = Some(fluid::liquid_surface(mesh, &self.elevation_r, &self.field, exaggeration));
        }
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
}
