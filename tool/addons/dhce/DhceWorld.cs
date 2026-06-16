using Godot;
using System.Collections.Generic;
using System.Diagnostics;

namespace DesolateHaven.Cartography;

/// In-editor (and in-game) world node: owns the `DhceEngine`, generates the world on demand, and
/// streams chunked terrain + liquid meshes around an externally-supplied focus (the editor camera in
/// the tool; the player camera in the game). `[Tool]` so it runs live in the editor.
///
/// The reshell's R2 core. Generated chunk nodes are added without an `owner`, so they're ephemeral
/// preview — never serialized into the `.tscn`; the world is regenerated from a `DhceWorldState`
/// resource on open (R5). Brush/picking (R3), the tool dock (R4), and persistence (R5) build on this.
[Tool]
[GlobalClass]
public partial class DhceWorld : Node3D
{
    [Export] public float WorldSizeKm = 30f;   // requested size; snapped to a whole number of chunks
    [Export] public float SpacingM = 10f;      // metres between regions (~3.6M @ 30 km / 10 m)
    [Export] public int Seed = 12345;
    [Export] public int Octaves = 6;
    [Export] public float TerrainHeightKm = 5.0f;
    [Export] public float ChunkSizeM = 256f;   // fixed chunk edge (m); smaller ⇒ finer, lighter streaming
    [Export] public int RenderDistance = 8;    // chunk tiles (Chebyshev) kept meshed around the focus
    [Export] public int ChunksPerFrame = 8;    // chunks tessellated per streaming tick
    [Export] public int LodDistance = 3;       // chunks within this (tiles) render full TIN; beyond → coarse LOD
    [Export] public int LodGridN = 8;          // coarse LOD grid resolution per chunk
    [Export] public DhceWorldState State;      // persisted snapshot; regenerated from on open (R5)
    [Export] public DhceScatterLibrary Scatter; // authored scatter model slots + rules (N3d)
    [Export] public Godot.Collections.Array<Vector4> Caves = new(); // volumetric carve spheres: xyz centre + w radius (N5)

    /// Normalized-elevation span the core clamps to (ELEV_MAX − ELEV_MIN in world.rs).
    private const float ElevSpan = 3.0f;

    private GodotObject _engine;
    private MeshInstance3D[] _chunks;
    private ArrayMesh[] _chunkMeshes;
    private MeshInstance3D[] _liquidChunks;
    private ArrayMesh[] _liquidChunkMeshes;
    private bool[] _built;
    private int[] _chunkLod; // per built chunk: 0 = full TIN, 1 = coarse LOD
    private StandardMaterial3D _mat, _dataMat, _liquidMat;
    private int _viewMode;
    private int _cols, _rows;
    private float _sx, _sy;             // chunk tile size in metres
    private float _exaggeration;
    private float _widthM, _heightM;
    private bool _genDone;
    private readonly List<(int dist, int ci, bool lod)> _pending = new();
    private readonly List<Node> _scatterPreview = new(); // ephemeral N3d scatter preview nodes
    private MeshInstance3D _cavePreview;        // ephemeral N5 carved-cave preview
    private StandardMaterial3D _caveMat;
    private Node3D _renderRoot;                 // ephemeral parent for all preview geometry, offset so
                                                // the world centres on the origin (the editor camera's focus)

    private static readonly Color WaterColor = new Color(0.20f, 0.45f, 0.75f, 0.6f);
    private static readonly Color LavaColor = new Color(0.95f, 0.35f, 0.10f, 0.9f);

    public GodotObject Engine => _engine;
    public float Exaggeration => _exaggeration;
    public bool GenDone => _genDone;
    public float WorldWidthM => _widthM;
    public float WorldHeightM => _heightM;
    public Vector3 WorldCenter => new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f);

    /// The ephemeral node all preview geometry hangs under. It's shifted by `-WorldCenter` so the
    /// world's middle sits on the scene origin (where the editor camera looks by default); core
    /// generation/picking stays in `[0, size]` space. Picking/focus code converts via `ToCore`/`ToEditor`.
    public Node3D RenderRoot => _renderRoot;

    /// Editor/global-space point → core (generation) space, `[0, size]`. Identity before Generate.
    public Vector3 ToCore(Vector3 editorPoint) => _renderRoot != null ? _renderRoot.ToLocal(editorPoint) : editorPoint;
    /// Core-space point → editor/global space (the centred preview). Identity before Generate.
    public Vector3 ToEditor(Vector3 corePoint) => _renderRoot != null ? _renderRoot.ToGlobal(corePoint) : corePoint;

    private void EnsureRenderRoot()
    {
        if (_renderRoot != null && GodotObject.IsInstanceValid(_renderRoot)) return;
        _renderRoot = new Node3D { Name = "DhceRender" };
        AddChild(_renderRoot); // owner left null → ephemeral, never serialized into the .tscn
    }

    private void EnsureEngine()
    {
        if (_engine != null) return;
        _engine = ClassDB.Instantiate("DhceEngine").AsGodotObject();
        if (_engine == null)
            GD.PrintErr("DhceEngine not found — enable the dhce GDExtension (addons/dhce/dhce.gdextension).");
        EnsureMaterials();
    }

    private void EnsureMaterials()
    {
        if (_mat != null) return;
        _mat = new StandardMaterial3D { VertexColorUseAsAlbedo = true, Roughness = 1.0f, Metallic = 0.0f };
        _mat.Set("cull_mode", 2); // CULL_DISABLED (Y-up remap flips winding)
        _dataMat = new StandardMaterial3D { VertexColorUseAsAlbedo = true };
        _dataMat.Set("shading_mode", 0); // UNSHADED — data views show the raw field
        _dataMat.Set("cull_mode", 2);
        _liquidMat = new StandardMaterial3D { VertexColorUseAsAlbedo = true, Roughness = 0.1f, Metallic = 0.0f };
        _liquidMat.Set("transparency", 1); // ALPHA
        _liquidMat.Set("cull_mode", 2);
    }

    /// Build the world from the current params (blocks ~seconds at full density) and create empty
    /// chunk nodes; streaming fills them in around the focus. Discards any previous world.
    public void Generate()
    {
        EnsureEngine();
        if (_engine == null) return;
        ClearChunks();
        _exaggeration = TerrainHeightKm * 1000f / ElevSpan;

        // Fixed-size chunks: snap the world to a whole number of ChunkSizeM tiles (so the requested
        // 20 km becomes e.g. 19.968 km at 256 m), giving even tiles and no thin partial edge.
        float chunk = Mathf.Max(ChunkSizeM, 32f);
        int perSide = Mathf.Max(1, Mathf.RoundToInt(WorldSizeKm * 1000f / chunk));
        _widthM = _heightM = perSide * chunk;
        _engine.Call("set_chunk_size_m", (double)chunk);

        // Centre the world on the origin: the core generates in [0, size] (origin at a corner), but the
        // editor camera looks at the origin — so shift the preview by -WorldCenter to put the world's
        // middle under the camera. Without this, Generate succeeds but the terrain sits ~10 km off-screen.
        EnsureRenderRoot();
        _renderRoot.Position = new Vector3(-_widthM * 0.5f, 0f, -_heightM * 0.5f);

        var sw = Stopwatch.StartNew();
        _engine.Call("build", _widthM, _heightM, SpacingM, (float)Seed, Octaves);
        sw.Stop();
        OnGenDone(sw.Elapsed.TotalMilliseconds);
    }

    private void OnGenDone(double genMs)
    {
        Vector2I grid = _engine.Call("chunk_grid").As<Vector2I>();
        _cols = grid.X;
        _rows = grid.Y;
        _sx = _sy = (float)_engine.Call("chunk_size_m").As<double>(); // fixed tile size
        int n = _engine.Call("chunk_count").As<int>();
        GD.Print($"[DHCE] regions={_engine.Call("region_count")} chunks={n} grid={_cols}x{_rows} tile={_sx:0}m gen {genMs:0} ms");

        // Lazy nodes: only the in-range ring is instantiated (in BuildChunk), so the live node count
        // tracks the visible area, not the whole map — a 20 km world at 256 m is ~6 000 tile *slots*
        // but only a few hundred ever exist at once. Arrays hold nulls until a chunk streams in.
        _chunks = new MeshInstance3D[n];
        _chunkMeshes = new ArrayMesh[n];
        _liquidChunks = new MeshInstance3D[n];
        _liquidChunkMeshes = new ArrayMesh[n];
        _built = new bool[n];
        _chunkLod = new int[n];
        _genDone = true;
        UpdateStreaming(WorldCenter); // seed the centre so something shows immediately
    }

    private void ClearChunks()
    {
        PreviewScatter(false); // drop any scatter preview before discarding the world
        PreviewCaves(false);
        StopRain(); // drop any rain cloud/particles before discarding the world
        if (_chunks != null) foreach (var mi in _chunks) mi?.QueueFree();
        if (_liquidChunks != null) foreach (var mi in _liquidChunks) mi?.QueueFree();
        _chunks = null;
        _chunkMeshes = null;
        _liquidChunks = null;
        _liquidChunkMeshes = null;
        _built = null;
        _genDone = false;
    }

    /// Stream chunks around a single focus (seed / game use): all three anchors collapse to it.
    public void UpdateStreaming(Vector3 focus) => UpdateStreaming(focus, focus, focus);

    /// Stream chunks around up to three world-space (core) anchors — the camera's own footprint
    /// (`camPos`), the look focus (`lookFocus`), and the last brush-cursor hit (`brush`). A chunk is
    /// kept if it's in range of *any* anchor and freed only when out of range of *all* of them, so the
    /// existing look ring still streams ahead. The 2×2 block of chunks directly under the camera is
    /// **pinned**: always meshed at full TIN and never freed — so looking up or around the horizon
    /// never unloads the ground beneath you (the look ray would otherwise project to the world edge).
    /// Called each frame by whoever owns the camera (the editor plugin in the tool; the game otherwise).
    public void UpdateStreaming(Vector3 camPos, Vector3 lookFocus, Vector3 brush)
    {
        if (!_genDone || _chunks == null) return;
        int maxGx = Mathf.Max(_cols - 1, 0), maxGy = Mathf.Max(_rows - 1, 0);
        int CellX(float x) => Mathf.Clamp((int)(x / _sx), 0, maxGx);
        int CellY(float z) => Mathf.Clamp((int)(z / _sy), 0, maxGy);
        int camGx = CellX(camPos.X), camGy = CellY(camPos.Z);
        int lookGx = CellX(lookFocus.X), lookGy = CellY(lookFocus.Z);
        int brGx = CellX(brush.X), brGy = CellY(brush.Z);

        // The four chunks whose centres bracket the camera footprint (the nearest 2×2 block); pin them.
        int bx = Mathf.Clamp(Mathf.FloorToInt(camPos.X / _sx - 0.5f), 0, maxGx);
        int by = Mathf.Clamp(Mathf.FloorToInt(camPos.Z / _sy - 0.5f), 0, maxGy);
        int bx2 = Mathf.Min(bx + 1, maxGx), by2 = Mathf.Min(by + 1, maxGy);
        int p0 = by * _cols + bx, p1 = by * _cols + bx2, p2 = by2 * _cols + bx, p3 = by2 * _cols + bx2;

        _pending.Clear();
        for (int ci = 0; ci < _chunks.Length; ci++)
        {
            int gx = ci % _cols;
            int gy = ci / _cols;
            bool pin = ci == p0 || ci == p1 || ci == p2 || ci == p3;
            int dist = Mathf.Min(Mathf.Max(Mathf.Abs(gx - camGx), Mathf.Abs(gy - camGy)),
                       Mathf.Min(Mathf.Max(Mathf.Abs(gx - lookGx), Mathf.Abs(gy - lookGy)),
                                 Mathf.Max(Mathf.Abs(gx - brGx), Mathf.Abs(gy - brGy))));
            if (!pin && dist > RenderDistance) { if (_built[ci]) FreeChunk(ci); continue; }
            bool lod = !pin && dist > LodDistance; // near = full TIN, far = coarse LOD; pinned always full
            // Build if not meshed, or re-mesh when a chunk crosses the LOD threshold as the camera moves.
            if (!_built[ci] || (_chunkLod[ci] == 1) != lod) _pending.Add((pin ? -1 : dist, ci, lod));
        }
        if (_pending.Count > 0)
        {
            _pending.Sort((a, b) => a.dist.CompareTo(b.dist)); // pinned (-1) first, then nearest
            int budget = Mathf.Min(ChunksPerFrame, _pending.Count);
            for (int k = 0; k < budget; k++) BuildChunk(_pending[k].ci, _pending[k].lod);
        }
    }

    private void BuildChunk(int i, bool lod)
    {
        if (_chunks[i] == null) // lazy: instantiate the tile node the first time it streams in
        {
            var m = new ArrayMesh();
            var mi = new MeshInstance3D { Mesh = m, MaterialOverride = CurrentViewMat() };
            _renderRoot.AddChild(mi); // owner left null → ephemeral preview, not serialized into the scene
            _chunks[i] = mi;
            _chunkMeshes[i] = m;
        }
        if (lod) _engine.Call("tessellate_chunk_lod", i, _exaggeration, LodGridN);
        else _engine.Call("tessellate_chunk", i, _exaggeration);
        _chunkLod[i] = lod ? 1 : 0;
        ArrayMesh am = _chunkMeshes[i];
        am.ClearSurfaces();
        var positions = _engine.Call("chunk_positions").As<Vector3[]>();
        if (positions.Length == 0) { _built[i] = true; return; } // empty tile
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
        if (lod) _liquidChunkMeshes[i]?.ClearSurfaces(); // far chunks skip the (expensive) liquid surface
        else BuildLiquidChunk(i);
        _built[i] = true;
    }

    private void BuildLiquidChunk(int i)
    {
        if (_liquidChunkMeshes == null) return;
        _engine.Call("tessellate_liquid_chunk", i, (double)_exaggeration);
        var positions = _engine.Call("liquid_chunk_positions").As<Vector3[]>();
        if (positions.Length == 0) { _liquidChunkMeshes[i]?.ClearSurfaces(); return; } // dry — clear any prior water
        if (_liquidChunks[i] == null) // lazy: only wet tiles get a water node
        {
            var lam = new ArrayMesh();
            var lmi = new MeshInstance3D { Mesh = lam, MaterialOverride = _liquidMat };
            _renderRoot.AddChild(lmi);
            _liquidChunks[i] = lmi;
            _liquidChunkMeshes[i] = lam;
        }
        ArrayMesh am = _liquidChunkMeshes[i];
        am.ClearSurfaces();
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

    private void FreeChunk(int i)
    {
        _chunks[i]?.QueueFree();
        _chunks[i] = null;
        _chunkMeshes[i] = null;
        _liquidChunks[i]?.QueueFree();
        _liquidChunks[i] = null;
        _liquidChunkMeshes[i] = null;
        _built[i] = false;
    }

    /// Re-tessellate the meshed chunks the last edit dirtied. Returns the dirty count.
    public int RepaintDirtyTerrain()
    {
        if (_chunkMeshes == null) return 0;
        int[] dirty = _engine.Call("take_dirty_chunks").As<int[]>();
        foreach (int ci in dirty)
            if (ci >= 0 && ci < _chunkMeshes.Length && _built[ci]) BuildChunk(ci, _chunkLod[ci] == 1);
        return dirty.Length;
    }

    /// Re-tessellate only the liquid chunks the last edit/sim changed that are currently meshed.
    public void RebuildLiquid()
    {
        if (_liquidChunkMeshes == null) return;
        int[] dirty = _engine.Call("take_dirty_liquid_chunks").As<int[]>();
        foreach (int ci in dirty)
            if (ci >= 0 && ci < _liquidChunkMeshes.Length && _built[ci]) BuildLiquidChunk(ci);
    }

    // --- progressive rain: gradual area rainfall + a drifting cloud / falling-rain visualization ---

    private bool _rainActive;
    private Vector3 _rainCenter;        // core space; XZ = area centre, Y ≈ surface height under it
    private float _rainRadius = 500f;
    private float _rainRate = 0.002f;
    private double _rainPhase;          // cloud-drift accumulator
    private Node3D _rainRig;            // ephemeral cloud + rain particles under _renderRoot

    public bool RainActive => _rainActive;

    /// Begin progressive rainfall over an area centred on `coreCenter` (core space; Y is the surface
    /// height under the cursor). Radius comes from the brush size; rate from the rain-rate control.
    public void StartRain(Vector3 coreCenter, float radius, float rate)
    {
        _rainCenter = coreCenter;
        _rainRadius = Mathf.Max(radius, 1f);
        _rainRate = Mathf.Max(rate, 0f);
        _rainActive = true;
        if (_rainRig != null && GodotObject.IsInstanceValid(_rainRig)) _rainRig.QueueFree();
        _rainRig = null; // rebuild so a new radius resizes the cloud + emitter
        EnsureRainRig();
        UpdateRainRig();
    }

    /// Drag the rain area to a new spot (cheap: move only, no rebuild).
    public void MoveRain(Vector3 coreCenter, float radius)
    {
        if (!_rainActive) { StartRain(coreCenter, radius, _rainRate); return; }
        _rainCenter = coreCenter;
        _rainRadius = Mathf.Max(radius, 1f);
        UpdateRainRig();
    }

    public void StopRain()
    {
        _rainActive = false;
        if (_rainRig != null && GodotObject.IsInstanceValid(_rainRig)) _rainRig.QueueFree();
        _rainRig = null;
    }

    /// Per-frame progressive rainfall: spread a little water across the area (smoothstep falloff),
    /// settle it a few steps so it pools and runs downhill, and drift the cloud. The plugin calls this
    /// each frame while the Rain tool is active — replaces the old one-shot whole-map flood.
    public void StepRain(double delta)
    {
        if (!_rainActive || _engine == null || !_genDone) return;
        _engine.Call("paint_liquid", (double)_rainCenter.X, (double)_rainCenter.Z, (double)_rainRadius, (double)_rainRate, 0);
        _engine.Call("step_fluid", 0.45, 0.0015, 6);
        RebuildLiquid();
        _rainPhase += delta * 0.4;
        UpdateRainRig();
    }

    private void EnsureRainRig()
    {
        if (_rainRig != null && GodotObject.IsInstanceValid(_rainRig)) return;
        EnsureRenderRoot();
        _rainRig = new Node3D { Name = "DhceRain" };
        _renderRoot.AddChild(_rainRig); // owner left null → ephemeral preview

        float r = _rainRadius;
        var cloudMat = new StandardMaterial3D
        {
            ShadingMode = BaseMaterial3D.ShadingModeEnum.Unshaded,
            AlbedoColor = new Color(0.55f, 0.58f, 0.62f, 0.55f),
            Transparency = BaseMaterial3D.TransparencyEnum.Alpha,
            CullMode = BaseMaterial3D.CullModeEnum.Disabled,
        };
        _rainRig.AddChild(new MeshInstance3D
        {
            Name = "Cloud",
            Mesh = new SphereMesh { Radius = r * 1.2f, Height = r * 0.9f },
            MaterialOverride = cloudMat,
            Scale = new Vector3(1f, 0.35f, 1f),
            CastShadow = GeometryInstance3D.ShadowCastingSetting.Off,
        });

        var rainMat = new StandardMaterial3D
        {
            ShadingMode = BaseMaterial3D.ShadingModeEnum.Unshaded,
            AlbedoColor = new Color(0.55f, 0.70f, 0.95f, 0.7f),
            Transparency = BaseMaterial3D.TransparencyEnum.Alpha,
            BillboardMode = BaseMaterial3D.BillboardModeEnum.Enabled,
        };
        var quad = new QuadMesh { Size = new Vector2(Mathf.Max(r * 0.01f, 1f), Mathf.Max(r * 0.12f, 12f)), Material = rainMat };
        var pm = new ParticleProcessMaterial
        {
            EmissionShape = ParticleProcessMaterial.EmissionShapeEnum.Box,
            EmissionBoxExtents = new Vector3(r, 10f, r),
            Direction = new Vector3(0f, -1f, 0f),
            Spread = 0f,
            Gravity = new Vector3(0f, -3000f, 0f),
            InitialVelocityMin = 400f,
            InitialVelocityMax = 700f,
        };
        _rainRig.AddChild(new GpuParticles3D
        {
            Name = "Fall",
            Amount = 600,
            Lifetime = 1.6,
            DrawPass1 = quad,
            ProcessMaterial = pm,
            CastShadow = GeometryInstance3D.ShadowCastingSetting.Off,
        });
    }

    private void UpdateRainRig()
    {
        if (_rainRig == null || !GodotObject.IsInstanceValid(_rainRig)) return;
        float cloudH = _rainCenter.Y + Mathf.Max(_rainRadius * 1.5f, 1500f);
        float driftX = Mathf.Cos((float)_rainPhase) * _rainRadius * 0.15f;
        float driftZ = Mathf.Sin((float)_rainPhase) * _rainRadius * 0.15f;
        if (_rainRig.GetNodeOrNull<Node3D>("Cloud") is { } cloud)
            cloud.Position = new Vector3(_rainCenter.X + driftX, cloudH, _rainCenter.Z + driftZ);
        if (_rainRig.GetNodeOrNull<GpuParticles3D>("Fall") is { } fall)
            fall.Position = new Vector3(_rainCenter.X, cloudH, _rainCenter.Z);
    }

    /// Switch the colour view (0 Natural … 4 Biome); data views use the unshaded material.
    public void SetViewMode(int mode)
    {
        _viewMode = mode;
        if (_engine == null) return;
        _engine.Call("set_view_mode", mode);
        var m = CurrentViewMat();
        if (_chunks != null) foreach (var mi in _chunks) if (mi != null) mi.MaterialOverride = m;
        if (_genDone) RepaintDirtyTerrain();
    }

    /// Live vertical-relief change (no rebuild): recompute exaggeration + re-tessellate meshed chunks.
    public void SetTerrainHeight(float km)
    {
        TerrainHeightKm = km;
        _exaggeration = km * 1000f / ElevSpan;
        if (!_genDone) return;
        for (int i = 0; i < _built.Length; i++) if (_built[i]) BuildChunk(i, _chunkLod[i] == 1);
    }

    private StandardMaterial3D CurrentViewMat() => _viewMode == 0 ? _mat : _dataMat;

    /// Add a volumetric carve sphere (N5): `centre` in world space, `radius` in metres. Carved into
    /// real geometry at export (Layer B); shown as a gizmo while the Cave tool is active.
    public void AddCave(Vector3 centre, float radius) => Caves.Add(new Vector4(centre.X, centre.Y, centre.Z, Mathf.Max(radius, 0.5f)));
    public void ClearCaves() => Caves.Clear();
    public int CaveCount => Caves.Count;

    /// Toggle an in-editor preview of the **carved** cave geometry (Surface-Nets over all carve
    /// volumes) — the same mesh the slicer bakes per level. No-op without caves / before gen.
    public void PreviewCaves(bool on)
    {
        if (_cavePreview != null && GodotObject.IsInstanceValid(_cavePreview)) _cavePreview.QueueFree();
        _cavePreview = null;
        if (!on || _engine == null || !_genDone || Caves.Count == 0) return;
        var flat = new float[Caves.Count * 4];
        for (int i = 0; i < Caves.Count; i++) { var c = Caves[i]; flat[i * 4] = c.X; flat[i * 4 + 1] = c.Y; flat[i * 4 + 2] = c.Z; flat[i * 4 + 3] = c.W; }
        _engine.Call("tessellate_caves", flat, (double)_exaggeration, 3.0);
        var pos = _engine.Call("cave_positions").As<Vector3[]>();
        if (pos.Length == 0) return;
        var arrays = new Godot.Collections.Array();
        arrays.Resize((int)Mesh.ArrayType.Max);
        arrays[(int)Mesh.ArrayType.Vertex] = pos;
        arrays[(int)Mesh.ArrayType.Normal] = _engine.Call("cave_normals").As<Vector3[]>();
        arrays[(int)Mesh.ArrayType.Index] = _engine.Call("cave_indices").As<int[]>();
        var am = new ArrayMesh();
        am.AddSurfaceFromArrays(Mesh.PrimitiveType.Triangles, arrays);
        if (_caveMat == null) { _caveMat = new StandardMaterial3D { AlbedoColor = new Color(0.40f, 0.38f, 0.36f), Roughness = 0.95f }; _caveMat.Set("cull_mode", 2); }
        _cavePreview = new MeshInstance3D { Mesh = am, MaterialOverride = _caveMat };
        _renderRoot.AddChild(_cavePreview); // owner left null → ephemeral preview
    }

    /// Toggle the in-editor scatter preview: per-slot MultiMesh (low-poly proxy/import-LOD), capped.
    /// The full real-mesh bake happens at export (the slicer). No-op without a library / before gen.
    public void PreviewScatter(bool on)
    {
        foreach (var n in _scatterPreview) if (GodotObject.IsInstanceValid(n)) n.QueueFree();
        _scatterPreview.Clear();
        if (!on || Scatter == null || Scatter.Slots.Count == 0 || _engine == null || !_genDone) return;
        _engine.Call("tessellate_scatter_rules", Scatter.ToRulesFlat(), (double)_exaggeration, (double)Seed);
        var data = _engine.Call("scatter_data").As<float[]>();
        int count = _engine.Call("scatter_count").As<int>();
        foreach (var node in DhceLevelSlicer.BuildScatterMeshes(Scatter, data, count, 30000, preferProxy: true, embed: false))
        {
            _renderRoot.AddChild(node); // owner left null → ephemeral preview, not serialized
            _scatterPreview.Add(node);
        }
    }

    // --- persistence (R5): regenerate from a compact DhceWorldState, no mesh bake ---

    /// On scene open, restore from the assigned state (regenerate + overwrite fields). No-op if none.
    public override void _Ready()
    {
        if (State != null && !_genDone) Load(State);
    }

    private string StatePath => $"res://{Name}_dhce.res";

    /// Snapshot params + every authored field into a new DhceWorldState (null until generated).
    public DhceWorldState Save()
    {
        if (_engine == null || !_genDone) return null;
        float[] T(int id) => _engine.Call("trait_field_export", id).As<float[]>();
        return new DhceWorldState
        {
            Seed = Seed, WorldSizeKm = WorldSizeKm, SpacingM = SpacingM, Octaves = Octaves,
            TerrainHeightKm = TerrainHeightKm, ChunkSizeM = ChunkSizeM,
            Elevation = _engine.Call("elevation_export").As<float[]>(),
            Biome = _engine.Call("biome_export").As<byte[]>(),
            BiomeLocked = _engine.Call("biome_locked_export").As<byte[]>(),
            Region = _engine.Call("region_export").As<byte[]>(),
            LiquidDepth = _engine.Call("liquid_depth_export").As<float[]>(),
            LiquidKind = _engine.Call("liquid_kind_export").As<byte[]>(),
            CourseMask = _engine.Call("course_mask_export").As<byte[]>(),
            Jaggedness = T(0), Relief = T(1), FoothillFalloff = T(2), Erosion = T(3),
            Temperature = T(4), Moisture = T(5), Vegetation = T(6), PaletteFamily = T(7),
            BasePalettes = _engine.Call("base_palettes_export").As<float[]>(),
            RegionLandform = _engine.Call("region_landform_export").As<float[]>(),
        };
    }

    /// Rebuild the world deterministically from a state's params, then overwrite every authored field.
    public void Load(DhceWorldState s)
    {
        if (s == null) return;
        Seed = s.Seed; WorldSizeKm = s.WorldSizeKm; SpacingM = s.SpacingM; Octaves = s.Octaves;
        TerrainHeightKm = s.TerrainHeightKm; ChunkSizeM = s.ChunkSizeM;
        Generate(); // deterministic mesh + chunk slots from the params
        if (_engine == null) return;

        void SetF(string fn, float[] a) { if (a is { Length: > 0 }) _engine.Call(fn, a); }
        void SetB(string fn, byte[] a) { if (a is { Length: > 0 }) _engine.Call(fn, a); }
        void SetT(int id, float[] a) { if (a is { Length: > 0 }) _engine.Call("set_trait_field", id, a); }

        SetF("set_elevation", s.Elevation);
        SetB("set_biome", s.Biome);
        SetB("set_biome_locked", s.BiomeLocked);
        SetB("set_region", s.Region);
        if (s.LiquidDepth is { Length: > 0 } && s.LiquidKind is { Length: > 0 })
            _engine.Call("set_liquid", s.LiquidDepth, s.LiquidKind);
        SetB("set_course_mask", s.CourseMask);
        SetT(0, s.Jaggedness); SetT(1, s.Relief); SetT(2, s.FoothillFalloff); SetT(3, s.Erosion);
        SetT(4, s.Temperature); SetT(5, s.Moisture); SetT(6, s.Vegetation); SetT(7, s.PaletteFamily);
        SetF("set_base_palettes", s.BasePalettes);
        SetF("set_region_landform_table", s.RegionLandform);
        _engine.Call("refresh_colors");
        for (int i = 0; i < _built.Length; i++) if (_built[i]) BuildChunk(i, _chunkLod[i] == 1); // re-tessellate the meshed ring
    }

    /// Save to a binary .res next to the scene and reference it (so reopening restores). Returns status.
    public string SaveToDisk()
    {
        var s = Save();
        if (s == null) return "Generate first, then Save.";
        var err = ResourceSaver.Save(s, StatePath, ResourceSaver.SaverFlags.Compress);
        if (err != Error.Ok) return $"Save failed: {err}";
        State = ResourceLoader.Load<DhceWorldState>(StatePath); // reference the on-disk copy, not an embed
        return $"Saved {StatePath}";
    }

    /// Load from the assigned State (or the on-disk .res) and regenerate. Returns status.
    public string LoadFromDisk()
    {
        var s = State;
        if (s == null && ResourceLoader.Exists(StatePath)) s = ResourceLoader.Load<DhceWorldState>(StatePath);
        if (s == null) return "No saved world to load.";
        Load(s);
        State = s;
        return "Loaded saved world.";
    }
}
