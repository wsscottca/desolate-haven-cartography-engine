// Desolate Haven Cartography Engine — web shell entry point.
// Phase 0: orbitable 3D terrain from a TS placeholder field. The Rust/WASM engine
// (dhce-core) takes over the height field from Phase 2.
import { OrbitCamera } from "./camera";
import { TerrainRenderer } from "./render/terrain";
import { generateHeightfield } from "./placeholder";
import { WORLD } from "./config";
import { buildSettingsPanel, DEFAULTS, type Settings } from "./settings";
import { Engine } from "./engine";

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
let engine: Engine | null = null;

const setStatus = (text: string): void => {
  const el = document.getElementById("engine-status");
  if (el) el.textContent = text;
};

const camera = new OrbitCamera([WORLD.size / 2, WORLD.size / 2, 0], WORLD.size * 1.15);

let needsDraw = true;
const requestDraw = () => {
  needsDraw = true;
};
camera.attach(canvas, requestDraw);
window.addEventListener("resize", requestDraw);

// `Detail` maps to mesh spacing — roughly the number of cells across the world.
const detailToSpacing = (detail: number): number => WORLD.size / detail;

function pushSurface(): void {
  if (!engine) return;
  const s = engine.surface(settings.engine.exaggeration);
  terrain.setMesh(s.positions, s.normals, s.heights, s.indices);
  setStatus(`engine: dhce-core ${engine.version} · ${s.regionCount.toLocaleString()} cells`);
}

// Full rebuild: mesh + elevation (seed / detail / octaves), then tessellate.
function regenerate(): void {
  if (engine) {
    engine.build(
      WORLD.size,
      WORLD.size,
      detailToSpacing(settings.engine.detail),
      settings.engine.seed,
      settings.engine.octaves,
    );
    pushSurface();
  } else {
    const n = settings.engine.detail;
    const heights = generateHeightfield(n, settings.engine.seed, settings.engine.octaves);
    terrain.setHeightfield(heights, n, WORLD.size, settings.engine.exaggeration);
  }
  requestDraw();
}

// Vertical scale only — re-tessellate without rebuilding the mesh (cheap).
function retessellate(): void {
  if (engine) {
    pushSurface();
    requestDraw();
  } else {
    regenerate();
  }
}
function applyLighting(): void {
  terrain.setLighting(settings.render.lightAzimuth, settings.render.lightElevation, settings.render.ambient);
  requestDraw();
}

const settingsRoot = document.getElementById("settings-body");
if (settingsRoot) {
  buildSettingsPanel(settingsRoot, settings, (group, key) => {
    if (group === "engine") {
      if (key === "exaggeration") retessellate();
      else regenerate();
    } else if (group === "render") {
      applyLighting();
    }
    // physics: stored on settings.physics for the liquid sim (Phase 3+)
  });
}

applyLighting();
regenerate(); // immediate render on the TS placeholder

// Upgrade to the Rust/WASM engine once it loads (falls back to the placeholder).
Engine.load().then((loaded) => {
  if (loaded) {
    engine = loaded;
    regenerate();
  } else {
    setStatus("engine: TS placeholder (run wasm-pack to build dhce-core)");
  }
});

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

// (engine status is managed by setStatus() / the Engine.load() handler above.)
