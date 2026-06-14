// Temporary TS height field so the 3D shell renders before the Rust/WASM engine
// lands (Phase 2). Mirrors the core's value-noise + fbm in spirit (not byte-parity).
function hash2(x: number, y: number, seed: number): number {
  let h = (Math.imul(x | 0, 0x9e3779b1) ^ Math.imul(y | 0, 0x85ebca77) ^ seed) >>> 0;
  h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35) >>> 0;
  h ^= h >>> 16;
  return (h >>> 0) / 4294967296;
}

const smooth = (t: number) => t * t * (3 - 2 * t);

function value2(x: number, y: number, seed: number): number {
  const xi = Math.floor(x);
  const yi = Math.floor(y);
  const xf = x - xi;
  const yf = y - yi;
  const v00 = hash2(xi, yi, seed);
  const v10 = hash2(xi + 1, yi, seed);
  const v01 = hash2(xi, yi + 1, seed);
  const v11 = hash2(xi + 1, yi + 1, seed);
  const u = smooth(xf);
  const v = smooth(yf);
  const a = v00 + (v10 - v00) * u;
  const b = v01 + (v11 - v01) * u;
  return (a + (b - a) * v) * 2 - 1;
}

function fbm(x: number, y: number, seed: number, octaves = 6): number {
  let sum = 0;
  let amp = 0.5;
  let freq = 1;
  let norm = 0;
  for (let o = 0; o < octaves; o++) {
    sum += amp * value2(x * freq, y * freq, (seed + o * 1013) | 0);
    norm += amp;
    amp *= 0.5;
    freq *= 2;
  }
  return norm > 0 ? sum / norm : 0;
}

/** An island-ish `n`×`n` height field in roughly [-1, 1]. */
export function generateHeightfield(n: number, seed: number, octaves = 6): Float32Array {
  const h = new Float32Array(n * n);
  const DOMAIN = 4; // noise repeats across the world this many times
  for (let j = 0; j < n; j++) {
    for (let i = 0; i < n; i++) {
      const u = (i / (n - 1)) * DOMAIN;
      const v = (j / (n - 1)) * DOMAIN;
      let e = fbm(u, v, seed, octaves);
      // Radial falloff so the land sits in water with a coastline.
      const cx = i / (n - 1) - 0.5;
      const cy = j / (n - 1) - 0.5;
      const d = Math.sqrt(cx * cx + cy * cy) * 2;
      e -= d * d * 0.6;
      h[j * n + i] = e;
    }
  }
  return h;
}
