---
status: active
date: 2026-06-14
owner: wsscottca
---

# DHCE tool — roadmap & session hand-off

Pick-up doc for a fresh session. Design is settled in the ADRs; this is **current state + how to
work + what's next**.

## What this is
The Desolate Haven Cartography Engine — a native **Godot 4.6 + C#** terrain-authoring tool driving a
shared **Rust core** through a GDExtension. It authors the canon world's base landscape (terrain +
biomes + procedural scatter) and exports an **editable Godot scene** the game editor finishes by
hand (place characters, tweak). Design:
- [ADR 0001](../adr/0001-shared-rust-core.md) — one shared Rust core, two adapters.
- [ADR 0002](../adr/0002-native-godot-stack.md) — native Godot 4 + C#, Rust powers all intensive compute.
- [ADR 0003](../adr/0003-edit-streaming-and-godot-export.md) — render-streaming + Godot-native export + biome scatter + **hybrid volumetric**.
- Core API: [docs/specs/dhce-core-contract.md](../specs/dhce-core-contract.md).

## Repo layout
- `crates/dhce-core` — shared Rust: gen + sim + **authoring (`world.rs` = the source of truth)** +
  chunking. Pure, deterministic, no engine deps.
- `crates/dhce-godot` — GDExtension adapter (`godot`/gdext **0.5**, pinned to **Godot 4.6**). Thin:
  every method forwards to `World`. Built standalone → `dhce_godot.dll`.
- `crates/dhce-wasm` — frozen web adapter, kept only as a native↔web determinism cross-check.
- `tool/` — the **Godot 4.6 C# project** (the tool). `tool/scripts/CartographerSpike.cs` (current
  N0/N1 driver) + `OrbitCamera.cs`; `tool/addons/dhce/` (`.gdextension` manifest + the DLL — DLL is
  gitignored, rebuild + copy). Scene root = `CartographerSpike` (`[GlobalClass]`), main scene `Main.tscn`.
- `web/` — retired TS/WebGL front-end. Do not develop.

## How to build / run / test (Windows / PowerShell)
- Prepend cargo to PATH: `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"`.
- Core tests: `cargo test -p dhce-core` (22+ tests; determinism, chunks, streams, fluid).
- **Build dir is off OneDrive** via `.cargo/config.toml` → `C:\Users\WSSco\.dhce-build`. When running
  cargo with `--manifest-path` from another CWD, also set `$env:CARGO_TARGET_DIR="C:\Users\WSSco\.dhce-build"`.
- **LNK1104 on test/exe link** = Windows Defender / OneDrive scanning the fresh `.exe`. Kill stale
  test procs by name and retry (clears on its own). A Defender exclusion for `.dhce-build` stops it.
- Build the DLL: `cargo build -p dhce-godot --release` → `crates/dhce-godot/target/release/dhce_godot.dll`
  (or `.dhce-build/release/` if CARGO_TARGET_DIR set). Copy to `tool/addons/dhce/dhce_godot.dll`.
- **The DLL is locked while the Godot editor is open** — to swap it: user closes Godot → copy → reopen.
  **C#-only changes don't need a DLL swap** — just Build in the editor and F5.
- Git: a repo-scoped push allow-rule exists. Commit + push per phase. **No `Co-Authored-By` on the
  user's creative/story commits; fine on code/design commits.**

## State — DONE
- **N0 ✅** — stack + perf validated. Naive full-mesh rebuild was ~150 ms/dab @617k regions (too slow);
  **chunked terrain** fixed it → ~10–36 ms/dab @1.7–4.3M regions, ~2–4 dirty chunks/dab.
- **N1 ✅** — authoring hoisted into `dhce_core::world::World`; both adapters thin. Chunk API
  (`chunk_count`/`tessellate_chunk`/`chunk_positions|normals|colors|indices`/`take_dirty_chunks`);
  chunks **sized from region count** (~12k regions/tile → flat edit cost at any world size).
- Rendering fixed: double-sided material (the Y-up vertex remap flips winding → top-down was
  backface-culled), ambient `WorldEnvironment` + dark background, vertical scale ~300, maximized window.
- Known finding to fix in N2: **startup gen freezes** (~5 s @1.7M — the global Rust `build()` blocks
  the main thread); tessellate-all adds ~0.5 s.

## NEXT: N2 — streaming tool shell  ← start here
Goal (ADR 0003 render-streaming): a 20 km world opens **instantly** and edits responsively.
1. **Threaded gen** — run `DhceEngine.build(...)` on a C# `Task`; show a "Generating…" splash; on
   completion `Callable.From(OnGenDone).CallDeferred()` to return to the main thread for mesh upload.
   Add `[Export] bool ThreadedGen = true` (sync fallback) — the cross-thread gdext call is the one
   piece not yet tested live, so keep the escape hatch.
2. **Progressive tessellation** — create the N chunk `MeshInstance3D`s, then tessellate a few per
   frame in `_Process` instead of all in `_Ready` (removes the upload hitch; window fills in).
3. **Render distance** — `[Export] int RenderDistance`; mesh only chunks within that tile-distance of
   the camera, free far ones, re-evaluate as the camera moves. Add a core helper for chunk centers /
   `(gx,gy)` (or expose `chunk_grid()` + chunk bounds) so C# maps camera → visible tiles with no
   per-region loops.
4. Default the spike to **20 km** (Width/Height = 20000) once streaming lands; expose vertical scale
   as a world-height (km) setting.
Verify: window opens immediately on a 20 km world; tiles stream around the camera; edits stay ~10 ms;
the per-dab timing line still prints.

## Then
- **N3** — sculpt/course/flood/biome tools, brush size + intensity UI, per-biome editor + **biome-driven
  terrain shaping** (apply per-biome `Landform` profiles so jagged/rolling/volcanic differ) + scatter
  model selection.
- **N4** — export pipeline → editable Godot `.tscn` (terrain `ArrayMesh`+collision, `MultiMesh` scatter,
  bake-to-instances) + authoring-project save/load.
- **N5** — authored **volumetric** caves / overhangs / tunnels carved into the heightfield (hybrid;
  local volumetric mesh merged on export).
- Later — generation-streaming/LOD only if a world outgrows RAM; Atmospheric Fog; Scatter Tool polish.

## Pending housekeeping
- Migrate the build-time license gate from `scripts/check-licenses.mjs` (Node) to `cargo-deny`.
- The frozen `dhce-wasm` should stay building (determinism cross-check) — don't delete it.
