// Perspective orbit camera: drag to orbit, wheel to zoom, shift-drag (or middle) to pan.
// World up is +Z (z = elevation), matching the terrain mesh.
import type { Mat4 } from "./math/mat4";
import { perspective, lookAt, multiply } from "./math/mat4";
import type { Vec3 } from "./math/vec3";
import { add, scale, cross, normalize, sub } from "./math/vec3";

export class OrbitCamera {
  target: Vec3;
  distance: number;
  yaw = Math.PI * 0.25; // around world up (+Z)
  pitch = Math.PI * 0.32; // elevation above the ground plane
  fov = (50 * Math.PI) / 180;
  near = 1;
  far = 100000;

  constructor(target: Vec3, distance: number) {
    this.target = target;
    this.distance = distance;
  }

  eye(): Vec3 {
    const cp = Math.cos(this.pitch);
    const dir: Vec3 = [cp * Math.cos(this.yaw), cp * Math.sin(this.yaw), Math.sin(this.pitch)];
    return add(this.target, scale(dir, this.distance));
  }

  viewProj(aspect: number): Mat4 {
    const view = lookAt(this.eye(), this.target, [0, 0, 1]);
    const proj = perspective(this.fov, aspect, this.near, this.far);
    return multiply(proj, view);
  }

  /** Wire pointer + wheel interaction; calls `onChange` when the view moves. */
  attach(canvas: HTMLCanvasElement, onChange: () => void): void {
    let mode: "orbit" | "pan" | null = null;
    let lastX = 0;
    let lastY = 0;

    canvas.addEventListener("pointerdown", (e) => {
      mode = e.button === 1 || e.shiftKey ? "pan" : "orbit";
      lastX = e.clientX;
      lastY = e.clientY;
      canvas.setPointerCapture(e.pointerId);
    });
    const end = (e: PointerEvent) => {
      mode = null;
      try {
        canvas.releasePointerCapture(e.pointerId);
      } catch {
        /* capture may already be released */
      }
    };
    canvas.addEventListener("pointerup", end);
    canvas.addEventListener("pointercancel", end);

    canvas.addEventListener("pointermove", (e) => {
      if (!mode) return;
      const dx = e.clientX - lastX;
      const dy = e.clientY - lastY;
      lastX = e.clientX;
      lastY = e.clientY;
      if (mode === "orbit") {
        this.yaw -= dx * 0.005;
        this.pitch = clamp(this.pitch + dy * 0.005, 0.05, Math.PI / 2 - 0.02);
      } else {
        const fwd = normalize(sub(this.target, this.eye()));
        const right = normalize(cross(fwd, [0, 0, 1]));
        const up = cross(right, fwd);
        const k = this.distance * 0.0015;
        this.target = add(this.target, add(scale(right, -dx * k), scale(up, dy * k)));
      }
      onChange();
    });

    canvas.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        this.distance = clamp(this.distance * Math.exp(e.deltaY * 0.001), this.near * 2, this.far * 0.5);
        onChange();
      },
      { passive: false },
    );
  }
}

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
