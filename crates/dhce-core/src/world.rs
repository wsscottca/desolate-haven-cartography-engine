//! Resident authoring world-state — the single source of truth for an in-progress map.
//!
//! `World` owns the mesh, per-region elevation/biome/liquid, the editable per-biome
//! profiles, and a spatial grid for O(brush) edits. Every interactive authoring
//! operation (sculpt, course rivers, procedural streams, flood, biome paint, select,
//! boundary) is a method here, so the front-end adapters (`dhce-wasm`, `dhce-godot`)
//! are thin marshalling shells: they hold render-buffer caches and convert types, but
//! run **no** per-region compute themselves.
//!
//! Determinism is preserved: all math here is polynomial / averaging / integer-hash /
//! threshold — **no transcendentals** — so the same authored edits reproduce identically
//! on wasm32 and native (the cross-target contract; see `docs/specs/dhce-core-contract.md`).

use crate::fluid::{self, LiquidField, LiquidSurface};
use crate::mesh::Mesh;
use crate::scatter::Instance;
use crate::{biomes, elevation, geometry, scatter, streams};
use std::collections::HashMap;

/// Channel-carve depth per unit tool intensity (Course tool).
const CHANNEL_DEPTH_GAIN: f64 = 6.0;
/// Thin water laid down per unit tool intensity as a Course stroke is painted.
const COURSE_WATER_GAIN: f64 = 3.0;
/// Per-region color smoothing: passes + blend toward the neighbour mean (softens hard
/// biome-block seams) and a subtle deterministic brightness jitter to break up flatness.
const COLOR_SMOOTH_ITERS: usize = 2;
const COLOR_SMOOTH_W: f32 = 0.4;
const COLOR_VAR: f32 = 0.04;
/// Elevation clamp shared by every sculpt/carve op (normalized terrain stays in range).
const ELEV_MIN: f64 = -1.5;
const ELEV_MAX: f64 = 1.5;
/// Target regions per rendering chunk. The chunk grid is sized from the region count so
/// each tile holds roughly this many regions regardless of world size or density — that
/// keeps edit cost flat (a dab re-tessellates a fixed amount) as the world scales up.
const TARGET_REGIONS_PER_CHUNK: usize = 12_000;

/// The authored world: generation output plus every interactive edit applied on top.
pub struct World {
    width: f64,
    height: f64,
    seed: u64,
    mesh: Option<Mesh>,
    elevation_r: Vec<f64>,
    neighbors: Vec<Vec<u32>>,
    field: LiquidField,
    sea_level: f64,
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
    // Rendering chunks: triangles partitioned by centroid into a grid so an edit can
    // re-tessellate only the affected tiles. `color_cache` is the per-region RGB the chunk
    // meshes read (full smoothing at build; per-region refresh on edit).
    chunk_tris: Vec<Vec<u32>>,
    region_chunks: Vec<Vec<u32>>,
    chunk_dirty: Vec<bool>,
    /// Dimensions of the (square) chunk grid; `chunk id = gy * chunk_cols + gx`. Both 0
    /// until [`build`] partitions the mesh. Stored so the front-end can map the camera to
    /// visible tiles (render-distance streaming) without re-deriving the layout.
    chunk_cols: usize,
    chunk_rows: usize,
    color_cache: Vec<f32>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// An empty world with per-biome profiles seeded from the roster. Call [`build`]
    /// before any other operation.
    pub fn new() -> World {
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
        World {
            width: 0.0,
            height: 0.0,
            seed: 0,
            mesh: None,
            elevation_r: Vec::new(),
            neighbors: Vec::new(),
            field: LiquidField::new(0),
            sea_level: 0.0,
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
            chunk_tris: Vec::new(),
            region_chunks: Vec::new(),
            chunk_dirty: Vec::new(),
            chunk_cols: 0,
            chunk_rows: 0,
            color_cache: Vec::new(),
        }
    }

    /// Build mesh + per-region elevation over `[0,width] × [0,height]` at `spacing`,
    /// seeded by `seed` with `octaves` of noise. Auto-classifies biomes and re-applies
    /// the stored sea level so water tracks the new terrain.
    pub fn build(&mut self, width: f64, height: f64, spacing: f64, seed: u64, octaves: u32) {
        let mesh = Mesh::new(width, height, spacing, seed);
        let nr = mesh.num_regions();
        self.seed = seed;
        self.elevation_r = elevation::assign_region_elevation(&mesh, width, height, seed, octaves);
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
            let moist = biomes::moisture_at(p[0], p[1], width, height, seed);
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

        // Partition into rendering chunks and cache smoothed colors for them.
        self.build_chunks();
        self.color_cache = self.region_color();
    }

    pub fn region_count(&self) -> usize {
        self.mesh.as_ref().map(|m| m.num_regions()).unwrap_or(0)
    }
    pub fn triangle_count(&self) -> usize {
        self.mesh.as_ref().map(|m| m.num_triangles()).unwrap_or(0)
    }

    // --- render-surface producers (the only place the core packs GPU buffers) ---

    /// Pack the terrain render surface at a vertical `exaggeration` (smoothed biome
    /// colors baked in). `None` until [`build`] has run.
    pub fn surface(&self, exaggeration: f64) -> Option<geometry::Surface> {
        let mesh = self.mesh.as_ref()?;
        let colors = self.region_color();
        Some(geometry::build_surface(mesh, &self.elevation_r, exaggeration, &colors))
    }

    /// Pack the liquid render surface at a vertical `exaggeration`. `None` until built.
    pub fn liquid_surface(&self, exaggeration: f64) -> Option<LiquidSurface> {
        let mesh = self.mesh.as_ref()?;
        Some(fluid::liquid_surface(mesh, &self.elevation_r, &self.field, &self.neighbors, exaggeration))
    }

    /// Deterministic decoration instances (rocks/trees); `density` 0..1.
    pub fn scatter_instances(&self, exaggeration: f64, density: f64, seed: u64) -> Vec<Instance> {
        match &self.mesh {
            Some(mesh) => scatter::scatter(seed, mesh, &self.elevation_r, &self.biome_r, exaggeration, density),
            None => Vec::new(),
        }
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

    // --- brush tools ---

    /// Sculpt the terrain under `(cx, cy)` within `radius`. `mode`: 0 raise, 1 carve,
    /// 2 level (toward the height at the brush center), 3 crest (sharp peak). `strength`
    /// is the per-application elevation delta. Returns the region ids touched (the brush
    /// footprint) so a front-end can patch just those vertices instead of re-uploading
    /// the whole mesh.
    pub fn paint_terrain(&mut self, cx: f64, cy: f64, radius: f64, strength: f64, mode: u32) -> Vec<u32> {
        if self.mesh.is_none() {
            return Vec::new();
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

        let mut touched = Vec::new();
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
                *e = e.clamp(ELEV_MIN, ELEV_MAX);
                touched.push(rid);
            }
        }
        // Elevation changed → re-classify the auto-biome cells under the brush so the
        // coloring tracks the new terrain (manually painted cells stay locked).
        self.reclassify(&touched);
        self.after_edit(&touched);
        touched
    }

    /// Place `amount` of liquid `kind` (0 water, 1 lava) under `(cx, cy)` within
    /// `radius`. Used by the Flood (pour) tool.
    pub fn paint_liquid(&mut self, cx: f64, cy: f64, radius: f64, amount: f64, kind: u8) {
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
                self.field.kind[ri] = kind;
            }
        }
    }

    /// Course tool: carve a river channel under `(cx, cy)` while laying thin water, and
    /// tag the painted cells as a main-river seed for [`generate_streams`]. The carve is
    /// idempotent under a dragged stroke (it lowers *toward* a bed, never past it).
    /// Returns the region ids touched.
    pub fn paint_course(&mut self, cx: f64, cy: f64, radius: f64, intensity: f64, kind: u8) -> Vec<u32> {
        if self.mesh.is_none() {
            return Vec::new();
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
            return Vec::new();
        }

        let target_depth = intensity * CHANNEL_DEPTH_GAIN;
        let mut touched = Vec::new();
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
                let bed = (e_ref - target_depth * prof).clamp(ELEV_MIN, ELEV_MAX);
                if self.elevation_r[ri] > bed {
                    self.elevation_r[ri] = bed;
                }
                let add = intensity * COURSE_WATER_GAIN * (t * t * (3.0 - 2.0 * t));
                if add > 0.0 {
                    self.field.depth[ri] += add;
                    self.field.kind[ri] = kind;
                }
                if ri < self.course_mask.len() {
                    self.course_mask[ri] = true;
                }
                touched.push(rid);
            }
        }
        self.reclassify(&touched);
        self.after_edit(&touched);
        touched
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
            self.elevation_r[r] = (self.elevation_r[r] + self.stream_carve[r]).clamp(ELEV_MIN, ELEV_MAX);
            self.stream_carve[r] = 0.0;
        }

        let num_b = self.mesh.as_ref().unwrap().num_boundary_regions();
        let res = streams::accumulate(&self.elevation_r, &self.neighbors, num_b, &self.course_mask, threshold, depth_gain);

        for r in 0..n {
            let cd = res.carve_delta[r];
            if cd > 0.0 {
                let before = self.elevation_r[r];
                let after = (before - cd).clamp(ELEV_MIN, ELEV_MAX);
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
        self.after_edit(&idx);
    }

    // --- biomes ---

    /// Assign `biome_id` (1..=14) to every region under `(cx, cy)` within `radius`,
    /// locking each from auto-reclassify. Returns the region ids touched.
    pub fn paint_biome(&mut self, cx: f64, cy: f64, radius: f64, biome_id: u8) -> Vec<u32> {
        if self.mesh.is_none() {
            return Vec::new();
        }
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);
        let mesh = self.mesh.as_ref().unwrap();
        let mut touched = Vec::new();
        for &rid in &candidates {
            let ri = rid as usize;
            let p = mesh.pos_of_r(ri);
            if (p[0] - cx).powi(2) + (p[1] - cy).powi(2) < r2 {
                self.biome_r[ri] = biome_id;
                if ri < self.biome_locked.len() {
                    self.biome_locked[ri] = true; // manual paint — protect from reclassify
                }
                touched.push(rid);
            }
        }
        self.after_edit(&touched);
        touched
    }

    /// Set the display color of biome `id` (1..=14).
    pub fn set_biome_color(&mut self, id: usize, r: f32, g: f32, b: f32) {
        if let Some(c) = self.biome_color.get_mut(id) {
            *c = [r, g, b];
        }
    }
    /// Current display color of biome `id` as `[r, g, b]` (mid-grey if out of range).
    pub fn biome_color_of(&self, id: usize) -> [f32; 3] {
        self.biome_color.get(id).copied().unwrap_or([0.5, 0.5, 0.5])
    }
    /// Editable landform profile of biome `id`: [peak_shape, hill_amp, valley_floor, roughness].
    pub fn biome_landform_of(&self, id: usize) -> [f32; 4] {
        self.biome_landform.get(id).copied().unwrap_or([0.0; 4])
    }
    /// Editable water profile of biome `id`: [raininess, rain_shadow, evaporation, flow, ocean_depth].
    pub fn biome_water_of(&self, id: usize) -> [f32; 5] {
        self.biome_water.get(id).copied().unwrap_or([0.0; 5])
    }
    /// Set field `idx` (0..4) of biome `id`'s landform profile.
    pub fn set_biome_landform(&mut self, id: usize, idx: usize, v: f32) {
        if let Some(a) = self.biome_landform.get_mut(id) {
            if idx < 4 {
                a[idx] = v;
            }
        }
    }
    /// Set field `idx` (0..5) of biome `id`'s water profile.
    pub fn set_biome_water(&mut self, id: usize, idx: usize, v: f32) {
        if let Some(a) = self.biome_water.get_mut(id) {
            if idx < 5 {
                a[idx] = v;
            }
        }
    }

    // --- selection / boundary tools ---

    /// Region nearest to world `(x, y)`, excluding the boundary frame; `None` if none.
    pub fn region_at(&self, x: f64, y: f64) -> Option<usize> {
        let mesh = self.mesh.as_ref()?;
        let mut radius = self.grid_cell.max(1.0);
        for _ in 0..6 {
            let cand = self.brush_candidates(x, y, radius);
            let mut best: Option<usize> = None;
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
                    best = Some(ri);
                }
            }
            if best.is_some() {
                return best;
            }
            radius *= 2.0;
        }
        None
    }

    /// Biome id (1..=14, 0 = none) at a region.
    pub fn biome_at(&self, region: usize) -> u8 {
        self.biome_r.get(region).copied().unwrap_or(0)
    }

    /// Reassign a single region's biome (Select right-click); locks it from reclassify.
    pub fn set_biome_of(&mut self, region: usize, id: u8) {
        if region < self.biome_r.len() {
            self.biome_r[region] = id;
            if region < self.biome_locked.len() {
                self.biome_locked[region] = true;
            }
            self.after_edit(&[region as u32]);
        }
    }

    /// Contiguous same-biome region ids reachable from `region` (flood select),
    /// excluding the boundary frame.
    pub fn select_contiguous(&self, region: usize) -> Vec<u32> {
        let start = region;
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
    /// Export the Course (main-river) seed mask for save/load.
    pub fn course_mask_export(&self) -> Vec<u8> {
        self.course_mask.iter().map(|&b| b as u8).collect()
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
    /// Restore a saved Course seed mask (size must match the current mesh).
    pub fn set_course_mask(&mut self, m: &[u8]) {
        if m.len() == self.course_mask.len() {
            for (i, &v) in m.iter().enumerate() {
                self.course_mask[i] = v != 0;
            }
        }
    }

    // --- internal compute ---

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

    // --- rendering chunks (incremental re-tessellation) ---

    /// Partition triangles into a chunk grid by centroid, and record which chunks each
    /// region participates in (so an edit can flag exactly the affected tiles).
    fn build_chunks(&mut self) {
        let mesh = match &self.mesh {
            Some(m) => m,
            None => return,
        };
        let nr = mesh.num_regions();
        let nt = mesh.num_triangles();
        // Square grid sized so each tile holds ~TARGET_REGIONS_PER_CHUNK regions.
        let axis = ((nr as f64 / TARGET_REGIONS_PER_CHUNK as f64).sqrt().ceil() as usize).max(1);
        let cols = axis;
        let rows = axis;
        let sx = (self.width / cols as f64).max(1.0);
        let sy = (self.height / rows as f64).max(1.0);
        let mut chunk_tris: Vec<Vec<u32>> = vec![Vec::new(); cols * rows];
        let mut region_chunks: Vec<Vec<u32>> = vec![Vec::new(); nr];
        for t in 0..nt {
            let a = mesh.r_begin_s(3 * t);
            let b = mesh.r_begin_s(3 * t + 1);
            let c = mesh.r_begin_s(3 * t + 2);
            let (pa, pb, pc) = (mesh.pos_of_r(a), mesh.pos_of_r(b), mesh.pos_of_r(c));
            let cxw = (pa[0] + pb[0] + pc[0]) / 3.0;
            let cyw = (pa[1] + pb[1] + pc[1]) / 3.0;
            let gx = ((cxw / sx) as usize).min(cols - 1);
            let gy = ((cyw / sy) as usize).min(rows - 1);
            let ci = (gy * cols + gx) as u32;
            chunk_tris[ci as usize].push(t as u32);
            for &r in &[a, b, c] {
                if !region_chunks[r].contains(&ci) {
                    region_chunks[r].push(ci);
                }
            }
        }
        self.chunk_tris = chunk_tris;
        self.region_chunks = region_chunks;
        self.chunk_dirty = vec![false; cols * rows];
        self.chunk_cols = cols;
        self.chunk_rows = rows;
    }

    /// After editing `regions`: refresh their cached colors (same jitter as `region_color`,
    /// minus the neighbour smoothing) and flag the chunks they belong to for re-tessellation.
    fn after_edit(&mut self, regions: &[u32]) {
        for &rid in regions {
            let r = rid as usize;
            if 3 * r + 2 < self.color_cache.len() {
                let id = self.biome_r.get(r).copied().unwrap_or(0) as usize;
                let base = self.biome_color.get(id).copied().unwrap_or([0.5, 0.5, 0.5]);
                let h = hash_u32(r as u32);
                let f = 1.0 + ((h & 0xffff) as f32 / 65535.0 - 0.5) * 2.0 * COLOR_VAR;
                for k in 0..3 {
                    self.color_cache[3 * r + k] = (base[k] * f).clamp(0.0, 1.0);
                }
            }
            if let Some(chunks) = self.region_chunks.get(r) {
                for &c in chunks {
                    if let Some(d) = self.chunk_dirty.get_mut(c as usize) {
                        *d = true;
                    }
                }
            }
        }
    }

    /// Number of rendering chunks.
    pub fn chunk_count(&self) -> usize {
        self.chunk_tris.len()
    }

    /// `(cols, rows)` of the rendering-chunk grid (square: `cols == rows`); `(0, 0)` until
    /// [`build`] has run. A chunk's grid cell is `(gx, gy) = (id % cols, id / cols)`.
    pub fn chunk_grid(&self) -> (usize, usize) {
        (self.chunk_cols, self.chunk_rows)
    }

    /// World-space center `(x, y)` of every chunk tile, flat `[x0, y0, x1, y1, …]` indexed
    /// by chunk id. Lets the front-end map the camera to visible tiles for render-distance
    /// streaming with no per-region work. Empty until [`build`] has run.
    pub fn chunk_centers(&self) -> Vec<f64> {
        let (cols, rows) = (self.chunk_cols, self.chunk_rows);
        if cols == 0 || rows == 0 {
            return Vec::new();
        }
        // Same tile size the partitioner used (see `build_chunks`).
        let sx = (self.width / cols as f64).max(1.0);
        let sy = (self.height / rows as f64).max(1.0);
        let mut out = Vec::with_capacity(cols * rows * 2);
        for gy in 0..rows {
            for gx in 0..cols {
                out.push((gx as f64 + 0.5) * sx);
                out.push((gy as f64 + 0.5) * sy);
            }
        }
        out
    }

    /// Chunk ids flagged dirty since the last call, clearing them. The front-end
    /// re-tessellates exactly these after an edit.
    pub fn take_dirty_chunks(&mut self) -> Vec<u32> {
        let mut out = Vec::new();
        for (i, d) in self.chunk_dirty.iter_mut().enumerate() {
            if *d {
                *d = false;
                out.push(i as u32);
            }
        }
        out
    }

    /// Pack one chunk's sub-mesh: a local vertex array (the regions used by the chunk's
    /// triangles), positions at `exaggeration`, cached colors, and area-weighted normals
    /// from this chunk's triangles. (Boundary normals can differ slightly across chunks — a
    /// faint seam, acceptable for interactive editing.) `None` on bad index / unbuilt mesh.
    pub fn chunk_surface(&self, chunk: usize, exaggeration: f64) -> Option<geometry::Surface> {
        let mesh = self.mesh.as_ref()?;
        let tris = self.chunk_tris.get(chunk)?;
        let mut local_of: HashMap<u32, u32> = HashMap::new();
        let mut globals: Vec<u32> = Vec::new();
        let mut indices: Vec<u32> = Vec::with_capacity(tris.len() * 3);
        for &t in tris {
            for k in 0..3 {
                let r = mesh.r_begin_s(3 * t as usize + k) as u32;
                let li = *local_of.entry(r).or_insert_with(|| {
                    let idx = globals.len() as u32;
                    globals.push(r);
                    idx
                });
                indices.push(li);
            }
        }
        let n = globals.len();
        let mut positions = vec![0.0f32; n * 3];
        let mut heights = vec![0.0f32; n];
        let mut colors = vec![0.0f32; n * 3];
        for (li, &r) in globals.iter().enumerate() {
            let ru = r as usize;
            let p = mesh.pos_of_r(ru);
            positions[3 * li] = p[0] as f32;
            positions[3 * li + 1] = p[1] as f32;
            positions[3 * li + 2] = (self.elevation_r[ru] * exaggeration) as f32;
            heights[li] = self.elevation_r[ru] as f32;
            if 3 * ru + 2 < self.color_cache.len() {
                colors[3 * li] = self.color_cache[3 * ru];
                colors[3 * li + 1] = self.color_cache[3 * ru + 1];
                colors[3 * li + 2] = self.color_cache[3 * ru + 2];
            } else {
                colors[3 * li] = 0.5;
                colors[3 * li + 1] = 0.5;
                colors[3 * li + 2] = 0.5;
            }
        }
        let mut accum = vec![0.0f64; n * 3];
        for tri in indices.chunks_exact(3) {
            let (la, lb, lc) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let ax = positions[3 * la] as f64;
            let ay = positions[3 * la + 1] as f64;
            let az = positions[3 * la + 2] as f64;
            let bx = positions[3 * lb] as f64;
            let by = positions[3 * lb + 1] as f64;
            let bz = positions[3 * lb + 2] as f64;
            let cx = positions[3 * lc] as f64;
            let cy = positions[3 * lc + 1] as f64;
            let cz = positions[3 * lc + 2] as f64;
            let (ux, uy, uz) = (bx - ax, by - ay, bz - az);
            let (vx, vy, vz) = (cx - ax, cy - ay, cz - az);
            let mut nx = uy * vz - uz * vy;
            let mut ny = uz * vx - ux * vz;
            let mut nz = ux * vy - uy * vx;
            if nz < 0.0 {
                nx = -nx;
                ny = -ny;
                nz = -nz;
            }
            for &li in &[la, lb, lc] {
                accum[3 * li] += nx;
                accum[3 * li + 1] += ny;
                accum[3 * li + 2] += nz;
            }
        }
        let mut normals = vec![0.0f32; n * 3];
        for li in 0..n {
            let (nx, ny, nz) = (accum[3 * li], accum[3 * li + 1], accum[3 * li + 2]);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            if len > 1e-12 {
                normals[3 * li] = (nx / len) as f32;
                normals[3 * li + 1] = (ny / len) as f32;
                normals[3 * li + 2] = (nz / len) as f32;
            } else {
                normals[3 * li + 2] = 1.0;
            }
        }
        Some(geometry::Surface {
            positions,
            normals,
            heights,
            colors,
            indices,
        })
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
        let intersects = ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}
