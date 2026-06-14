// 3D terrain-surface renderer. Draws an indexed mesh (position + normal + height)
// with procedural altitude shading and engine-level lighting. Our own WebGL2 +
// shaders, no deps. The mesh comes from the WASM engine (a TIN over the dual mesh);
// the placeholder grid path is kept as a fallback.
import type { Mat4 } from "../math/mat4";

const VERT = `#version 300 es
precision highp float;
layout(location=0) in vec3 a_pos;
layout(location=1) in vec3 a_normal;
layout(location=2) in float a_h;
uniform mat4 u_viewProj;
out vec3 v_normal;
out float v_h;
void main() {
  v_normal = a_normal;
  v_h = a_h;
  gl_Position = u_viewProj * vec4(a_pos, 1.0);
}`;

const FRAG = `#version 300 es
precision highp float;
in vec3 v_normal;
in float v_h;
out vec4 fragColor;

uniform vec3 u_lightDir;  // sun direction (engine-level lighting)
uniform float u_ambient;  // ambient floor 0..1

// Procedural altitude colormap (our palette), h in roughly [-1, 1].
// Colors are hardcoded for now; biomes own the palette from Phase 5.
vec3 ramp(float h) {
  vec3 deep    = vec3(0.06, 0.10, 0.20);
  vec3 shallow = vec3(0.12, 0.28, 0.34);
  vec3 sand    = vec3(0.52, 0.50, 0.34);
  vec3 grass   = vec3(0.27, 0.36, 0.21);
  vec3 rock    = vec3(0.40, 0.38, 0.34);
  vec3 snow    = vec3(0.88, 0.90, 0.92);
  if (h < -0.04) return mix(deep, shallow, clamp((h + 0.5) / 0.46, 0.0, 1.0));
  if (h <  0.02) return sand;
  if (h <  0.28) return mix(grass, rock, clamp((h - 0.02) / 0.26, 0.0, 1.0));
  if (h <  0.62) return mix(rock, snow, clamp((h - 0.28) / 0.34, 0.0, 1.0));
  return snow;
}

void main() {
  vec3 n = normalize(v_normal);
  float diff = clamp(dot(n, normalize(u_lightDir)), 0.0, 1.0);
  vec3 col = ramp(v_h) * (u_ambient + (1.0 - u_ambient) * diff);
  fragColor = vec4(col, 1.0);
}`;

export class TerrainRenderer {
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private uViewProj: WebGLUniformLocation | null;
  private uLightDir: WebGLUniformLocation | null;
  private uAmbient: WebGLUniformLocation | null;
  private vao: WebGLVertexArrayObject | null = null;
  private indexCount = 0;
  // Engine-level lighting; defaults match the original hardcoded look.
  private light = { dir: [0.404, 0.5, 0.766] as [number, number, number], ambient: 0.38 };

  constructor(gl: WebGL2RenderingContext) {
    this.gl = gl;
    this.prog = link(gl, VERT, FRAG);
    this.uViewProj = gl.getUniformLocation(this.prog, "u_viewProj");
    this.uLightDir = gl.getUniformLocation(this.prog, "u_lightDir");
    this.uAmbient = gl.getUniformLocation(this.prog, "u_ambient");
  }

  /** Engine-level lighting from azimuth/elevation degrees + ambient (0..1). */
  setLighting(azimuthDeg: number, elevationDeg: number, ambient: number): void {
    const az = (azimuthDeg * Math.PI) / 180;
    const el = (elevationDeg * Math.PI) / 180;
    this.light.dir = [Math.cos(el) * Math.cos(az), Math.cos(el) * Math.sin(az), Math.sin(el)];
    this.light.ambient = ambient;
  }

  /** Upload an engine surface (positions/normals scaled, heights normalized). */
  setMesh(positions: Float32Array, normals: Float32Array, heights: Float32Array, indices: Uint32Array): void {
    this.uploadMesh(positions, normals, heights, indices);
  }

  /** Build + upload a regular-grid surface from an `n`×`n` height field (fallback). */
  setHeightfield(field: Float32Array, n: number, worldSize: number, heightScale: number): void {
    const cell = worldSize / (n - 1);
    const positions = new Float32Array(n * n * 3);
    const normals = new Float32Array(n * n * 3);
    const heights = new Float32Array(n * n);
    const at = (i: number, j: number) => field[clampi(j, 0, n - 1) * n + clampi(i, 0, n - 1)];

    for (let j = 0; j < n; j++) {
      for (let i = 0; i < n; i++) {
        const o = j * n + i;
        const h = field[o];
        positions[o * 3] = i * cell;
        positions[o * 3 + 1] = j * cell;
        positions[o * 3 + 2] = h * heightScale;
        const hl = at(i - 1, j) * heightScale;
        const hr = at(i + 1, j) * heightScale;
        const hd = at(i, j - 1) * heightScale;
        const hu = at(i, j + 1) * heightScale;
        const nx = hl - hr;
        const ny = hd - hu;
        const nz = 2 * cell;
        const inv = 1 / Math.hypot(nx, ny, nz);
        normals[o * 3] = nx * inv;
        normals[o * 3 + 1] = ny * inv;
        normals[o * 3 + 2] = nz * inv;
        heights[o] = h;
      }
    }

    const idx = new Uint32Array((n - 1) * (n - 1) * 6);
    let p = 0;
    for (let j = 0; j < n - 1; j++) {
      for (let i = 0; i < n - 1; i++) {
        const a = j * n + i;
        const b = a + 1;
        const c = a + n;
        const d = c + 1;
        idx[p++] = a; idx[p++] = c; idx[p++] = b;
        idx[p++] = b; idx[p++] = c; idx[p++] = d;
      }
    }
    this.uploadMesh(positions, normals, heights, idx);
  }

  draw(viewProj: Mat4): void {
    const gl = this.gl;
    if (!this.vao || !this.indexCount) return;
    gl.useProgram(this.prog);
    gl.uniformMatrix4fv(this.uViewProj, false, viewProj);
    gl.uniform3fv(this.uLightDir, this.light.dir);
    gl.uniform1f(this.uAmbient, this.light.ambient);
    gl.bindVertexArray(this.vao);
    gl.drawElements(gl.TRIANGLES, this.indexCount, gl.UNSIGNED_INT, 0);
    gl.bindVertexArray(null);
  }

  private uploadMesh(
    positions: Float32Array,
    normals: Float32Array,
    heights: Float32Array,
    indices: Uint32Array,
  ): void {
    const gl = this.gl;
    this.vao = gl.createVertexArray();
    gl.bindVertexArray(this.vao);

    const attrib = (data: Float32Array, loc: number, size: number) => {
      const buf = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, buf);
      gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
    };
    attrib(positions, 0, 3);
    attrib(normals, 1, 3);
    attrib(heights, 2, 1);

    const ibo = gl.createBuffer();
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, indices, gl.STATIC_DRAW);

    gl.bindVertexArray(null);
    this.indexCount = indices.length;
  }
}

const clampi = (v: number, lo: number, hi: number) => (v < lo ? lo : v > hi ? hi : v);

function link(gl: WebGL2RenderingContext, vsSrc: string, fsSrc: string): WebGLProgram {
  const p = gl.createProgram();
  if (!p) throw new Error("createProgram failed");
  gl.attachShader(p, compile(gl, gl.VERTEX_SHADER, vsSrc));
  gl.attachShader(p, compile(gl, gl.FRAGMENT_SHADER, fsSrc));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    throw new Error("Program link failed: " + gl.getProgramInfoLog(p));
  }
  return p;
}

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const s = gl.createShader(type);
  if (!s) throw new Error("createShader failed");
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
    throw new Error("Shader compile failed: " + gl.getShaderInfoLog(s));
  }
  return s;
}
