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
/// Trait-border blend (the transition buffer): neighbour-average weight per pass, and the
/// pass-count cap so a wide Transition width can't stall the explicit Apply. See
/// [`World::blend_traits`].
const BLEND_W: f64 = 0.5;
const MAX_BLEND_ITERS: usize = 24;
/// Data-view modes for [`World::set_view_mode`] — which per-cell field drives the vertex colour.
/// `Natural` is the composed terrain look; the rest are direct field readouts (heatmaps) of the
/// same meshes, paintable in place. The minimap follows the same mode.
pub const VIEW_NATURAL: u8 = 0;
pub const VIEW_TEMPERATURE: u8 = 1;
pub const VIEW_MOISTURE: u8 = 2;
pub const VIEW_ELEVATION: u8 = 3;
pub const VIEW_BIOME: u8 = 4;
/// Shaping (landform dials → terrain height): noise frequencies (cycles across the map), max added
/// normalized elevation at dial = 1, and erosion smoothing of the added detail. All deterministic
/// (fbm2 + lerp / averaging). See [`World::shape_terrain`].
const SHAPE_JAG_FREQ: f64 = 22.0;
const SHAPE_JAG_AMP: f64 = 0.30;
const SHAPE_RELIEF_FREQ: f64 = 7.0;
const SHAPE_RELIEF_AMP: f64 = 0.18;
const SHAPE_EROSION_PASSES: usize = 3;
const SHAPE_EROSION_W: f64 = 0.5;
const SHAPE_JAG_SALT: u64 = 0x9E37_79B9_7F4A_7C15;
const SHAPE_RELIEF_SALT: u64 = 0x2545_F491_4F6C_DD1D;
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
    /// Per-region moisture (0..1), cached at build from the noise channel used by
    /// classification, so colouring can resolve the palette ramp without re-sampling noise.
    moisture_r: Vec<f64>,
    neighbors: Vec<Vec<u32>>,
    field: LiquidField,
    sea_level: f64,
    biome_r: Vec<u8>,
    /// Representative swatch colour per Region preset (its `--mk-*` accent); for `biome_color_of`.
    biome_color: Vec<[f32; 3]>,
    /// The 7 shared base palettes (editable in the colour editor), indexed by `biomes::fam::*`.
    base_palettes: Vec<biomes::BasePalette>,
    // Per-cell trait fields (the trait-composition model; ADR 0004). Ground colour resolves from
    // these via `cell_color`; the landform traits feed the (later) shaping pass.
    jaggedness_r: Vec<f64>,
    relief_r: Vec<f64>,
    foothill_falloff_r: Vec<f64>,
    erosion_r: Vec<f64>,
    temperature_r: Vec<f64>,
    vegetation_r: Vec<u8>,
    palette_family_r: Vec<u8>,
    /// Painted-base snapshots of the scalar trait fields. The brushes write these alongside the
    /// live `*_r` fields above; [`blend_traits`](World::blend_traits) always diffuses *from* the
    /// base, so re-applying it with the same width is idempotent (the skirt never compounds).
    /// Enums (vegetation/palette_family) don't diffuse, so they need no base.
    jaggedness_base: Vec<f64>,
    relief_base: Vec<f64>,
    foothill_falloff_base: Vec<f64>,
    erosion_base: Vec<f64>,
    temperature_base: Vec<f64>,
    moisture_base: Vec<f64>,
    /// Legacy per-Region landform profile [jaggedness, relief, foothill_falloff, erosion] — kept
    /// for the `biome_landform` getter/setter; the live values are the per-cell trait fields above.
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
    /// Per-cell elevation delta (signed) applied by the last [`shape_terrain`] pass. Restored then
    /// recomputed each run, so re-shaping is idempotent and adapts to edits made in between.
    shape_delta: Vec<f64>,
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
    /// Active data-view mode (see the `VIEW_*` consts). Off `Natural`, the colour cache holds a
    /// direct readout of one field instead of the composed terrain colour.
    view_mode: u8,
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
            biome_color[i + 1] = b.representative();
            let t = &b.traits;
            biome_landform[i + 1] = [t.jaggedness, t.relief, t.foothill_falloff, t.erosion];
            let w = &b.water;
            biome_water[i + 1] = [w.raininess, w.rain_shadow, w.evaporation, w.flow, w.ocean_depth];
        }
        World {
            width: 0.0,
            height: 0.0,
            seed: 0,
            mesh: None,
            elevation_r: Vec::new(),
            moisture_r: Vec::new(),
            neighbors: Vec::new(),
            field: LiquidField::new(0),
            sea_level: 0.0,
            biome_r: Vec::new(),
            biome_color,
            base_palettes: biomes::base_palettes().to_vec(),
            jaggedness_r: Vec::new(),
            relief_r: Vec::new(),
            foothill_falloff_r: Vec::new(),
            erosion_r: Vec::new(),
            temperature_r: Vec::new(),
            vegetation_r: Vec::new(),
            palette_family_r: Vec::new(),
            jaggedness_base: Vec::new(),
            relief_base: Vec::new(),
            foothill_falloff_base: Vec::new(),
            erosion_base: Vec::new(),
            temperature_base: Vec::new(),
            moisture_base: Vec::new(),
            biome_landform,
            biome_water,
            biome_locked: Vec::new(),
            course_mask: Vec::new(),
            stream_carve: Vec::new(),
            shape_delta: Vec::new(),
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
            view_mode: VIEW_NATURAL,
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
        self.shape_delta = vec![0.0; nr];
        fluid::sea_fill(&mut self.field, &self.elevation_r, self.sea_level);

        // Auto-classify biomes from elevation + moisture + distance-from-center.
        let cx = width * 0.5;
        let cy = height * 0.5;
        let max_d = 0.5 * (width * width + height * height).sqrt();
        let mut biome_r = vec![0u8; nr];
        let mut moisture_r = vec![0.0f64; nr];
        for r in 0..nr {
            let p = mesh.pos_of_r(r);
            let dist = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt() / max_d;
            let moist = biomes::moisture_at(p[0], p[1], width, height, seed);
            moisture_r[r] = moist;
            biome_r[r] = biomes::classify(self.elevation_r[r], moist, dist);
        }
        self.biome_r = biome_r;
        self.moisture_r = moisture_r;

        // Seed the per-cell trait fields from each cell's classified Region preset (the author
        // paints/edits over this). `moisture_r` keeps its noise value (more varied than the preset).
        let mut jag = vec![0.0f64; nr];
        let mut rel = vec![0.0f64; nr];
        let mut foot = vec![0.0f64; nr];
        let mut ero = vec![0.0f64; nr];
        let mut temp = vec![0.0f64; nr];
        let mut vegc = vec![0u8; nr];
        let mut famc = vec![0u8; nr];
        for r in 0..nr {
            let tr = biomes::default_traits_for(self.biome_r[r]);
            jag[r] = tr.jaggedness as f64;
            rel[r] = tr.relief as f64;
            foot[r] = tr.foothill_falloff as f64;
            ero[r] = tr.erosion as f64;
            temp[r] = tr.temperature as f64;
            vegc[r] = tr.vegetation;
            famc[r] = tr.palette_family;
        }
        self.jaggedness_r = jag;
        self.relief_r = rel;
        self.foothill_falloff_r = foot;
        self.erosion_r = ero;
        self.temperature_r = temp;
        self.vegetation_r = vegc;
        self.palette_family_r = famc;
        // The painted base starts equal to the seeded fields (nothing painted yet); the brushes
        // keep base + live in step, and `blend_traits` diffuses live from base.
        self.jaggedness_base = self.jaggedness_r.clone();
        self.relief_base = self.relief_r.clone();
        self.foothill_falloff_base = self.foothill_falloff_r.clone();
        self.erosion_base = self.erosion_r.clone();
        self.temperature_base = self.temperature_r.clone();
        self.moisture_base = self.moisture_r.clone();

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
                let old_e = self.elevation_r[ri];
                let mut ne = old_e;
                match mode {
                    0 => ne += strength * w,
                    1 => ne -= strength * w,
                    2 => ne += (center_e - ne) * w * 0.5,
                    3 => ne += strength * (t * t * t) * 2.5, // sharper, peaked
                    _ => {}
                }
                ne = ne.clamp(ELEV_MIN, ELEV_MAX);
                self.elevation_r[ri] = ne;
                // Raising land displaces the water it rises through, so terrain lifted above the
                // surface reads as dry land instead of a submerged blue patch.
                let rise = ne - old_e;
                if rise > 0.0 && self.field.depth[ri] > 0.0 {
                    self.field.depth[ri] = (self.field.depth[ri] - rise).max(0.0);
                }
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

    // --- traits (the trait-composition model; ADR 0004) ---

    /// Paint one trait into the brush footprint. `trait_id`: 0 jaggedness, 1 relief,
    /// 2 foothill_falloff, 3 erosion, 4 temperature, 5 moisture, 6 vegetation (value = enum idx),
    /// 7 palette_family (value = enum idx). Scalars ease toward `value` by the smoothstep falloff;
    /// enums snap inside the brush. Returns the cells touched.
    pub fn paint_trait(&mut self, cx: f64, cy: f64, radius: f64, trait_id: u32, value: f64) -> Vec<u32> {
        if self.mesh.is_none() {
            return Vec::new();
        }
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);
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
                let w = t * t * (3.0 - 2.0 * t);
                // Ease both the live field (what renders now) and the painted base (what a later
                // `blend_traits` grades from), so a re-blend reproduces this stroke's footprint.
                match trait_id {
                    0 => { ease_into(&mut self.jaggedness_r, ri, value, w); ease_into(&mut self.jaggedness_base, ri, value, w); }
                    1 => { ease_into(&mut self.relief_r, ri, value, w); ease_into(&mut self.relief_base, ri, value, w); }
                    2 => { ease_into(&mut self.foothill_falloff_r, ri, value, w); ease_into(&mut self.foothill_falloff_base, ri, value, w); }
                    3 => { ease_into(&mut self.erosion_r, ri, value, w); ease_into(&mut self.erosion_base, ri, value, w); }
                    4 => { ease_into(&mut self.temperature_r, ri, value, w); ease_into(&mut self.temperature_base, ri, value, w); }
                    5 => { ease_into(&mut self.moisture_r, ri, value, w); ease_into(&mut self.moisture_base, ri, value, w); }
                    6 => {
                        if w > 0.5 && ri < self.vegetation_r.len() {
                            self.vegetation_r[ri] = value as u8;
                        }
                    }
                    7 => {
                        if w > 0.5 && ri < self.palette_family_r.len() {
                            self.palette_family_r[ri] = value as u8;
                        }
                    }
                    _ => {}
                }
                touched.push(rid);
            }
        }
        self.after_edit(&touched);
        touched
    }

    /// Stamp a Region preset's full trait bundle into the footprint ("this area is X"), locking
    /// those cells' biome label. Returns the cells touched.
    pub fn paint_region_traits(&mut self, cx: f64, cy: f64, radius: f64, biome_id: u8) -> Vec<u32> {
        if self.mesh.is_none() {
            return Vec::new();
        }
        let tr = biomes::default_traits_for(biome_id);
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);
        let mut touched = Vec::new();
        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
                let p = mesh.pos_of_r(ri);
                if (p[0] - cx).powi(2) + (p[1] - cy).powi(2) >= r2 {
                    continue;
                }
                if ri < self.jaggedness_r.len() {
                    self.jaggedness_r[ri] = tr.jaggedness as f64;
                    self.relief_r[ri] = tr.relief as f64;
                    self.foothill_falloff_r[ri] = tr.foothill_falloff as f64;
                    self.erosion_r[ri] = tr.erosion as f64;
                    self.temperature_r[ri] = tr.temperature as f64;
                    self.vegetation_r[ri] = tr.vegetation;
                    self.palette_family_r[ri] = tr.palette_family;
                    // The stamp is the new painted base for these cells (a later blend grades from
                    // it). Moisture keeps its live value — the region preset doesn't define it.
                    self.jaggedness_base[ri] = tr.jaggedness as f64;
                    self.relief_base[ri] = tr.relief as f64;
                    self.foothill_falloff_base[ri] = tr.foothill_falloff as f64;
                    self.erosion_base[ri] = tr.erosion as f64;
                    self.temperature_base[ri] = tr.temperature as f64;
                    self.moisture_base[ri] = self.moisture_r[ri];
                }
                if ri < self.biome_r.len() {
                    self.biome_r[ri] = biome_id;
                }
                if ri < self.biome_locked.len() {
                    self.biome_locked[ri] = true;
                }
                touched.push(rid);
            }
        }
        self.after_edit(&touched);
        touched
    }

    /// Read a per-cell trait at world `(x, y)` (nearest cell). `trait_id` as in [`paint_trait`];
    /// enum traits return their index as `f64`. `None` outside the map.
    pub fn trait_at(&self, x: f64, y: f64, trait_id: u32) -> Option<f64> {
        let r = self.region_at(x, y)?;
        Some(match trait_id {
            0 => self.jaggedness_r.get(r).copied().unwrap_or(0.0),
            1 => self.relief_r.get(r).copied().unwrap_or(0.0),
            2 => self.foothill_falloff_r.get(r).copied().unwrap_or(0.0),
            3 => self.erosion_r.get(r).copied().unwrap_or(0.0),
            4 => self.temperature_r.get(r).copied().unwrap_or(0.0),
            5 => self.moisture_r.get(r).copied().unwrap_or(0.0),
            6 => self.vegetation_r.get(r).copied().unwrap_or(0) as f64,
            7 => self.palette_family_r.get(r).copied().unwrap_or(0) as f64,
            _ => 0.0,
        })
    }

    /// One slot of a base palette as `[r, g, b]`. `slot`: 0 water_deep, 1 water_shallow, 2 low,
    /// 3 rock, 4 cap_warm, 5 cap_cold. Mid-grey if out of range.
    pub fn base_palette_color(&self, family: usize, slot: usize) -> [f32; 3] {
        let bp = match self.base_palettes.get(family) {
            Some(b) => b,
            None => return [0.5, 0.5, 0.5],
        };
        match slot {
            0 => bp.water_deep,
            1 => bp.water_shallow,
            2 => bp.low,
            3 => bp.rock,
            4 => bp.cap_warm,
            5 => bp.cap_cold,
            _ => [0.5, 0.5, 0.5],
        }
    }

    /// Set a base-palette slot, then recompute the colour cache and flag every chunk dirty so the
    /// 3D view + minimap re-render with the new colour.
    pub fn set_base_palette_color(&mut self, family: usize, slot: usize, r: f32, g: f32, b: f32) {
        let v = [r, g, b];
        let changed = if let Some(bp) = self.base_palettes.get_mut(family) {
            match slot {
                0 => bp.water_deep = v,
                1 => bp.water_shallow = v,
                2 => bp.low = v,
                3 => bp.rock = v,
                4 => bp.cap_warm = v,
                5 => bp.cap_cold = v,
                _ => {}
            }
            true
        } else {
            false
        };
        if changed && !self.elevation_r.is_empty() {
            self.color_cache = self.region_color();
            for d in self.chunk_dirty.iter_mut() {
                *d = true;
            }
        }
    }

    /// Diffuse the scalar trait fields across their painted-region borders so abrupt paints ease
    /// into natural skirts — the **transition buffer**. `transition_width_m` sets the band width;
    /// it maps to a count of neighbour-average (Laplacian) passes over the cell graph. The pass
    /// always starts from the painted base (`*_base`), so re-applying with the same width is
    /// idempotent — it never compounds. A locally-uniform region is a fixed point of the average,
    /// so only the bands around discontinuities move: the diffusion *is* the buffer.
    ///
    /// Each scalar grades at its own rate (they diffuse independently); enum traits (vegetation,
    /// palette_family) keep their painted values. Recolours the world and flags every chunk dirty
    /// (re-tessellate the meshed ones via [`take_dirty_chunks`](World::take_dirty_chunks); the
    /// rest pick up the new colour when they next stream in).
    pub fn blend_traits(&mut self, transition_width_m: f64) {
        let n = self.elevation_r.len();
        if n == 0 || self.neighbors.len() != n {
            return;
        }
        // Width (m) → pass count. One pass spreads ~one cell-ring; the cell pitch is the mean
        // spacing of the point set (√(area / cells)). Clamp so a wide setting can't stall Apply.
        let pitch = (self.width * self.height / n as f64).sqrt().max(1.0);
        let iters = ((transition_width_m / pitch).round() as usize).clamp(0, MAX_BLEND_ITERS);

        self.jaggedness_r = diffuse_field(&self.jaggedness_base, &self.neighbors, iters);
        self.relief_r = diffuse_field(&self.relief_base, &self.neighbors, iters);
        self.foothill_falloff_r = diffuse_field(&self.foothill_falloff_base, &self.neighbors, iters);
        self.erosion_r = diffuse_field(&self.erosion_base, &self.neighbors, iters);
        self.temperature_r = diffuse_field(&self.temperature_base, &self.neighbors, iters);
        self.moisture_r = diffuse_field(&self.moisture_base, &self.neighbors, iters);

        // Colour reads temperature/moisture (+ the unchanged enums) → recolour the whole map.
        self.color_cache = self.region_color();
        for d in self.chunk_dirty.iter_mut() {
            *d = true;
        }
    }

    /// Switch the colour view (see the `VIEW_*` consts): `Natural` shows the composed terrain
    /// colour; the data views recolour the same meshes by a single field (temperature/moisture/
    /// elevation heatmaps, or flat biome accents) so the author can read and paint it directly.
    /// Recolours the world and flags every chunk dirty (re-tessellate via [`take_dirty_chunks`]);
    /// the minimap reads the same cache, so it follows automatically.
    pub fn set_view_mode(&mut self, mode: u8) {
        self.view_mode = mode;
        if !self.elevation_r.is_empty() {
            self.color_cache = self.region_color();
            for d in self.chunk_dirty.iter_mut() {
                *d = true;
            }
        }
    }

    /// The active data-view mode (see the `VIEW_*` consts).
    pub fn view_mode(&self) -> u8 {
        self.view_mode
    }

    /// Bake the landform trait dials into the terrain height. Each dial drives a deterministic
    /// elevation perturbation layered on top of the sculpted base:
    /// - **jaggedness** adds high-frequency roughness, concentrated on high ground (peaks jag,
    ///   lowlands stay smooth);
    /// - **relief** adds mid-frequency rolling hills across land;
    /// - **foothill_falloff** widens how far down-slope that detail reaches (broad skirt vs
    ///   peaks-only);
    /// - **erosion** rounds the added detail (neighbour smoothing of the delta).
    ///
    /// `strength` is a global gain (1.0 nominal). The previous shaping is restored first, so
    /// re-running — or changing strength/dials — is idempotent and never compounds. Recolours +
    /// flags all chunks dirty (geometry changed → re-tessellate via [`take_dirty_chunks`]).
    pub fn shape_terrain(&mut self, strength: f64) {
        if self.mesh.is_none() {
            return;
        }
        let n = self.elevation_r.len();
        if self.shape_delta.len() != n {
            self.shape_delta = vec![0.0; n];
        }
        // Restore the previous shaping so the pass always builds from the sculpted base.
        for r in 0..n {
            self.elevation_r[r] -= self.shape_delta[r];
            self.shape_delta[r] = 0.0;
        }

        let (w, h, seed) = (self.width, self.height, self.seed);
        // 1. Raw per-cell delta from jaggedness + relief, gated by altitude / land.
        let mut d = vec![0.0f64; n];
        {
            let mesh = self.mesh.as_ref().unwrap();
            for r in 0..n {
                let p = mesh.pos_of_r(r);
                let e = self.elevation_r[r];
                let jag = self.jaggedness_r[r];
                let rel = self.relief_r[r];
                let foot = self.foothill_falloff_r[r];
                let ero = self.erosion_r[r];
                // foothill_falloff widens the skirt: a smaller divisor lets detail reach full
                // strength at lower elevations (broad foothills); larger keeps it near the peaks.
                let skirt = lerpf(0.25, 0.70, 1.0 - clamp01f(foot));
                let alt = clamp01f(e / skirt);
                let land = clamp01f((e + 0.10) / 0.20); // ~0 below sea, 1 above
                let jag_amp = SHAPE_JAG_AMP * (1.0 - 0.6 * clamp01f(ero)); // erosion damps roughness
                let nj = crate::noise::fbm2(p[0] / w * SHAPE_JAG_FREQ, p[1] / h * SHAPE_JAG_FREQ, seed ^ SHAPE_JAG_SALT, 4);
                let nr_ = crate::noise::fbm2(p[0] / w * SHAPE_RELIEF_FREQ, p[1] / h * SHAPE_RELIEF_FREQ, seed ^ SHAPE_RELIEF_SALT, 3);
                d[r] = (nj * jag_amp * jag * alt + nr_ * SHAPE_RELIEF_AMP * rel * land) * strength;
            }
        }
        // 2. Erosion rounds the added detail (neighbour-mean smoothing, weighted per cell).
        if self.neighbors.len() == n {
            for _ in 0..SHAPE_EROSION_PASSES {
                let mut next = d.clone();
                for r in 0..n {
                    let nb = &self.neighbors[r];
                    if nb.is_empty() {
                        continue;
                    }
                    let mut sum = 0.0;
                    for &j in nb {
                        sum += d[j as usize];
                    }
                    let mean = sum / nb.len() as f64;
                    let wgt = clamp01f(self.erosion_r[r]) * SHAPE_EROSION_W;
                    next[r] = d[r] + (mean - d[r]) * wgt;
                }
                d = next;
            }
        }
        // 3. Apply, recording the actual (clamped) delta; risen land sheds the water it rose through.
        for r in 0..n {
            let before = self.elevation_r[r];
            let after = (before + d[r]).clamp(ELEV_MIN, ELEV_MAX);
            self.shape_delta[r] = after - before;
            self.elevation_r[r] = after;
            let rise = after - before;
            if rise > 0.0 && self.field.depth[r] > 0.0 {
                self.field.depth[r] = (self.field.depth[r] - rise).max(0.0);
            }
        }

        // Elevation changed everywhere → recolour (the ramp reads elevation) and re-tessellate all.
        // Auto-biome reclassify is skipped: colour is trait-driven (not biome-driven), so a stale
        // auto-biome would only show in the Biome view — left to the next edit to keep this snappy.
        self.color_cache = self.region_color();
        for dch in self.chunk_dirty.iter_mut() {
            *dch = true;
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

    /// Normalized terrain elevation at world ground point `(x, y)` (the nearest region's
    /// elevation), or `None` outside the map. Cheap (one grid-accelerated nearest lookup).
    pub fn height_at(&self, x: f64, y: f64) -> Option<f64> {
        self.region_at(x, y).map(|r| self.elevation_r[r])
    }

    /// March a ray (Godot space, Y-up) against the rendered heightfield and return the
    /// surface hit `[x, y, z]` (Godot space), or `None` on a miss. The terrain surface is
    /// `Y = height_at(x, z) * exaggeration` (core ground = Godot XZ). Used by the brush so a
    /// stroke lands under the cursor on the actual surface, not the `Y = 0` plane. Coarse
    /// fixed-step march to the first crossing, then a binary refine — deterministic, and
    /// cheap because each height sample is a single nearest-region lookup.
    pub fn raycast_terrain(
        &self,
        ox: f64, oy: f64, oz: f64,
        dx: f64, dy: f64, dz: f64,
        exaggeration: f64,
    ) -> Option<[f64; 3]> {
        if self.mesh.is_none() {
            return None;
        }
        // Surface height (Godot Y) at a ground point; off-map ⇒ unreachable (never a hit).
        let surf = |x: f64, z: f64| -> f64 {
            if x < 0.0 || z < 0.0 || x > self.width || z > self.height {
                return f64::NEG_INFINITY;
            }
            match self.region_at(x, z) {
                Some(r) => self.elevation_r[r] * exaggeration,
                None => f64::NEG_INFINITY,
            }
        };

        // Bound the march so it always reaches terrain even when the camera is far away:
        // distance from the origin to the world centre, plus the world's full diagonal.
        let cx = self.width * 0.5;
        let cz = self.height * 0.5;
        let to_center = ((ox - cx).powi(2) + oy.powi(2) + (oz - cz).powi(2)).sqrt();
        let diag = (self.width.powi(2) + self.height.powi(2) + (3.0 * exaggeration).powi(2)).sqrt();
        let max_t = to_center + diag + 1.0;
        let step = self.grid_cell.max(1.0).min(max_t / 8.0);

        let mut t = 0.0;
        let mut prev_above = oy > surf(ox, oz);
        while t < max_t {
            let nt = t + step;
            let py = oy + dy * nt;
            let above = py > surf(ox + dx * nt, oz + dz * nt);
            if prev_above && !above {
                // Crossed between t and nt — binary refine for a clean hit point.
                let (mut lo, mut hi) = (t, nt);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if oy + dy * mid > surf(ox + dx * mid, oz + dz * mid) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let ht = 0.5 * (lo + hi);
                return Some([ox + dx * ht, oy + dy * ht, oz + dz * ht]);
            }
            prev_above = above;
            t = nt;
        }
        None
    }

    /// Render a top-down minimap as an `n×n` RGBA image (row-major; row 0 = north, y = 0):
    /// smoothed biome colour, NW hill-shading, elevation contour lines, and any painted liquid
    /// (blue water / orange lava) overlaid. Drives the overview map panel. Empty until built.
    pub fn minimap(&self, n: usize, light_x: f64, light_y: f64) -> Vec<u8> {
        let mut out = vec![0u8; n * n * 4];
        if self.mesh.is_none() || n == 0 {
            return out;
        }

        // Pass 1: nearest region + its elevation per cell (NaN where off-map).
        let mut elev = vec![f64::NAN; n * n];
        let mut reg = vec![u32::MAX; n * n];
        for gy in 0..n {
            let y = (gy as f64 + 0.5) / n as f64 * self.height;
            for gx in 0..n {
                let x = (gx as f64 + 0.5) / n as f64 * self.width;
                if let Some(r) = self.region_at(x, y) {
                    reg[gy * n + gx] = r as u32;
                    elev[gy * n + gx] = self.elevation_r[r];
                }
            }
        }

        const INTERVAL: f64 = 0.12; // contour spacing in normalized elevation
        let band = |e: f64| (e / INTERVAL).floor() as i64;
        for gy in 0..n {
            for gx in 0..n {
                let i = gy * n + gx;
                let r = reg[i];
                if r == u32::MAX {
                    continue; // off-map → transparent
                }
                let r = r as usize;
                let e = elev[i];

                let (mut cr, mut cg, mut cb) = if 3 * r + 2 < self.color_cache.len() {
                    (self.color_cache[3 * r] as f64, self.color_cache[3 * r + 1] as f64, self.color_cache[3 * r + 2] as f64)
                } else {
                    (0.5, 0.5, 0.5)
                };

                // NW hill-shade from the elevation gradient (edges clamp to self).
                let at = |gx2: usize, gy2: usize| elev[gy2 * n + gx2];
                let xl = if gx > 0 { at(gx - 1, gy) } else { e };
                let xr = if gx + 1 < n { at(gx + 1, gy) } else { e };
                let yu = if gy > 0 { at(gx, gy - 1) } else { e };
                let yd = if gy + 1 < n { at(gx, gy + 1) } else { e };
                let dzdx = if xr.is_nan() || xl.is_nan() { 0.0 } else { xr - xl };
                let dzdy = if yd.is_nan() || yu.is_nan() { 0.0 } else { yd - yu };
                let shade = (0.62 + (dzdx * light_x + dzdy * light_y) * 7.0).clamp(0.4, 1.3);
                cr *= shade;
                cg *= shade;
                cb *= shade;

                // Contour where the elevation band changes vs the right / down neighbour.
                let br = if !xr.is_nan() { band(xr) } else { band(e) };
                let bd = if !yd.is_nan() { band(yd) } else { band(e) };
                if band(e) != br || band(e) != bd {
                    cr *= 0.5;
                    cg *= 0.5;
                    cb *= 0.5;
                }

                // Depth cue: a cell just below a higher contour (to the north) is shaded — reads
                // as "under" the line so relief doesn't look inverted.
                if gy > 0 {
                    let up = elev[(gy - 1) * n + gx];
                    if !up.is_nan() && band(up) > band(e) {
                        cr *= 0.8;
                        cg *= 0.8;
                        cb *= 0.8;
                    }
                }

                // Painted-liquid overlay (Natural view only — blue water would clash with the
                // heat/moisture ramps in a data view).
                if self.view_mode == VIEW_NATURAL && r < self.field.depth.len() && self.field.depth[r] > 0.02 {
                    let (wr, wg, wb) = if self.field.kind[r] == 1 { (0.95, 0.35, 0.10) } else { (0.20, 0.45, 0.78) };
                    cr = cr * 0.4 + wr * 0.6;
                    cg = cg * 0.4 + wg * 0.6;
                    cb = cb * 0.4 + wb * 0.6;
                }

                let px = i * 4;
                out[px] = (cr.clamp(0.0, 1.0) * 255.0) as u8;
                out[px + 1] = (cg.clamp(0.0, 1.0) * 255.0) as u8;
                out[px + 2] = (cb.clamp(0.0, 1.0) * 255.0) as u8;
                out[px + 3] = 255;
            }
        }
        out
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

    /// Base ground colour for one region from its biome palette + elevation + moisture
    /// (before neighbour smoothing / jitter). The shared ramp ([`biomes::ground_color`]) is
    /// what ties biomes together; per-biome tokens give identity. Neutral if unsized.
    fn cell_color(&self, r: usize) -> [f32; 3] {
        // Data views: recolour by a single field directly (heatmap), decoupled from Natural colour.
        match self.view_mode {
            VIEW_TEMPERATURE => {
                return biomes::heat_ramp(self.temperature_r.get(r).copied().unwrap_or(0.5) as f32);
            }
            VIEW_MOISTURE => {
                return biomes::wet_ramp(self.moisture_r.get(r).copied().unwrap_or(0.5) as f32);
            }
            VIEW_ELEVATION => {
                return biomes::elevation_ramp(self.elevation_r.get(r).copied().unwrap_or(0.0) as f32);
            }
            VIEW_BIOME => {
                let id = self.biome_r.get(r).copied().unwrap_or(0) as usize;
                return self.biome_color.get(id).copied().unwrap_or([0.5, 0.5, 0.5]);
            }
            _ => {}
        }
        let fam = self.palette_family_r.get(r).copied().unwrap_or(0) as usize;
        let base = if fam < self.base_palettes.len() {
            self.base_palettes[fam]
        } else if !self.base_palettes.is_empty() {
            self.base_palettes[0]
        } else {
            biomes::base_palettes()[0]
        };
        let veg = self.vegetation_r.get(r).copied().unwrap_or(biomes::veg::GRASS);
        let e = self.elevation_r.get(r).copied().unwrap_or(0.0) as f32;
        let temp = self.temperature_r.get(r).copied().unwrap_or(0.5) as f32;
        let moist = self.moisture_r.get(r).copied().unwrap_or(0.5) as f32;
        biomes::resolve_color(&base, veg, e, temp, moist)
    }

    /// Per-region RGB color (3 floats per region) resolved from each region's biome palette
    /// by elevation + moisture, softened across neighbours (so biome seams read as gradients)
    /// with a subtle deterministic brightness jitter to break up flatness.
    fn region_color(&self) -> Vec<f32> {
        let nr = self.elevation_r.len();
        let mut c = vec![0.0f32; nr * 3];
        for r in 0..nr {
            let col = self.cell_color(r);
            c[3 * r] = col[0];
            c[3 * r + 1] = col[1];
            c[3 * r + 2] = col[2];
        }

        // Data views show the raw field — no neighbour smoothing or jitter, so the heatmap is exact.
        if self.view_mode != VIEW_NATURAL {
            return c;
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
                let base = self.cell_color(r);
                // Data views show the raw field; Natural adds the deterministic brightness jitter.
                let f = if self.view_mode == VIEW_NATURAL {
                    let h = hash_u32(r as u32);
                    1.0 + ((h & 0xffff) as f32 / 65535.0 - 0.5) * 2.0 * COLOR_VAR
                } else {
                    1.0
                };
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

/// Clamp to [0, 1] (f64).
fn clamp01f(x: f64) -> f64 {
    if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

/// Linear interpolate `a → b` by `t` (f64).
fn lerpf(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Ease a scalar trait field at `i` toward `target` by weight `w` (brush falloff).
fn ease_into(v: &mut [f64], i: usize, target: f64, w: f64) {
    if i < v.len() {
        let cur = v[i];
        v[i] = cur + (target - cur) * w;
    }
}

/// `iters` Jacobi passes of neighbour-mean blending (weight [`BLEND_W`]) over the cell graph,
/// starting from `base`. Averaging only — deterministic and convergent, with no transcendentals,
/// so it holds the cross-target contract. A locally-constant region is a fixed point (the mean of
/// equal neighbours is itself), so the field only changes in the bands around discontinuities.
fn diffuse_field(base: &[f64], neighbors: &[Vec<u32>], iters: usize) -> Vec<f64> {
    let n = base.len();
    let mut cur = base.to_vec();
    if iters == 0 || neighbors.len() != n {
        return cur;
    }
    let mut next = cur.clone();
    for _ in 0..iters {
        for r in 0..n {
            let nb = &neighbors[r];
            if nb.is_empty() {
                next[r] = cur[r];
                continue;
            }
            let mut sum = 0.0;
            for &j in nb {
                sum += cur[j as usize];
            }
            let mean = sum / nb.len() as f64;
            next[r] = cur[r] + (mean - cur[r]) * BLEND_W;
        }
        std::mem::swap(&mut cur, &mut next);
    }
    cur
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
