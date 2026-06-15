# DHCE Cartographer — in-editor terrain/world authoring addon

A Godot 4.6 (.NET) **EditorPlugin** that authors the *Desolate Haven* canon landscape (terrain +
biomes + traits + hydrology) live in the editor viewport, driving a shared Rust core (`dhce-core`)
through the `DhceEngine` GDExtension. Drop it into any project's `addons/` and enable it.

## What's in here

| File | Role |
|---|---|
| `plugin.cfg` | Plugin manifest (entry = `DhcePlugin.cs`). |
| `dhce.gdextension` + `dhce_godot.dll` | The Rust core bridge (`DhceEngine`). The DLL is gitignored — build it (below). |
| `DhcePlugin.cs` | EditorPlugin: hosts the dock, streams chunks around the editor camera, and forwards 3D viewport input into brush strokes (with the brush-footprint gizmo). |
| `DhceWorld.cs` | `[GlobalClass] Node3D` that owns the engine, generates on demand, streams chunked terrain + liquid, and Saves/Loads a `DhceWorldState`. |
| `DhceDock.cs` | The authoring panel (tools, brush, world, views, regions, traits, region landform, shaping, transitions, palette, physics, save/load). |
| `DhceMinimap.cs` | Whole-world overview map (zoom/pan + camera-focus marker). |
| `DhceWorldState.cs` | `[GlobalClass] Resource`: the compact, regenerate-from-this snapshot (params + authored fields). |

## Enable it

1. Build + copy the GDExtension DLL (Windows / PowerShell):
   ```
   cargo build --release --manifest-path crates/dhce-godot/Cargo.toml
   # copy C:\Users\…\.dhce-build\release\dhce_godot.dll → tool/addons/dhce/dhce_godot.dll
   ```
   (Godot must be **closed** to overwrite the DLL.)
2. `dotnet build` the host project (the C# tool scripts must be compiled before the plugin loads).
3. **Project → Project Settings → Plugins → enable "DHCE Cartographer."**

## Author a world

- Open **`DhceHost.tscn`** (or add a **DhceWorld** node to any 3D scene; give the scene a
  `DirectionalLight3D` + `WorldEnvironment` for lighting — the plugin does **not** manage a sun).
- In the **DHCE Cartographer** dock: set params → **Generate**. Fly the editor camera to stream the
  world in (only a render-distance ring is meshed; the **MAP** shows the whole world).
- **Select the DhceWorld node**, then **left-drag in the viewport** to paint with the active tool. The
  brush is a 3D sphere under the cursor (works from any angle). Pick tools / views / regions / palette
  in the dock.
- **Save world** writes a compressed `res://<NodeName>_dhce.res` and references it from the node;
  reopening the scene regenerates the identical world from it (deterministic core + restored fields).

## Notes

- Chunks are a fixed physical size (default 256 m, `DhceWorld.ChunkSizeM`); the world snaps to a whole
  number of tiles. Nodes are created lazily, so the live node count tracks the visible ring.
- Persistence v1 doesn't store `shape_delta` or the per-trait painted *base* — re-running Apply
  shaping / Blend borders after a load re-bases once (one-time, non-destructive).
- Determinism: all shared generation math lives in `dhce-core` and is bit-reproducible across targets
  (see `docs/adr/0001-shared-rust-core.md`).
