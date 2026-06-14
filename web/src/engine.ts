// Loads the Rust/WASM engine (wasm-pack output copied to build/_engine.js) and wraps
// the surface-generation API. Returns null when the engine isn't built, so the shell
// can fall back to the TS placeholder.

export interface SurfaceMesh {
  positions: Float32Array;
  normals: Float32Array;
  colors: Float32Array;
  indices: Uint32Array;
  regionCount: number;
  triangleCount: number;
}

export interface LiquidMesh {
  positions: Float32Array;
  normals: Float32Array;
  types: Float32Array;
  indices: Uint32Array;
}

interface WasmModule {
  default: (input?: unknown) => Promise<unknown>;
  version: () => string;
  WasmEngine: new () => WasmEngineHandle;
}

interface WasmEngineHandle {
  build(width: number, height: number, spacing: number, seed: number, octaves: number): void;
  tessellate(exaggeration: number): void;
  positions(): Float32Array;
  normals(): Float32Array;
  colors(): Float32Array;
  indices(): Uint32Array;
  region_count(): number;
  triangle_count(): number;
  // liquid sim
  set_sea_level(level: number): void;
  rain(amount: number): void;
  step_fluid(flowRate: number, evaporation: number, substeps: number): void;
  clear_liquid(): void;
  tessellate_liquid(exaggeration: number): void;
  liquid_positions(): Float32Array;
  liquid_normals(): Float32Array;
  liquid_types(): Float32Array;
  liquid_indices(): Uint32Array;
  // brush tools
  paint_terrain(cx: number, cy: number, radius: number, strength: number, mode: number): void;
  paint_liquid(cx: number, cy: number, radius: number, amount: number, kind: number): void;
  // biomes + save/load
  paint_biome(cx: number, cy: number, radius: number, biomeId: number): void;
  set_biome_color(id: number, r: number, g: number, b: number): void;
  biome_color_of(id: number): Float32Array;
  elevation_export(): Float32Array;
  biome_export(): Uint8Array;
  liquid_depth_export(): Float32Array;
  liquid_kind_export(): Uint8Array;
  set_elevation(e: Float32Array): void;
  set_biome(b: Uint8Array): void;
  set_liquid(depth: Float32Array, kind: Uint8Array): void;
}

export class Engine {
  readonly version: string;
  private readonly handle: WasmEngineHandle;

  private constructor(version: string, handle: WasmEngineHandle) {
    this.version = version;
    this.handle = handle;
  }

  /** Load the WASM engine, or null if it hasn't been built yet (run wasm-pack). */
  static async load(): Promise<Engine | null> {
    try {
      // Resolve relative to the bundle in build/, where _engine.js + the .wasm live.
      // Using a runtime URL keeps esbuild from trying to bundle the wasm glue.
      const url = new URL("./_engine.js", import.meta.url).href;
      const mod = (await import(url)) as unknown as WasmModule;
      await mod.default();
      return new Engine(mod.version(), new mod.WasmEngine());
    } catch {
      return null;
    }
  }

  /** Build the mesh + elevation (on seed / detail / octaves change). */
  build(width: number, height: number, spacing: number, seed: number, octaves: number): void {
    this.handle.build(width, height, spacing, seed, octaves);
  }

  /** Tessellate at a vertical exaggeration and return the flat surface arrays. */
  surface(exaggeration: number): SurfaceMesh {
    this.handle.tessellate(exaggeration);
    return {
      positions: this.handle.positions(),
      normals: this.handle.normals(),
      colors: this.handle.colors(),
      indices: this.handle.indices(),
      regionCount: this.handle.region_count(),
      triangleCount: this.handle.triangle_count(),
    };
  }

  // --- liquid simulation ---
  /** Fill below `level` with water (instant sea + lakes). */
  setSeaLevel(level: number): void {
    this.handle.set_sea_level(level);
  }
  /** Add uniform rainfall over land above the current sea level. */
  rain(amount: number): void {
    this.handle.rain(amount);
  }
  /** Advance the hydraulic solver `substeps` relaxation steps. */
  stepFluid(flowRate: number, evaporation: number, substeps: number): void {
    this.handle.step_fluid(flowRate, evaporation, substeps);
  }
  /** Remove all liquid. */
  clearLiquid(): void {
    this.handle.clear_liquid();
  }
  /** Tessellate the liquid surface at a vertical exaggeration. */
  liquidSurface(exaggeration: number): LiquidMesh {
    this.handle.tessellate_liquid(exaggeration);
    return {
      positions: this.handle.liquid_positions(),
      normals: this.handle.liquid_normals(),
      types: this.handle.liquid_types(),
      indices: this.handle.liquid_indices(),
    };
  }

  // --- brush tools ---
  /** Sculpt terrain: mode 0 raise, 1 carve, 2 level, 3 crest. */
  paintTerrain(cx: number, cy: number, radius: number, strength: number, mode: number): void {
    this.handle.paint_terrain(cx, cy, radius, strength, mode);
  }
  /** Place liquid `kind` (0 water, 1 lava) under the brush. */
  paintLiquid(cx: number, cy: number, radius: number, amount: number, kind: number): void {
    this.handle.paint_liquid(cx, cy, radius, amount, kind);
  }

  // --- biomes ---
  paintBiome(cx: number, cy: number, radius: number, biomeId: number): void {
    this.handle.paint_biome(cx, cy, radius, biomeId);
  }
  setBiomeColor(id: number, r: number, g: number, b: number): void {
    this.handle.set_biome_color(id, r, g, b);
  }
  biomeColor(id: number): [number, number, number] {
    const c = this.handle.biome_color_of(id);
    return [c[0], c[1], c[2]];
  }

  // --- save / load (authored-state arrays) ---
  exportElevation(): Float32Array {
    return this.handle.elevation_export();
  }
  exportBiome(): Uint8Array {
    return this.handle.biome_export();
  }
  exportLiquidDepth(): Float32Array {
    return this.handle.liquid_depth_export();
  }
  exportLiquidKind(): Uint8Array {
    return this.handle.liquid_kind_export();
  }
  importElevation(e: Float32Array): void {
    this.handle.set_elevation(e);
  }
  importBiome(b: Uint8Array): void {
    this.handle.set_biome(b);
  }
  importLiquid(depth: Float32Array, kind: Uint8Array): void {
    this.handle.set_liquid(depth, kind);
  }
}
