// Liquid-surface renderer. Draws the wet triangles as translucent water / emissive
// lava, procedurally (no textures), blended over the terrain. Shares the engine-level
// lighting with the terrain pass. Per-vertex `a_type` selects the liquid appearance.
import type { Mat4 } from "../math/mat4";

const VERT = `#version 300 es
precision highp float;
layout(location=0) in vec3 a_pos;
layout(location=1) in vec3 a_normal;
layout(location=2) in float a_type;
uniform mat4 u_viewProj;
out vec3 v_normal;
out float v_type;
out vec3 v_world;
void main() {
  v_normal = a_normal;
  v_type = a_type;
  v_world = a_pos;
  gl_Position = u_viewProj * vec4(a_pos, 1.0);
}`;

const FRAG = `#version 300 es
precision highp float;
in vec3 v_normal;
in float v_type;
in vec3 v_world;
out vec4 fragColor;

uniform vec3 u_lightDir;
uniform float u_ambient;
uniform vec3 u_camPos;
uniform float u_fog;
uniform vec3 u_fogColor;

void main() {
  vec3 n = normalize(v_normal);
  float diff = clamp(dot(n, normalize(u_lightDir)), 0.0, 1.0);
  bool isLava = v_type > 0.5;
  vec3 col;
  float alpha;
  if (isLava) {
    // Emissive: glows regardless of shadow, faint flow shading.
    col = vec3(0.95, 0.30, 0.08) * (0.85 + 0.15 * diff);
    alpha = 0.96;
  } else {
    // Translucent water: lit, with a soft sky tint on up-facing surfaces.
    vec3 water = vec3(0.10, 0.28, 0.42);
    col = water * (u_ambient + (1.0 - u_ambient) * diff);
    col += vec3(0.04, 0.06, 0.09) * pow(clamp(n.z, 0.0, 1.0), 2.0);
    alpha = 0.62;
  }
  float d = distance(v_world, u_camPos);
  col = mix(col, u_fogColor, clamp(1.0 - exp(-d * u_fog), 0.0, 1.0));
  fragColor = vec4(col, alpha);
}`;

export class LiquidRenderer {
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

  setMesh(positions: Float32Array, normals: Float32Array, types: Float32Array, indices: Uint32Array): void {
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
    attrib(types, 2, 1);
    const ibo = gl.createBuffer();
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, indices, gl.STATIC_DRAW);
    gl.bindVertexArray(null);
    this.indexCount = indices.length;
  }

  /** Draw blended over the terrain. Depth test on; depth writes off (translucent). */
  draw(viewProj: Mat4, camPos: [number, number, number]): void {
    const gl = this.gl;
    if (!this.vao || !this.indexCount) return;
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.depthMask(false);

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

    gl.depthMask(true);
    gl.disable(gl.BLEND);
  }
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
