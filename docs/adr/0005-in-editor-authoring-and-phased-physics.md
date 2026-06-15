---
status: active
date: 2026-06-15
owner: wsscottca
---

# ADR 0005 — In-editor authoring pivot + phased physics

## Status
Accepted. **Supersedes the "standalone desktop application" delivery decision in
[ADR 0002](0002-native-godot-stack.md)** — its Rust-core / compute-in-Rust decisions still hold; only
the *shell* (standalone app → editor plugin) changes.

## Context
DHCE shipped as a standalone Godot 4 + C# runtime app (F5) driving the `dhce-godot` GDExtension over
the shared `dhce-core` ([ADR 0001](0001-shared-rust-core.md), [ADR 0002](0002-native-godot-stack.md)).
A re-evaluation was prompted by [TerraBrush](https://godotengine.org/asset-library/asset/2700), an
in-editor heightmap terrain plugin: why a separate app instead of authoring in the editor?

Key realizations:
- The tool and the game are **not separate products — they're phases of one game's development**
  (environment/terraform first; the game `..\desolate-haven` is greenfield, barely set up).
- A Rust engine runs fine as an in-editor GDExtension (rapier proves it), so `dhce-core` can drive an
  **editor plugin**, not just a standalone app.
- Terraforming must stay **live throughout development** — tweak terrain in the same editor session
  while placing models/props — which a standalone app + export pipeline cannot do.
- The bespoke deterministic core + semantic world model (biomes/traits/regions/hydrology,
  [ADR 0004](0004-biome-region-territory-model.md)) are the real reason to build vs adopt TerraBrush;
  the standalone *shell* + export pipeline are the weak parts and go away.

The slow water that started this thread is a perf problem in our own renderer (whole-surface liquid
re-tessellation every tick), **not** a reason to swap physics engines — see the analysis below.

## Decision
1. **Deliver DHCE as an in-editor Godot plugin** (`EditorPlugin` / `[Tool]`), packaged as a
   self-contained, enable-able addon (`addons/dhce/` = the GDExtension DLL + the C# editor scripts)
   plus the `dhce-core` crate. Install it into the game project; terraform in the editor; the authored
   world is then simply *present* in the project — **no export pipeline**.
2. **Keep the Rust core and every system unchanged.** `dhce-core` (gen, traits, biomes, shaping,
   views, hydrology) and the `dhce-godot` GDExtension API carry over verbatim; only the C# shell
   changes from a runtime app to an editor plugin.
3. **Phased physics:**
   - *Authoring / terraform phase:* the existing **heightfield water sim** (cheap, deterministic,
     editor-time), optimized for responsiveness since it stays live for tweaks.
   - *Gameplay phase:* adopt **`godot-rapier-physics`** for rigid-body / collision dynamics; the
     authored terrain provides a collider. The heightfield hydrology is not used at runtime.
   - **Salva SPH is rejected for terrain water** (particle SPH is infeasible at ~1.7 M-cell basin
     scale and abandons cross-target determinism); only a candidate for *local* in-game water later.
4. **Reusable:** the addon is self-contained, so it drops into future projects ("game 2").

## Consequences
- **+** In-editor, live terraform alongside prop/scene work; no export step; one project, phased.
- **+** Reusable authoring addon; the deterministic core + semantic model are preserved intact.
- **+** Each physics engine stays where it's strong (heightfield for basin hydrology at authoring
  time; rapier for gameplay dynamics).
- **−** A real **C# reshell**: runtime app (`Node3D` + `CanvasLayer` UI + F5) → `EditorPlugin`
  (editor dock + viewport gizmos via `forward_3d_gui_input` + gen-on-demand). The Rust side is
  untouched.
- **−** Editor lifecycle/perf constraints: the ~5 s blocking gen + 1.7 M-cell streaming must behave
  in-editor — gen on a button, likely a lower default authoring density with a high-density "bake",
  and no background-thread gdext calls (same constraint as today).
- `tool/` becomes the dev/test **host** project (with the addon enabled) rather than a shipped app.

## Alternatives rejected
- **Adopt TerraBrush wholesale** — a heightmap raster + splat tool with no deterministic shared core
  and no semantic biome/trait/region/hydrology model (ADR 0004). Borrow its ideas (clipmap LOD,
  in-editor collision generation), don't replace with it.
- **Stay a standalone app + export** — loses live in-editor tweaking; the export pipeline existed
  only to bridge the app↔game split this pivot removes.
- **Salva SPH for terrain water / rain** — slower at scale, breaks determinism (see above).
