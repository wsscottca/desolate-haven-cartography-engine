---
status: active
date: 2026-06-15
owner: wsscottca
---

# DHCE — next-session backlog

Captured at end of the 2026-06-15 session (after the dock theme/collapsible + unified contextual
brushes redesign).

## Progress — 2026-06-16

- **Items 1–4** (UI moves): done in commit `1b041be` (prior session).
- **Item 6** (climate-driven rain): done. New core `rainfall::compute_rainfall` + `fluid::add_rain_field`
  + `world::apply_rainfall`; GDExtension `apply_rainfall`. Manual Rain brush + cloud/particle rig
  removed from `DhceWorld`/`DhcePlugin`/`DhceDock`/`ToolState`; dock "Apply rainfall" button added.
- **Item 8** (default sea level): done. Core `min_elevation`; `DhceWorld.OnGenDone` seats sea level
  ~1 km above the lowest basin on a fresh generate; dock slider syncs; `SeaLevel` persists in state.
- **Item 7** (per-biome scatter): done with a true `biome_mask` (small core change — the "no core
  change" assumption was wrong: `region_mask` gates the named Region, not the painted biome).
  `ScatterRule.biome_mask` + gate; 8→9-float rule array; `DhceScatterSlot.BiomeMask` + dock biome grid.
- **Item 5** (transitions brush): done alongside the global bake. Core `world::blend_brush` +
  `diffuse_subset`; GDExtension `blend_brush`; `ToolKind.Transition` + dock brush + width slider +
  `transition.svg` icon. Global "Blend borders" kept.
- **Bug 10** (all-white map): addressed by removing the rain cloud/particle rig (item 6, the leading
  suspect). Needs a visual confirm in the editor.
- **Bug 9** (WASD freelook): diagnosed — no DHCE code defect. The plugin forwards keyboard + RMB, so
  Godot's native editor flythrough (hold RMB + WASD) is not blocked; the play-mode `OrbitCamera`
  RMB→WASD path is correct. Most likely an expectation/context issue; revisit only if it still
  misbehaves after a real repro (optionally add a no-RMB "fly" toggle).

Core tests green (`cargo test -p dhce-core`, 33 tests incl. new rainfall/biome_mask); DLL rebuilt +
copied; C# project builds (0 errors). Remaining: open the editor to import `transition.svg` + reload
the DLL, then the visual checks for bugs 9/10.

Original captured items below (for reference).

## Context / where we left off

- The whole DHCE authoring UI is back in the **sidebar dock**, restyled with the game parchment theme
  (`tool/scripts/ToolTheme.cs` via `tool/addons/dhce/DhceUi.cs`) and made **collapsible** per section.
- All paint tools are grouped under one **BRUSHES** section; selecting a tool reveals only that
  brush's options below it (`RefreshBrushOptions` in `DhceDock.cs`).
- The minimap + readouts (Biome/Region/Temp-Moist) + the **View / map-layer selector** still float
  **under the minimap** (`DhceMapPanel`).
- Cave carving is a drag-**brush**; the world **regenerates on editor load** (`DhceWorld._Ready` →
  Load(State) else Generate, gated by `[Export] RegenerateOnLoad`).
- **Known broken / to investigate (see Bugs):** WASD freelook; the all-white map.

## UI / UX

1. **Move the map-layer selector back into the editor dock.** The View dropdown (Natural /
   Temperature / Moisture / Elevation / Biome / Region) currently lives under the minimap in
   `DhceMapPanel`; move it into the sidebar `DhceDock`. (Decide whether the Biome/Region/Temp-Moist
   readouts move with it or stay under the minimap — lean: readouts stay under minimap, View moves.)
2. **Promote the Trait sub-brushes to top-level brushes.** Today Temperature / Moisture / Vegetation /
   Palette-family are nested under a single "Trait" tool. Make each its own brush button in the
   BRUSHES row, each with its own contextual options (value slider / enum). Drop the umbrella "Trait"
   tool. Reuse the existing `temperature/moisture/vegetation/palette` SVGs.
3. **Icons for the non-sculpt brush buttons** (currently text):
   - **Biome** → two trees.
   - **Cave** → a cave/arch with an arrow pointing into it.
   - **Region** → needs a concept (ideas: a map pin/flag, a stamp, a dashed territory outline). Pick
     one next session.
   - Temperature/Moisture/Vegetation/Palette already have icons — apply when promoted (item 2).
4. **Move Save/Load to directly above Export** (bottom of the dock).
5. **Maybe turn Transitions into a brush.** Instead of the global "Blend borders (width m)" bake, a
   brush you drag across a biome boundary that tells the engine *what's within the brushed bounds* so
   it can decide how to merge the two biomes. Needs design: what info to pass (the biomes/traits under
   the brush, the boundary), how the core consumes it. Give it an icon. Open to a better mechanism or
   placement.

## Engine / simulation

6. **Rain rework — climate-driven.** Temporarily **remove the Rain button/tool**. Build a rainfall
   *model* in the core from temperature + biome water traits (raininess / rain-shadow / evaporation,
   see `crates/dhce-core/src/biomes.rs` `WaterProfile`) + vegetation + elevation/rain-shadow, then
   drive the fluid sim from that derived rainfall instead of a manual area brush. (The progressive
   per-area rain code + cloud/particle visuals in `DhceWorld` can be removed or repurposed.)
7. **Per-biome scatter (was Phase 7) — fully plan + build.** Object types defined **per biome**
   (reuse `DhceScatterSlot.RegionMask`; the core already gates by it in `scatter.rs`). Authoring UI:
   pick a biome → its object types (mesh + density + scale + elevation band). No core change needed,
   per earlier exploration.
8. **Default water level = 1 km above the terrain's lowest point.** At generate, find the min
   elevation and set sea level so the water surface sits ~1 km above it (convert 1 km → normalized
   elevation via the exaggeration/elev-span). Replaces the current default sea level 0.0.

## Bugs / investigations

9. **WASD freelook still not working.** Determine the root cause (editor freelook needs the 3D
   viewport to hold keyboard focus — overlay/dock controls may be stealing it; or the runtime
   `OrbitCamera` path). Confirm and fix.
10. **Map is all white right now.** Investigate. Leading suspect: the rain/cloud feature
    (GPUParticles3D / translucent cloud mesh) — if so, removing it (item 6) resolves it, no worries.
    Other hypotheses to rule out: regenerate-on-load producing an empty/unlit world, the
    WorldEnvironment ProceduralSky/exposure washing everything out, or a themed overlay covering the
    viewport. Diagnose before deleting.

## Docs / explanations

11. **Shaping explainer (answered live this session, recorded here):** the **REGION LANDFORM** dials
    (Jaggedness, Relief, Foothill falloff, Erosion) set each region's *intended terrain character* but
    don't change geometry by themselves. **"Apply shaping"** (`shape_terrain(strength)`) bakes those
    per-region dials into the actual heightfield — deforming elevation so the terrain expresses the
    character (sharper peaks, more/less relief amplitude, foothill falloff, erosion), with **Strength**
    scaling how strongly. Transitions (`blend_traits(width)`) then smooths trait values across region
    borders so biomes don't change abruptly.

## Suggested order

Quick wins first (UI moves 1/2/3/4), then the bugs (9/10 — 10 likely falls out of the rain rework),
then the bigger engine work (6 rain model, 8 water default, 7 per-biome scatter), then the design-y
item (5 transitions-as-brush).
