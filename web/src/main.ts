// Desolate Haven Cartography Engine — web shell entry point.
// Phase 0: orbitable 3D terrain from a TS placeholder field. The Rust/WASM engine
// (dhce-core) takes over the height field from Phase 2.
import { OrbitCamera } from "./camera";
import { TerrainRenderer } from "./render/terrain";
import { generateHeightfield } from "./placeholder";
import { WORLD } from "./config";
import { buildSettingsPanel, DEFAULTS, type Settings } from "./settings";

const canvas = document.getElementById("view") as HTMLCanvasElement;
const gl = canvas.getContext("webgl2", { antialias: true });
if (!gl) {
  const msg = document.createElement("p");
  msg.textContent = "WebGL2 is required and is not available in this browser.";
  msg.style.cssText = "color:#ECE3D0;font:16px serif;padding:2rem";
  document.body.replaceChildren(msg);
  throw new Error("WebGL2 unavailable");
}

const terrain = new TerrainRenderer(gl);
const settings: Settings = structuredClone(DEFAULTS);

const camera = new OrbitCamera([WORLD.size / 2, WORLD.size / 2, 0], WORLD.size * 1.15);

let needsDraw = true;
const requestDraw = () => {
  needsDraw = true;
};
camera.attach(canvas, requestDraw);
window.addEventListener("resize", requestDraw);

// Engine settings regenerate the world live; physics settings are stored on
// `settings.physics` for the liquid sim (consumed from the fluid phase onward).
function regenerate(): void {
  const n = settings.engine.detail;
  const heights = generateHeightfield(n, settings.engine.seed, settings.engine.octaves);
  terrain.setHeightfield(heights, n, WORLD.size, settings.engine.exaggeration);
  requestDraw();
}
const settingsRoot = document.getElementById("settings-body");
if (settingsRoot) {
  buildSettingsPanel(settingsRoot, settings, (group) => {
    if (group === "engine") regenerate();
  });
}
regenerate();

gl.enable(gl.DEPTH_TEST);

function resize(): void {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = Math.floor(canvas.clientWidth * dpr);
  const h = Math.floor(canvas.clientHeight * dpr);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
    needsDraw = true;
  }
}

function frame(): void {
  resize();
  if (needsDraw) {
    needsDraw = false;
    gl!.viewport(0, 0, canvas.width, canvas.height);
    gl!.clearColor(0.078, 0.067, 0.055, 1); // --bg
    gl!.clear(gl!.COLOR_BUFFER_BIT | gl!.DEPTH_BUFFER_BIT);
    const aspect = canvas.width / Math.max(1, canvas.height);
    terrain.draw(camera.viewProj(aspect));
  }
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

// Tool dock — Phase 0 shows selection state only; wiring lands in Phase 4.
for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-tool]"))) {
  btn.addEventListener("click", () => {
    for (const b of Array.from(document.querySelectorAll("[data-tool]"))) b.classList.remove("active");
    btn.classList.add("active");
  });
}

// Engine status: the WASM engine wires in from Phase 1/2; for now the shell runs on
// the TS placeholder field.
const status = document.getElementById("engine-status");
if (status) status.textContent = "engine: TS placeholder (build the Rust core for dhce-core)";
