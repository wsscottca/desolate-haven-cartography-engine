// Loads the Rust/WASM engine (wasm-pack output copied to build/_engine.js) and wraps
// the surface-generation API. Returns null when the engine isn't built, so the shell
// can fall back to the TS placeholder.

export interface SurfaceMesh {
  positions: Float32Array;
  normals: Float32Array;
  heights: Float32Array;
  indices: Uint32Array;
  regionCount: number;
  triangleCount: number;
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
  heights(): Float32Array;
  indices(): Uint32Array;
  region_count(): number;
  triangle_count(): number;
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
      heights: this.handle.heights(),
      indices: this.handle.indices(),
      regionCount: this.handle.region_count(),
      triangleCount: this.handle.triangle_count(),
    };
  }
}
