// Editor settings — engine + physics parameters surfaced in the right dock.
// Engine controls regenerate the world live; physics controls feed the liquid sim
// (consumed from the fluid phase onward). Grows as each phase adds capability.

export interface EngineSettings {
  seed: number;
  detail: number; // mesh density (placeholder: grid resolution per side)
  octaves: number; // fbm octaves
  exaggeration: number; // vertical scale
}

export interface ToolSettings {
  brushRadius: number; // world units
  strength: number; // sculpt delta / liquid amount factor
}

export interface RenderSettings {
  lightAzimuth: number; // degrees
  lightElevation: number; // degrees above the ground plane
  ambient: number; // 0..1
}

export interface LiquidSettings {
  seaLevel: number; // fill terrain below this elevation with water
}

export interface PhysicsSettings {
  gravity: number;
  viscosity: number; // water ≈ 1; lava is low
  flowRate: number;
  evaporation: number;
  solverIters: number;
}

export interface Settings {
  engine: EngineSettings;
  tools: ToolSettings;
  render: RenderSettings;
  liquids: LiquidSettings;
  physics: PhysicsSettings;
}

export type Group = keyof Settings;

export const DEFAULTS: Settings = {
  engine: { seed: 12345, detail: 256, octaves: 6, exaggeration: 120 },
  tools: { brushRadius: 60, strength: 0.04 },
  render: { lightAzimuth: 51, lightElevation: 50, ambient: 0.38 },
  liquids: { seaLevel: 0.0 },
  physics: { gravity: 9.8, viscosity: 1.0, flowRate: 0.5, evaporation: 0.002, solverIters: 40 },
};

interface Spec {
  group: Group;
  key: string;
  label: string;
  min: number;
  max: number;
  step: number;
  integer?: boolean;
  number?: boolean; // render as a number field instead of a slider
}

const SPECS: Spec[] = [
  { group: "engine", key: "seed", label: "Seed", min: 0, max: 9_999_999, step: 1, integer: true, number: true },
  { group: "engine", key: "detail", label: "Detail", min: 64, max: 512, step: 32, integer: true },
  { group: "engine", key: "octaves", label: "Noise octaves", min: 1, max: 8, step: 1, integer: true },
  { group: "engine", key: "exaggeration", label: "Vertical scale", min: 0, max: 300, step: 5, integer: true },
  { group: "tools", key: "brushRadius", label: "Brush size", min: 10, max: 250, step: 5, integer: true },
  { group: "tools", key: "strength", label: "Strength", min: 0.005, max: 0.15, step: 0.005 },
  { group: "render", key: "lightAzimuth", label: "Light azimuth", min: 0, max: 360, step: 1, integer: true },
  { group: "render", key: "lightElevation", label: "Light elevation", min: 0, max: 90, step: 1, integer: true },
  { group: "render", key: "ambient", label: "Ambient", min: 0, max: 1, step: 0.02 },
  { group: "liquids", key: "seaLevel", label: "Sea level", min: -1, max: 1, step: 0.02 },
  { group: "physics", key: "gravity", label: "Gravity", min: 0, max: 20, step: 0.1 },
  { group: "physics", key: "viscosity", label: "Viscosity", min: 0.05, max: 2, step: 0.05 },
  { group: "physics", key: "flowRate", label: "Flow rate", min: 0, max: 1, step: 0.05 },
  { group: "physics", key: "evaporation", label: "Evaporation", min: 0, max: 0.02, step: 0.001 },
  { group: "physics", key: "solverIters", label: "Solver steps", min: 5, max: 120, step: 5, integer: true },
];

const fmt = (v: number, spec: Spec): string =>
  spec.integer ? String(v) : v.toFixed(spec.step < 0.01 ? 3 : 2);

/** Populate `root` with grouped Engine/Physics controls bound to `settings`. */
export function buildSettingsPanel(
  root: HTMLElement,
  settings: Settings,
  onChange: (group: Group, key: string) => void,
): void {
  root.replaceChildren();
  const labels: Record<Group, string> = { engine: "Engine", tools: "Tools", render: "Lighting", liquids: "Liquids", physics: "Physics" };
  for (const group of ["engine", "tools", "render", "liquids", "physics"] as Group[]) {
    const header = document.createElement("div");
    header.className = "group-label";
    header.textContent = labels[group];
    root.appendChild(header);

    if (group === "physics") {
      const note = document.createElement("p");
      note.className = "panel-note";
      note.textContent = "Feeds the liquid sim (active from the fluid phase).";
      root.appendChild(note);
    }

    for (const spec of SPECS.filter((s) => s.group === group)) {
      root.appendChild(buildControl(spec, settings, onChange));
    }
  }
}

function buildControl(
  spec: Spec,
  settings: Settings,
  onChange: (g: Group, k: string) => void,
): HTMLElement {
  const bag = settings[spec.group] as unknown as Record<string, number>;

  const wrap = document.createElement("label");
  wrap.className = "setting";

  const row = document.createElement("span");
  row.className = "setting-row";
  const name = document.createElement("span");
  name.textContent = spec.label;
  const valEl = document.createElement("span");
  valEl.className = "setting-val";
  if (!spec.number) valEl.textContent = fmt(bag[spec.key], spec);
  row.append(name, valEl);
  wrap.appendChild(row);

  const input = document.createElement("input");
  input.type = spec.number ? "number" : "range";
  input.min = String(spec.min);
  input.max = String(spec.max);
  input.step = String(spec.step);
  input.value = String(bag[spec.key]);
  if (spec.number) input.className = "setting-number";
  input.addEventListener("input", () => {
    const raw = Number(input.value);
    if (Number.isNaN(raw)) return;
    const v = spec.integer ? Math.round(raw) : raw;
    bag[spec.key] = v;
    if (!spec.number) valEl.textContent = fmt(v, spec);
    onChange(spec.group, spec.key);
  });
  wrap.appendChild(input);

  return wrap;
}
