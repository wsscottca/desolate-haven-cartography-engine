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
    [Export] public bool RegenerateOnLoad = true; // on open: Load(State) if assigned, else Generate() from params
    [Export] public DhceWorldState State;      // persisted snapshot; regenerated from on open (R5)
    [Export] public DhceScatterLibrary Scatter; // authored scatter model slots + rules (N3d)
    [Export] public Godot.Collections.Array<Vector4> Caves = new(); // volumetric carve spheres: xyz centre + w radius (N5)
    [Export] public Texture2D RegionMap; // optional region-coloured PNG → overrides the built-in canon layout (paint each region in its accent colour; other/transparent reads as ocean; image row 0 = north)
    [Export] public float LapseRate = 0.6f;           // climate #1: temperature drop per unit elevation (cold peaks)
    [Export] public float OrographicStrength = 0.45f; // climate #2: 0..1 windward-wet / lee-dry pull on moisture
    [Export] public float WindDeg = 0f;               // prevailing wind direction (degrees; 0 = +X, west→east)
    [Export] public float ShapeStrength = 1.0f;       // relief gain: bakes per-region landform (mountains/hills) into terrain at Generate
    [Export] public float RiverDepthGain = 0.015f;    // river EROSION strength: hydraulic stream-power incision
                                                      // carves valleys along the drainage (×3 internally). Higher ⇒ deeper valleys.
    [Export] public float BaseBlendM = 1800f;         // width (m) regions' base-elevation trunk blends — softer steps between places

    /// Normalized-elevation span the core clamps to (ELEV_MAX − ELEV_MIN in world.rs).
    private const float ElevSpan = 3.0f;

    private GodotObject _engine;
    private MeshInstance3D[] _chunks;
    private ArrayMesh[] _chunkMeshes;
    private MeshInstance3D[] _liquidChunks;
    private ArrayMesh[] _liquidChunkMeshes;
    private bool[] _built;
    private int[] _chunkLod; // per built chunk: 0 = full TIN, 1 = coarse LOD
    private ShaderMaterial _mat, _dataMat, _liquidMat;
    private int _viewMode;
    private int _cols, _rows;
    private float _sx, _sy;             // chunk tile size in metres
    private float _exaggeration;
    private float _widthM, _heightM;
    private bool _genDone;
    private bool _streamWholeWorld;              // editor: small worlds build every chunk up-front (no camera streaming)
    private const int WholeWorldChunkCap = 2500; // ≲ ~13 km @ 256 m — above this, fall back to per-frame streaming
    private readonly List<(int dist, int ci, bool lod)> _pending = new();
    private readonly List<Node> _scatterPreview = new(); // ephemeral N3d scatter preview nodes
    private MeshInstance3D _cavePreview;        // ephemeral N5 carved-cave preview
    private StandardMaterial3D _caveMat;
    private Node3D _renderRoot;                 // ephemeral parent for all preview geometry, offset so
                                                // the world centres on the origin (the editor camera's focus)

    private static readonly Color WaterColor = new Color(0.20f, 0.45f, 0.75f, 0.6f);
    private static readonly Color LavaColor = new Color(0.95f, 0.35f, 0.10f, 0.9f);

    /// Region-map import key — 14 DISTINCT flat colours (index = region id) + ocean at [0]. Distinct
    /// because several regions SHARE a `--mk-*` accent (Sacred/Great Lake, Underdeep/Scattered,
    /// Temperate/Plains), so matching a painted map on accents can't tell those pairs apart. Paint each
    /// region its colour below (north = top of the image); leave the sea transparent (A&lt;0.5) or navy.
    private static readonly string[] RegionKeyHex =
    {
        "0c1c36", // 0  Ocean (deep navy) — or just leave transparent
        "c7a24b", // 1  Jagged Mountains   (gold)
        "2e5c9e", // 2  Sacred Woods & Plateau (blue)
        "19c3c3", // 3  Great Lake         (cyan)
        "5a9a4a", // 4  Temperate Forest   (green)
        "e6d24a", // 5  Open Plains        (yellow)
        "9aa0aa", // 6  Underdeep          (grey)
        "6e5a82", // 7  Deep Wood          (purple)
        "6fa9ce", // 8  Frozen Reaches     (light blue)
        "2e8c8c", // 9  Lost Isles         (teal)
        "b23a2e", // 10 Blisterwood        (red)
        "d6883a", // 11 Volcanic Scape     (orange)
        "3a2e4a", // 12 Blight Ruins       (dark violet)
        "c77fa8", // 13 Scattered Isles    (pink)
        "6e7a4b", // 14 Marsh & Bog        (olive)
    };

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
        // Per-vertex biome colour → albedo via an explicit shader. StandardMaterial3D's
        // VertexColorUseAsAlbedo does NOT drive albedo under the Compatibility (OpenGL) renderer in
        // this setup — the colours upload fine (matched arrays, real values) yet read white — so a
        // trivial spatial shader that samples COLOR directly is used; it works on every backend.
        // cull_disabled throughout because the Y-up remap flips triangle winding.
        // Surface-conforming brush cursor: a 3D-sphere highlight evaluated per-pixel on the real
        // surface (so it wraps peaks/slopes), driven by `SetBrushHighlight`. `brush_pos` is GLOBAL
        // space (matches `v_world` = MODEL_MATRIX·VERTEX); the same sphere the core paints into.
        const string brushUniforms =
            "uniform vec3 brush_pos;\nuniform float brush_radius;\nuniform float brush_active;\n" +
            "varying vec3 v_world;\n" +
            "void vertex() { v_world = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz; }\n";
        const string brushOverlay =
            "  if (brush_active > 0.5 && brush_radius > 0.0) {\n" +
            "    float nd = distance(v_world, brush_pos) / brush_radius;\n" + // 0 centre → 1 rim
            "    if (nd < 1.0) {\n" +
            "      float fill = (1.0 - nd) * 0.18;\n" +                       // soft disc fill
            "      float rim = smoothstep(0.88, 0.97, nd) * (1.0 - smoothstep(0.97, 1.0, nd));\n" + // bright edge
            "      ALBEDO = mix(ALBEDO, vec3(1.0, 0.92, 0.30), clamp(fill + rim * 0.85, 0.0, 1.0));\n" +
            "    }\n  }\n";
        _mat = VertexColorShaderMat( // Natural view: lit terrain (relief shows through the lighting)
            "shader_type spatial;\nrender_mode cull_disabled;\n" + brushUniforms +
            "void fragment() { ALBEDO = COLOR.rgb; ROUGHNESS = 1.0; METALLIC = 0.0;\n" + brushOverlay + "}");
        _dataMat = VertexColorShaderMat( // data views: UNSHADED so the raw field reads true, lighting-independent
            "shader_type spatial;\nrender_mode cull_disabled, unshaded;\n" + brushUniforms +
            "void fragment() { ALBEDO = COLOR.rgb;\n" + brushOverlay + "}");
        _liquidMat = VertexColorShaderMat( // translucent water/lava; COLOR.a carries the surface alpha (no brush highlight)
            "shader_type spatial;\nrender_mode cull_disabled;\n" +
            "void fragment() { ALBEDO = COLOR.rgb; ALPHA = COLOR.a; ROUGHNESS = 0.2; METALLIC = 0.0; }");
    }

    /// A ShaderMaterial whose fragment shader reads the mesh's per-vertex COLOR as albedo — the
    /// renderer-independent replacement for StandardMaterial3D.VertexColorUseAsAlbedo.
    private static ShaderMaterial VertexColorShaderMat(string code) =>
        new ShaderMaterial { Shader = new Shader { Code = code } };

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
        _engine.Call("set_base_blend_m", (double)BaseBlendM); // softer per-region base-elevation steps

        // Centre the world on the origin: the core generates in [0, size] (origin at a corner), but the
        // editor camera looks at the origin — so shift the preview by -WorldCenter to put the world's
        // middle under the camera. Without this, Generate succeeds but the terrain sits ~10 km off-screen.
        EnsureRenderRoot();
        _renderRoot.Position = new Vector3(-_widthM * 0.5f, 0f, -_heightM * 0.5f);

        // Region layout: inject the imported region-coloured PNG (if any) so the canon pipeline
        // samples it; otherwise the built-in canon anchors are used.
        ApplyRegionMap();
        // Climate params drive temperature/moisture inside build — push them before generating.
        PushClimateParams();

        var sw = Stopwatch.StartNew();
        _engine.Call("build", _widthM, _heightM, SpacingM, (float)Seed, Octaves);
        sw.Stop();
        OnGenDone(sw.Elapsed.TotalMilliseconds);
    }

    /// Resolve the assigned region-map PNG (if any) into a region-id grid and inject it before build;
    /// clears the override (→ built-in canon anchors) when none is set. Each pixel is matched to the
    /// nearest of the 14 region accents (`biome_color_of`) or ocean — paint each region in its accent
    /// colour; anything else (or transparent) reads as ocean. Image row 0 is north (north-up).
    private void ApplyRegionMap()
    {
        if (_engine == null) return;
        if (RegionMap == null) { _engine.Call("clear_region_layout"); return; }
        Image img = RegionMap.GetImage();
        if (img == null) { _engine.Call("clear_region_layout"); return; }
        if (img.IsCompressed()) img.Decompress();
        if (img.GetFormat() != Image.Format.Rgba8) img.Convert(Image.Format.Rgba8);

        const int RegionCount = 14;
        var refs = new Color[RegionCount + 1];
        for (int id = 0; id <= RegionCount; id++) refs[id] = new Color(RegionKeyHex[id]);

        int imgW = img.GetWidth(), imgH = img.GetHeight();
        int cols = Mathf.Min(imgW, 256), rows = Mathf.Min(imgH, 256);
        if (cols < 1 || rows < 1) { _engine.Call("clear_region_layout"); return; }
        var ids = new byte[cols * rows];
        for (int gy = 0; gy < rows; gy++)
        {
            int py = Mathf.Clamp((int)((gy + 0.5f) * imgH / rows), 0, imgH - 1);
            for (int gx = 0; gx < cols; gx++)
            {
                int px = Mathf.Clamp((int)((gx + 0.5f) * imgW / cols), 0, imgW - 1);
                Color p = img.GetPixel(px, py);
                byte best = 0;
                float bestD = float.MaxValue;
                for (int id = 0; id <= RegionCount; id++)
                {
                    float dr = p.R - refs[id].R, dg = p.G - refs[id].G, db = p.B - refs[id].B;
                    float d = dr * dr + dg * dg + db * db;
                    if (d < bestD) { bestD = d; best = (byte)id; }
                }
                if (p.A < 0.5f) best = 0; // transparent → ocean
                ids[gy * cols + gx] = best;
            }
        }
        _engine.Call("set_region_layout", ids, cols, rows);
        GD.Print($"[DHCE] region map applied: {cols}x{rows} from {imgW}x{imgH} PNG");
    }

    /// Push the climate params (lapse rate, orographic strength, wind) into the engine. Wind is sent
    /// as a vector (cos/sin of the angle) so the core stays transcendental-free.
    private void PushClimateParams()
    {
        if (_engine == null) return;
        _engine.Call("set_lapse_rate", (double)LapseRate);
        _engine.Call("set_orographic_strength", (double)OrographicStrength);
        float rad = Mathf.DegToRad(WindDeg);
        _engine.Call("set_wind", (double)Mathf.Cos(rad), (double)Mathf.Sin(rad));
    }

    /// Re-derive temperature + moisture from the current climate params (the live slider path) and
    /// re-tessellate the meshed chunks so the recolour shows without a full regen.
    public void RecomputeClimate()
    {
        if (_engine == null || !_genDone) return;
        PushClimateParams();
        _engine.Call("recompute_climate");
        RepaintDirtyTerrain();
    }

    private void OnGenDone(double genMs)
    {
        Vector2I grid = _engine.Call("chunk_grid").As<Vector2I>();
        _cols = grid.X;
        _rows = grid.Y;
        _sx = _sy = (float)_engine.Call("chunk_size_m").As<double>(); // fixed tile size
        int n = _engine.Call("chunk_count").As<int>();
        GD.Print($"[DHCE] gen world={WorldSizeKm}km render={RenderDistance} cpf={ChunksPerFrame} regions={_engine.Call("region_count")} chunks={n} grid={_cols}x{_rows} tile={_sx:0}m {genMs:0}ms");

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
        if (!_loadingState)
        {
            // Two-tier water. The OCEAN is a single global sea level ~1 km above the lowest basin
            // (floods the rim/edges). _exaggeration is metres per normalized unit, so 1000/_exaggeration
            // is 1 km in normalized elevation. Perched LAKES/ponds are filled separately (fill_lakes) —
            // held in highland basins above the ocean, like Lake Tahoe. (Load() restores both instead.)
            float minElev = (float)_engine.Call("min_elevation").As<double>();
            SeaLevelNorm = minElev + 1000f / _exaggeration;
            _engine.Call("set_sea_level", (double)SeaLevelNorm);
            // Unified relief + hydrology pass: bake the per-region landform (mountains/hills), then carve
            // rivers, pond perched lakes behind their carved outlets, and connect each lake's outflow —
            // one coherent watershed on shaped terrain. (Load() restores the baked result instead.)
            _engine.Call("reshape_and_reflow", (double)ShapeStrength, (double)RiverDepthGain);
        }
        // Iteration-sized worlds: build every chunk up-front so the 3D view shows the *whole* authored
        // world (like the minimap) and never depends on per-frame streaming — which a C# hot-reload can
        // leave stuck (the recurring "only the centre loads" sliver). Larger worlds still stream.
        _streamWholeWorld = n <= WholeWorldChunkCap;
        if (_streamWholeWorld) BuildAllChunks();
        else UpdateStreaming(WorldCenter); // seed the centre so something shows immediately
    }

    /// Build every chunk slot at full detail (editor whole-world view for small maps). One up-front cost
    /// at Generate instead of camera-driven streaming, so the preview is complete and reload-proof.
    private void BuildAllChunks()
    {
        if (_chunks == null) return;
        for (int i = 0; i < _chunks.Length; i++)
            if (!_built[i]) BuildChunk(i, false); // full TIN, no LOD
    }

    private void ClearChunks()
    {
        PreviewScatter(false); // drop any scatter preview before discarding the world
        PreviewCaves(false);
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
        if (_streamWholeWorld) return; // small world: every chunk is already built, nothing to stream/free
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

    // --- sea level ---

    private bool _loadingState; // true while Load() runs, so a fresh-generate default doesn't fire

    /// Current water level (normalized elevation), tracked here for save/load. Defaulted at generate
    /// to 1 km above the terrain's lowest basin (see OnGenDone).
    public float SeaLevelNorm { get; private set; }

    /// Set the water level (normalized elevation), refill, and track it for save/load.
    public void SetSeaLevel(double level)
    {
        SeaLevelNorm = (float)level;
        if (_engine == null) return;
        _engine.Call("set_sea_level", level);
        // Re-flow the watershed at the new ocean level (relief is unchanged, so no reshape): re-carve
        // rivers on the current terrain, re-pond lakes behind them, and reconnect the outlets.
        _engine.Call("generate_rivers", (double)RiverDepthGain);
        _engine.Call("fill_lakes"); // per-Region thresholds
        _engine.Call("connect_lake_outlets");
        _engine.Call("grade_shorelines"); // ease the shores down (else a slider move re-cliffs them)
        RepaintDirtyTerrain(); // rivers carve terrain → re-tessellate affected tiles
        RebuildLiquid();
    }

    // --- climate-driven rainfall: derive a per-cell rain field in the core, then settle it ---

    /// Deposit climate-driven rainfall — a per-cell field the core derives from each biome's water
    /// profile (raininess / rain-shadow / evaporation) plus the per-cell moisture / temperature
    /// traits and an orographic term — then settle it a few steps so it pools and runs downhill.
    /// Replaces the old manual rain brush + drifting-cloud rig.
    public void ApplyRainfall()
    {
        if (_engine == null || !_genDone) return;
        _engine.Call("apply_rainfall");
        _engine.Call("step_fluid", 0.45, 0.0015, 8);
        RebuildLiquid();
    }

    /// Drive the surface-conforming brush cursor baked into the terrain shaders (`_mat` + `_dataMat`,
    /// so it survives a Natural↔data view switch). `globalPos` is editor/global space — pass
    /// `ToEditor(hit)` (the raycast hit is core space). `radius` in metres. Off when `on` is false or
    /// before the materials exist. The liquid material is intentionally left out (no tint on water/lava).
    public void SetBrushHighlight(Vector3 globalPos, float radius, bool on)
    {
        if (_mat == null) return;
        float a = on ? 1f : 0f;
        _mat.SetShaderParameter("brush_active", a);
        _dataMat.SetShaderParameter("brush_active", a);
        if (!on) return;
        _mat.SetShaderParameter("brush_pos", globalPos);
        _mat.SetShaderParameter("brush_radius", radius);
        _dataMat.SetShaderParameter("brush_pos", globalPos);
        _dataMat.SetShaderParameter("brush_radius", radius);
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

    private ShaderMaterial CurrentViewMat() => _viewMode == 0 ? _mat : _dataMat;

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

    /// On scene open: restore the assigned saved state if present, otherwise regenerate from the node's
    /// params (so reopening always shows a world without a manual Generate). Turn off via RegenerateOnLoad.
    public override void _Ready()
    {
        if (_genDone) return;
        if (State != null) Load(State);
        else if (RegenerateOnLoad) Generate();
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
            TerrainHeightKm = TerrainHeightKm, ChunkSizeM = ChunkSizeM, BaseBlendM = BaseBlendM, SeaLevel = SeaLevelNorm,
            LapseRate = LapseRate, OrographicStrength = OrographicStrength, WindDeg = WindDeg,
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
            RegionLakeDepth = _engine.Call("region_lake_depth_export").As<float[]>(),
            RegionRiverThreshold = _engine.Call("river_threshold_export").As<float[]>(),
            RegionErosion = _engine.Call("erosion_export").As<float[]>(),
        };
    }

    /// Rebuild the world deterministically from a state's params, then overwrite every authored field.
    public void Load(DhceWorldState s)
    {
        if (s == null) return;
        Seed = s.Seed; WorldSizeKm = s.WorldSizeKm; SpacingM = s.SpacingM; Octaves = s.Octaves;
        TerrainHeightKm = s.TerrainHeightKm; ChunkSizeM = s.ChunkSizeM; BaseBlendM = s.BaseBlendM;
        LapseRate = s.LapseRate; OrographicStrength = s.OrographicStrength; WindDeg = s.WindDeg;
        _loadingState = true;
        Generate(); // deterministic mesh + chunk slots from the params (skips the default sea level)
        _loadingState = false;
        if (_engine == null) return;

        // Restore the saved sea level before the saved liquid overrides the fill below.
        SeaLevelNorm = s.SeaLevel;
        _engine.Call("set_sea_level", (double)s.SeaLevel);

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
        SetF("set_region_lake_depth_table", s.RegionLakeDepth); // per-Region lake thresholds (saved liquid already holds the lakes)
        SetF("set_river_threshold_table", s.RegionRiverThreshold); // per-Region river thresholds (saved liquid already holds the rivers)
        SetF("set_erosion_table", s.RegionErosion); // per-Region erosion strength
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
