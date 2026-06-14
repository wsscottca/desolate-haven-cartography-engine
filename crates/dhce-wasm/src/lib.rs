//! Thin wasm-bindgen adapter exposing `dhce-core` to the browser front-end.
//!
//! The resident `WasmEngine` holds the mesh, elevation, and a liquid field between
//! calls. `build` constructs mesh + elevation; `tessellate`/`tessellate_liquid` pack
//! the render surfaces; the liquid API (`set_sea_level`, `rain`, `step_fluid`) runs
//! the hydraulic sim. Phase 6 moves this behind a worker + shared memory views.

use dhce_core::fluid::{self, LiquidField, LiquidSurface};
use dhce_core::mesh::Mesh;
use dhce_core::{biomes, elevation, geometry, streams};
use wasm_bindgen::prelude::*;

/// Channel-carve depth per unit tool intensity (Course tool).
const CHANNEL_DEPTH_GAIN: f64 = 6.0;
/// Thin water laid down per unit tool intensity as a Course stroke is painted.
const COURSE_WATER_GAIN: f64 = 3.0;
/// Per-region color smoothing: passes + blend toward the neighbour mean (softens hard
/// biome-block seams) and a subtle deterministic brightness jitter to break up flatness.
const COLOR_SMOOTH_ITERS: usize = 2;
const COLOR_SMOOTH_W: f32 = 0.4;
const COLOR_VAR: f32 = 0.04;

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
    seed: u64,
    biome_r: Vec<u8>,
    biome_color: Vec<[f32; 3]>,
    /// Per-biome (id 1..=14) editable landform profile: peak_shape, hill_amp,
    /// valley_floor, roughness. Seeded from the roster; surfaced in the biome editor.
    biome_landform: Vec<[f32; 4]>,
    /// Per-biome water profile: raininess, rain_shadow, evaporation, flow, ocean_depth.
    biome_water: Vec<[f32; 5]>,
    /// True where the user manually set a region's biome — protected from auto-reclassify.
    biome_locked: Vec<bool>,
    /// True where the user painted a Course (main-river) stroke — the stream seed mask.
    course_mask: Vec<bool>,
    /// Per-region elevation lowering currently applied by the last stream pass, so a
    /// re-run restores then re-carves (idempotent + adapts to edits in between).
    stream_carve: Vec<f64>,
    grid_cell: f64,
    grid_cols: usize,
    grid_rows: usize,
    grid: Vec<Vec<u32>>,
    scatter_buf: Vec<f32>,
    scatter_n: usize,
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
        let roster = biomes::roster();
        let mut biome_color = vec![[0.5f32, 0.5, 0.5]; biomes::BIOME_COUNT + 1];
        let mut biome_landform = vec![[0.0f32; 4]; biomes::BIOME_COUNT + 1];
        let mut biome_water = vec![[0.0f32; 5]; biomes::BIOME_COUNT + 1];
        for (i, b) in roster.iter().enumerate() {
            biome_color[i + 1] = b.color;
            let l = &b.landform;
            biome_landform[i + 1] = [l.peak_shape, l.hill_amp, l.valley_floor, l.roughness];
            let w = &b.water;
            biome_water[i + 1] = [w.raininess, w.rain_shadow, w.evaporation, w.flow, w.ocean_depth];
        }
        WasmEngine {
            width: 0.0,
            height: 0.0,
            mesh: None,
            elevation_r: Vec::new(),
            neighbors: Vec::new(),
            field: LiquidField::new(0),
            sea_level: 0.0,
            seed: 0,
            biome_r: Vec::new(),
            biome_color,
            biome_landform,
            biome_water,
            biome_locked: Vec::new(),
            course_mask: Vec::new(),
            stream_carve: Vec::new(),
            grid_cell: 0.0,
            grid_cols: 0,
            grid_rows: 0,
            grid: Vec::new(),
            scatter_buf: Vec::new(),
            scatter_n: 0,
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
        self.seed = seed as u64;
        self.elevation_r = elevation::assign_region_elevation(&mesh, width, height, seed as u64, octaves);
        self.neighbors = mesh.region_neighbors();
        self.field = LiquidField::new(nr);
        self.biome_locked = vec![false; nr];
        self.course_mask = vec![false; nr];
        self.stream_carve = vec![0.0; nr];
        fluid::sea_fill(&mut self.field, &self.elevation_r, self.sea_level);

        // Auto-classify biomes from elevation + moisture + distance-from-center.
        let cx = width * 0.5;
        let cy = height * 0.5;
        let max_d = 0.5 * (width * width + height * height).sqrt();
        let mut biome_r = vec![0u8; nr];
        for r in 0..nr {
            let p = mesh.pos_of_r(r);
            let dist = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt() / max_d;
            let moist = biomes::moisture_at(p[0], p[1], width, height, seed as u64);
            biome_r[r] = biomes::classify(self.elevation_r[r], moist, dist);
        }
        self.biome_r = biome_r;

        // Spatial grid for O(brush) brush queries (keeps painting fast at high detail).
        let grid_cell = (width.max(height) / 64.0).max(1.0);
        let cols = (width / grid_cell).ceil() as usize + 1;
        let rows = (height / grid_cell).ceil() as usize + 1;
        let mut grid: Vec<Vec<u32>> = vec![Vec::new(); cols * rows];
        for r in 0..nr {
            let p = mesh.pos_of_r(r);
            let gx = ((p[0] / grid_cell) as usize).min(cols - 1);
            let gy = ((p[1] / grid_cell) as usize).min(rows - 1);
            grid[gy * cols + gx].push(r as u32);
        }
        self.grid_cell = grid_cell;
        self.grid_cols = cols;
        self.grid_rows = rows;
        self.grid = grid;

        self.width = width;
        self.height = height;
        self.mesh = Some(mesh);
        self.surface = None;
        self.liquid = None;
    }

    /// Pack the terrain render surface at a vertical `exaggeration`.
    pub fn tessellate(&mut self, exaggeration: f64) {
        let colors = self.region_color();
        if let Some(mesh) = &self.mesh {
            self.surface = Some(geometry::build_surface(mesh, &self.elevation_r, exaggeration, &colors));
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
    pub fn colors(&self) -> Vec<f32> {
        self.surface.as_ref().map(|s| s.colors.clone()).unwrap_or_default()
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
        if self.mesh.is_none() {
            return;
        }
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);

        // Center height for the level tool (nearest candidate to the cursor).
        let mut center_e = 0.0;
        if mode == 2 {
            let mesh = self.mesh.as_ref().unwrap();
            let mut best = f64::INFINITY;
            for &ri in &candidates {
                let p = mesh.pos_of_r(ri as usize);
                let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2);
                if d2 < best {
                    best = d2;
                    center_e = self.elevation_r[ri as usize];
                }
            }
        }

        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
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
        // Elevation changed → re-classify the auto-biome cells under the brush so the
        // coloring tracks the new terrain (manually painted cells stay locked).
        self.reclassify(&candidates);
    }

    /// Place `amount` of liquid `kind` (0 water, 1 lava) under `(cx, cy)` within
    /// `radius`. Used by the Course (trickle) and Flood (pour) tools.
    pub fn paint_liquid(&mut self, cx: f64, cy: f64, radius: f64, amount: f64, kind: u32) {
        if self.mesh.is_none() {
            return;
        }
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);
        let mesh = self.mesh.as_ref().unwrap();
        for &rid in &candidates {
            let ri = rid as usize;
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

    /// Course tool: carve a river channel under `(cx, cy)` while laying thin water, and
    /// tag the painted cells as a main-river seed for [`generate_streams`]. The carve is
    /// idempotent under a dragged stroke (it lowers *toward* a bed, never past it).
    pub fn paint_course(&mut self, cx: f64, cy: f64, radius: f64, intensity: f64, kind: u32) {
        if self.mesh.is_none() {
            return;
        }
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);

        // Bed reference = lowest elevation under the brush this dab (the channel follows
        // the existing downhill grade rather than cutting a flat trench).
        let mut e_ref = f64::INFINITY;
        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
                let p = mesh.pos_of_r(ri);
                if (p[0] - cx).powi(2) + (p[1] - cy).powi(2) < r2 && self.elevation_r[ri] < e_ref {
                    e_ref = self.elevation_r[ri];
                }
            }
        }
        if !e_ref.is_finite() {
            return;
        }

        let target_depth = intensity * CHANNEL_DEPTH_GAIN;
        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
                let p = mesh.pos_of_r(ri);
                let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2);
                if d2 >= r2 {
                    continue;
                }
                let t = 1.0 - (d2 / r2).sqrt();
                let prof = t * t; // U-channel cross-section
                let bed = (e_ref - target_depth * prof).clamp(-1.5, 1.5);
                if self.elevation_r[ri] > bed {
                    self.elevation_r[ri] = bed;
                }
                let add = intensity * COURSE_WATER_GAIN * (t * t * (3.0 - 2.0 * t));
                if add > 0.0 {
                    self.field.depth[ri] += add;
                    self.field.kind[ri] = kind as u8;
                }
                if ri < self.course_mask.len() {
                    self.course_mask[ri] = true;
                }
            }
        }
        self.reclassify(&candidates);
    }

    /// Grow procedural tributary streams that drain into the painted Course rivers.
    /// `threshold` ∈ [0,1] (lower ⇒ more, finer tributaries); `depth_gain` scales the
    /// channel depth with flow. A no-op until at least one Course stroke is painted.
    pub fn generate_streams(&mut self, threshold: f64, depth_gain: f64) {
        if self.mesh.is_none() {
            return;
        }
        let n = self.elevation_r.len();
        if self.stream_carve.len() != n {
            self.stream_carve = vec![0.0; n];
        }
        if self.course_mask.len() != n {
            self.course_mask = vec![false; n];
        }
        // Restore the previous stream carving first, so re-running is idempotent and
        // adapts to any sculpting done in between.
        for r in 0..n {
            self.elevation_r[r] = (self.elevation_r[r] + self.stream_carve[r]).clamp(-1.5, 1.5);
            self.stream_carve[r] = 0.0;
        }

        let num_b = self.mesh.as_ref().unwrap().num_boundary_regions();
        let res = streams::accumulate(&self.elevation_r, &self.neighbors, num_b, &self.course_mask, threshold, depth_gain);

        for r in 0..n {
            let cd = res.carve_delta[r];
            if cd > 0.0 {
                let before = self.elevation_r[r];
                let after = (before - cd).clamp(-1.5, 1.5);
                self.stream_carve[r] = before - after; // record the actual lowering
                self.elevation_r[r] = after;
                let wt = ((before - after) * 0.5).min(0.2);
                if self.field.depth[r] < wt {
                    self.field.depth[r] = wt;
                }
                self.field.kind[r] = 0; // water
            }
        }

        let idx: Vec<u32> = (0..n).filter(|&r| res.is_stream[r]).map(|r| r as u32).collect();
        self.reclassify(&idx);
    }

    /// Pack the liquid render surface at a vertical `exaggeration`.
    pub fn tessellate_liquid(&mut self, exaggeration: f64) {
        if let Some(mesh) = &self.mesh {
            self.liquid = Some(fluid::liquid_surface(mesh, &self.elevation_r, &self.field, &self.neighbors, exaggeration));
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

    // --- biomes (Phase 5) ---
    /// Assign `biome_id` (1..=14) to every region under `(cx, cy)` within `radius`.
    pub fn paint_biome(&mut self, cx: f64, cy: f64, radius: f64, biome_id: u32) {
        if self.mesh.is_none() {
            return;
        }
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);
        let mesh = self.mesh.as_ref().unwrap();
        for &rid in &candidates {
            let ri = rid as usize;
            let p = mesh.pos_of_r(ri);
            if (p[0] - cx).powi(2) + (p[1] - cy).powi(2) < r2 {
                self.biome_r[ri] = biome_id as u8;
                if ri < self.biome_locked.len() {
                    self.biome_locked[ri] = true; // manual paint — protect from reclassify
                }
            }
        }
    }

    /// Set the display color of biome `id` (1..=14).
    pub fn set_biome_color(&mut self, id: u32, r: f32, g: f32, b: f32) {
        if let Some(c) = self.biome_color.get_mut(id as usize) {
            *c = [r, g, b];
        }
    }

    /// Current display color of biome `id` as `[r, g, b]`.
    pub fn biome_color_of(&self, id: u32) -> Vec<f32> {
        self.biome_color
            .get(id as usize)
            .map(|c| c.to_vec())
            .unwrap_or_else(|| vec![0.5, 0.5, 0.5])
    }

    /// Editable landform profile of biome `id`: [peak_shape, hill_amp, valley_floor, roughness].
    pub fn biome_landform_of(&self, id: u32) -> Vec<f32> {
        self.biome_landform.get(id as usize).map(|a| a.to_vec()).unwrap_or_else(|| vec![0.0; 4])
    }
    /// Editable water profile of biome `id`: [raininess, rain_shadow, evaporation, flow, ocean_depth].
    pub fn biome_water_of(&self, id: u32) -> Vec<f32> {
        self.biome_water.get(id as usize).map(|a| a.to_vec()).unwrap_or_else(|| vec![0.0; 5])
    }
    /// Set field `idx` (0..4) of biome `id`'s landform profile.
    pub fn set_biome_landform(&mut self, id: u32, idx: u32, v: f32) {
        if let Some(a) = self.biome_landform.get_mut(id as usize) {
            if (idx as usize) < 4 {
                a[idx as usize] = v;
            }
        }
    }
    /// Set field `idx` (0..5) of biome `id`'s water profile.
    pub fn set_biome_water(&mut self, id: u32, idx: u32, v: f32) {
        if let Some(a) = self.biome_water.get_mut(id as usize) {
            if (idx as usize) < 5 {
                a[idx as usize] = v;
            }
        }
    }

    // --- selection / boundary tools ---
    /// Region nearest to world `(x, y)`, excluding the boundary frame; `-1` if none.
    pub fn region_at(&self, x: f64, y: f64) -> i32 {
        let mesh = match &self.mesh {
            Some(m) => m,
            None => return -1,
        };
        let mut radius = self.grid_cell.max(1.0);
        for _ in 0..6 {
            let cand = self.brush_candidates(x, y, radius);
            let mut best = -1i32;
            let mut bd = f64::INFINITY;
            for &rid in &cand {
                let ri = rid as usize;
                if mesh.is_boundary_r(ri) {
                    continue;
                }
                let p = mesh.pos_of_r(ri);
                let d = (p[0] - x).powi(2) + (p[1] - y).powi(2);
                if d < bd {
                    bd = d;
                    best = ri as i32;
                }
            }
            if best >= 0 {
                return best;
            }
            radius *= 2.0;
        }
        -1
    }

    /// Biome id (1..=14, 0 = none) at a region.
    pub fn biome_at(&self, region: u32) -> u8 {
        self.biome_r.get(region as usize).copied().unwrap_or(0)
    }

    /// Reassign a single region's biome (Select right-click); locks it from reclassify.
    pub fn set_biome_of(&mut self, region: u32, id: u32) {
        let ri = region as usize;
        if ri < self.biome_r.len() {
            self.biome_r[ri] = id as u8;
            if ri < self.biome_locked.len() {
                self.biome_locked[ri] = true;
            }
        }
    }

    /// Contiguous same-biome region ids reachable from `region` (flood select),
    /// excluding the boundary frame.
    pub fn select_contiguous(&self, region: u32) -> Vec<u32> {
        let start = region as usize;
        if start >= self.biome_r.len() || self.neighbors.len() != self.biome_r.len() {
            return Vec::new();
        }
        let target = self.biome_r[start];
        let mut seen = vec![false; self.biome_r.len()];
        let mut stack = vec![start];
        seen[start] = true;
        let mut out = Vec::new();
        while let Some(r) = stack.pop() {
            out.push(r as u32);
            for &nb in &self.neighbors[r] {
                let nb = nb as usize;
                if seen[nb] || self.biome_r.get(nb).copied() != Some(target) {
                    continue;
                }
                if let Some(m) = &self.mesh {
                    if m.is_boundary_r(nb) {
                        continue;
                    }
                }
                seen[nb] = true;
                stack.push(nb);
            }
        }
        out
    }

    /// Triangle indices (3 per triangle) whose three corners are all in `regions` —
    /// the fill geometry for the selection / territory highlight overlay.
    pub fn selection_indices(&self, regions: &[u32]) -> Vec<u32> {
        let mesh = match &self.mesh {
            Some(m) => m,
            None => return Vec::new(),
        };
        let nr = mesh.num_regions();
        let mut sel = vec![false; nr];
        for &r in regions {
            if (r as usize) < nr {
                sel[r as usize] = true;
            }
        }
        let mut out = Vec::new();
        for t in 0..mesh.num_triangles() {
            let a = mesh.r_begin_s(3 * t);
            let b = mesh.r_begin_s(3 * t + 1);
            let c = mesh.r_begin_s(3 * t + 2);
            if sel[a] && sel[b] && sel[c] {
                out.push(a as u32);
                out.push(b as u32);
                out.push(c as u32);
            }
        }
        out
    }

    /// Region ids whose centroid falls inside the polygon `(xs[i], ys[i])` (ray-cast
    /// point-in-polygon), excluding the boundary frame. Foundation for territories.
    pub fn regions_in_polygon(&self, xs: &[f64], ys: &[f64]) -> Vec<u32> {
        let mesh = match &self.mesh {
            Some(m) => m,
            None => return Vec::new(),
        };
        let np = xs.len().min(ys.len());
        if np < 3 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for r in 0..mesh.num_regions() {
            if mesh.is_boundary_r(r) {
                continue;
            }
            let p = mesh.pos_of_r(r);
            if point_in_poly(p[0], p[1], xs, ys, np) {
                out.push(r as u32);
            }
        }
        out
    }

    /// Export the Course (main-river) seed mask for save/load.
    pub fn course_mask_export(&self) -> Vec<u8> {
        self.course_mask.iter().map(|&b| b as u8).collect()
    }
    /// Restore a saved Course seed mask (size must match the current mesh).
    pub fn set_course_mask(&mut self, m: &[u8]) {
        if m.len() == self.course_mask.len() {
            for (i, &v) in m.iter().enumerate() {
                self.course_mask[i] = v != 0;
            }
        }
    }

    // --- save / load (authored state) ---
    pub fn elevation_export(&self) -> Vec<f32> {
        self.elevation_r.iter().map(|&e| e as f32).collect()
    }
    pub fn biome_export(&self) -> Vec<u8> {
        self.biome_r.clone()
    }
    pub fn liquid_depth_export(&self) -> Vec<f32> {
        self.field.depth.iter().map(|&d| d as f32).collect()
    }
    pub fn liquid_kind_export(&self) -> Vec<u8> {
        self.field.kind.clone()
    }
    pub fn set_elevation(&mut self, e: &[f32]) {
        if e.len() == self.elevation_r.len() {
            for (i, &v) in e.iter().enumerate() {
                self.elevation_r[i] = v as f64;
            }
        }
    }
    pub fn set_biome(&mut self, b: &[u8]) {
        if b.len() == self.biome_r.len() {
            self.biome_r.copy_from_slice(b);
        }
    }
    pub fn set_liquid(&mut self, depth: &[f32], kind: &[u8]) {
        if depth.len() == self.field.depth.len() && kind.len() == self.field.kind.len() {
            for (i, &d) in depth.iter().enumerate() {
                self.field.depth[i] = d as f64;
            }
            self.field.kind.copy_from_slice(kind);
        }
    }

    // --- decoration scatter (Phase 7) ---
    /// Place decoration instances (rocks/trees) deterministically; `density` 0..1.
    pub fn tessellate_scatter(&mut self, exaggeration: f64, density: f64, seed: f64) {
        let inst = if let Some(mesh) = &self.mesh {
            dhce_core::scatter::scatter(seed as u64, mesh, &self.elevation_r, &self.biome_r, exaggeration, density)
        } else {
            Vec::new()
        };
        let mut data = Vec::with_capacity(inst.len() * 5);
        for i in &inst {
            data.extend_from_slice(&[i.x, i.y, i.z, i.scale, i.species]);
        }
        self.scatter_n = inst.len();
        self.scatter_buf = data;
    }
    /// Flat instance buffer: 5 floats per instance (x, y, z, scale, species).
    pub fn scatter_data(&self) -> Vec<f32> {
        self.scatter_buf.clone()
    }
    pub fn scatter_count(&self) -> usize {
        self.scatter_n
    }
}

impl WasmEngine {
    /// Candidate region ids whose grid cells overlap the brush's bounding box.
    fn brush_candidates(&self, cx: f64, cy: f64, radius: f64) -> Vec<u32> {
        if self.grid.is_empty() || self.grid_cols == 0 {
            return (0..self.elevation_r.len() as u32).collect();
        }
        let c = self.grid_cell;
        let clampx = |v: f64| (v.max(0.0) as usize).min(self.grid_cols - 1);
        let clampy = |v: f64| (v.max(0.0) as usize).min(self.grid_rows - 1);
        let gx0 = clampx(((cx - radius) / c).floor());
        let gx1 = clampx(((cx + radius) / c).floor());
        let gy0 = clampy(((cy - radius) / c).floor());
        let gy1 = clampy(((cy + radius) / c).floor());
        let mut out = Vec::new();
        for gy in gy0..=gy1 {
            for gx in gx0..=gx1 {
                out.extend_from_slice(&self.grid[gy * self.grid_cols + gx]);
            }
        }
        out
    }

    /// Per-region RGB color (3 floats per region) from each region's biome, softened
    /// across neighbours (so hard biome-block seams read as gradients) with a subtle
    /// deterministic brightness jitter to break up flatness.
    fn region_color(&self) -> Vec<f32> {
        let nr = self.elevation_r.len();
        let mut c = vec![0.0f32; nr * 3];
        for r in 0..nr {
            let id = self.biome_r.get(r).copied().unwrap_or(0) as usize;
            let col = self.biome_color.get(id).copied().unwrap_or([0.5, 0.5, 0.5]);
            c[3 * r] = col[0];
            c[3 * r + 1] = col[1];
            c[3 * r + 2] = col[2];
        }

        // Light-touch Laplacian smoothing toward the neighbour mean.
        if self.neighbors.len() == nr {
            for _ in 0..COLOR_SMOOTH_ITERS {
                let mut next = c.clone();
                for r in 0..nr {
                    let mut sum = [0.0f32; 3];
                    let mut cnt = 0.0f32;
                    for &nb in &self.neighbors[r] {
                        let nb = nb as usize;
                        sum[0] += c[3 * nb];
                        sum[1] += c[3 * nb + 1];
                        sum[2] += c[3 * nb + 2];
                        cnt += 1.0;
                    }
                    if cnt > 0.0 {
                        for k in 0..3 {
                            let mean = sum[k] / cnt;
                            next[3 * r + k] = c[3 * r + k] + (mean - c[3 * r + k]) * COLOR_SMOOTH_W;
                        }
                    }
                }
                c = next;
            }
        }

        // Subtle per-region brightness variation (deterministic hash of the index).
        for r in 0..nr {
            let h = hash_u32(r as u32);
            let v = ((h & 0xffff) as f32 / 65535.0 - 0.5) * 2.0 * COLOR_VAR; // [-VAR, VAR]
            let f = 1.0 + v;
            for k in 0..3 {
                c[3 * r + k] = (c[3 * r + k] * f).clamp(0.0, 1.0);
            }
        }
        c
    }

    /// Re-classify the auto-biome (unlocked) regions in `candidates` from their current
    /// elevation, so coloring tracks terrain edits. Moisture + distance are static per
    /// region, so only elevation drives the change.
    fn reclassify(&mut self, candidates: &[u32]) {
        if self.mesh.is_none() {
            return;
        }
        let (width, height, seed) = (self.width, self.height, self.seed);
        let cx = width * 0.5;
        let cy = height * 0.5;
        let max_d = 0.5 * (width * width + height * height).sqrt();
        let mesh = self.mesh.as_ref().unwrap();
        for &rid in candidates {
            let ri = rid as usize;
            if ri >= self.biome_r.len() || self.biome_locked.get(ri).copied().unwrap_or(false) {
                continue;
            }
            let p = mesh.pos_of_r(ri);
            let dist = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt() / max_d;
            let moist = biomes::moisture_at(p[0], p[1], width, height, seed);
            self.biome_r[ri] = biomes::classify(self.elevation_r[ri], moist, dist);
        }
    }
}

/// Fast integer hash (fmix32-style) for deterministic per-region color jitter.
fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Ray-casting point-in-polygon over the first `np` vertices of `(xs, ys)`.
fn point_in_poly(px: f64, py: f64, xs: &[f64], ys: &[f64], np: usize) -> bool {
    let mut inside = false;
    let mut j = np - 1;
    for i in 0..np {
        let (xi, yi) = (xs[i], ys[i]);
        let (xj, yj) = (xs[j], ys[j]);
        let intersects = ((yi > py) != (yj > py))
            && (px < (xj - xi) * (py - yi) / (yj - yi) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}
