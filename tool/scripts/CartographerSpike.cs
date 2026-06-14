using Godot;
using System.Collections.Generic;
using System.Diagnostics;

namespace DesolateHaven.Cartography;

/// N2 streaming tool shell: a large world (default 20 km) opens *instantly* and edits stay
/// responsive. Three pieces deliver that (ADR 0003 render-streaming):
///   1. Splash-deferred gen — the window shows a "Generating…" splash immediately; `build()`
///      runs a few frames later so the splash is visible during it. (Generation runs on the
///      MAIN thread: gdext methods can't be called from a background thread — Godot rejects
///      the cross-thread `Call` with error #1337 — so the C#-`Task` approach is not viable.)
///   2. Progressive tessellation — chunk `MeshInstance3D`s are created empty, then a few are
///      tessellated per frame in `_Process` (no upload hitch; the window fills in).
///   3. Render distance — only chunks within `RenderDistance` tiles of the camera focus are
///      meshed; far ones are freed. The core's `chunk_grid` maps the camera to tiles with no
///      per-region work.
///
/// Units: **1 Godot unit = 1 m**, which is also Godot's own convention (physics/lighting/audio
/// are metre-tuned). Author-facing sizes are in km where the scale warrants it (world, relief)
/// and m for fine-scale tools (spacing, brush); all are converted to metres before crossing
/// into the Rust core, whose `build(width, height, …)` takes metres.
///
/// All compute is in Rust (the `dhce-godot` GDExtension); C# only uploads buffers and runs the
/// camera. The engine is a GDExtension class, so it is driven via `Call(...)`.
[GlobalClass]
public partial class CartographerSpike : Node3D
{
    /// World size in km (square map): 20 ⇒ a 20 km × 20 km world. Cost scales with AREA —
    /// doubling this is ~4× the regions (and ~4× the gen time).
    [Export] public float WorldSizeKm = 20f;
    [Export] public float SpacingM = 12f;      // metres between regions; smaller ⇒ denser (~1.7M @ 20 km)
    [Export] public int Seed = 12345;
    [Export] public int Octaves = 6;
    /// Vertical relief in km. The core clamps normalized elevation to ~[-1.5, 1.5] (span
    /// `ElevSpan`), so the on-screen exaggeration is `TerrainHeightKm * 1000 / ElevSpan`
    /// (0.9 km ⇒ 300, the N0 baseline look).
    [Export] public float TerrainHeightKm = 1.2f;
    [Export] public float BrushRadiusM = 350f; // brush footprint radius, metres
    [Export] public float BrushStrength = 0.06f;

    /// Tiles (Chebyshev) around the camera focus kept meshed; chunks beyond this are freed.
    [Export] public int RenderDistance = 6;
    /// Chunks tessellated per frame while streaming in (caps the per-frame upload cost; at
    /// ~3-4 ms/chunk, 4 keeps inside a 60 fps frame while still filling the view in well under 1 s).
    [Export] public int ChunksPerFrame = 4;

    /// Normalized-elevation span the core clamps to (ELEV_MAX - ELEV_MIN in world.rs).
    private const float ElevSpan = 3.0f;
    /// Frames to let the splash render before the (blocking) main-thread gen runs.
    private const int WarmupFrames = 3;

    private GodotObject _engine;
    private OrbitCamera _cam;
    private CanvasLayer _splash;

    // Chunk render state. Nodes are created once (empty); `_built[i]` tracks whether chunk i
    // currently holds a tessellated surface.
    private MeshInstance3D[] _chunks;
    private ArrayMesh[] _chunkMeshes;
    private bool[] _built;
    private StandardMaterial3D _mat;
    private int _cols, _rows;
    private float _sx, _sy;          // tile size in metres (X, Z)
    private float _exaggeration;
    private float _widthM, _heightM; // world size in metres = WorldSizeKm * 1000 (1 unit = 1 m)
    private bool _pendingGen;        // gen armed in _Ready, fired from _Process after warmup
    private int _warmupFrames;
    private bool _genDone;
    private double _genMs;

    // Reused per-frame scratch for nearest-first streaming (no per-frame allocation).
    private readonly List<(int dist, int ci)> _pending = new();

    private int _dabs;
    private double _totalMs;
    private long _dirtyAccum;

    /// Shared tool state: the UI mutates it, left-drag applies it. Seeded from the brush exports.
    public ToolState Tool { get; } = new ToolState();
    public GodotObject Engine => _engine;
    public float Exaggeration => _exaggeration;

    public override void _Ready()
    {
        _engine = ClassDB.Instantiate("DhceEngine").AsGodotObject();
        if (_engine == null)
        {
            GD.PrintErr("DhceEngine not found — enable the dhce GDExtension (addons/dhce/dhce.gdextension).");
            return;
        }

        GetWindow().Set("mode", 2); // maximize the window (2 = Window.MODE_MAXIMIZED)
        _exaggeration = TerrainHeightKm * 1000f / ElevSpan;
        _widthM = _heightM = WorldSizeKm * 1000f; // km → metres (1 Godot unit = 1 m)
        Tool.RadiusM = BrushRadiusM;
        Tool.Strength = BrushStrength;

        // Scene dressing + camera are mesh-independent, so set them up first — the window is
        // live the instant `_Ready` returns, showing the splash while gen is pending.
        SetupSceneAndCamera();
        ShowSplash("Generating world…");
        _pendingGen = true; // fired from _Process once the splash has drawn (see WarmupFrames)
    }

    public override void _Process(double delta)
    {
        if (_pendingGen)
        {
            // Generation blocks the main thread for a few seconds, so hold it off until the
            // splash has rendered — the window then opens instantly with "Generating…" instead
            // of going blank. (Cross-thread gen is impossible here, see the class header.)
            if (_warmupFrames++ >= WarmupFrames)
            {
                _pendingGen = false;
                GenerateWorld();
            }
            return;
        }

        if (!_genDone) return;

        // Camera focus → grid cell. FocusPoint is where the view ray meets the ground; moving
        // the camera (orbit/pan/freelook) streams the world along.
        Vector3 focus = _cam?.FocusPoint ?? new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f);
        int camGx = Mathf.Clamp((int)(focus.X / _sx), 0, Mathf.Max(_cols - 1, 0));
        int camGy = Mathf.Clamp((int)(focus.Z / _sy), 0, Mathf.Max(_rows - 1, 0));

        _pending.Clear();
        for (int ci = 0; ci < _chunks.Length; ci++)
        {
            int gx = ci % _cols;
            int gy = ci / _cols;
            int dist = Mathf.Max(Mathf.Abs(gx - camGx), Mathf.Abs(gy - camGy));
            if (dist <= RenderDistance)
            {
                if (!_built[ci]) _pending.Add((dist, ci));
            }
            else if (_built[ci])
            {
                FreeChunk(ci); // out of range — release its GPU surface
            }
        }

        // Build nearest-first so the world fills outward from where you're looking.
        if (_pending.Count > 0)
        {
            _pending.Sort((a, b) => a.dist.CompareTo(b.dist));
            int budget = Mathf.Min(ChunksPerFrame, _pending.Count);
            for (int k = 0; k < budget; k++)
                BuildChunk(_pending[k].ci);
        }
    }

    /// Build the world on the main thread, then arm streaming. Heavy (~seconds for a 20 km
    /// world); the splash covers it. Mesh upload is lazy — `OnGenDone` only creates empty nodes
    /// and `_Process` tessellates them around the camera.
    private void GenerateWorld()
    {
        var sw = Stopwatch.StartNew();
        _engine.Call("build", _widthM, _heightM, SpacingM, (float)Seed, Octaves);
        sw.Stop();
        _genMs = sw.Elapsed.TotalMilliseconds;
        OnGenDone();
    }

    private void OnGenDone()
    {
        Vector2I grid = _engine.Call("chunk_grid").As<Vector2I>();
        _cols = grid.X;
        _rows = grid.Y;
        _sx = _cols > 0 ? _widthM / _cols : _widthM;
        _sy = _rows > 0 ? _heightM / _rows : _heightM;

        int n = _engine.Call("chunk_count").As<int>();
        GD.Print($"[DHCE] regions={_engine.Call("region_count")} triangles={_engine.Call("triangle_count")} chunks={n} grid={_cols}x{_rows}");
        GD.Print($"[DHCE] gen {_genMs:0} ms; streaming {ChunksPerFrame} chunks/frame within {RenderDistance} tiles");

        _chunks = new MeshInstance3D[n];
        _chunkMeshes = new ArrayMesh[n];
        _built = new bool[n];
        for (int i = 0; i < n; i++)
        {
            var am = new ArrayMesh();
            var mi = new MeshInstance3D { Mesh = am, MaterialOverride = _mat };
            AddChild(mi);
            _chunks[i] = mi;
            _chunkMeshes[i] = am;
        }

        HideSplash();
        _genDone = true;
    }

    private void SetupSceneAndCamera()
    {
        _mat = new StandardMaterial3D
        {
            VertexColorUseAsAlbedo = true,
            Roughness = 1.0f,
            Metallic = 0.0f,
        };
        // Double-sided: the Y-up remap flips triangle winding, so backface culling hides the
        // terrain top-down. (cull_mode 2 = CULL_DISABLED, set by id to dodge enum-name risk.)
        _mat.Set("cull_mode", 2);

        // Flat ambient fill + a distinct dark background so the terrain reads against it.
        // (background_mode 1 = COLOR, ambient_light_source 2 = COLOR.)
        var env = new Godot.Environment();
        env.Set("background_mode", 1);
        env.Set("background_color", new Color(0.10f, 0.12f, 0.16f));
        env.Set("ambient_light_source", 2);
        env.Set("ambient_light_color", new Color(0.70f, 0.75f, 0.82f));
        env.Set("ambient_light_energy", 1.2f);
        AddChild(new WorldEnvironment { Environment = env });

        AddSun(new Vector3(-50, -40, 0), 0.9f);
        AddSun(new Vector3(-85, 20, 0), 0.4f);

        _cam = new OrbitCamera();
        AddChild(_cam);
        _cam.FrameOverhead(new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f), Mathf.Max(_widthM, _heightM) * 0.9f);
        _cam.Current = true;
    }

    private void AddSun(Vector3 rotationDegrees, float energy)
    {
        AddChild(new DirectionalLight3D { RotationDegrees = rotationDegrees, LightEnergy = energy });
    }

    /// A full-screen splash shown while the world generates, so the window is never blank.
    private void ShowSplash(string text)
    {
        _splash = new CanvasLayer();
        var bg = new ColorRect { Color = new Color(0.06f, 0.07f, 0.09f) };
        bg.SetAnchorsAndOffsetsPreset(Control.LayoutPreset.FullRect);
        var label = new Label
        {
            Text = text,
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
        };
        label.SetAnchorsAndOffsetsPreset(Control.LayoutPreset.FullRect);
        _splash.AddChild(bg);
        _splash.AddChild(label);
        AddChild(_splash);
    }

    private void HideSplash()
    {
        _splash?.QueueFree();
        _splash = null;
    }

    /// Re-pack one chunk in Rust and upload its ArrayMesh. Cost ∝ chunk size, not the whole
    /// mesh — what makes high-density sculpting (and streaming) interactive.
    private void BuildChunk(int i)
    {
        _engine.Call("tessellate_chunk", i, _exaggeration);
        ArrayMesh am = _chunkMeshes[i];
        am.ClearSurfaces();
        var positions = _engine.Call("chunk_positions").As<Vector3[]>();
        if (positions.Length == 0) { _built[i] = true; return; } // empty tile — nothing to draw
        var normals = _engine.Call("chunk_normals").As<Vector3[]>();
        var colors = _engine.Call("chunk_colors").As<Color[]>();
        var indices = _engine.Call("chunk_indices").As<int[]>();

        var arrays = new Godot.Collections.Array();
        arrays.Resize((int)Mesh.ArrayType.Max);
        arrays[(int)Mesh.ArrayType.Vertex] = positions;
        arrays[(int)Mesh.ArrayType.Normal] = normals;
        arrays[(int)Mesh.ArrayType.Color] = colors;
        arrays[(int)Mesh.ArrayType.Index] = indices;
        am.AddSurfaceFromArrays(Mesh.PrimitiveType.Triangles, arrays);
        _built[i] = true;
    }

    /// Release a chunk's GPU surface when it leaves render distance (the node stays; it just
    /// goes empty until the chunk comes back into range and re-tessellates).
    private void FreeChunk(int i)
    {
        _chunkMeshes[i].ClearSurfaces();
        _built[i] = false;
    }

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

    /// Re-tessellate the whole liquid surface and re-upload it. Implemented in Task 2.
    public void RebuildLiquid() { }

    public override void _UnhandledInput(InputEvent e)
    {
        bool down = e is InputEventMouseButton { ButtonIndex: MouseButton.Left, Pressed: true };
        bool drag = e is InputEventMouseMotion mm && (mm.ButtonMask & MouseButtonMask.Left) != 0;
        if (down) PaintAt(((InputEventMouseButton)e).Position);
        else if (drag) PaintAt(((InputEventMouseMotion)e).Position);
    }

    private void PaintAt(Vector2 screen)
    {
        if (!_genDone) return;
        var cam = GetViewport().GetCamera3D();
        if (cam == null) return;
        Vector3 from = cam.ProjectRayOrigin(screen);
        Vector3 dir = cam.ProjectRayNormal(screen);
        if (Mathf.IsZeroApprox(dir.Y)) return;
        float t = -from.Y / dir.Y;            // intersect the ground plane Y = 0
        if (t < 0f) return;
        Vector3 hit = from + dir * t;         // core (x, y) = (hit.X, hit.Z)

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
    }
}
