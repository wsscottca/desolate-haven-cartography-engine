// Desolate Haven Cartography Engine — web shell entry point.
// Phase 0: orbitable 3D terrain from a TS placeholder field. The Rust/WASM engine
// (dhce-core) takes over the height field from Phase 2.
import { OrbitCamera } from "./camera";
import { TerrainRenderer } from "./render/terrain";
import { generateHeightfield } from "./placeholder";
import { WORLD } from "./config";
import { buildSettingsPanel, DEFAULTS, type Settings } from "./settings";
import { Engine } from "./engine";
import { LiquidRenderer } from "./render/liquid";

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
const liquid = new LiquidRenderer(gl);
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

function pushLiquid(): void {
  if (!engine) return;
  const l = engine.liquidSurface(settings.engine.exaggeration);
  liquid.setMesh(l.positions, l.normals, l.types, l.indices);
}

// Full rebuild: mesh + elevation (seed / detail / octaves), re-apply sea level.
function regenerate(): void {
  if (engine) {
    engine.build(
      WORLD.size,
      WORLD.size,
      detailToSpacing(settings.engine.detail),
      settings.engine.seed,
      settings.engine.octaves,
    );
    engine.setSeaLevel(settings.liquids.seaLevel);
    pushSurface();
    pushLiquid();
  } else {
    const n = settings.engine.detail;
    const heights = generateHeightfield(n, settings.engine.seed, settings.engine.octaves);
    terrain.setHeightfield(heights, n, WORLD.size, settings.engine.exaggeration);
  }
  requestDraw();
}

// Vertical scale only — re-tessellate both surfaces without rebuilding the mesh.
function retessellate(): void {
  if (engine) {
    pushSurface();
    pushLiquid();
    requestDraw();
  } else {
    regenerate();
  }
}
function applyLighting(): void {
  const { lightAzimuth, lightElevation, ambient } = settings.render;
  terrain.setLighting(lightAzimuth, lightElevation, ambient);
  liquid.setLighting(lightAzimuth, lightElevation, ambient);
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
    } else if (group === "liquids" && engine) {
      engine.setSeaLevel(settings.liquids.seaLevel);
      pushLiquid();
      requestDraw();
    }
    // physics: stored on settings.physics; used by the settle animation
  });
}

// Liquid settle: rain, then relax the solver over several frames so the user can
// watch water flow downhill and pool. Driven by the Physics sliders.
let settleFrames = 0;
const physicsFlowRate = (): number => {
  const p = settings.physics;
  return Math.max(0.01, Math.min(0.5, p.flowRate * p.viscosity * (p.gravity / 9.8) * 0.5));
};
document.getElementById("btn-settle")?.addEventListener("click", () => {
  if (!engine) return;
  engine.rain(0.05);
  settleFrames = settings.physics.solverIters;
});
document.getElementById("btn-drain")?.addEventListener("click", () => {
  if (!engine) return;
  engine.clearLiquid();
  engine.setSeaLevel(settings.liquids.seaLevel);
  settleFrames = 0;
  pushLiquid();
  requestDraw();
});

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
  // Advance the liquid settle animation (one relaxation step per frame).
  if (engine && settleFrames > 0) {
    engine.stepFluid(physicsFlowRate(), settings.physics.evaporation, 1);
    pushLiquid();
    settleFrames--;
    needsDraw = true;
  }
  if (needsDraw) {
    needsDraw = false;
    gl!.viewport(0, 0, canvas.width, canvas.height);
    gl!.clearColor(0.078, 0.067, 0.055, 1); // --bg
    gl!.clear(gl!.COLOR_BUFFER_BIT | gl!.DEPTH_BUFFER_BIT);
    const aspect = canvas.width / Math.max(1, canvas.height);
    const vp = camera.viewProj(aspect);
    terrain.draw(vp);
    liquid.draw(vp);
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
