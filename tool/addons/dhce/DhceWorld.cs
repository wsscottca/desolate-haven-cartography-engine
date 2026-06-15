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
    [Export] public float WorldSizeKm = 20f;   // requested size; snapped to a whole number of chunks
    [Export] public float SpacingM = 12f;      // metres between regions (~1.7M @ 20 km / 12 m)
    [Export] public int Seed = 12345;
    [Export] public int Octaves = 6;
    [Export] public float TerrainHeightKm = 2.4f;
    [Export] public float ChunkSizeM = 256f;   // fixed chunk edge (m); smaller ⇒ finer, lighter streaming
    [Export] public int RenderDistance = 8;    // chunk tiles (Chebyshev) kept meshed around the focus
    [Export] public int ChunksPerFrame = 8;    // chunks tessellated per streaming tick

    /// Normalized-elevation span the core clamps to (ELEV_MAX − ELEV_MIN in world.rs).
    private const float ElevSpan = 3.0f;

    private GodotObject _engine;
    private MeshInstance3D[] _chunks;
    private ArrayMesh[] _chunkMeshes;
    private MeshInstance3D[] _liquidChunks;
    private ArrayMesh[] _liquidChunkMeshes;
    private bool[] _built;
    private StandardMaterial3D _mat, _dataMat, _liquidMat;
    private int _viewMode;
    private int _cols, _rows;
    private float _sx, _sy;             // chunk tile size in metres
    private float _exaggeration;
    private float _widthM, _heightM;
    private bool _genDone;
    private readonly List<(int dist, int ci)> _pending = new();

    private static readonly Color WaterColor = new Color(0.20f, 0.45f, 0.75f, 0.6f);
    private static readonly Color LavaColor = new Color(0.95f, 0.35f, 0.10f, 0.9f);

    public GodotObject Engine => _engine;
    public float Exaggeration => _exaggeration;
    public bool GenDone => _genDone;
    public float WorldWidthM => _widthM;
    public float WorldHeightM => _heightM;
    public Vector3 WorldCenter => new Vector3(_widthM * 0.5f, 0f, _heightM * 0.5f);

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
        _genDone = true;
        UpdateStreaming(WorldCenter); // seed the centre so something shows immediately
    }

    private void ClearChunks()
    {
        if (_chunks != null) foreach (var mi in _chunks) mi?.QueueFree();
        if (_liquidChunks != null) foreach (var mi in _liquidChunks) mi?.QueueFree();
        _chunks = null;
        _chunkMeshes = null;
        _liquidChunks = null;
        _liquidChunkMeshes = null;
        _built = null;
        _genDone = false;
    }

    /// Stream chunks around `focus` (world-space): build in-range, free out-of-range. Called each
    /// frame by whoever owns the camera (the editor plugin in the tool; the game otherwise).
    public void UpdateStreaming(Vector3 focus)
    {
        if (!_genDone || _chunks == null) return;
        int camGx = Mathf.Clamp((int)(focus.X / _sx), 0, Mathf.Max(_cols - 1, 0));
        int camGy = Mathf.Clamp((int)(focus.Z / _sy), 0, Mathf.Max(_rows - 1, 0));

        _pending.Clear();
        for (int ci = 0; ci < _chunks.Length; ci++)
        {
            int gx = ci % _cols;
            int gy = ci / _cols;
            int dist = Mathf.Max(Mathf.Abs(gx - camGx), Mathf.Abs(gy - camGy));
            if (dist <= RenderDistance) { if (!_built[ci]) _pending.Add((dist, ci)); }
            else if (_built[ci]) FreeChunk(ci);
        }
        if (_pending.Count > 0)
        {
            _pending.Sort((a, b) => a.dist.CompareTo(b.dist));
            int budget = Mathf.Min(ChunksPerFrame, _pending.Count);
            for (int k = 0; k < budget; k++) BuildChunk(_pending[k].ci);
        }
    }

    private void BuildChunk(int i)
    {
        if (_chunks[i] == null) // lazy: instantiate the tile node the first time it streams in
        {
            var m = new ArrayMesh();
            var mi = new MeshInstance3D { Mesh = m, MaterialOverride = CurrentViewMat() };
            AddChild(mi); // owner left null → ephemeral preview, not serialized into the scene
            _chunks[i] = mi;
            _chunkMeshes[i] = m;
        }
        _engine.Call("tessellate_chunk", i, _exaggeration);
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
        BuildLiquidChunk(i);
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
            AddChild(lmi);
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
            if (ci >= 0 && ci < _chunkMeshes.Length && _built[ci]) BuildChunk(ci);
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
        for (int i = 0; i < _built.Length; i++) if (_built[i]) BuildChunk(i);
    }

    private StandardMaterial3D CurrentViewMat() => _viewMode == 0 ? _mat : _dataMat;
}
