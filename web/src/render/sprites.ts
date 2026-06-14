// Decoration sprite renderer: instanced, upright (cylindrical) billboards whose
// shape is synthesized procedurally in the fragment shader (tree / rock) — no
// textures. Instances come from the engine's deterministic scatter. Includes a
// simple distance fog shared with the terrain/liquid passes.
import type { Mat4 } from "../math/mat4";

// Unit quad in UV space (x centered at draw time, y from 0=base to 1=top).
const QUAD = new Float32Array([0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1]);

const VERT = `#version 300 es
precision highp float;
layout(location=0) in vec2 a_uv;
layout(location=1) in vec4 a_inst;     // x, y, z, scale
layout(location=2) in float a_species; // 0 tree, 1 rock
uniform mat4 u_viewProj;
uniform vec3 u_camRight;
out vec2 v_uv;
out float v_species;
out vec3 v_world;
void main() {
  vec3 base = a_inst.xyz;
  float s = a_inst.w;
  vec3 up = vec3(0.0, 0.0, 1.0);
  vec3 world = base + (a_uv.x - 0.5) * 2.0 * s * u_camRight + a_uv.y * (s * 1.8) * up;
  v_uv = a_uv;
  v_species = a_species;
  v_world = world;
  gl_Position = u_viewProj * vec4(world, 1.0);
}`;

const FRAG = `#version 300 es
precision highp float;
in vec2 v_uv;
in float v_species;
in vec3 v_world;
out vec4 fragColor;
uniform vec3 u_camPos;
uniform float u_fog;
uniform vec3 u_fogColor;
void main() {
  vec2 p = v_uv;
  vec3 col;
  if (v_species < 0.5) {
    // tree: thin trunk + tapering canopy
    bool trunk = abs(p.x - 0.5) < 0.07 && p.y < 0.4;
    float canopyW = 0.5 * (1.0 - clamp((p.y - 0.3) / 0.7, 0.0, 1.0));
    bool canopy = p.y > 0.3 && abs(p.x - 0.5) < canopyW;
    if (!trunk && !canopy) discard;
    col = trunk ? vec3(0.30, 0.21, 0.12) : vec3(0.17, 0.33, 0.15);
  } else {
    // rock: rounded lump sitting on the ground
    float r = length((p - vec2(0.5, 0.30)) / vec2(0.40, 0.32));
    if (r > 1.0 || p.y < 0.02) discard;
    col = vec3(0.42, 0.40, 0.38) * (1.0 - 0.3 * r);
  }
  col *= mix(0.65, 1.1, p.y); // fake top-light
  float d = distance(v_world, u_camPos);
  col = mix(col, u_fogColor, clamp(1.0 - exp(-d * u_fog), 0.0, 1.0));
  fragColor = vec4(col, 1.0);
}`;

export class SpriteRenderer {
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private uViewProj: WebGLUniformLocation | null;
  private uCamRight: WebGLUniformLocation | null;
  private uCamPos: WebGLUniformLocation | null;
  private uFog: WebGLUniformLocation | null;
  private uFogColor: WebGLUniformLocation | null;
  private vao: WebGLVertexArrayObject;
  private instVbo: WebGLBuffer;
  private count = 0;
  private fog = { density: 0, color: [0.078, 0.067, 0.055] as [number, number, number] };

  constructor(gl: WebGL2RenderingContext) {
    this.gl = gl;
    this.prog = link(gl, VERT, FRAG);
    this.uViewProj = gl.getUniformLocation(this.prog, "u_viewProj");
    this.uCamRight = gl.getUniformLocation(this.prog, "u_camRight");
    this.uCamPos = gl.getUniformLocation(this.prog, "u_camPos");
    this.uFog = gl.getUniformLocation(this.prog, "u_fog");
    this.uFogColor = gl.getUniformLocation(this.prog, "u_fogColor");

    this.vao = gl.createVertexArray()!;
    gl.bindVertexArray(this.vao);
    const quad = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, quad);
    gl.bufferData(gl.ARRAY_BUFFER, QUAD, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

    this.instVbo = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instVbo);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, 20, 0); // x,y,z,scale
    gl.vertexAttribDivisor(1, 1);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 1, gl.FLOAT, false, 20, 16); // species
    gl.vertexAttribDivisor(2, 1);
    gl.bindVertexArray(null);
  }

  setFog(density: number, color: [number, number, number]): void {
    this.fog.density = density;
    this.fog.color = color;
  }

  /** `data` is 5 floats per instance: x, y, z, scale, species. */
  setInstances(data: Float32Array, count: number): void {
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instVbo);
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.DYNAMIC_DRAW);
    this.count = count;
  }

  draw(viewProj: Mat4, camRight: [number, number, number], camPos: [number, number, number]): void {
    const gl = this.gl;
    if (!this.count) return;
    gl.useProgram(this.prog);
    gl.uniformMatrix4fv(this.uViewProj, false, viewProj);
    gl.uniform3fv(this.uCamRight, camRight);
    gl.uniform3fv(this.uCamPos, camPos);
    gl.uniform1f(this.uFog, this.fog.density);
    gl.uniform3fv(this.uFogColor, this.fog.color);
    gl.bindVertexArray(this.vao);
    gl.drawArraysInstanced(gl.TRIANGLES, 0, 6, this.count);
    gl.bindVertexArray(null);
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
