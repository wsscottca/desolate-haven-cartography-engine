# N0 — Godot sculpt-loop risk spike: setup

Goal: confirm the Godot stack is viable — orbit a true-3D terrain built by the Rust
`DhceEngine`, sculpt it with the mouse, and measure the per-stroke cost at full density.
All compute is in Rust; C# only uploads buffers and runs the camera.

**Already done for you (in this repo):**
- `addons/dhce/dhce_godot.dll` — the release GDExtension (rebuild: `cargo build -p dhce-godot --release`, then copy from `crates/dhce-godot/target/release/`).
- `addons/dhce/dhce.gdextension` — the extension manifest (auto-loads on project open).
- `scripts/OrbitCamera.cs`, `scripts/CartographerSpike.cs` — the spike (self-contained: spawns camera, light, and terrain).

**Your steps (need the Godot 4.x .NET editor — not on my PATH):**

1. **Create the project here.** Godot Project Manager → *Import* or *New Project*, set the
   project path to this `tool/` folder (it keeps the existing `addons/` + `scripts/`),
   *Create & Edit*. Must be the **.NET / C#** build of Godot.

2. **Extension loads automatically** from `addons/dhce/dhce.gdextension` on open. If the
   editor reports it can't load the library, note your exact Godot version (Help → About)
   and tell me — gdext (0.2.4) is pinned to a Godot 4.x API and I'll align + rebuild.

3. **Build the C# solution.** The editor offers to create the C# solution on first need
   (Project → Tools → C#), then press the hammer / **Build**. This generates the
   `.csproj`/`.sln` (editor-managed — I deliberately did not hand-author them) and compiles
   the two scripts.

4. **Make the scene.** New Scene → add a node of type **CartographerSpike** as the root
   (it's a `[GlobalClass]`; or add a `Node3D` and attach `scripts/CartographerSpike.cs`).
   Save as `Main.tscn`, set it as the main scene (Project → Project Settings → Run).

5. **Run (F5).** Controls: **right-drag** orbit, **wheel** zoom, **middle-drag** pan,
   **left-drag** sculpt (raise). The Output panel prints `regions=… triangles=…` and a
   rolling `paint+retess avg … ms`.

6. **Stress it.** Select the root node and lower the **Spacing** export (e.g. 12 → 8 → 6)
   to push the region count up toward ~445k. Note where it lands and whether left-drag
   stays smooth.

**Report back:** region count at your test spacing, the avg `paint+retess` ms, and whether
dragging feels smooth. That decides whether full re-pack + `ArrayMesh` rebuild is enough, or
we add the `RenderingServer.mesh_surface_update_vertex_region` partial-update path (the
engine already returns the touched region ids from `paint_terrain` for exactly this).
