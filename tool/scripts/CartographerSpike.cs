using Godot;

namespace DesolateHaven.Cartography;

/// N0 risk spike: build a `DhceEngine` world, render it as a grid of chunk meshes, sculpt
/// with the left mouse, and measure the per-stroke cost. Chunking is the perf fix: an edit
/// re-tessellates + re-uploads only the chunks it touches, not the whole mesh.
///
/// All compute is in Rust (the `dhce-godot` GDExtension); C# only uploads buffers and runs
/// the camera. The engine is a GDExtension class, so it is driven via `Call(...)` — C# has
/// no static binding for it.
[GlobalClass]
public partial class CartographerSpike : Node3D
{
    [Export] public float Width = 6000f;
    [Export] public float Height = 6000f;
    [Export] public float Spacing = 12f;      // smaller ⇒ more regions (lower toward 6 to stress-test)
    [Export] public int Seed = 12345;
    [Export] public int Octaves = 6;
    [Export] public float Exaggeration = 300f; // vertical scale (web tool used ~120–300)
    [Export] public float BrushRadius = 350f;
    [Export] public float BrushStrength = 0.06f;

    private GodotObject _engine;
    private MeshInstance3D[] _chunks;
    private ArrayMesh[] _chunkMeshes;
    private int _dabs;
    private double _totalMs;
    private long _dirtyAccum;

    public override void _Ready()
    {
        _engine = ClassDB.Instantiate("DhceEngine").AsGodotObject();
        if (_engine == null)
        {
            GD.PrintErr("DhceEngine not found — enable the dhce GDExtension (addons/dhce/dhce.gdextension).");
            return;
        }

        _engine.Call("build", Width, Height, Spacing, (float)Seed, Octaves);
        GD.Print($"[DHCE] regions={_engine.Call("region_count")} triangles={_engine.Call("triangle_count")} chunks={_engine.Call("chunk_count")}");

        var mat = new StandardMaterial3D
        {
            VertexColorUseAsAlbedo = true,
            Roughness = 1.0f,
            Metallic = 0.0f,
        };
        // Double-sided: the Y-up remap flips triangle winding, so backface culling hides
        // the terrain when viewed top-down. Disable culling so it shows from any angle.
        // (Set by property id to avoid enum-name fragility: cull_mode 2 = CULL_DISABLED.)
        mat.Set("cull_mode", 2);

        // One MeshInstance3D per chunk; edits rebuild only the dirty ones.
        int n = _engine.Call("chunk_count").As<int>();
        _chunks = new MeshInstance3D[n];
        _chunkMeshes = new ArrayMesh[n];
        for (int i = 0; i < n; i++)
        {
            var am = new ArrayMesh();
            var mi = new MeshInstance3D { Mesh = am, MaterialOverride = mat };
            AddChild(mi);
            _chunks[i] = mi;
            _chunkMeshes[i] = am;
            BuildChunk(i);
        }

        // Flat ambient fill (so faces turned from the sun aren't black) + a distinct dark
        // background so the terrain reads against it. Property ids dodge enum-name risk:
        // background_mode 1 = COLOR, ambient_light_source 2 = COLOR.
        var env = new Godot.Environment();
        env.Set("background_mode", 1);
        env.Set("background_color", new Color(0.10f, 0.12f, 0.16f));
        env.Set("ambient_light_source", 2);
        env.Set("ambient_light_color", new Color(0.70f, 0.75f, 0.82f));
        env.Set("ambient_light_energy", 1.2f);
        AddChild(new WorldEnvironment { Environment = env });

        // A key light + a near-overhead light add relief on top of the ambient.
        AddSun(new Vector3(-50, -40, 0), 0.9f);
        AddSun(new Vector3(-85, 20, 0), 0.4f);

        var cam = new OrbitCamera
        {
            Target = new Vector3(Width * 0.5f, 0f, Height * 0.5f),
            Distance = Mathf.Max(Width, Height) * 0.9f,
        };
        AddChild(cam);
        cam.Current = true;
    }

    private void AddSun(Vector3 rotationDegrees, float energy)
    {
        AddChild(new DirectionalLight3D { RotationDegrees = rotationDegrees, LightEnergy = energy });
    }

    /// Re-pack one chunk in Rust and rebuild its ArrayMesh. Cost ∝ chunk size, not the
    /// whole mesh — this is what makes high-density sculpting interactive.
    private void BuildChunk(int i)
    {
        _engine.Call("tessellate_chunk", i, Exaggeration);
        ArrayMesh am = _chunkMeshes[i];
        am.ClearSurfaces();
        var positions = _engine.Call("chunk_positions").As<Vector3[]>();
        if (positions.Length == 0) return; // empty tile
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
    }

    public override void _UnhandledInput(InputEvent e)
    {
        bool down = e is InputEventMouseButton { ButtonIndex: MouseButton.Left, Pressed: true };
        bool drag = e is InputEventMouseMotion mm && (mm.ButtonMask & MouseButtonMask.Left) != 0;
        if (down) PaintAt(((InputEventMouseButton)e).Position);
        else if (drag) PaintAt(((InputEventMouseMotion)e).Position);
    }

    private void PaintAt(Vector2 screen)
    {
        var cam = GetViewport().GetCamera3D();
        if (cam == null) return;
        Vector3 from = cam.ProjectRayOrigin(screen);
        Vector3 dir = cam.ProjectRayNormal(screen);
        if (Mathf.IsZeroApprox(dir.Y)) return;
        float t = -from.Y / dir.Y;            // intersect the ground plane Y = 0
        if (t < 0f) return;
        Vector3 hit = from + dir * t;         // core (x, y) = (hit.X, hit.Z)

        ulong t0 = Time.GetTicksUsec();
        _engine.Call("paint_terrain", (double)hit.X, (double)hit.Z, (double)BrushRadius, (double)BrushStrength, 0);
        int[] dirty = _engine.Call("take_dirty_chunks").As<int[]>();
        foreach (int ci in dirty)
        {
            if (ci >= 0 && ci < _chunkMeshes.Length) BuildChunk(ci);
        }
        double ms = (Time.GetTicksUsec() - t0) / 1000.0;

        _totalMs += ms;
        _dirtyAccum += dirty.Length;
        if (++_dabs % 30 == 0)
            GD.Print($"[DHCE] {_dabs} dabs: {_totalMs / _dabs:0.0} ms/dab over {(double)_dirtyAccum / _dabs:0.0} dirty chunks/dab");
    }
}
