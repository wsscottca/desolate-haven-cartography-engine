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
    [Export] public float TerrainHeightKm = 2.4f;
    [Export] public float BrushRadiusM = 350f; // brush footprint radius, metres
    [Export] public float BrushStrength = 0.06f;

    /// Tiles (Chebyshev) around the camera focus kept meshed; chunks beyond this are freed.
    [Export] public int RenderDistance = 3;
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
    private ToolUi _ui;

    // Chunk render state. Nodes are created once (empty); `_built[i]` tracks whether chunk i
    // currently holds a tessellated surface.
    private MeshInstance3D[] _chunks;
    private ArrayMesh[] _chunkMeshes;
    private bool[] _built;
    private StandardMaterial3D _mat;
    private StandardMaterial3D _dataMat; // unshaded material for data views (colours are the raw field)
    private int _viewMode;               // 0 Natural; see SetViewMode / DhceEngine view_mode
    private MeshInstance3D[] _liquidChunks;     // per-chunk liquid, streamed with the terrain chunks
    private ArrayMesh[] _liquidChunkMeshes;
    private StandardMaterial3D _liquidMat;
    private static readonly Color WaterColor = new Color(0.20f, 0.45f, 0.75f, 0.6f);
    private static readonly Color LavaColor = new Color(0.95f, 0.35f, 0.10f, 0.9f);
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
    public Vector3 CameraFocus => _cam?.FocusPoint ?? new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f);
    public float WorldWidthM => _widthM;
    public float WorldHeightM => _heightM;
    public Vector2 SunMapDir => _sun?.MapDir ?? new Vector2(0.7f, 0.7f);
    public void SetSunBrightness(float v) => _sun?.SetBrightness(v);
    public void SetSunWarmth(float v) => _sun?.SetWarmth(v);
    public void SetSunHue(float v) => _sun?.SetHue(v);

    /// Recenter the camera over a ground point (overview look-down) at a sensible altitude — the
    /// minimap calls this on click to fly there.
    public void FlyTo(float worldX, float worldZ)
    {
        if (_cam == null) return;
        float dist = Mathf.Clamp(_cam.GlobalPosition.Y, _widthM * 0.04f, _widthM);
        _cam.FrameOverhead(new Vector3(worldX, 0f, worldZ), dist);
    }

    // Brush stroke state: spacing (don't pile dabs on one spot) + liquid settle on release.
    private const float BrushSpacingFrac = 0.25f;
    private const int SettleSubsteps = 16;
    private const float MinBrushM = 15f;
    private const float MaxBrushM = 6000f;
    private Vector3 _lastPaintPos;
    private bool _hasLastPaint;
    private bool _painting;
    private bool _strokeTouchedLiquid;
    private bool _minimapDirty;
    private int _minimapTick;

    // Brush preview gizmo (ring + translucent disc; green additive / red subtractive).
    private Node3D _brush;
    private MeshInstance3D _brushFill, _brushRim;
    private StandardMaterial3D _brushFillMat, _brushRimMat;
    private WorldSun _sun;
    private bool _sunDragging;

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
        Tool.StrengthM = BrushStrength * _exaggeration; // export is normalized; the tool works in metres

        // Scene dressing + camera are mesh-independent, so set them up first — the window is
        // live the instant `_Ready` returns, showing the splash while gen is pending.
        SetupSceneAndCamera();
        _ui = new ToolUi { Root = this };
        AddChild(_ui);
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

        // A stroke released over a UI panel won't reach _UnhandledInput — settle here too.
        if (_painting && !Input.IsMouseButtonPressed(MouseButton.Left)) { _painting = false; EndStroke(); }

        // Repaint the minimap shortly after the world changes (debounced; never mid-stroke).
        if (_minimapDirty && !_painting && ++_minimapTick >= 12) { _minimapTick = 0; _minimapDirty = false; _ui?.RefreshMinimap(); }

        UpdateBrushGizmo();
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
        _liquidChunks = new MeshInstance3D[n];
        _liquidChunkMeshes = new ArrayMesh[n];
        _built = new bool[n];
        for (int i = 0; i < n; i++)
        {
            var am = new ArrayMesh();
            var mi = new MeshInstance3D { Mesh = am, MaterialOverride = CurrentViewMat() };
            AddChild(mi);
            _chunks[i] = mi;
            _chunkMeshes[i] = am;

            // Liquid chunk node, parallel to the terrain chunk; built/freed alongside it.
            var lam = new ArrayMesh();
            var lmi = new MeshInstance3D { Mesh = lam, MaterialOverride = _liquidMat };
            AddChild(lmi);
            _liquidChunks[i] = lmi;
            _liquidChunkMeshes[i] = lam;
        }

        HideSplash();
        _genDone = true;
        _ui?.RefreshMinimap();
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

        // Data-view material: unshaded, so the heatmap colours read as the raw field (no sun
        // shading muddying the readout). Same double-sided culling as the terrain material.
        _dataMat = new StandardMaterial3D { VertexColorUseAsAlbedo = true };
        _dataMat.Set("shading_mode", 0); // SHADING_MODE_UNSHADED
        _dataMat.Set("cull_mode", 2);    // CULL_DISABLED

        _liquidMat = new StandardMaterial3D
        {
            VertexColorUseAsAlbedo = true,
            Roughness = 0.1f,
            Metallic = 0.0f,
        };
        _liquidMat.Set("transparency", 1);  // BaseMaterial3D.Transparency.Alpha
        _liquidMat.Set("cull_mode", 2);     // CULL_DISABLED (same Y-up winding flip as terrain)

        // Flat ambient fill + a distinct dark background so the terrain reads against it.
        // (background_mode 1 = COLOR, ambient_light_source 2 = COLOR.)
        var env = new Godot.Environment();
        env.Set("background_mode", 1);
        env.Set("background_color", new Color(0.10f, 0.12f, 0.16f));
        env.Set("ambient_light_source", 2);
        env.Set("ambient_light_color", new Color(0.70f, 0.75f, 0.82f));
        env.Set("ambient_light_energy", 1.2f);
        AddChild(new WorldEnvironment { Environment = env });

        _sun = new WorldSun();
        AddChild(_sun);
        _sun.Configure(new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f), Mathf.Max(_widthM, _heightM));
        AddSun(new Vector3(-85, 20, 0), 0.35f); // dim fixed fill so shadowed faces aren't pure black

        _cam = new OrbitCamera();
        AddChild(_cam);
        _cam.FrameOverhead(new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f), Mathf.Max(_widthM, _heightM) * 0.9f);
        _cam.Current = true;

        BuildBrushGizmo();
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
        BuildLiquidChunk(i); // liquid streams with its terrain chunk
        _built[i] = true;
    }

    /// Re-pack one chunk's liquid sub-mesh (only its wet triangles). Empty ⇒ cleared (dry chunk).
    /// The core caches the smoothed surface once per liquid change; this only marshals + uploads.
    private void BuildLiquidChunk(int i)
    {
        if (_liquidChunkMeshes == null) return;
        _engine.Call("tessellate_liquid_chunk", i, (double)_exaggeration);
        ArrayMesh am = _liquidChunkMeshes[i];
        am.ClearSurfaces();
        var positions = _engine.Call("liquid_chunk_positions").As<Vector3[]>();
        if (positions.Length == 0) return; // no water in this chunk — leave it cleared
        var normals = _engine.Call("liquid_chunk_normals").As<Vector3[]>();
        var types = _engine.Call("liquid_chunk_types").As<float[]>();
        var indices = _engine.Call("liquid_chunk_indices").As<int[]>();

        var colors = new Color[positions.Length];
        for (int k = 0; k < positions.Length; k++)
            colors[k] = (k < types.Length && types[k] > 0.5f) ? LavaColor : WaterColor;

        var arrays = new Godot.Collections.Array();
        arrays.Resize((int)Mesh.ArrayType.Max);
        arrays[(int)Mesh.ArrayType.Vertex] = positions;
        arrays[(int)Mesh.ArrayType.Normal] = normals;
        arrays[(int)Mesh.ArrayType.Color] = colors;
        arrays[(int)Mesh.ArrayType.Index] = indices;
        am.AddSurfaceFromArrays(Mesh.PrimitiveType.Triangles, arrays);
    }

    /// Release a chunk's GPU surfaces (terrain + liquid) when it leaves render distance (the nodes
    /// stay; they just go empty until the chunk comes back into range and re-tessellates).
    private void FreeChunk(int i)
    {
        _chunkMeshes[i].ClearSurfaces();
        _liquidChunkMeshes[i].ClearSurfaces();
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

    /// Switch the colour view (0 Natural, 1 Temperature, 2 Moisture, 3 Elevation, 4 Biome). Data
    /// views recolour the same meshes by one field (a heatmap) via an unshaded material, so the
    /// colours are the raw field. The core recolours + flags all chunks dirty; we swap the material
    /// on every chunk node and re-tessellate the meshed ones (out-of-range chunks pick it up when
    /// they next stream in). The minimap reads the same cache, so it follows too.
    public void SetViewMode(int mode)
    {
        _viewMode = mode;
        _engine.Call("set_view_mode", mode);
        var m = CurrentViewMat();
        if (_chunks != null) foreach (var mi in _chunks) if (mi != null) mi.MaterialOverride = m;
        if (_genDone) RepaintDirtyTerrain();
        _ui?.RefreshMinimap();
    }

    private StandardMaterial3D CurrentViewMat() => _viewMode == 0 ? _mat : _dataMat;

    /// Re-tessellate only the liquid chunks the last edit/sim changed that are currently in render
    /// range (out-of-range dirty chunks re-tessellate when they next stream in). Replaces the old
    /// whole-surface rebuild + full re-upload that ran every tick — the slow path.
    public void RebuildLiquid()
    {
        if (_liquidChunkMeshes == null) return;
        _minimapDirty = true; // liquid changed → repaint the map
        int[] dirty = _engine.Call("take_dirty_liquid_chunks").As<int[]>();
        foreach (int ci in dirty)
            if (ci >= 0 && ci < _liquidChunkMeshes.Length && _built[ci]) BuildLiquidChunk(ci);
    }

    /// Rebuild the world from the current Seed/Octaves/WorldSizeKm/SpacingM (set by the UI).
    /// Discards edits; reuses the splash-deferred main-thread gen path (_Process warmup).
    public void Regenerate()
    {
        if (_chunks != null) foreach (var mi in _chunks) mi?.QueueFree();
        if (_liquidChunks != null) foreach (var mi in _liquidChunks) mi?.QueueFree();
        _chunks = null; _chunkMeshes = null; _built = null;
        _liquidChunks = null; _liquidChunkMeshes = null;
        _genDone = false;

        _widthM = _heightM = WorldSizeKm * 1000f;
        _exaggeration = TerrainHeightKm * 1000f / ElevSpan;
        _cam?.FrameOverhead(new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f), Mathf.Max(_widthM, _heightM) * 0.9f);

        ShowSplash("Regenerating world…");
        _warmupFrames = 0;
        _pendingGen = true;
    }

    /// Live vertical-relief change (no rebuild): recompute exaggeration, re-tessellate every meshed
    /// chunk + the liquid. `km` is world-height in km.
    public void SetTerrainHeight(float km)
    {
        TerrainHeightKm = km;
        _exaggeration = km * 1000f / ElevSpan;
        if (!_genDone) return;
        for (int i = 0; i < _built.Length; i++) if (_built[i]) BuildChunk(i);
        RebuildLiquid();
    }

    // --- brush preview gizmo (ring + translucent disc on the terrain under the cursor) ---

    private void BuildBrushGizmo()
    {
        _brush = new Node3D { Visible = false };
        AddChild(_brush);
        _brushFillMat = UnshadedMat(new Color(0.2f, 1f, 0.3f, 0.18f));
        _brushRimMat = UnshadedMat(new Color(0.2f, 1f, 0.3f, 0.85f));
        _brushFill = new MeshInstance3D { Mesh = MakeDisc(1f, 48), MaterialOverride = _brushFillMat };
        _brushRim = new MeshInstance3D { Mesh = MakeRing(0.93f, 1f, 64), MaterialOverride = _brushRimMat };
        _brush.AddChild(_brushFill);
        _brush.AddChild(_brushRim);
    }

    /// Position/scale/colour the brush gizmo under the cursor each frame: green = additive
    /// (raise/crest), red = subtractive (carve), blue = water, yellow = level, neutral = biome.
    private void UpdateBrushGizmo()
    {
        if (_brush == null) return;
        if (!TerrainHit(GetViewport().GetMousePosition(), out Vector3 p, out Vector3 from)) { _brush.Visible = false; return; }
        Tool.RadiusM = EffectiveRadius(from, p);
        _brush.Visible = true;
        _brush.GlobalPosition = p + new Vector3(0f, _exaggeration * 0.003f, 0f); // lift to dodge z-fight
        _brush.Scale = new Vector3(Tool.RadiusM, 1f, Tool.RadiusM);
        Color c = BrushColor(Tool.Active, Tool.LiquidKind);
        _brushFillMat.AlbedoColor = new Color(c.R, c.G, c.B, 0.18f);
        _brushRimMat.AlbedoColor = new Color(c.R, c.G, c.B, 0.85f);
    }

    private static Color BrushColor(ToolKind active, int liquidKind) => active switch
    {
        ToolKind.Raise or ToolKind.Crest => new Color(0.2f, 1f, 0.3f), // additive — green
        ToolKind.Carve => new Color(1f, 0.25f, 0.2f),                  // subtractive — red
        ToolKind.Level => new Color(0.85f, 0.85f, 0.3f),              // neutral — yellow
        ToolKind.River => new Color(0.3f, 0.6f, 1f),                  // water — blue
        ToolKind.Flood => liquidKind == 1 ? new Color(1f, 0.5f, 0.15f) : new Color(0.3f, 0.6f, 1f),
        _ => new Color(0.9f, 0.9f, 0.95f),                            // biome — neutral
    };

    private static StandardMaterial3D UnshadedMat(Color c)
    {
        var m = new StandardMaterial3D { AlbedoColor = c };
        m.Set("shading_mode", 0);      // SHADING_MODE_UNSHADED
        m.Set("transparency", 1);      // ALPHA
        m.Set("cull_mode", 2);         // CULL_DISABLED
        m.Set("no_depth_test", true);  // draw on top so the brush is always visible
        return m;
    }

    private static ArrayMesh MakeDisc(float radius, int seg)
    {
        var verts = new Vector3[seg + 2];
        verts[0] = Vector3.Zero;
        for (int i = 0; i <= seg; i++)
        {
            float a = Mathf.Tau * i / seg;
            verts[i + 1] = new Vector3(Mathf.Cos(a) * radius, 0f, Mathf.Sin(a) * radius);
        }
        var idx = new int[seg * 3];
        for (int i = 0; i < seg; i++) { idx[i * 3] = 0; idx[i * 3 + 1] = i + 1; idx[i * 3 + 2] = i + 2; }
        return MeshFrom(verts, idx);
    }

    private static ArrayMesh MakeRing(float inner, float outer, int seg)
    {
        var verts = new Vector3[(seg + 1) * 2];
        for (int i = 0; i <= seg; i++)
        {
            float a = Mathf.Tau * i / seg;
            float cs = Mathf.Cos(a), sn = Mathf.Sin(a);
            verts[i * 2] = new Vector3(cs * inner, 0f, sn * inner);
            verts[i * 2 + 1] = new Vector3(cs * outer, 0f, sn * outer);
        }
        var idx = new int[seg * 6];
        for (int i = 0; i < seg; i++)
        {
            int a = i * 2, b = i * 2 + 1, c = i * 2 + 2, d = i * 2 + 3;
            idx[i * 6] = a; idx[i * 6 + 1] = b; idx[i * 6 + 2] = c;
            idx[i * 6 + 3] = b; idx[i * 6 + 4] = d; idx[i * 6 + 5] = c;
        }
        return MeshFrom(verts, idx);
    }

    private static ArrayMesh MeshFrom(Vector3[] verts, int[] idx)
    {
        var arr = new Godot.Collections.Array();
        arr.Resize((int)Mesh.ArrayType.Max);
        arr[(int)Mesh.ArrayType.Vertex] = verts;
        arr[(int)Mesh.ArrayType.Index] = idx;
        var m = new ArrayMesh();
        m.AddSurfaceFromArrays(Mesh.PrimitiveType.Triangles, arr);
        return m;
    }

    public override void _UnhandledInput(InputEvent e)
    {
        if (e is InputEventMouseButton mb && mb.ButtonIndex == MouseButton.Left)
        {
            if (mb.Pressed)
            {
                if (SunPick(mb.Position)) { _sunDragging = true; return; } // grabbed the sun — don't paint
                _painting = true; _strokeTouchedLiquid = false; _hasLastPaint = false; PaintAt(mb.Position);
            }
            else
            {
                _sunDragging = false;
                if (_painting) { _painting = false; EndStroke(); }
            }
            return;
        }
        if (e is InputEventMouseMotion mm && (mm.ButtonMask & MouseButtonMask.Left) != 0)
        {
            if (_sunDragging) DragSun(mm.Position);
            else PaintAt(mm.Position);
        }
    }

    /// True if `screen` is over the sun sphere (screen-space pick) — grabs it for dragging.
    private bool SunPick(Vector2 screen)
    {
        if (_sun == null) return false;
        var cam = GetViewport().GetCamera3D();
        if (cam == null) return false;
        Vector3 sp = _sun.SunWorldPos;
        if (cam.IsPositionBehind(sp)) return false;
        return screen.DistanceTo(cam.UnprojectPosition(sp)) < 55f;
    }

    /// Move the sun to where the cursor points on the sky dome (a sphere of the sun's radius about
    /// the world centre), then refresh the minimap shading.
    private void DragSun(Vector2 screen)
    {
        if (_sun == null) return;
        var cam = GetViewport().GetCamera3D();
        if (cam == null) return;
        Vector3 o = cam.ProjectRayOrigin(screen);
        Vector3 d = cam.ProjectRayNormal(screen);
        Vector3 c = _sun.Center;
        float r = _sun.Radius;
        Vector3 oc = o - c;
        float b = oc.Dot(d);
        float disc = b * b - (oc.Dot(oc) - r * r);
        if (disc < 0f) return;
        float t = -b + Mathf.Sqrt(disc); // exit point (camera sits inside the sky sphere)
        if (t <= 0f) return;
        Vector3 dir = (o + d * t - c).Normalized();
        if (dir.Y < 0.05f) dir.Y = 0.05f; // keep the sun above the horizon
        _sun.SetDirToSun(dir.Normalized());
        _ui?.RefreshMinimap();
    }

    private void PaintAt(Vector2 screen)
    {
        if (!_genDone) return;
        if (!TerrainHit(screen, out Vector3 hit, out Vector3 from)) return; // cursor not over terrain
        // core (x, y) = (hit.X, hit.Z)
        Tool.RadiusM = EffectiveRadius(from, hit); // brush radius scales with how far you're zoomed

        // Brush spacing: skip dabs that haven't moved far enough, so a slow drag doesn't pile
        // many dabs on one spot (which produced spikes / water pillars).
        if (_hasLastPaint && hit.DistanceTo(_lastPaintPos) < Tool.RadiusM * BrushSpacingFrac) return;
        _lastPaintPos = hit;
        _hasLastPaint = true;

        ulong t0 = Time.GetTicksUsec();
        EditResult res = Tool.Apply(_engine, hit, _exaggeration);
        int dirtyCount = 0;
        if (res.HasFlag(EditResult.Terrain)) dirtyCount = RepaintDirtyTerrain();
        if (res.HasFlag(EditResult.Liquid)) { _strokeTouchedLiquid = true; RebuildLiquid(); }
        if (res != EditResult.None) _minimapDirty = true; // map repaints shortly after the stroke
        double ms = (Time.GetTicksUsec() - t0) / 1000.0;

        _totalMs += ms;
        _dirtyAccum += dirtyCount;
        if (++_dabs % 30 == 0)
        {
            string line = $"{_dabs} dabs: {_totalMs / _dabs:0.0} ms/dab over {(double)_dirtyAccum / _dabs:0.0} dirty chunks/dab";
            GD.Print($"[DHCE] {line}");
            _ui?.SetStatus(line);
        }
    }

    /// Ray from the camera through `screen` to the terrain surface (via the core's analytic
    /// raycast — lands on the actual surface under the cursor from any view angle, not the Y = 0
    /// plane). Returns false if the cursor isn't over terrain; `from` is the ray origin (camera).
    private bool TerrainHit(Vector2 screen, out Vector3 hit, out Vector3 from)
    {
        hit = Vector3.Zero;
        from = Vector3.Zero;
        var cam = GetViewport().GetCamera3D();
        if (cam == null) return false;
        from = cam.ProjectRayOrigin(screen);
        Vector3 dir = cam.ProjectRayNormal(screen);
        var hits = _engine.Call("raycast_terrain", from, dir, (double)_exaggeration).As<Vector3[]>();
        if (hits.Length == 0) return false;
        hit = hits[0];
        return true;
    }

    /// Brush radius scaled to the zoom: a fraction of the camera→cursor distance, so the brush is
    /// small up close (edit like a person) and large when pulled back (edit like a giant).
    private float EffectiveRadius(Vector3 from, Vector3 hit)
        => Mathf.Clamp(Tool.RadiusFraction * from.DistanceTo(hit), MinBrushM, MaxBrushM);

    /// Settle the fluid a few steps when a liquid stroke ends, so water flows downhill and levels
    /// into basins instead of standing in columns where it was poured.
    private void EndStroke()
    {
        if (_strokeTouchedLiquid)
        {
            _strokeTouchedLiquid = false;
            _engine.Call("step_fluid", 0.45, 0.0, SettleSubsteps); // settle poured/river water
        }
        RebuildLiquid(); // reflect drained water (raising land displaces it) or settling
    }
}
