# N3a — Tool Shell + World/Physics Panels + Liquid Rendering — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the N2 streaming spike into a usable authoring shell — an on-screen toolbar that drives every brush the core already exposes, plus world and physics (fluid) panels and a rendered liquid surface — all pure C#, no DLL rebuild.

**Architecture:** Split the `CartographerSpike` god-class into three responsibilities: `CartographerSpike` (render/stream/lifecycle + a small public API), `ToolState` (active tool + brush params + the one place that maps a stroke to a `DhceEngine` call), and `ToolUi` (a `CanvasLayer` overlay that mutates `ToolState` and calls into `CartographerSpike`). The core (`dhce_core::world::World` via the `DhceEngine` GDExtension) is unchanged — N3a only wires ops that already exist (`paint_terrain`/`paint_course`/`paint_liquid`/`paint_biome`/`generate_streams`/`set_sea_level`/`rain`/`step_fluid`/`clear_liquid`/`tessellate_liquid`).

**Tech Stack:** Godot 4.6 .NET (C#), the `dhce_godot` GDExtension (`DhceEngine` driven via `GodotObject.Call`). See [docs/specs/n3-tooling-design.md](../specs/n3-tooling-design.md) §3 (N3a) and [docs/specs/dhce-core-contract.md](../specs/dhce-core-contract.md).

---

## Testing approach (read first)

This slice is Godot-node + UI glue: it can't be unit-tested without a Godot test harness (GdUnit4 / a `dotnet test` project), which is **out of scope for N3a**. So each task uses two gates:

1. **Compile gate (automated):** from `tool/`, run `dotnet build`. Expected: `Build succeeded`. (If the project isn't restored yet, open the Godot 4.6 .NET editor once so it regenerates `tool/*.csproj`, then `dotnet build` works. The gdextension DLL is not needed to compile C#.)
2. **Visual gate (manual — user):** the Godot editor is not on the agent's PATH, so the F5 checks are run by the user. Under subagent-driven execution, **pause at each task boundary for the user to F5-verify** before continuing.

The brush-dispatch logic lives in `ToolState` precisely so the one piece worth reasoning about in isolation is small and readable.

No DLL rebuild anywhere in N3a — C#-only changes are picked up by the editor's Build (hammer) + F5.

---

## File structure

- **Modify** `tool/scripts/CartographerSpike.cs` — keep gen/splash/streaming; add the public API (`Engine`, `Exaggeration`, `Tool`, `RepaintDirtyTerrain`, `RebuildLiquid`, `Regenerate`, `SetTerrainHeight`, `SetStatus` plumbing); add liquid rendering; route left-drag through `ToolState.Apply`.
- **Create** `tool/scripts/ToolState.cs` — `Tool` enum, `EditResult` flags, `ToolState` (params + `Apply`).
- **Create** `tool/scripts/ToolUi.cs` — `CanvasLayer` overlay: tools, brush, world, physics sections + status line.
- `tool/scripts/OrbitCamera.cs` — unchanged.
- `tool/Main.tscn` — unchanged (root stays `CartographerSpike`; `ToolUi` is created in code so the scene needs no edits).

---

## Task 1: Extract `ToolState` and route the existing brush through it

Behaviour stays identical (left-drag still raises) but dispatch moves out of `CartographerSpike` into `ToolState`, and all four sculpt modes + river/flood/biome become reachable by switching `ToolState.Active`.

**Files:**
- Create: `tool/scripts/ToolState.cs`
- Modify: `tool/scripts/CartographerSpike.cs` (the `_UnhandledInput`/`PaintAt` region + add public API; seed `Tool` from the brush exports in `_Ready`)

- [ ] **Step 1: Create `ToolState.cs`**

```csharp
using Godot;

namespace DesolateHaven.Cartography;

/// The authoring tools, in toolbar order. Shortcuts 1–7 map to these (see ToolUi).
public enum Tool { Raise, Carve, Level, Crest, River, Flood, Biome }

/// What a stroke changed, so the caller knows which render surface(s) to refresh.
[System.Flags]
public enum EditResult { None = 0, Terrain = 1, Liquid = 2 }

/// Active tool + brush parameters, and the single place that maps a stroke to a DhceEngine
/// call. Kept apart from rendering (CartographerSpike) and UI (ToolUi): the UI mutates these
/// fields; CartographerSpike calls Apply() on left-drag. Radius/strength are metres / normalized
/// elevation (1 Godot unit = 1 m); the core's brush ops take metres.
public sealed class ToolState
{
    public Tool Active = Tool.Raise;
    public float RadiusM = 350f;       // brush footprint radius, metres
    public float Strength = 0.06f;     // sculpt step (normalized elevation)
    public int BiomeId = 1;            // 1..=14 for the Biome tool
    public int LiquidKind = 0;         // 0 water, 1 lava (River + Flood)
    public float CourseIntensity = 0.5f;
    public float FloodAmount = 0.25f;

    /// Apply the active tool at world-ground point `hit` (Godot XZ plane → core x,y). Returns
    /// which surfaces changed. `engine` is the DhceEngine; all calls go through Variant marshalling.
    public EditResult Apply(GodotObject engine, Vector3 hit)
    {
        double x = hit.X, z = hit.Z, r = RadiusM, s = Strength;
        switch (Active)
        {
            case Tool.Raise: engine.Call("paint_terrain", x, z, r, s, 0); return EditResult.Terrain;
            case Tool.Carve: engine.Call("paint_terrain", x, z, r, s, 1); return EditResult.Terrain;
            case Tool.Level: engine.Call("paint_terrain", x, z, r, s, 2); return EditResult.Terrain;
            case Tool.Crest: engine.Call("paint_terrain", x, z, r, s, 3); return EditResult.Terrain;
            case Tool.River: engine.Call("paint_course", x, z, r, (double)CourseIntensity, LiquidKind);
                             return EditResult.Terrain | EditResult.Liquid;
            case Tool.Flood: engine.Call("paint_liquid", x, z, r, (double)FloodAmount, LiquidKind);
                             return EditResult.Liquid;
            case Tool.Biome: engine.Call("paint_biome", x, z, r, BiomeId); return EditResult.Terrain;
            default: return EditResult.None;
        }
    }
}
```

- [ ] **Step 2: Add the public API + `ToolState` to `CartographerSpike`**

In `CartographerSpike` add the field and accessors (place near the other private fields / after the constructor region). `Engine` and `Exaggeration` expose what `ToolUi` and tools need; `Tool` is the shared `ToolState`.

```csharp
    /// Shared tool state: the UI mutates it, left-drag applies it. Seeded from the brush exports.
    public ToolState Tool { get; } = new ToolState();
    public GodotObject Engine => _engine;
    public float Exaggeration => _exaggeration;
```

In `_Ready`, after `_widthM = _heightM = WorldSizeKm * 1000f;`, seed the tool from the existing brush exports:

```csharp
        Tool.RadiusM = BrushRadiusM;
        Tool.Strength = BrushStrength;
```

- [ ] **Step 3: Replace the hardcoded paint with tool dispatch**

Replace the body of `PaintAt` (currently calls `paint_terrain(... , 0)` and repaints) so it dispatches through `ToolState` and repaints based on the result. Replace from the `ulong t0 = Time.GetTicksUsec();` line through the end of the method:

```csharp
        ulong t0 = Time.GetTicksUsec();
        EditResult res = Tool.Apply(_engine, hit);
        int dirtyCount = 0;
        if (res.HasFlag(EditResult.Terrain)) dirtyCount = RepaintDirtyTerrain();
        if (res.HasFlag(EditResult.Liquid)) RebuildLiquid();
        double ms = (Time.GetTicksUsec() - t0) / 1000.0;

        _totalMs += ms;
        _dirtyAccum += dirtyCount;
        if (++_dabs % 30 == 0)
            GD.Print($"[DHCE] {_dabs} dabs: {_totalMs / _dabs:0.0} ms/dab over {(double)_dirtyAccum / _dabs:0.0} dirty chunks/dab");
```

- [ ] **Step 4: Add `RepaintDirtyTerrain` (extracted from the old loop)**

Add this method to `CartographerSpike` (near `BuildChunk`). It is the existing dirty-chunk repaint, now reusable by tools and the physics/world panels:

```csharp
    /// Re-tessellate the chunks the last edit dirtied that are currently meshed. Out-of-range
    /// dirty chunks re-tessellate fresh (reflecting the edit) when they next stream in. Returns
    /// the dirty-chunk count (for the per-dab timing line).
    public int RepaintDirtyTerrain()
    {
        int[] dirty = _engine.Call("take_dirty_chunks").As<int[]>();
        foreach (int ci in dirty)
            if (ci >= 0 && ci < _chunkMeshes.Length && _built[ci]) BuildChunk(ci);
        return dirty.Length;
    }
```

- [ ] **Step 5: Add a temporary `RebuildLiquid` stub (filled in Task 2)**

So the project compiles now; Task 2 implements the body.

```csharp
    /// Re-tessellate the whole liquid surface and re-upload it. Implemented in Task 2.
    public void RebuildLiquid() { }
```

- [ ] **Step 6: Compile**

Run: `dotnet build` (from `tool/`)
Expected: `Build succeeded`.

- [ ] **Step 7: Manual verify (user)**

Build (hammer) + F5. Expected: world generates as before; left-drag still raises terrain; the per-dab timing line still prints. (Other tools aren't selectable yet — that's Task 3.)

- [ ] **Step 8: Commit**

```bash
git add tool/scripts/ToolState.cs tool/scripts/CartographerSpike.cs
git commit -m "DHCE N3a: extract ToolState; route brush through it (behaviour unchanged)"
```

---

## Task 2: Liquid surface rendering

Add one `MeshInstance3D` for the liquid, fed by `tessellate_liquid` + the `liquid_*` getters, with per-vertex colour from `liquid_types` (water vs lava). This makes the Flood and River tools (and the Physics panel) visible.

**Files:**
- Modify: `tool/scripts/CartographerSpike.cs` (fields, `SetupSceneAndCamera` for the material, `OnGenDone` to create the node, `RebuildLiquid` body)

- [ ] **Step 1: Add liquid fields**

Near the chunk render-state fields:

```csharp
    private MeshInstance3D _liquid;
    private ArrayMesh _liquidMesh;
    private StandardMaterial3D _liquidMat;
    private static readonly Color WaterColor = new Color(0.20f, 0.45f, 0.75f, 0.6f);
    private static readonly Color LavaColor = new Color(0.95f, 0.35f, 0.10f, 0.9f);
```

- [ ] **Step 2: Create the liquid material in `SetupSceneAndCamera`**

After the `_mat` setup block, add a translucent, double-sided, vertex-coloured material:

```csharp
        _liquidMat = new StandardMaterial3D
        {
            VertexColorUseAsAlbedo = true,
            Roughness = 0.1f,
            Metallic = 0.0f,
        };
        _liquidMat.Set("transparency", 1);  // BaseMaterial3D.Transparency.Alpha
        _liquidMat.Set("cull_mode", 2);     // CULL_DISABLED (same Y-up winding flip as terrain)
```

- [ ] **Step 3: Create the liquid node in `OnGenDone`**

After the chunk-node creation loop (before `HideSplash()`):

```csharp
        _liquidMesh = new ArrayMesh();
        _liquid = new MeshInstance3D { Mesh = _liquidMesh, MaterialOverride = _liquidMat };
        AddChild(_liquid);
```

- [ ] **Step 4: Implement `RebuildLiquid` (replace the Task 1 stub)**

```csharp
    public void RebuildLiquid()
    {
        if (_liquidMesh == null) return;
        _engine.Call("tessellate_liquid", (double)_exaggeration);
        _liquidMesh.ClearSurfaces();
        var positions = _engine.Call("liquid_positions").As<Vector3[]>();
        if (positions.Length == 0) return; // no liquid — empty surface
        var normals = _engine.Call("liquid_normals").As<Vector3[]>();
        var types = _engine.Call("liquid_types").As<float[]>();
        var indices = _engine.Call("liquid_indices").As<int[]>();

        var colors = new Color[positions.Length];
        for (int i = 0; i < positions.Length; i++)
            colors[i] = (i < types.Length && types[i] > 0.5f) ? LavaColor : WaterColor;

        var arrays = new Godot.Collections.Array();
        arrays.Resize((int)Mesh.ArrayType.Max);
        arrays[(int)Mesh.ArrayType.Vertex] = positions;
        arrays[(int)Mesh.ArrayType.Normal] = normals;
        arrays[(int)Mesh.ArrayType.Color] = colors;
        arrays[(int)Mesh.ArrayType.Index] = indices;
        _liquidMesh.AddSurfaceFromArrays(Mesh.PrimitiveType.Triangles, arrays);
    }
```

- [ ] **Step 5: Compile**

Run: `dotnet build` (from `tool/`)
Expected: `Build succeeded`.

- [ ] **Step 6: Manual verify (user)**

F5. Temporarily set `Tool.Active = Tool.Flood` by editing `_Ready` (or wait for Task 3's UI). With Flood active, left-drag over terrain — a translucent blue sheet should appear and sit at terrain height. Revert any temporary edit.

- [ ] **Step 7: Commit**

```bash
git add tool/scripts/CartographerSpike.cs
git commit -m "DHCE N3a: render the liquid surface (water/lava vertex colour)"
```

---

## Task 3: `ToolUi` overlay — tools + brush params

A `CanvasLayer` with a single left-side panel (sections fill in over Tasks 3–5). This task adds the **Tools** section (7 toggle buttons) and the **Brush** section (radius/strength sliders, biome picker, liquid-kind picker). The panel consumes its own mouse input (default `MouseFilter = Stop` on `PanelContainer`), so clicking it never paints terrain.

**Files:**
- Create: `tool/scripts/ToolUi.cs`
- Modify: `tool/scripts/CartographerSpike.cs` (`_Ready`: create + bind the UI; add `_ui` field + `SetStatus`)

- [ ] **Step 1: Create `ToolUi.cs` with the builder helpers + Tools/Brush sections**

```csharp
using Godot;

namespace DesolateHaven.Cartography;

/// On-screen authoring overlay (runtime tool, F5 — not an editor dock). One left-side panel
/// with sections: Tools, Brush, World, Physics, and a status line. Mutates the shared ToolState
/// and calls back into CartographerSpike for regenerate / repaint / liquid. The panel is a
/// PanelContainer (MouseFilter = Stop), so clicks on it are consumed before terrain painting.
public partial class ToolUi : CanvasLayer
{
    /// Set by CartographerSpike before AddChild; the UI's single dependency.
    public CartographerSpike Root;

    private readonly ButtonGroup _toolGroup = new ButtonGroup();
    private Label _status;
    private OptionButton _biome, _liquidKind;
    private static readonly string[] BiomeNames =
    {
        "Jagged Mountains", "Sacred Woods Plateau", "Great Lake", "Temperate Forest",
        "Open Plains", "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles",
        "Blisterwood", "Volcanic Scape", "Blight Ruins", "Scattered Isles", "Marsh Bog",
    };

    public override void _Ready()
    {
        var panel = new PanelContainer();
        panel.SetAnchorsAndOffsetsPreset(Control.LayoutPreset.TopLeft);
        panel.Position = new Vector2(8, 8);
        AddChild(panel);

        var col = new VBoxContainer { CustomMinimumSize = new Vector2(240, 0) };
        panel.AddChild(col);

        Section(col, "TOOLS");
        AddToolButton(col, "Raise", Tool.Raise);
        AddToolButton(col, "Carve", Tool.Carve);
        AddToolButton(col, "Level", Tool.Level);
        AddToolButton(col, "Crest", Tool.Crest);
        AddToolButton(col, "River", Tool.River);
        AddToolButton(col, "Flood", Tool.Flood);
        AddToolButton(col, "Biome", Tool.Biome);

        Section(col, "BRUSH");
        Slider(col, "Radius (m)", 10, 2000, 5, Root.Tool.RadiusM, v => Root.Tool.RadiusM = (float)v);
        Slider(col, "Strength", 0.005, 0.3, 0.005, Root.Tool.Strength, v => Root.Tool.Strength = (float)v);

        _biome = new OptionButton();
        for (int i = 0; i < BiomeNames.Length; i++) _biome.AddItem($"{i + 1}. {BiomeNames[i]}", i + 1);
        _biome.Selected = 0;
        _biome.ItemSelected += idx => Root.Tool.BiomeId = (int)_biome.GetItemId((int)idx);
        Labeled(col, "Biome", _biome);

        _liquidKind = new OptionButton();
        _liquidKind.AddItem("Water", 0);
        _liquidKind.AddItem("Lava", 1);
        _liquidKind.Selected = 0;
        _liquidKind.ItemSelected += idx => Root.Tool.LiquidKind = (int)idx;
        Labeled(col, "Liquid", _liquidKind);

        _status = new Label { Text = "ready", AutowrapMode = TextServer.AutowrapMode.WordSmart };
        col.AddChild(new HSeparator());
        col.AddChild(_status);
    }

    public void SetStatus(string text) => _status.Text = text;

    // --- builder helpers (DRY) ---

    private void Section(Container parent, string title)
    {
        parent.AddChild(new HSeparator());
        parent.AddChild(new Label { Text = title });
    }

    private void AddToolButton(Container parent, string text, Tool tool)
    {
        var b = new Button { Text = text, ToggleMode = true, ButtonGroup = _toolGroup };
        b.Pressed += () => Root.Tool.Active = tool;
        if (tool == Root.Tool.Active) b.ButtonPressed = true; // pre-select the default tool
        parent.AddChild(b);
    }

    /// A labelled HSlider row; calls `onChange` on every move. Returns the slider for callers
    /// that need to drive it (e.g. keyboard brush-size shortcuts in Task 6).
    private HSlider Slider(Container parent, string label, double min, double max, double step,
                           double val, System.Action<double> onChange)
    {
        var lbl = new Label { Text = $"{label}: {val:0.###}" };
        parent.AddChild(lbl);
        var s = new HSlider { MinValue = min, MaxValue = max, Step = step, Value = val };
        s.ValueChanged += v => { lbl.Text = $"{label}: {v:0.###}"; onChange(v); };
        parent.AddChild(s);
        return s;
    }

    private void Labeled(Container parent, string label, Control control)
    {
        parent.AddChild(new Label { Text = label });
        parent.AddChild(control);
    }
}
```

- [ ] **Step 2: Create + bind the UI from `CartographerSpike._Ready`**

Add an `_ui` field near the other node fields:

```csharp
    private ToolUi _ui;
```

In `_Ready`, after `SetupSceneAndCamera();` (so `Tool` is already seeded), create the overlay:

```csharp
        _ui = new ToolUi { Root = this };
        AddChild(_ui);
```

And make the per-dab line also update the status (replace the `GD.Print(... dabs ...)` line in `PaintAt` with both):

```csharp
        if (++_dabs % 30 == 0)
        {
            string line = $"{_dabs} dabs: {_totalMs / _dabs:0.0} ms/dab over {(double)_dirtyAccum / _dabs:0.0} dirty chunks/dab";
            GD.Print($"[DHCE] {line}");
            _ui?.SetStatus(line);
        }
```

- [ ] **Step 3: Compile**

Run: `dotnet build` (from `tool/`)
Expected: `Build succeeded`.

- [ ] **Step 4: Manual verify (user)**

F5. Expected: a panel appears top-left with TOOLS + BRUSH sections. Selecting Carve/Level/Crest/River/Flood/Biome and left-dragging applies that tool (carve lowers, river lays a channel + water, flood pours water, biome recolours). Radius/strength sliders change the brush live. Clicking the panel does **not** paint terrain.

- [ ] **Step 5: Commit**

```bash
git add tool/scripts/ToolUi.cs tool/scripts/CartographerSpike.cs
git commit -m "DHCE N3a: tool overlay (tools + brush params), wired to ToolState"
```

---

## Task 4: World panel (settings + Regenerate + live terrain height)

Adds a **World** section: seed / octaves / world size / spacing spinboxes + a Regenerate button (rebuilds), and a terrain-height slider that re-tessellates live (no rebuild).

**Files:**
- Modify: `tool/scripts/CartographerSpike.cs` (`Regenerate`, `SetTerrainHeight`, teardown)
- Modify: `tool/scripts/ToolUi.cs` (World section)

- [ ] **Step 1: Add `Regenerate` + `SetTerrainHeight` to `CartographerSpike`**

`Regenerate` reads the (UI-updated) export fields, tears down current meshes, and re-runs the existing splash-deferred gen path. `SetTerrainHeight` is the live, no-rebuild relief change.

```csharp
    /// Rebuild the world from the current Seed/Octaves/WorldSizeKm/SpacingM (set by the UI).
    /// Discards all edits. Reuses the splash-deferred main-thread gen path (_Process warmup).
    public void Regenerate()
    {
        if (_chunks != null) foreach (var mi in _chunks) mi?.QueueFree();
        _chunks = null; _chunkMeshes = null; _built = null;
        _liquid?.QueueFree(); _liquid = null; _liquidMesh = null;
        _genDone = false;

        _widthM = _heightM = WorldSizeKm * 1000f;
        _exaggeration = TerrainHeightKm * 1000f / ElevSpan;
        _cam?.FrameOverhead(new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f), Mathf.Max(_widthM, _heightM) * 0.9f);

        ShowSplash("Regenerating world…");
        _warmupFrames = 0;
        _pendingGen = true;
    }

    /// Live vertical-relief change (no rebuild): recompute exaggeration and re-tessellate every
    /// meshed chunk + the liquid. `km` is world-height in km (ElevSpan maps it to exaggeration).
    public void SetTerrainHeight(float km)
    {
        TerrainHeightKm = km;
        _exaggeration = km * 1000f / ElevSpan;
        if (!_genDone) return;
        for (int i = 0; i < _built.Length; i++) if (_built[i]) BuildChunk(i);
        RebuildLiquid();
    }
```

- [ ] **Step 2: Add the World section to `ToolUi._Ready`** (after the BRUSH section, before the status label)

```csharp
        Section(col, "WORLD");
        var seed = SpinRow(col, "Seed", 0, 999999, 1, Root.Seed);
        var oct = SpinRow(col, "Octaves", 1, 12, 1, Root.Octaves);
        var size = SpinRow(col, "Size (km)", 1, 60, 1, Root.WorldSizeKm);
        var spacing = SpinRow(col, "Spacing (m)", 4, 60, 1, Root.SpacingM);
        Slider(col, "Height (km)", 0.1, 10, 0.1, Root.TerrainHeightKm, v => Root.SetTerrainHeight((float)v));

        var regen = new Button { Text = "Regenerate (discards edits)" };
        regen.Pressed += () =>
        {
            Root.Seed = (int)seed.Value;
            Root.Octaves = (int)oct.Value;
            Root.WorldSizeKm = (float)size.Value;
            Root.SpacingM = (float)spacing.Value;
            Root.Regenerate();
        };
        col.AddChild(regen);
```

- [ ] **Step 3: Add the `SpinRow` helper to `ToolUi`** (next to the other helpers)

```csharp
    private SpinBox SpinRow(Container parent, string label, double min, double max, double step, double val)
    {
        parent.AddChild(new Label { Text = label });
        var sb = new SpinBox { MinValue = min, MaxValue = max, Step = step, Value = val };
        parent.AddChild(sb);
        return sb;
    }
```

- [ ] **Step 4: Compile**

Run: `dotnet build` (from `tool/`)
Expected: `Build succeeded`.

- [ ] **Step 5: Manual verify (user)**

F5. Expected: the Height (km) slider changes relief instantly without a rebuild. Changing Seed/Size/Spacing and pressing Regenerate shows the splash and rebuilds a new world (edits gone). Smaller spacing → denser/slower; the status/Output region counts change accordingly.

- [ ] **Step 6: Commit**

```bash
git add tool/scripts/CartographerSpike.cs tool/scripts/ToolUi.cs
git commit -m "DHCE N3a: world panel (settings + regenerate + live terrain height)"
```

---

## Task 5: Physics panel (fluid sim controls)

Adds a **Physics** section driving the existing fluid ops: sea level, rain, a Settle step (`step_fluid`), Clear, and a Simulate toggle that steps continuously (throttled).

**Files:**
- Modify: `tool/scripts/ToolUi.cs` (Physics section + `_Process` throttled simulate)

- [ ] **Step 1: Add Physics fields to `ToolUi`** (near the other fields)

```csharp
    private CheckButton _simulate;
    private double _simFlow = 0.25, _simEvap = 0.001;
    private int _simSubsteps = 2;
    private int _simTick;
    private const int SimEveryNFrames = 6;
```

- [ ] **Step 2: Add the Physics section to `_Ready`** (after the WORLD section, before the status label)

```csharp
        Section(col, "PHYSICS");
        Slider(col, "Sea level", -1.0, 1.0, 0.01, 0.0, v =>
        {
            Root.Engine.Call("set_sea_level", v);
            Root.RebuildLiquid();
        });
        Slider(col, "Flow rate", 0.0, 0.5, 0.01, _simFlow, v => _simFlow = v);
        Slider(col, "Evaporation", 0.0, 0.02, 0.0005, _simEvap, v => _simEvap = v);
        Slider(col, "Substeps", 1, 8, 1, _simSubsteps, v => _simSubsteps = (int)v);

        var rain = new Button { Text = "Rain" };
        rain.Pressed += () => { Root.Engine.Call("rain", 0.05); Root.RebuildLiquid(); };
        col.AddChild(rain);

        var settle = new Button { Text = "Settle (1 step)" };
        settle.Pressed += () =>
        {
            Root.Engine.Call("step_fluid", _simFlow, _simEvap, _simSubsteps);
            Root.RebuildLiquid();
        };
        col.AddChild(settle);

        var clear = new Button { Text = "Clear liquid" };
        clear.Pressed += () => { Root.Engine.Call("clear_liquid"); Root.RebuildLiquid(); };
        col.AddChild(clear);

        _simulate = new CheckButton { Text = "Simulate" };
        col.AddChild(_simulate);
```

- [ ] **Step 3: Add throttled continuous stepping in `ToolUi._Process`**

Add a `_Process` override (the class has none yet):

```csharp
    public override void _Process(double delta)
    {
        if (_simulate == null || !_simulate.ButtonPressed) return;
        if (++_simTick < SimEveryNFrames) return;
        _simTick = 0;
        Root.Engine.Call("step_fluid", _simFlow, _simEvap, _simSubsteps);
        Root.RebuildLiquid();
    }
```

- [ ] **Step 4: Compile**

Run: `dotnet build` (from `tool/`)
Expected: `Build succeeded`.

- [ ] **Step 5: Manual verify (user)**

F5. Expected: raising Sea level floods low ground; Rain adds water; with water present, Settle (and the Simulate toggle) makes it flow downhill / level out; Clear removes it. Flood-tool puddles then Settle should spread/level realistically.

- [ ] **Step 6: Commit**

```bash
git add tool/scripts/ToolUi.cs
git commit -m "DHCE N3a: physics panel (sea level, rain, settle, simulate, clear)"
```

---

## Task 6: Keyboard shortcuts + Generate Streams button

Editor-style muscle memory: `1`–`7` select tools, `[` / `]` shrink/grow the brush. Plus a **Generate Streams** button (an action, not a brush) in the Tools section.

**Files:**
- Modify: `tool/scripts/ToolUi.cs` (keep a reference to the radius slider + tool buttons by index; `_UnhandledInput` for keys; Streams button)

- [ ] **Step 1: Track the radius slider and tool buttons for shortcut driving**

Add fields:

```csharp
    private HSlider _radius;
    private readonly System.Collections.Generic.List<Button> _toolButtons = new();
```

Change the radius `Slider(...)` call in the BRUSH section to capture it:

```csharp
        _radius = Slider(col, "Radius (m)", 10, 2000, 5, Root.Tool.RadiusM, v => Root.Tool.RadiusM = (float)v);
```

And in `AddToolButton`, record the button:

```csharp
        _toolButtons.Add(b);
```

- [ ] **Step 2: Add the Generate Streams button** (end of the TOOLS section, after the Biome tool button)

```csharp
        var streams = new Button { Text = "Generate Streams" };
        streams.Pressed += () =>
        {
            Root.Engine.Call("generate_streams", 0.5, 1.0);
            Root.RepaintDirtyTerrain();
            Root.RebuildLiquid();
        };
        col.AddChild(streams);
```

- [ ] **Step 3: Add key handling**

```csharp
    public override void _UnhandledInput(InputEvent e)
    {
        if (e is not InputEventKey { Pressed: true, Echo: false } k) return;
        switch (k.Keycode)
        {
            case Key.Key1: case Key.Key2: case Key.Key3: case Key.Key4:
            case Key.Key5: case Key.Key6: case Key.Key7:
                int idx = (int)k.Keycode - (int)Key.Key1;
                if (idx < _toolButtons.Count) _toolButtons[idx].ButtonPressed = true; // fires Pressed → sets Active
                break;
            case Key.Bracketleft:
                _radius.Value = Mathf.Max(_radius.MinValue, _radius.Value - 4 * _radius.Step);
                break;
            case Key.Bracketright:
                _radius.Value = Mathf.Min(_radius.MaxValue, _radius.Value + 4 * _radius.Step);
                break;
        }
    }
```

> Note: setting `ButtonPressed = true` on a `ToggleMode` button in a `ButtonGroup` emits `Pressed`, which both updates `ToolState.Active` and visually selects the button. Keys are unused by terrain painting, so both `ToolUi` and `CartographerSpike` `_UnhandledInput` coexist (disjoint event types).

- [ ] **Step 4: Compile**

Run: `dotnet build` (from `tool/`)
Expected: `Build succeeded`.

- [ ] **Step 5: Manual verify (user)**

F5. Expected: pressing `1`–`7` switches tools (toolbar highlight follows); `[` / `]` change the radius slider live; paint a River stroke then press Generate Streams → tributaries carve in and thin water appears.

- [ ] **Step 6: Commit**

```bash
git add tool/scripts/ToolUi.cs
git commit -m "DHCE N3a: shortcuts (1-7 tools, [ ] brush size) + Generate Streams"
```

---

## Task 7: Roadmap status + final full-pass verification

**Files:**
- Modify: `docs/plans/dhce-tool-roadmap.md` (mark N3a done with live findings)
- Modify: `docs/plans/n3a-tool-shell.md` (this file — check all boxes)

- [ ] **Step 1: Full manual pass (user)**

F5 and exercise the whole shell in one sitting:
- Each tool (Raise/Carve/Level/Crest/River/Flood/Biome) paints; sliders + biome/liquid pickers work.
- World: live Height; Regenerate with a new Seed/Size/Spacing.
- Physics: Sea level, Rain, Settle, Simulate, Clear.
- Shortcuts 1–7 and `[` `]`. UI clicks never paint terrain.
- Note region/tri counts + ms/dab from the status line at 20 km.

- [ ] **Step 2: Update the roadmap**

Add an `## N3a — tool shell ✅ DONE` entry under the N3 area summarizing what landed (tools wired, world+physics panels, liquid rendering, shortcuts) + the live region count / ms/dab observed in Step 1, mirroring the N2 write-up style.

- [ ] **Step 3: Commit**

```bash
git add docs/plans/dhce-tool-roadmap.md docs/plans/n3a-tool-shell.md
git commit -m "DHCE N3a: mark done in roadmap + plan (verified live)"
```

- [ ] **Step 4: Push**

```bash
git push
```

---

## Self-review (done while writing)

- **Spec coverage** (design §3 N3a): brush tools (Raise/Carve/Level/Crest/River/Flood/Biome) → Tasks 1, 3, 6; brush radius/strength + contextual (biome/liquid) → Task 3; World panel + Regenerate + live height → Task 4; Physics panel (sea level/rain/flow/evap/substeps/Settle/Clear/Simulate) → Task 5; liquid rendering → Task 2; HUD/status line → Tasks 3, 7; shortcuts (1–7, `[`/`]`) → Task 6; Generate Streams → Task 6; CartographerSpike split → Tasks 1–4. All §3 N3a items mapped.
- **Open items resolved:** `paint_biome`/`paint_course`/`generate_streams` mark chunks dirty via `after_edit` (verified in `world.rs`), so `RepaintDirtyTerrain` covers recolour/river/streams; `paint_liquid` does not touch terrain chunks, so Flood uses `RebuildLiquid` (whole-surface). Liquid stays whole-surface for N3a (chunked liquid deferred).
- **Type consistency:** `RebuildLiquid` / `RepaintDirtyTerrain` / `Regenerate` / `SetTerrainHeight` / `SetStatus` / `Engine` / `Exaggeration` / `Tool` names are used identically across `CartographerSpike` and `ToolUi`. `EditResult` flags match `ToolState.Apply` returns. `Tool` enum order (Raise…Biome) matches shortcut indices 1–7 and the toolbar button order.
- **No placeholders:** the only stub (`RebuildLiquid` in Task 1 Step 5) is explicitly replaced in Task 2 Step 4.
