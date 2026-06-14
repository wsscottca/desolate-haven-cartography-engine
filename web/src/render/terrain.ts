// 3D terrain-surface renderer. Draws an indexed mesh (position + normal + per-vertex
// color) with engine-level lighting. Color is the region's biome color (from the WASM
// engine); the placeholder grid path computes an altitude ramp in JS. Our own WebGL2.
import type { Mat4 } from "../math/mat4";

const VERT = `#version 300 es
precision highp float;
layout(location=0) in vec3 a_pos;
layout(location=1) in vec3 a_normal;
layout(location=2) in vec3 a_color;
uniform mat4 u_viewProj;
out vec3 v_normal;
out vec3 v_color;
out vec3 v_world;
void main() {
  v_normal = a_normal;
  v_color = a_color;
  v_world = a_pos;
  gl_Position = u_viewProj * vec4(a_pos, 1.0);
}`;

const FRAG = `#version 300 es
precision highp float;
in vec3 v_normal;
in vec3 v_color;
in vec3 v_world;
out vec4 fragColor;

uniform vec3 u_lightDir;  // sun direction (engine-level lighting)
uniform float u_ambient;  // ambient floor 0..1
uniform vec3 u_camPos;
uniform float u_fog;
uniform vec3 u_fogColor;

void main() {
  vec3 n = normalize(v_normal);
  float diff = clamp(dot(n, normalize(u_lightDir)), 0.0, 1.0);
  vec3 col = v_color * (u_ambient + (1.0 - u_ambient) * diff);
  float d = distance(v_world, u_camPos);
  col = mix(col, u_fogColor, clamp(1.0 - exp(-d * u_fog), 0.0, 1.0));
  fragColor = vec4(col, 1.0);
}`;

export class TerrainRenderer {
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private uViewProj: WebGLUniformLocation | null;
  private uLightDir: WebGLUniformLocation | null;
  private uAmbient: WebGLUniformLocation | null;
  private uCamPos: WebGLUniformLocation | null;
  private uFog: WebGLUniformLocation | null;
  private uFogColor: WebGLUniformLocation | null;
  private vao: WebGLVertexArrayObject | null = null;
  private indexCount = 0;
  private light = { dir: [0.404, 0.5, 0.766] as [number, number, number], ambient: 0.38 };
  private fog = { density: 0, color: [0.078, 0.067, 0.055] as [number, number, number] };

  constructor(gl: WebGL2RenderingContext) {
    this.gl = gl;
    this.prog = link(gl, VERT, FRAG);
    this.uViewProj = gl.getUniformLocation(this.prog, "u_viewProj");
    this.uLightDir = gl.getUniformLocation(this.prog, "u_lightDir");
    this.uAmbient = gl.getUniformLocation(this.prog, "u_ambient");
    this.uCamPos = gl.getUniformLocation(this.prog, "u_camPos");
    this.uFog = gl.getUniformLocation(this.prog, "u_fog");
    this.uFogColor = gl.getUniformLocation(this.prog, "u_fogColor");
  }

  setFog(density: number, color: [number, number, number]): void {
    this.fog.density = density;
    this.fog.color = color;
  }

  setLighting(azimuthDeg: number, elevationDeg: number, ambient: number): void {
    const az = (azimuthDeg * Math.PI) / 180;
    const el = (elevationDeg * Math.PI) / 180;
    this.light.dir = [Math.cos(el) * Math.cos(az), Math.cos(el) * Math.sin(az), Math.sin(el)];
    this.light.ambient = ambient;
  }

  /** Upload an engine surface (positions/normals scaled, per-vertex RGB color). */
  setMesh(positions: Float32Array, normals: Float32Array, colors: Float32Array, indices: Uint32Array): void {
    this.uploadMesh(positions, normals, colors, indices);
  }

  /** Build + upload a regular-grid surface from an `n`×`n` height field (fallback). */
  setHeightfield(field: Float32Array, n: number, worldSize: number, heightScale: number): void {
    const cell = worldSize / (n - 1);
    const positions = new Float32Array(n * n * 3);
    const normals = new Float32Array(n * n * 3);
    const colors = new Float32Array(n * n * 3);
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
        const c = rampColor(h);
        colors[o * 3] = c[0];
        colors[o * 3 + 1] = c[1];
        colors[o * 3 + 2] = c[2];
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
    this.uploadMesh(positions, normals, colors, idx);
  }

  draw(viewProj: Mat4, camPos: [number, number, number]): void {
    const gl = this.gl;
    if (!this.vao || !this.indexCount) return;
    gl.useProgram(this.prog);
    gl.uniformMatrix4fv(this.uViewProj, false, viewProj);
    gl.uniform3fv(this.uLightDir, this.light.dir);
    gl.uniform1f(this.uAmbient, this.light.ambient);
    gl.uniform3fv(this.uCamPos, camPos);
    gl.uniform1f(this.uFog, this.fog.density);
    gl.uniform3fv(this.uFogColor, this.fog.color);
    gl.bindVertexArray(this.vao);
    gl.drawElements(gl.TRIANGLES, this.indexCount, gl.UNSIGNED_INT, 0);
    gl.bindVertexArray(null);
  }

  private uploadMesh(
    positions: Float32Array,
    normals: Float32Array,
    colors: Float32Array,
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
    attrib(colors, 2, 3);
    const ibo = gl.createBuffer();
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, indices, gl.STATIC_DRAW);
    gl.bindVertexArray(null);
    this.indexCount = indices.length;
  }
}

const clampi = (v: number, lo: number, hi: number) => (v < lo ? lo : v > hi ? hi : v);

// Altitude colormap for the placeholder fallback (h ~ [-1, 1]).
function rampColor(h: number): [number, number, number] {
  const cl = (x: number) => Math.max(0, Math.min(1, x));
  const mix = (a: number[], b: number[], t: number): [number, number, number] => [
    a[0] + (b[0] - a[0]) * t,
    a[1] + (b[1] - a[1]) * t,
    a[2] + (b[2] - a[2]) * t,
  ];
  const deep = [0.06, 0.1, 0.2];
  const shallow = [0.12, 0.28, 0.34];
  const sand = [0.52, 0.5, 0.34];
  const grass = [0.27, 0.36, 0.21];
  const rock = [0.4, 0.38, 0.34];
  const snow = [0.88, 0.9, 0.92];
  if (h < -0.04) return mix(deep, shallow, cl((h + 0.5) / 0.46));
  if (h < 0.02) return [sand[0], sand[1], sand[2]];
  if (h < 0.28) return mix(grass, rock, cl((h - 0.02) / 0.26));
  if (h < 0.62) return mix(rock, snow, cl((h - 0.28) / 0.34));
  return [snow[0], snow[1], snow[2]];
}

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
