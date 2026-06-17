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
use crate::{elevation, geometry, rainfall, regionmap, regions, scatter, streams, volumetric};
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
/// Named-Region membership view: colour each cell by its Region's accent (grey if unassigned).
pub const VIEW_REGION: u8 = 5;
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
/// Default edge length of a rendering chunk, in world metres (overridable via
/// [`World::set_chunk_size_m`]). Chunks are a **fixed physical size** (not derived from the region
/// count), so each tile covers the same ground area regardless of density: finer streaming
/// granularity and a smaller, cheaper unit to show/hide as the camera moves. The world dimensions
/// are expected to be (near) a whole multiple of this; a partial edge tile is fine.
const DEFAULT_CHUNK_SIZE_M: f64 = 256.0;
/// Lower bound on a settable chunk size — keeps the chunk grid from exploding on a bad input.
const MIN_CHUNK_SIZE_M: f64 = 32.0;
/// Canon build defaults: how wide (world metres) the region **base-elevation trunk** grades across
/// territory borders (Pass 2), and how wide the seeded **traits** taper across them (Pass 3b). The
/// trunk grades broadly so macro relief has no cliffs; traits taper more tightly for crisp identity.
const DEFAULT_BASE_BLEND_M: f64 = 900.0;
const DEFAULT_TRANSITION_WIDTH_M: f64 = 300.0;
/// Climate-derivation defaults (ADR — the climate links). `LAPSE_RATE`: temperature drop per unit
/// normalized elevation above sea (mountains cold). `OROGRAPHIC_STRENGTH`: how strongly the
/// windward-wet / lee-dry rainfall field pulls moisture off its regional baseline. `MOISTURE_NOISE_AMP`:
/// spread of the moisture baseline around the region preset (varied, but region-centred).
const DEFAULT_LAPSE_RATE: f64 = 0.6;
const DEFAULT_OROGRAPHIC_STRENGTH: f64 = 0.45;
const MOISTURE_NOISE_AMP: f64 = 0.5;

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
    /// Climate-derivation params (the physical drivers behind the temperature/moisture traits).
    /// `lapse_rate` cools high ground (elevation→temperature); `orographic_strength` weights the
    /// windward-wet / lee-dry rainfall into moisture; `wind` is the prevailing-wind vector (the
    /// front-end supplies `[cosθ, sinθ]`, so the core stays transcendental-free). See [`derive_climate`].
    lapse_rate: f64,
    orographic_strength: f64,
    wind: [f64; 2],
    /// Per-cell **canon region** id (`0` = ocean, `1..=REGION_COUNT`). The authoritative tier: it is
    /// both the territory partition AND the biome (1:1 with the canon places). Set by the canon
    /// layout at [`build`], edited via [`paint_region`]/[`assign_region`], shown by `VIEW_REGION` in
    /// the region's accent. Persisted; the seam future per-region level slicing cuts on.
    region_r: Vec<u8>,
    /// Optional imported region-map override `(ids, cols, rows)` — a PNG the front-end resolved to a
    /// region-id grid. When `Some`, [`build`] samples it instead of the built-in canon anchors.
    region_layout_override: Option<(Vec<u8>, usize, usize)>,
    /// Representative swatch colour per Region preset (its `--mk-*` accent); for `biome_color_of`.
    biome_color: Vec<[f32; 3]>,
    /// The 7 shared base palettes (editable in the colour editor), indexed by `regions::fam::*`.
    base_palettes: Vec<regions::BasePalette>,
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
    /// Per-Region lake-fill threshold (normalized basin depth), id `1..=REGION_COUNT`. The slider-
    /// tunable knob [`fill_lakes`](World::fill_lakes) reads; seeded from the moisture-tied
    /// [`regions::default_lake_min_depth`].
    region_lake_depth: Vec<f64>,
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
    /// Edge length of a rendering chunk in world metres (see [`set_chunk_size_m`]); the chunk grid
    /// is `ceil(dim / chunk_size_m)` tiles each side. Set before [`build`]; defaults sensibly.
    chunk_size_m: f64,
    /// Dimensions of the (square) chunk grid; `chunk id = gy * chunk_cols + gx`. Both 0
    /// until [`build`] partitions the mesh. Stored so the front-end can map the camera to
    /// visible tiles (render-distance streaming) without re-deriving the layout.
    chunk_cols: usize,
    chunk_rows: usize,
    color_cache: Vec<f32>,
    /// Active data-view mode (see the `VIEW_*` consts). Off `Natural`, the colour cache holds a
    /// direct readout of one field instead of the composed terrain colour.
    view_mode: u8,
    /// Transient brush-sphere parameters set per stroke by [`set_brush_sphere`]: the Godot-space Y
    /// of the brush centre (the raycast hit) and the vertical exaggeration. The footprint brushes
    /// gate cells by **3D** distance — horizontal XZ plus the vertical `elev·exag − hit_y` term — so
    /// the brush is a sphere that bites the surface under the cursor from any view angle (the
    /// directional-brush fix, ADR 0005 §R3). Both default to 0 ⇒ a flat 2D footprint (top-down).
    brush_hit_y: f64,
    brush_exag: f64,
    /// Liquid render caches (parallel to the terrain chunk system): the smoothed liquid-surface
    /// height + wet mask per region, recomputed lazily when liquid changes (`liquid_cache_dirty`),
    /// plus a per-chunk dirty flag. The front-end streams + re-tessellates only in-range, changed
    /// liquid chunks — the whole surface was rebuilt + re-uploaded every tick before (the slow path).
    liquid_surf_cache: Vec<f64>,
    liquid_wet_cache: Vec<bool>,
    liquid_cache_dirty: bool,
    liquid_chunk_dirty: Vec<bool>,
    /// Active-set sim state: the regions the fluid solver still iterates (wet + not settled), a
    /// membership flag for O(1) dedup, and reusable per-region scratch for the relaxation. Settled
    /// water drops out, so a level basin costs nothing per step (the big sim win).
    liquid_active: Vec<u32>,
    liquid_active_flag: Vec<bool>,
    liquid_delta: Vec<f64>,
    liquid_touched_flag: Vec<bool>,
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
        let roster = regions::region_presets();
        let mut biome_color = vec![[0.5f32, 0.5, 0.5]; regions::REGION_COUNT + 1];
        let mut biome_landform = vec![[0.0f32; 4]; regions::REGION_COUNT + 1];
        let mut biome_water = vec![[0.0f32; 5]; regions::REGION_COUNT + 1];
        for (i, b) in roster.iter().enumerate() {
            biome_color[i + 1] = b.representative();
            let t = &b.traits;
            biome_landform[i + 1] = [t.jaggedness, t.relief, t.foothill_falloff, t.erosion];
            let w = &b.water;
            biome_water[i + 1] = [w.raininess, w.rain_shadow, w.evaporation, w.flow, w.ocean_depth];
        }
        let mut region_lake_depth = vec![0.0f64; regions::REGION_COUNT + 1];
        for (i, slot) in region_lake_depth.iter_mut().enumerate() {
            *slot = regions::default_lake_min_depth(i as u8);
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
            lapse_rate: DEFAULT_LAPSE_RATE,
            orographic_strength: DEFAULT_OROGRAPHIC_STRENGTH,
            wind: rainfall::WIND,
            region_r: Vec::new(),
            region_layout_override: None,
            biome_color,
            base_palettes: regions::base_palettes().to_vec(),
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
            region_lake_depth,
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
            chunk_size_m: DEFAULT_CHUNK_SIZE_M,
            chunk_cols: 0,
            chunk_rows: 0,
            color_cache: Vec::new(),
            view_mode: VIEW_NATURAL,
            brush_hit_y: 0.0,
            brush_exag: 0.0,
            liquid_surf_cache: Vec::new(),
            liquid_wet_cache: Vec::new(),
            liquid_cache_dirty: true,
            liquid_chunk_dirty: Vec::new(),
            liquid_active: Vec::new(),
            liquid_active_flag: Vec::new(),
            liquid_delta: Vec::new(),
            liquid_touched_flag: Vec::new(),
        }
    }

    /// Build the **canon map** over `[0,width] × [0,height]` at `spacing`, seeded by `seed`/`octaves`:
    /// region layout (anchors or an imported PNG) → territories → region-guided elevation → trait
    /// seeding (Region → Traits) → border blend, then re-applies the stored sea level so the ocean +
    /// Great Lake track the new terrain.
    pub fn build(&mut self, width: f64, height: f64, spacing: f64, seed: u64, octaves: u32) {
        let mesh = Mesh::new(width, height, spacing, seed);
        let nr = mesh.num_regions();
        self.seed = seed;
        self.width = width;
        self.height = height;
        self.neighbors = mesh.region_neighbors();
        self.field = LiquidField::new(nr);
        self.course_mask = vec![false; nr];
        self.stream_carve = vec![0.0; nr];
        self.shape_delta = vec![0.0; nr];

        // --- Canon-map pipeline (ADR 0004 / the canon-map generator) ---------------------------
        // Pass 1 — region layout (territories): a built-in canonical anchor map, or an injected
        // PNG-derived id grid override. 0 = ocean, 1..=REGION_COUNT = a canon region. This single
        // field IS the territory partition and the authoritative region/biome tier.
        let region_ids = self.canon_region_ids(&mesh, width, height);
        self.region_r = region_ids.clone();
        // Regions are *authored*, not auto-classified → lock every cell so terrain edits never
        // reclassify them away (`reclassify` then no-ops across the canon map).
        self.biome_locked = vec![true; nr];

        // Pass 2 — region-guided elevation: a diffused per-region base trunk + low-freq noise +
        // a banded ocean rim. Then flood the ocean + the Great Lake basin to the stored sea level.
        self.elevation_r = elevation::assign_region_elevation_canon(
            &mesh, width, height, seed, octaves, &region_ids, &self.neighbors, DEFAULT_BASE_BLEND_M,
        );
        fluid::sea_fill(&mut self.field, &self.elevation_r, self.sea_level);

        // Pass 3 — biome pass (Region → Traits): seed each cell's trait primitives from its region
        // preset. temperature & moisture are then *replaced* by the physical climate derivation (3b).
        let region = &self.region_r;
        let tr: Vec<regions::CellTraits> = crate::util::par_map(nr, |r| regions::default_traits_for(region[r]));
        self.jaggedness_r = tr.iter().map(|t| t.jaggedness as f64).collect();
        self.relief_r = tr.iter().map(|t| t.relief as f64).collect();
        self.foothill_falloff_r = tr.iter().map(|t| t.foothill_falloff as f64).collect();
        self.erosion_r = tr.iter().map(|t| t.erosion as f64).collect();
        self.temperature_r = tr.iter().map(|t| t.temperature as f64).collect();
        self.moisture_r = tr.iter().map(|t| t.moisture as f64).collect();
        self.vegetation_r = tr.iter().map(|t| t.vegetation).collect();
        self.palette_family_r = tr.iter().map(|t| t.palette_family).collect();
        // Painted base for the landform traits (the climate bases are set by `derive_climate`).
        self.jaggedness_base = self.jaggedness_r.clone();
        self.relief_base = self.relief_r.clone();
        self.foothill_falloff_base = self.foothill_falloff_r.clone();
        self.erosion_base = self.erosion_r.clone();

        // Pass 3b — climate (physical traits): elevation→temperature (lapse) + orographic→moisture,
        // with the region presets as biases. Uses `self.width/height/seed` (set at the top) for the
        // moisture-noise lookup; sets `temperature_r`/`moisture_r` and their painted bases.
        let positions: Vec<[f64; 2]> = crate::util::par_map(nr, |r| mesh.pos_of_r(r));
        self.derive_climate(&positions);

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

        self.mesh = Some(mesh);

        // Partition into rendering chunks and cache smoothed colors for them.
        self.build_chunks();
        // Pass 3b — neighbour-aware taper: diffuse the seeded traits across territory borders
        // (blended transitions) from the base snapshots; this also caches the smoothed colours.
        self.blend_traits(DEFAULT_TRANSITION_WIDTH_M);
        // `blend_traits` flags every chunk dirty; a fresh build has nothing to re-tessellate yet
        // (chunks stream/tessellate on demand), so clear the flags so the first edit reads clean.
        for d in self.chunk_dirty.iter_mut() {
            *d = false;
        }
        // Liquid render caches (smoothed surface + wet mask), recomputed lazily on first request.
        self.liquid_surf_cache = vec![0.0; nr];
        self.liquid_wet_cache = vec![false; nr];
        self.liquid_cache_dirty = true;
        // Active-set sim scratch. The active set starts empty: the build's sea fill is already a
        // level surface (settled), so there's nothing to iterate until an edit/rain/sculpt wakes it.
        self.liquid_active = Vec::new();
        self.liquid_active_flag = vec![false; nr];
        self.liquid_delta = vec![0.0; nr];
        self.liquid_touched_flag = vec![false; nr];
    }

    /// Inject a region-map override: a front-end-resolved id grid (a region-coloured PNG matched to
    /// the 14 region accents + ocean, row 0 = north). The next [`build`] samples this instead of the
    /// built-in canon anchors. An empty/ill-sized grid clears the override.
    pub fn set_region_layout(&mut self, ids: &[u8], cols: usize, rows: usize) {
        if cols == 0 || rows == 0 || ids.len() < cols * rows {
            self.region_layout_override = None;
        } else {
            self.region_layout_override = Some((ids.to_vec(), cols, rows));
        }
    }

    /// Drop any imported region-map override → [`build`] reverts to the built-in canon anchors.
    pub fn clear_region_layout(&mut self) {
        self.region_layout_override = None;
    }

    /// Resolve the per-cell region ids for [`build`]: the imported PNG grid if one is set, else the
    /// built-in canonical anchor layout.
    fn canon_region_ids(&self, mesh: &Mesh, width: f64, height: f64) -> Vec<u8> {
        match &self.region_layout_override {
            Some((ids, cols, rows)) => regionmap::layout_from_grid(mesh, width, height, ids, *cols, *rows),
            None => regionmap::layout_from_anchors(mesh, width, height),
        }
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
            Some(mesh) => scatter::scatter(seed, mesh, &self.elevation_r, &self.region_r, exaggeration, density),
            None => Vec::new(),
        }
    }

    // --- liquid simulation ---

    /// Lowest authored elevation across the world (normalized); `0.0` before [`build`]. Lets the
    /// front-end seat a default sea level a fixed height above the terrain's deepest basin.
    pub fn min_elevation(&self) -> f64 {
        if self.elevation_r.is_empty() {
            return 0.0;
        }
        self.elevation_r.iter().copied().fold(f64::INFINITY, f64::min)
    }

    /// Fill every region below `level` with water (instant sea + lakes).
    pub fn set_sea_level(&mut self, level: f64) {
        self.sea_level = level;
        fluid::sea_fill(&mut self.field, &self.elevation_r, level);
        self.mark_all_liquid_changed();
    }

    /// Add a uniform `amount` of rainfall to land above the current sea level.
    pub fn rain(&mut self, amount: f64) {
        fluid::add_rain(&mut self.field, &self.elevation_r, self.sea_level, amount);
        self.mark_all_liquid_changed();
    }

    /// Deposit climate-driven rainfall: a per-cell field derived from each biome's water profile
    /// (raininess / rain-shadow / evaporation) plus the per-cell moisture / temperature traits and
    /// an orographic term (terrain rising into the prevailing wind rains more; the lee sits in
    /// shadow). Run [`step_fluid`](World::step_fluid) afterwards to let it pool and drain.
    pub fn apply_rainfall(&mut self) {
        let n = self.elevation_r.len();
        if n == 0 || self.mesh.is_none() {
            return;
        }
        let positions: Vec<[f64; 2]> = {
            let mesh = self.mesh.as_ref().unwrap();
            (0..n).map(|r| mesh.pos_of_r(r)).collect()
        };
        let rain = rainfall::compute_rainfall(
            &self.elevation_r,
            &self.moisture_r,
            &self.temperature_r,
            &self.region_r,
            &self.biome_water,
            &positions,
            &self.neighbors,
            self.sea_level,
            self.wind,
        );
        fluid::add_rain_field(&mut self.field, &self.elevation_r, self.sea_level, &rain);
        self.mark_all_liquid_changed();
    }

    // --- climate derivation (the physical drivers behind temperature + moisture) ---

    /// Derive the **physical** temperature + moisture traits from the current elevation, the region
    /// presets (as biases), and the climate params:
    /// - **#1 elevation→temperature (lapse):** `temperature = region_temp − lapse·max(0, elevation)`,
    ///   so high ground is colder and snow caps / treelines emerge.
    /// - **#2 orographic→moisture:** a regional moisture baseline (region preset + noise) shifted by
    ///   the windward-wet / lee-dry rainfall field ([`rainfall::compute_rainfall`]).
    ///
    /// Sets `temperature_r`/`moisture_r` and their painted bases. Deterministic: `par_map` + lerp /
    /// clamp + the transcendental-free rainfall pass; the max-normalise is an order-independent fold.
    /// The cycle (rain needs moisture, moisture needs rain) is broken by feeding the *baseline*
    /// moisture into the rainfall pass and folding the rainfall *output* into the final moisture.
    fn derive_climate(&mut self, positions: &[[f64; 2]]) {
        let n = self.elevation_r.len();
        if n == 0 || self.neighbors.len() != n {
            return;
        }
        let (width, height, seed, lapse) = (self.width, self.height, self.seed, self.lapse_rate);
        let region = &self.region_r;
        let elev = &self.elevation_r;
        // #1 — temperature lapse (region preset is the bias / "latitude").
        let temperature: Vec<f64> = crate::util::par_map(n, |r| {
            let base = regions::default_traits_for(region[r]).temperature as f64;
            (base - lapse * elev[r].max(0.0)).clamp(0.0, 1.0)
        });
        // Moisture baseline: region preset (bias) + noise variety, centred on the region.
        let base_moist: Vec<f64> = crate::util::par_map(n, |r| {
            let base = regions::default_traits_for(region[r]).moisture as f64;
            let noise = regions::moisture_at(positions[r][0], positions[r][1], width, height, seed);
            (base + (noise - 0.5) * MOISTURE_NOISE_AMP).clamp(0.0, 1.0)
        });
        // #2 — orographic rainfall → moisture (baseline moisture feeds in; the output shifts it).
        let rain = rainfall::compute_rainfall(
            &self.elevation_r, &base_moist, &temperature, &self.region_r, &self.biome_water,
            positions, &self.neighbors, self.sea_level, self.wind,
        );
        let max_rain = rain.iter().copied().fold(0.0_f64, f64::max);
        let w = self.orographic_strength;
        let moisture: Vec<f64> = if max_rain > 0.0 {
            crate::util::par_map(n, |r| {
                let rain_norm = rain[r] / max_rain;
                (base_moist[r] * (1.0 - w) + rain_norm * w).clamp(0.0, 1.0)
            })
        } else {
            base_moist
        };
        self.temperature_r = temperature;
        self.moisture_r = moisture;
        self.temperature_base = self.temperature_r.clone();
        self.moisture_base = self.moisture_r.clone();
    }

    /// Re-derive temperature + moisture from the current climate params + elevation, then re-taper +
    /// recolour — the live path behind the climate sliders (no full regen). No-op before [`build`].
    pub fn recompute_climate(&mut self) {
        let n = self.elevation_r.len();
        let positions: Vec<[f64; 2]> = match &self.mesh {
            Some(m) if m.num_regions() == n => (0..n).map(|r| m.pos_of_r(r)).collect(),
            _ => return,
        };
        self.derive_climate(&positions);
        self.blend_traits(DEFAULT_TRANSITION_WIDTH_M); // re-tapers from the new bases + recolours
    }

    /// Temperature lapse strength (°/elevation). Re-run [`recompute_climate`] to apply.
    pub fn set_lapse_rate(&mut self, v: f64) {
        self.lapse_rate = v.max(0.0);
    }
    pub fn lapse_rate(&self) -> f64 {
        self.lapse_rate
    }
    /// Orographic weight (0..1): how strongly windward-wet / lee-dry rainfall pulls moisture off its
    /// regional baseline. Re-run [`recompute_climate`] to apply.
    pub fn set_orographic_strength(&mut self, v: f64) {
        self.orographic_strength = v.clamp(0.0, 1.0);
    }
    pub fn orographic_strength(&self) -> f64 {
        self.orographic_strength
    }
    /// Set the prevailing wind vector (the front-end supplies `[cosθ, sinθ]`, keeping the core
    /// transcendental-free). Normalised; a zero vector falls back to the default west→east.
    pub fn set_wind(&mut self, wx: f64, wy: f64) {
        let len = (wx * wx + wy * wy).sqrt();
        self.wind = if len > 1e-9 { [wx / len, wy / len] } else { rainfall::WIND };
    }
    pub fn wind(&self) -> [f64; 2] {
        self.wind
    }

    /// Advance the hydraulic solver `substeps` relaxation steps.
    /// Advance the hydraulic solver `substeps` relaxation steps over the **active set** (wet, not yet
    /// settled). Settled water isn't iterated, and each step flags only the chunks whose water moved,
    /// so a level basin (and an idle map) cost nothing. Stops early once the active set drains.
    pub fn step_fluid(&mut self, flow_rate: f64, evaporation: f64, substeps: u32) {
        let n = self.elevation_r.len();
        if n == 0 {
            return;
        }
        if self.liquid_delta.len() != n {
            self.liquid_delta = vec![0.0; n];
        }
        if self.liquid_touched_flag.len() != n {
            self.liquid_touched_flag = vec![false; n];
        }
        if self.liquid_active_flag.len() != n {
            self.liquid_active_flag = vec![false; n];
        }
        let mut changed: Vec<u32> = Vec::new();
        let mut moved = false;
        for _ in 0..substeps {
            if self.liquid_active.is_empty() {
                break;
            }
            changed.clear();
            fluid::relax_step_active(
                &mut self.field,
                &self.elevation_r,
                &self.neighbors,
                flow_rate,
                evaporation,
                &self.liquid_active,
                &mut self.liquid_delta,
                &mut self.liquid_touched_flag,
                &mut changed,
            );
            // Clear the current membership; rebuild the active set from what moved this step.
            for &r in &self.liquid_active {
                self.liquid_active_flag[r as usize] = false;
            }
            self.liquid_active.clear();
            if changed.is_empty() {
                break; // settled — nothing moved
            }
            moved = true;
            // Flag the chunks of the regions that moved (render-dirty).
            for &r in &changed {
                if let Some(chunks) = self.region_chunks.get(r as usize) {
                    for &c in chunks {
                        if let Some(d) = self.liquid_chunk_dirty.get_mut(c as usize) {
                            *d = true;
                        }
                    }
                }
            }
            // Next active = moved regions ∪ their neighbours (so a new gradient at the frontier is
            // reconsidered). Gather first to avoid aliasing `neighbors` with the active push.
            let mut next: Vec<u32> = Vec::new();
            for &r in &changed {
                next.push(r);
                if let Some(nbs) = self.neighbors.get(r as usize) {
                    next.extend_from_slice(nbs);
                }
            }
            for r in next {
                let ru = r as usize;
                if !self.liquid_active_flag[ru] {
                    self.liquid_active_flag[ru] = true;
                    self.liquid_active.push(r);
                }
            }
        }
        if moved {
            self.liquid_cache_dirty = true;
        }
    }

    /// Number of regions the fluid solver is still iterating (0 ⇒ settled; the front-end can stop
    /// ticking the sim).
    pub fn liquid_active_count(&self) -> usize {
        self.liquid_active.len()
    }

    /// Remove all liquid.
    pub fn clear_liquid(&mut self) {
        self.field.clear();
        self.mark_all_liquid_changed();
    }

    /// Deposit **perched lakes**: standing water in every closed basin whose pour-point surface sits
    /// *above* the global sea level, filled to that pour point via the priority-flood. This is the
    /// lake tier of the two-tier water model: the ocean is `set_sea_level` (a single global surface);
    /// lakes/ponds perch in highland basins, held by their ring like a real lake (e.g. Lake Tahoe),
    /// independent of the ocean.
    ///
    /// The ponding **threshold** is per-cell: each cell takes its **Region's** lake-fill depth
    /// ([`region_lake_depth_of`]) — itself seeded from the region's moisture trait and slider-tunable —
    /// modulated by the cell's **local moisture** (wetter cells pond with shallower basins). So a basin
    /// must be deeper than its region+trait threshold to hold water; shallower dips stay dry, which is
    /// what gates terrain noise out of arid regions while letting Marsh/Great-Lake pond readily.
    ///
    /// It is a **static** fill (like the sea): it renders but does not wake the relaxation sim, which
    /// — with no inflow — would drain a pour-point lake over its spill. Run after the ocean
    /// `set_sea_level`; re-run if the sea level or a region threshold changes.
    pub fn fill_lakes(&mut self) {
        let n = self.elevation_r.len();
        let num_boundary = match &self.mesh {
            Some(m) => m.num_boundary_regions(),
            None => return,
        };
        if self.neighbors.len() != n {
            return;
        }
        let filled = streams::fill_depressions(&self.elevation_r, &self.neighbors, num_boundary);
        for r in num_boundary..n {
            if !filled[r].is_finite() {
                continue;
            }
            let lake = filled[r] - self.elevation_r[r];
            // A perched lake: a closed basin whose water surface is above the ocean. (Ocean cells
            // drain to the boundary ⇒ filled ≈ terrain ⇒ skipped; their water is `sea_fill`'s.)
            if lake <= 0.0 || filled[r] <= self.sea_level {
                continue;
            }
            // Threshold = the cell's Region default/slider, modulated by its local moisture trait.
            let region = self.region_r.get(r).copied().unwrap_or(0) as usize;
            let base = self.region_lake_depth.get(region).copied().unwrap_or(0.05);
            let moisture = self.moisture_r.get(r).copied().unwrap_or(0.5).clamp(0.0, 1.0);
            let factor = (1.4 - 0.8 * moisture).max(0.1); // dry 1.4× … wet 0.6× (centred at 0.5 → 1×)
            if lake > base * factor {
                self.field.depth[r] = lake.max(self.field.depth[r]);
                self.field.kind[r] = crate::liquids::LiquidType::Water as u8;
            }
        }
        // Render the new water, but leave the sim asleep (a static fill, like the sea).
        self.liquid_cache_dirty = true;
        for d in self.liquid_chunk_dirty.iter_mut() {
            *d = true;
        }
    }

    /// Region `id`'s lake-fill threshold (normalized basin depth); `0` if out of range.
    pub fn region_lake_depth_of(&self, id: usize) -> f64 {
        self.region_lake_depth.get(id).copied().unwrap_or(0.0)
    }
    /// Set region `id`'s lake-fill threshold (clamped ≥ 0). Re-run [`fill_lakes`] to apply.
    pub fn set_region_lake_depth(&mut self, id: usize, value: f64) {
        if let Some(v) = self.region_lake_depth.get_mut(id) {
            *v = value.max(0.0);
        }
    }
    /// Export the per-Region lake-depth table (one threshold per region id) for save/load.
    pub fn region_lake_depth_export(&self) -> Vec<f32> {
        self.region_lake_depth.iter().map(|&v| v as f32).collect()
    }
    /// Restore the per-Region lake-depth table.
    pub fn set_region_lake_depth_table(&mut self, vals: &[f32]) {
        for (i, v) in self.region_lake_depth.iter_mut().enumerate() {
            if i < vals.len() {
                *v = vals[i] as f64;
            }
        }
    }

    // --- brush tools ---

    /// Set the active brush's vertical sphere: `hit_y` is the Godot-space Y of the brush centre (the
    /// raycast hit's height = `elev·exaggeration`), `exaggeration` the current vertical scale. The
    /// footprint brushes then gate cells by 3D distance (XZ + the vertical `elev·exag − hit_y` term),
    /// so the brush bites the surface under the cursor from any angle. Call once per stroke before
    /// painting; `(0, 0)` (the default) restores a flat 2D footprint. Mirrors [`set_view_mode`].
    pub fn set_brush_sphere(&mut self, hit_y: f64, exaggeration: f64) {
        self.brush_hit_y = hit_y;
        self.brush_exag = exaggeration;
    }

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
        let (bexag, bhy) = (self.brush_exag, self.brush_hit_y);

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
                let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
                let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv;
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
        self.mark_liquid_changed(&touched); // risen land displaced the water it rose through
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
        let (bexag, bhy) = (self.brush_exag, self.brush_hit_y);
        let mut touched = Vec::new();
        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
                let p = mesh.pos_of_r(ri);
                let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
                let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv;
                if d2 >= r2 {
                    continue;
                }
                let t = 1.0 - (d2 / r2).sqrt();
                let add = amount * t * t * (3.0 - 2.0 * t);
                if add > 0.0 {
                    self.field.depth[ri] += add;
                    self.field.kind[ri] = kind;
                    touched.push(rid);
                }
            }
        }
        self.mark_liquid_changed(&touched);
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
        let (bexag, bhy) = (self.brush_exag, self.brush_hit_y);

        // Bed reference = lowest elevation under the brush this dab (the channel follows
        // the existing downhill grade rather than cutting a flat trench).
        let mut e_ref = f64::INFINITY;
        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
                let p = mesh.pos_of_r(ri);
                let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
                let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv;
                if d2 < r2 && self.elevation_r[ri] < e_ref {
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
                let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
                let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv;
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
        self.mark_liquid_changed(&touched);
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
        self.mark_all_liquid_changed(); // streams carve channels + lay water across the map
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
        let (bexag, bhy) = (self.brush_exag, self.brush_hit_y);
        let mesh = self.mesh.as_ref().unwrap();
        let mut touched = Vec::new();
        for &rid in &candidates {
            let ri = rid as usize;
            let p = mesh.pos_of_r(ri);
            let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
            if (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv < r2 {
                self.region_r[ri] = biome_id;
                if ri < self.biome_locked.len() {
                    self.biome_locked[ri] = true; // manual paint — protect from reclassify
                }
                touched.push(rid);
            }
        }
        self.after_edit(&touched);
        touched
    }

    /// Brush: assign every cell in the footprint to named Region `region_id` (the Region tool). Like
    /// [`paint_biome`] but writes the Region tier (no biome lock); shows live under `VIEW_REGION`.
    /// 3D-sphere falloff via the active brush state, consistent with the other footprint brushes.
    pub fn paint_region(&mut self, cx: f64, cy: f64, radius: f64, region_id: u8) -> Vec<u32> {
        if self.mesh.is_none() {
            return Vec::new();
        }
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);
        let (bexag, bhy) = (self.brush_exag, self.brush_hit_y);
        let mesh = self.mesh.as_ref().unwrap();
        let mut touched = Vec::new();
        for &rid in &candidates {
            let ri = rid as usize;
            let p = mesh.pos_of_r(ri);
            let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
            if (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv < r2 {
                if ri < self.region_r.len() {
                    self.region_r[ri] = region_id;
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

    /// Set one landform component (`idx`: 0 jaggedness, 1 relief, 2 foothill_falloff, 3 erosion)
    /// of region `region_id`, and stamp it onto every cell currently in that region (live + painted
    /// base), so a later [`shape_terrain`] reshapes that region's whole area. This ties the landform
    /// dials to the Regions rather than a per-cell brush. Invisible until shaping bakes it, so no
    /// recolour here. (Cells re-classified into the region after this won't carry the value until
    /// re-applied — same caveat as the rest of the auto-classify path.)
    pub fn set_region_landform(&mut self, region_id: u8, idx: usize, value: f64) {
        if idx >= 4 {
            return;
        }
        if let Some(a) = self.biome_landform.get_mut(region_id as usize) {
            a[idx] = value as f32;
        }
        let n = self.region_r.len();
        for r in 0..n {
            if self.region_r[r] != region_id {
                continue;
            }
            match idx {
                0 => { self.jaggedness_r[r] = value; self.jaggedness_base[r] = value; }
                1 => { self.relief_r[r] = value; self.relief_base[r] = value; }
                2 => { self.foothill_falloff_r[r] = value; self.foothill_falloff_base[r] = value; }
                3 => { self.erosion_r[r] = value; self.erosion_base[r] = value; }
                _ => {}
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
        let (bexag, bhy) = (self.brush_exag, self.brush_hit_y);
        let mut touched = Vec::new();
        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
                let p = mesh.pos_of_r(ri);
                let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
                let d2 = (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv;
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
        let tr = regions::default_traits_for(biome_id);
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);
        let (bexag, bhy) = (self.brush_exag, self.brush_hit_y);
        let mut touched = Vec::new();
        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
                let p = mesh.pos_of_r(ri);
                let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
                if (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv >= r2 {
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
                if ri < self.region_r.len() {
                    self.region_r[ri] = biome_id;
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

    /// Locally blend the scalar trait fields within the brush footprint — the **transition brush**.
    /// A drag across a biome seam softens just the cells under the brush (Laplacian passes over the
    /// footprint only), unlike the global [`blend_traits`] bake. `blend_width_m` maps to a pass count
    /// the same way as the global bake. Smooths the live `*_r` fields in place (strokes accumulate);
    /// leaves the painted `*_base` snapshots untouched, so a later global blend still grades from the
    /// original paints. Returns the cells touched (for the front-end's dirty repaint).
    pub fn blend_brush(&mut self, cx: f64, cy: f64, radius: f64, blend_width_m: f64) -> Vec<u32> {
        let n = self.elevation_r.len();
        if self.mesh.is_none() || n == 0 || self.neighbors.len() != n {
            return Vec::new();
        }
        let r2 = radius * radius;
        let candidates = self.brush_candidates(cx, cy, radius);
        let (bexag, bhy) = (self.brush_exag, self.brush_hit_y);
        let mut footprint: Vec<u32> = Vec::new();
        {
            let mesh = self.mesh.as_ref().unwrap();
            for &rid in &candidates {
                let ri = rid as usize;
                let p = mesh.pos_of_r(ri);
                let dv = self.elevation_r[ri] * bexag - bhy; // 3D sphere: vertical term
                if (p[0] - cx).powi(2) + (p[1] - cy).powi(2) + dv * dv < r2 {
                    footprint.push(rid);
                }
            }
        }
        if footprint.is_empty() {
            return footprint;
        }
        // Width (m) → pass count, same mapping as blend_traits (one pass ≈ one cell-ring).
        let pitch = (self.width * self.height / n as f64).sqrt().max(1.0);
        let iters = ((blend_width_m / pitch).round() as usize).clamp(1, MAX_BLEND_ITERS);
        diffuse_subset(&mut self.jaggedness_r, &self.neighbors, &footprint, iters);
        diffuse_subset(&mut self.relief_r, &self.neighbors, &footprint, iters);
        diffuse_subset(&mut self.foothill_falloff_r, &self.neighbors, &footprint, iters);
        diffuse_subset(&mut self.erosion_r, &self.neighbors, &footprint, iters);
        diffuse_subset(&mut self.temperature_r, &self.neighbors, &footprint, iters);
        diffuse_subset(&mut self.moisture_r, &self.neighbors, &footprint, iters);
        self.after_edit(&footprint);
        footprint
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

    /// Human descriptor of the emergent biome at world `(x, y)` (the nearest cell's traits), e.g.
    /// "temperate forest hills". `None` outside the map. See [`regions::biome_label`].
    pub fn biome_label_at(&self, x: f64, y: f64) -> Option<String> {
        let r = self.region_at(x, y)?;
        Some(regions::biome_label(
            self.elevation_r[r],
            self.jaggedness_r.get(r).copied().unwrap_or(0.0),
            self.relief_r.get(r).copied().unwrap_or(0.0),
            self.temperature_r.get(r).copied().unwrap_or(0.5),
            self.moisture_r.get(r).copied().unwrap_or(0.5),
            self.vegetation_r.get(r).copied().unwrap_or(0),
        ))
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
        self.region_r.get(region).copied().unwrap_or(0)
    }

    /// Reassign a single region's biome (Select right-click); locks it from reclassify.
    pub fn set_biome_of(&mut self, region: usize, id: u8) {
        if region < self.region_r.len() {
            self.region_r[region] = id;
            if region < self.biome_locked.len() {
                self.biome_locked[region] = true;
            }
            self.after_edit(&[region as u32]);
        }
    }

    // --- named-Region tier (Stage 4): the canon places as cell-membership sets ---

    /// Assign `cells` to named Region `region_id` (`0` clears; `1..=REGION_COUNT`). Pair with
    /// [`regions_in_polygon`] / [`select_contiguous`] for area assignment. Recolours the touched
    /// cells (so `VIEW_REGION` updates live) and flags their chunks.
    pub fn assign_region(&mut self, cells: &[u32], region_id: u8) {
        let mut touched = Vec::with_capacity(cells.len());
        for &c in cells {
            let ci = c as usize;
            if ci < self.region_r.len() {
                self.region_r[ci] = region_id;
                touched.push(c);
            }
        }
        self.after_edit(&touched);
    }
    /// Named-Region id of `cell` (`0` if unassigned / out of range).
    pub fn region_of(&self, cell: usize) -> u8 {
        self.region_r.get(cell).copied().unwrap_or(0)
    }
    /// Named-Region id at world `(x, y)` (`0` unassigned, `-1` off-map) — for the cursor readout.
    pub fn region_id_at(&self, x: f64, y: f64) -> i64 {
        match self.region_at(x, y) {
            Some(c) => self.region_r.get(c).copied().unwrap_or(0) as i64,
            None => -1,
        }
    }

    /// Contiguous same-biome region ids reachable from `region` (flood select),
    /// excluding the boundary frame.
    pub fn select_contiguous(&self, region: usize) -> Vec<u32> {
        let start = region;
        if start >= self.region_r.len() || self.neighbors.len() != self.region_r.len() {
            return Vec::new();
        }
        let target = self.region_r[start];
        let mut seen = vec![false; self.region_r.len()];
        let mut stack = vec![start];
        seen[start] = true;
        let mut out = Vec::new();
        while let Some(r) = stack.pop() {
            out.push(r as u32);
            for &nb in &self.neighbors[r] {
                let nb = nb as usize;
                if seen[nb] || self.region_r.get(nb).copied() != Some(target) {
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
        self.region_r.clone()
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
        if b.len() == self.region_r.len() {
            self.region_r.copy_from_slice(b);
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

    /// Export the manual-biome lock mask (cells the author painted, protected from auto-reclassify).
    pub fn biome_locked_export(&self) -> Vec<u8> {
        self.biome_locked.iter().map(|&b| b as u8).collect()
    }
    /// Restore the manual-biome lock mask (size must match the mesh), so loaded manual paints survive
    /// the next reclassify just as they did before saving.
    pub fn set_biome_locked(&mut self, m: &[u8]) {
        if m.len() == self.biome_locked.len() {
            for (i, &v) in m.iter().enumerate() {
                self.biome_locked[i] = v != 0;
            }
        }
    }

    /// Export the named-Region membership (per-cell Region id) for save/load.
    pub fn region_export(&self) -> Vec<u8> {
        self.region_r.clone()
    }
    /// Restore named-Region membership (size must match the mesh). Save compat: a pre-canon save has
    /// `region` all-zero (regions were unpainted) while its `biome` array carried the data, so an
    /// all-ocean slice is ignored — `set_biome` (which now writes the same field) keeps that data.
    pub fn set_region(&mut self, r: &[u8]) {
        if r.len() == self.region_r.len() && r.iter().any(|&v| v != 0) {
            self.region_r.copy_from_slice(r);
        }
    }

    /// Export per-cell trait field `trait_id` (as in [`paint_trait`]: 0 jaggedness … 5 moisture
    /// scalars; 6 vegetation, 7 palette_family as enum indices) as `f32`, for save/load.
    pub fn trait_field_export(&self, trait_id: u32) -> Vec<f32> {
        match trait_id {
            0 => self.jaggedness_r.iter().map(|&v| v as f32).collect(),
            1 => self.relief_r.iter().map(|&v| v as f32).collect(),
            2 => self.foothill_falloff_r.iter().map(|&v| v as f32).collect(),
            3 => self.erosion_r.iter().map(|&v| v as f32).collect(),
            4 => self.temperature_r.iter().map(|&v| v as f32).collect(),
            5 => self.moisture_r.iter().map(|&v| v as f32).collect(),
            6 => self.vegetation_r.iter().map(|&v| v as f32).collect(),
            7 => self.palette_family_r.iter().map(|&v| v as f32).collect(),
            _ => Vec::new(),
        }
    }

    /// Restore a saved trait field (size must match the mesh). Scalars (0–5) also reset the painted
    /// base to the loaded values, so the render matches and a later [`blend_traits`] grades from here
    /// (a re-blend after load re-bases once — the same one-time caveat as `shape_delta`; see the
    /// reshell persistence note). Enums (6–7) snap to the nearest index.
    pub fn set_trait_field(&mut self, trait_id: u32, vals: &[f32]) {
        let n = self.elevation_r.len();
        if vals.len() != n {
            return;
        }
        match trait_id {
            0 => for i in 0..n { let v = vals[i] as f64; self.jaggedness_r[i] = v; self.jaggedness_base[i] = v; },
            1 => for i in 0..n { let v = vals[i] as f64; self.relief_r[i] = v; self.relief_base[i] = v; },
            2 => for i in 0..n { let v = vals[i] as f64; self.foothill_falloff_r[i] = v; self.foothill_falloff_base[i] = v; },
            3 => for i in 0..n { let v = vals[i] as f64; self.erosion_r[i] = v; self.erosion_base[i] = v; },
            4 => for i in 0..n { let v = vals[i] as f64; self.temperature_r[i] = v; self.temperature_base[i] = v; },
            5 => for i in 0..n { let v = vals[i] as f64; self.moisture_r[i] = v; self.moisture_base[i] = v; },
            6 => for i in 0..n { self.vegetation_r[i] = vals[i].round().max(0.0) as u8; },
            7 => for i in 0..n { self.palette_family_r[i] = vals[i].round().max(0.0) as u8; },
            _ => {}
        }
    }

    /// Export the 7 shared base palettes flat: `[family][slot 0..5][r,g,b]` (7×6×3 = 126 f32).
    pub fn base_palettes_export(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.base_palettes.len() * 18);
        for bp in &self.base_palettes {
            for c in [bp.water_deep, bp.water_shallow, bp.low, bp.rock, bp.cap_warm, bp.cap_cold] {
                out.extend_from_slice(&c);
            }
        }
        out
    }
    /// Restore the base palettes from [`base_palettes_export`] layout (trailing/short data ignored).
    pub fn set_base_palettes(&mut self, vals: &[f32]) {
        const STRIDE: usize = 18; // 6 slots × 3 channels
        for (fi, bp) in self.base_palettes.iter_mut().enumerate() {
            let base = fi * STRIDE;
            if base + STRIDE > vals.len() {
                break;
            }
            let g = |s: usize| [vals[base + s * 3], vals[base + s * 3 + 1], vals[base + s * 3 + 2]];
            bp.water_deep = g(0);
            bp.water_shallow = g(1);
            bp.low = g(2);
            bp.rock = g(3);
            bp.cap_warm = g(4);
            bp.cap_cold = g(5);
        }
    }

    /// Export the per-Region landform dial table flat: `[region][jag, relief, foothill, erosion]`.
    pub fn region_landform_export(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.biome_landform.len() * 4);
        for a in &self.biome_landform {
            out.extend_from_slice(a);
        }
        out
    }
    /// Restore the per-Region landform dial table (the UI dials + what `set_region_landform` stamps
    /// from); per-cell landform is restored separately via the trait fields (0–3).
    pub fn set_region_landform_table(&mut self, vals: &[f32]) {
        for (i, a) in self.biome_landform.iter_mut().enumerate() {
            let b = i * 4;
            if b + 4 > vals.len() {
                break;
            }
            *a = [vals[b], vals[b + 1], vals[b + 2], vals[b + 3]];
        }
    }

    /// Recompute the colour cache and flag every chunk (terrain + liquid) dirty — call once after a
    /// batch restore (load) so the render reflects the loaded fields.
    pub fn refresh_colors(&mut self) {
        if self.elevation_r.is_empty() {
            return;
        }
        self.color_cache = self.region_color();
        for d in self.chunk_dirty.iter_mut() {
            *d = true;
        }
        self.mark_all_liquid_changed();
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
    /// (before neighbour smoothing / jitter). The shared ramp ([`regions::ground_color`]) is
    /// what ties biomes together; per-biome tokens give identity. Neutral if unsized.
    fn cell_color(&self, r: usize) -> [f32; 3] {
        // Data views: recolour by a single field directly (heatmap), decoupled from Natural colour.
        match self.view_mode {
            VIEW_TEMPERATURE => {
                return regions::heat_ramp(self.temperature_r.get(r).copied().unwrap_or(0.5) as f32);
            }
            VIEW_MOISTURE => {
                return regions::wet_ramp(self.moisture_r.get(r).copied().unwrap_or(0.5) as f32);
            }
            VIEW_ELEVATION => {
                return regions::elevation_ramp(self.elevation_r.get(r).copied().unwrap_or(0.0) as f32);
            }
            VIEW_BIOME => {
                // The *emergent* ecological biome, composed from the trait primitives (Traits →
                // Biome) — distinct from VIEW_REGION, which shows the authored canon place.
                let e = self.elevation_r.get(r).copied().unwrap_or(0.0) as f32;
                let temp = self.temperature_r.get(r).copied().unwrap_or(0.5) as f32;
                let moist = self.moisture_r.get(r).copied().unwrap_or(0.5) as f32;
                let veg = self.vegetation_r.get(r).copied().unwrap_or(regions::veg::GRASS);
                return regions::emergent_biome_color(e, temp, moist, veg);
            }
            VIEW_REGION => {
                let id = self.region_r.get(r).copied().unwrap_or(0) as usize;
                // Unassigned reads neutral grey; assigned cells take the Region's accent.
                return if id == 0 { [0.32, 0.32, 0.34] } else { self.biome_color.get(id).copied().unwrap_or([0.5, 0.5, 0.5]) };
            }
            _ => {}
        }
        let fam = self.palette_family_r.get(r).copied().unwrap_or(0) as usize;
        let base = if fam < self.base_palettes.len() {
            self.base_palettes[fam]
        } else if !self.base_palettes.is_empty() {
            self.base_palettes[0]
        } else {
            regions::base_palettes()[0]
        };
        let veg = self.vegetation_r.get(r).copied().unwrap_or(regions::veg::GRASS);
        let e = self.elevation_r.get(r).copied().unwrap_or(0.0) as f32;
        let temp = self.temperature_r.get(r).copied().unwrap_or(0.5) as f32;
        let moist = self.moisture_r.get(r).copied().unwrap_or(0.5) as f32;
        regions::resolve_color(&base, veg, e, temp, moist)
    }

    /// Per-region RGB color (3 floats per region) resolved from each region's biome palette
    /// by elevation + moisture, softened across neighbours (so biome seams read as gradients)
    /// with a subtle deterministic brightness jitter to break up flatness.
    fn region_color(&self) -> Vec<f32> {
        let nr = self.elevation_r.len();
        // Each pass below is a pure index→value map (the smoothing reads the *previous* buffer, never
        // the one it writes) → parallelised bit-identically to the serial version. Held as `[f32;3]`
        // per cell, flattened once at the end.
        let mut cols: Vec<[f32; 3]> = crate::util::par_map(nr, |r| self.cell_color(r));

        // Data views show the raw field — no neighbour smoothing or jitter (heatmap stays exact).
        if self.view_mode == VIEW_NATURAL {
            // Light-touch Laplacian smoothing toward the neighbour mean (double-buffered per iter).
            if self.neighbors.len() == nr {
                for _ in 0..COLOR_SMOOTH_ITERS {
                    let next: Vec<[f32; 3]> = crate::util::par_map(nr, |r| {
                        let mut sum = [0.0f32; 3];
                        let mut cnt = 0.0f32;
                        for &nb in &self.neighbors[r] {
                            let nb = nb as usize;
                            sum[0] += cols[nb][0];
                            sum[1] += cols[nb][1];
                            sum[2] += cols[nb][2];
                            cnt += 1.0;
                        }
                        let mut out = cols[r];
                        if cnt > 0.0 {
                            for k in 0..3 {
                                let mean = sum[k] / cnt;
                                out[k] = cols[r][k] + (mean - cols[r][k]) * COLOR_SMOOTH_W;
                            }
                        }
                        out
                    });
                    cols = next;
                }
            }
            // Subtle per-region brightness variation (deterministic hash of the index).
            let jittered: Vec<[f32; 3]> = crate::util::par_map(nr, |r| {
                let h = hash_u32(r as u32);
                let f = 1.0 + ((h & 0xffff) as f32 / 65535.0 - 0.5) * 2.0 * COLOR_VAR; // [-VAR, VAR]
                let cr = cols[r];
                [(cr[0] * f).clamp(0.0, 1.0), (cr[1] * f).clamp(0.0, 1.0), (cr[2] * f).clamp(0.0, 1.0)]
            });
            cols = jittered;
        }

        let mut c = vec![0.0f32; nr * 3];
        for r in 0..nr {
            c[3 * r] = cols[r][0];
            c[3 * r + 1] = cols[r][1];
            c[3 * r + 2] = cols[r][2];
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
            if ri >= self.region_r.len() || self.biome_locked.get(ri).copied().unwrap_or(false) {
                continue;
            }
            let p = mesh.pos_of_r(ri);
            let dist = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt() / max_d;
            let moist = regions::moisture_at(p[0], p[1], width, height, seed);
            self.region_r[ri] = regions::classify(self.elevation_r[ri], moist, dist);
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
        // Fixed-size tiles: a chunk is CHUNK_SIZE_M on a side, so a 20 km world is ~39×39 small tiles
        // rather than a handful of huge ones. The grid covers the whole map (ceil); the last row/col
        // may be a partial tile when the world isn't an exact multiple. `sx`/`sy` ARE the tile size.
        let cols = (self.width / self.chunk_size_m).ceil().max(1.0) as usize;
        let rows = (self.height / self.chunk_size_m).ceil().max(1.0) as usize;
        let sx = self.chunk_size_m;
        let sy = self.chunk_size_m;
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
        self.liquid_chunk_dirty = vec![false; cols * rows];
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

    /// Set the rendering-chunk edge length (world metres), clamped to a sane minimum. Call **before**
    /// [`build`] — the chunk grid is partitioned during build. Smaller tiles ⇒ finer, lighter
    /// streaming (and more, but cheaper, tiles). The world size should be a whole multiple of this.
    pub fn set_chunk_size_m(&mut self, m: f64) {
        self.chunk_size_m = m.max(MIN_CHUNK_SIZE_M);
    }
    /// Current rendering-chunk edge length, in world metres.
    pub fn chunk_size_m(&self) -> f64 {
        self.chunk_size_m
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
        // Same fixed tile size the partitioner used (see `build_chunks`).
        let sx = self.chunk_size_m;
        let sy = self.chunk_size_m;
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
        Some(self.surface_from_tris(mesh, tris, exaggeration))
    }

    /// Terrain submesh of every triangle whose three cells are all in named Region `region_id`,
    /// compacted to a local vertex set (the per-Region level geometry — see the slicing spec). Empty
    /// if the Region has no fully-interior triangle. Boundary triangles between Regions are dropped.
    pub fn region_terrain_surface(&self, region_id: u8, exaggeration: f64) -> Option<geometry::Surface> {
        let mesh = self.mesh.as_ref()?;
        let want = |r: usize| self.region_r.get(r).copied() == Some(region_id);
        let mut tris: Vec<u32> = Vec::new();
        for t in 0..mesh.num_triangles() {
            let a = mesh.r_begin_s(3 * t);
            let b = mesh.r_begin_s(3 * t + 1);
            let c = mesh.r_begin_s(3 * t + 2);
            if want(a) && want(b) && want(c) {
                tris.push(t as u32);
            }
        }
        Some(self.surface_from_tris(mesh, &tris, exaggeration))
    }

    /// Compact terrain submesh from a set of triangle ids: vertices remapped to a local set, smooth
    /// normals from incident faces. Shared by the chunk + per-Region surface producers.
    fn surface_from_tris(&self, mesh: &crate::mesh::Mesh, tris: &[u32], exaggeration: f64) -> geometry::Surface {
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
        geometry::Surface {
            positions,
            normals,
            heights,
            colors,
            indices,
        }
    }

    /// A **coarse** regular-grid heightmesh over one chunk's footprint (terrain LOD): an `n×n` grid of
    /// `height_at` samples → `2n²` triangles, far fewer than the chunk's full TIN. Used for distant
    /// chunks so a larger area can stay rendered cheaply (near chunks keep [`chunk_surface`]). Positions
    /// in core space (the binding applies the Y-up remap, as for the full chunk). Seams with full-detail
    /// neighbours aren't stitched (v1) — fog + distance hide the small cracks.
    pub fn chunk_lod_surface(&self, chunk: usize, exaggeration: f64, n: usize) -> Option<geometry::Surface> {
        if self.mesh.is_none() || self.chunk_cols == 0 {
            return None;
        }
        let n = n.max(1);
        let cs = self.chunk_size_m;
        let gx = (chunk % self.chunk_cols) as f64;
        let gy = (chunk / self.chunk_cols) as f64;
        let x0 = gx * cs;
        let x1 = ((gx + 1.0) * cs).min(self.width);
        let y0 = gy * cs;
        let y1 = ((gy + 1.0) * cs).min(self.height);
        if x1 <= x0 || y1 <= y0 {
            return Some(geometry::Surface { positions: vec![], normals: vec![], heights: vec![], colors: vec![], indices: vec![] });
        }
        let w = n + 1;
        let mut positions = vec![0.0f32; w * w * 3];
        let mut heights = vec![0.0f32; w * w];
        let mut colors = vec![0.5f32; w * w * 3];
        for j in 0..w {
            for i in 0..w {
                let px = x0 + (x1 - x0) * (i as f64 / n as f64);
                let py = y0 + (y1 - y0) * (j as f64 / n as f64);
                let r = self.region_at(px, py);
                let e = r.map(|r| self.elevation_r[r]).unwrap_or(0.0);
                let idx = j * w + i;
                positions[3 * idx] = px as f32;
                positions[3 * idx + 1] = py as f32;
                positions[3 * idx + 2] = (e * exaggeration) as f32;
                heights[idx] = e as f32;
                if let Some(r) = r {
                    if 3 * r + 2 < self.color_cache.len() {
                        colors[3 * idx] = self.color_cache[3 * r];
                        colors[3 * idx + 1] = self.color_cache[3 * r + 1];
                        colors[3 * idx + 2] = self.color_cache[3 * r + 2];
                    }
                }
            }
        }
        let mut indices = Vec::with_capacity(n * n * 6);
        for j in 0..n {
            for i in 0..n {
                let a = (j * w + i) as u32;
                let b = a + 1;
                let c = a + w as u32;
                let dd = c + 1;
                indices.extend_from_slice(&[a, c, b, b, c, dd]);
            }
        }
        // Smooth normals, oriented upward (matches build_surface; the chunk material is double-sided).
        let mut accum = vec![0.0f64; w * w * 3];
        for tri in indices.chunks_exact(3) {
            let (ia, ib, ic) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let p = |idx: usize| [positions[3 * idx] as f64, positions[3 * idx + 1] as f64, positions[3 * idx + 2] as f64];
            let (a, b, c) = (p(ia), p(ib), p(ic));
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let (mut nx, mut ny, mut nz) = (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]);
            if nz < 0.0 {
                nx = -nx;
                ny = -ny;
                nz = -nz;
            }
            for &idx in &[ia, ib, ic] {
                accum[3 * idx] += nx;
                accum[3 * idx + 1] += ny;
                accum[3 * idx + 2] += nz;
            }
        }
        let mut normals = vec![0.0f32; w * w * 3];
        for idx in 0..w * w {
            let (nx, ny, nz) = (accum[3 * idx], accum[3 * idx + 1], accum[3 * idx + 2]);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            if len > 1e-12 {
                normals[3 * idx] = (nx / len) as f32;
                normals[3 * idx + 1] = (ny / len) as f32;
                normals[3 * idx + 2] = (nz / len) as f32;
            } else {
                normals[3 * idx + 2] = 1.0;
            }
        }
        Some(geometry::Surface { positions, normals, heights, colors, indices })
    }

    /// `[region_a, region_b, shared_cell_pairs]` for each unordered pair of named Regions whose cells
    /// touch (both non-zero). Drives the adjacency manifest the level slicer writes for later gates.
    pub fn region_adjacency(&self) -> Vec<[u32; 3]> {
        let n = self.region_r.len();
        if self.neighbors.len() != n {
            return Vec::new();
        }
        let mut counts: std::collections::BTreeMap<(u8, u8), u32> = std::collections::BTreeMap::new();
        for r in 0..n {
            let ra = self.region_r[r];
            if ra == 0 {
                continue;
            }
            for &nb in &self.neighbors[r] {
                let rb = self.region_r[nb as usize];
                if rb == 0 || rb == ra {
                    continue;
                }
                let key = if ra < rb { (ra, rb) } else { (rb, ra) };
                *counts.entry(key).or_insert(0) += 1;
            }
        }
        // Each adjacent cell-pair is counted from both sides → halve for the pair count.
        counts.into_iter().map(|((a, b), c)| [a as u32, b as u32, c / 2]).collect()
    }

    /// Number of cells assigned to named Region `region_id` (for the slicer's per-Region report).
    pub fn region_cell_count(&self, region_id: u8) -> usize {
        self.region_r.iter().filter(|&&r| r == region_id).count()
    }

    /// Liquid submesh for named Region `region_id` — the wet triangles whose three cells are all in
    /// the Region, compacted (reusing the smoothed whole-surface so it matches the editor water).
    pub fn region_liquid_surface(&self, region_id: u8, exaggeration: f64) -> Option<LiquidSurface> {
        let full = self.liquid_surface(exaggeration)?;
        let want = |r: usize| self.region_r.get(r).copied() == Some(region_id);
        let mut remap = vec![u32::MAX; full.positions.len() / 3];
        let mut positions: Vec<f32> = Vec::new();
        let mut normals: Vec<f32> = Vec::new();
        let mut types: Vec<f32> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for tri in full.indices.chunks_exact(3) {
            let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if !(want(a) && want(b) && want(c)) {
                continue;
            }
            for &g in &[a, b, c] {
                if remap[g] == u32::MAX {
                    remap[g] = (positions.len() / 3) as u32;
                    positions.extend_from_slice(&full.positions[3 * g..3 * g + 3]);
                    normals.extend_from_slice(&full.normals[3 * g..3 * g + 3]);
                    types.push(full.types[g]);
                }
                indices.push(remap[g]);
            }
        }
        Some(LiquidSurface { positions, normals, types, indices })
    }

    /// Decoration instances (proxy scatter) whose ground position falls in named Region `region_id`.
    pub fn region_scatter_instances(&self, region_id: u8, exaggeration: f64, density: f64, seed: u64) -> Vec<Instance> {
        self.scatter_instances(exaggeration, density, seed)
            .into_iter()
            .filter(|i| self.region_at(i.x as f64, i.y as f64).map(|c| self.region_of(c) == region_id).unwrap_or(false))
            .collect()
    }

    /// Rule-based scatter (the N3d model-slot library): deterministic placement from authored rules
    /// over the trait fields (vegetation / biome / region / elevation). `Instance::species` is the
    /// slot index.
    pub fn scatter_by_rules(&self, rules: &[scatter::ScatterRule], exaggeration: f64, seed: u64) -> Vec<Instance> {
        match &self.mesh {
            Some(mesh) => scatter::scatter_by_rules(
                seed,
                mesh,
                &self.elevation_r,
                &self.vegetation_r,
                &self.region_r,
                &self.region_r,
                rules,
                exaggeration,
            ),
            None => Vec::new(),
        }
    }

    /// Carve the authored volumetric features (N5) into a Surface-Nets mesh: `density = terrainY − y`
    /// (rock below the surface) minus the union of carve spheres, over the spheres' bounding box. Each
    /// cave is `[x, y, z, radius]` in **Godot space**; the result is a self-contained mesh (Godot
    /// coords). Empty without caves. The grid is capped (`MAX_DIM`) — a huge brush widens the cell
    /// rather than exploding the sample count.
    pub fn volumetric_mesh(&self, caves: &[[f64; 4]], exaggeration: f64, cell_size: f64) -> volumetric::VolumeMesh {
        let empty = volumetric::VolumeMesh { positions: Vec::new(), normals: Vec::new(), indices: Vec::new() };
        if caves.is_empty() || self.mesh.is_none() {
            return empty;
        }
        let mut mn = [f64::INFINITY; 3];
        let mut mx = [f64::NEG_INFINITY; 3];
        for c in caves {
            for a in 0..3 {
                mn[a] = mn[a].min(c[a] - c[3]);
                mx[a] = mx[a].max(c[a] + c[3]);
            }
        }
        let margin = cell_size.max(0.5) * 2.0;
        for a in 0..3 {
            mn[a] -= margin;
            mx[a] += margin;
        }
        const MAX_DIM: usize = 96;
        let mut cell = cell_size.max(0.5);
        let mut dims = [2usize; 3];
        loop {
            let mut ok = true;
            for a in 0..3 {
                dims[a] = (((mx[a] - mn[a]) / cell).ceil() as usize + 1).max(2);
                if dims[a] > MAX_DIM {
                    ok = false;
                }
            }
            if ok {
                break;
            }
            cell *= 1.5;
        }
        let caves_owned: Vec<[f64; 4]> = caves.to_vec();
        let density = |x: f64, y: f64, z: f64| -> f64 {
            let terr = match self.region_at(x, z) {
                Some(r) => self.elevation_r[r] * exaggeration,
                None => return f64::NEG_INFINITY, // off-map ⇒ air
            };
            let mut dens = terr - y; // > 0 below the surface (rock)
            for c in &caves_owned {
                let (dx, dy, dz) = (x - c[0], y - c[1], z - c[2]);
                let sd = (dx * dx + dy * dy + dz * dz).sqrt() - c[3]; // sphere SDF (< 0 inside)
                if sd < dens {
                    dens = sd; // carve: union of spheres becomes air
                }
            }
            dens
        };
        volumetric::surface_nets(mn, dims, cell, &density)
    }

    /// Rule-based scatter filtered to named Region `region_id` (for baking that Region's level).
    pub fn region_scatter_by_rules(&self, region_id: u8, rules: &[scatter::ScatterRule], exaggeration: f64, seed: u64) -> Vec<Instance> {
        self.scatter_by_rules(rules, exaggeration, seed)
            .into_iter()
            .filter(|i| self.region_at(i.x as f64, i.y as f64).map(|c| self.region_of(c) == region_id).unwrap_or(false))
            .collect()
    }

    // --- liquid rendering chunks (mirror the terrain chunk path) ---

    /// Mark the liquid render caches stale and flag the chunks of `regions` for re-tessellation.
    fn mark_liquid_changed(&mut self, regions: &[u32]) {
        self.liquid_cache_dirty = true;
        for &rid in regions {
            if let Some(chunks) = self.region_chunks.get(rid as usize) {
                for &c in chunks {
                    if let Some(d) = self.liquid_chunk_dirty.get_mut(c as usize) {
                        *d = true;
                    }
                }
            }
        }
        // Wake the edited cells + their neighbours so the next step_fluid re-settles the area (a
        // risen cell may have left its neighbours with a new gradient). Gather first, then activate,
        // to avoid aliasing `neighbors` with the active push.
        let mut wake: Vec<u32> = Vec::new();
        for &rid in regions {
            wake.push(rid);
            if let Some(nbs) = self.neighbors.get(rid as usize) {
                wake.extend_from_slice(nbs);
            }
        }
        for r in wake {
            let ru = r as usize;
            if ru < self.liquid_active_flag.len() && !self.liquid_active_flag[ru] {
                self.liquid_active_flag[ru] = true;
                self.liquid_active.push(r);
            }
        }
    }

    /// Mark the liquid caches stale and flag every liquid chunk (for global liquid ops).
    fn mark_all_liquid_changed(&mut self) {
        self.liquid_cache_dirty = true;
        for d in self.liquid_chunk_dirty.iter_mut() {
            *d = true;
        }
        // Active set = every currently-wet region (sea fill / rain / clear / streams touch the map
        // broadly, so re-seed from scratch rather than guess a footprint).
        self.liquid_active.clear();
        for f in self.liquid_active_flag.iter_mut() {
            *f = false;
        }
        for r in 0..self.field.depth.len() {
            if self.field.depth[r] > fluid::MIN_RENDER_DEPTH && r < self.liquid_active_flag.len() {
                self.liquid_active_flag[r] = true;
                self.liquid_active.push(r as u32);
            }
        }
    }

    /// Recompute the smoothed liquid-surface height + wet mask caches (lazy — only when stale). This
    /// is the render-side calm that used to live inside [`fluid::liquid_surface`], hoisted so chunked
    /// tessellation reads one shared cache instead of re-smoothing per chunk.
    fn ensure_liquid_cache(&mut self) {
        if !self.liquid_cache_dirty {
            return;
        }
        let nr = self.elevation_r.len();
        let mut surf = vec![0.0f64; nr];
        let mut wet = vec![false; nr];
        for r in 0..nr {
            surf[r] = self.elevation_r[r] + self.field.depth[r];
            wet[r] = self.field.depth[r] > fluid::MIN_RENDER_DEPTH;
        }
        if self.neighbors.len() == nr {
            for _ in 0..fluid::RENDER_SMOOTH_ITERS {
                let mut next = surf.clone();
                for r in 0..nr {
                    if !wet[r] {
                        continue;
                    }
                    let mut sum = 0.0;
                    let mut cnt = 0.0;
                    for &nb in &self.neighbors[r] {
                        let nb = nb as usize;
                        if wet[nb] {
                            sum += surf[nb];
                            cnt += 1.0;
                        }
                    }
                    if cnt > 0.0 {
                        let mean = sum / cnt;
                        next[r] = surf[r] + (mean - surf[r]) * fluid::RENDER_SMOOTH_W;
                    }
                }
                surf = next;
            }
        }
        for r in 0..nr {
            if surf[r] < self.elevation_r[r] {
                surf[r] = self.elevation_r[r];
            }
        }
        self.liquid_surf_cache = surf;
        self.liquid_wet_cache = wet;
        self.liquid_cache_dirty = false;
    }

    /// Pack one chunk's liquid sub-mesh from the (lazily recomputed) liquid caches: a local vertex
    /// array of the chunk's wet regions at their smoothed surface height, area-weighted normals from
    /// this chunk's wet triangles, and per-vertex liquid type. Returns an empty surface (not `None`)
    /// for a chunk with no wet triangles, so the front-end can clear a chunk that dried up. `None`
    /// only on a bad index / unbuilt mesh.
    pub fn liquid_chunk_surface(&mut self, chunk: usize, exaggeration: f64) -> Option<LiquidSurface> {
        self.ensure_liquid_cache();
        let mesh = self.mesh.as_ref()?;
        let tris = self.chunk_tris.get(chunk)?;
        let mut local_of: HashMap<u32, u32> = HashMap::new();
        let mut globals: Vec<u32> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for &t in tris {
            let a = mesh.r_begin_s(3 * t as usize) as u32;
            let b = mesh.r_begin_s(3 * t as usize + 1) as u32;
            let c = mesh.r_begin_s(3 * t as usize + 2) as u32;
            if !self.liquid_wet_cache[a as usize]
                || !self.liquid_wet_cache[b as usize]
                || !self.liquid_wet_cache[c as usize]
            {
                continue;
            }
            for &r in &[a, b, c] {
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
        let mut types = vec![0.0f32; n];
        for (li, &r) in globals.iter().enumerate() {
            let ru = r as usize;
            let p = mesh.pos_of_r(ru);
            positions[3 * li] = p[0] as f32;
            positions[3 * li + 1] = p[1] as f32;
            positions[3 * li + 2] = (self.liquid_surf_cache[ru] * exaggeration) as f32;
            types[li] = self.field.kind[ru] as f32;
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
        Some(LiquidSurface { positions, normals, types, indices })
    }

    /// Liquid chunk ids changed since the last call (cleared by this call); re-tessellate exactly
    /// these (intersected with what's in render range on the front-end).
    pub fn take_dirty_liquid_chunks(&mut self) -> Vec<u32> {
        let mut out = Vec::new();
        for (i, d) in self.liquid_chunk_dirty.iter_mut().enumerate() {
            if *d {
                *d = false;
                out.push(i as u32);
            }
        }
        out
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
pub(crate) fn diffuse_field(base: &[f64], neighbors: &[Vec<u32>], iters: usize) -> Vec<f64> {
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

/// Laplacian-smooth only the `subset` cells of `field` in place (Jacobi iteration: each pass reads
/// the field state from the start of that pass, so it's order-independent and deterministic).
/// Neighbours outside the subset act as fixed anchors — that's what pulls the brushed seam toward
/// its surroundings. Used by [`World::blend_brush`].
fn diffuse_subset(field: &mut [f64], neighbors: &[Vec<u32>], subset: &[u32], iters: usize) {
    if iters == 0 || subset.is_empty() {
        return;
    }
    let mut new_vals = vec![0.0f64; subset.len()];
    for _ in 0..iters {
        for (k, &rid) in subset.iter().enumerate() {
            let r = rid as usize;
            let nb = &neighbors[r];
            if nb.is_empty() {
                new_vals[k] = field[r];
                continue;
            }
            let mut sum = 0.0;
            for &j in nb {
                sum += field[j as usize];
            }
            let mean = sum / nb.len() as f64;
            new_vals[k] = field[r] + (mean - field[r]) * BLEND_W;
        }
        for (k, &rid) in subset.iter().enumerate() {
            field[rid as usize] = new_vals[k];
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
        let intersects = ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}
