using Godot;

namespace DesolateHaven.Cartography;

/// N0 risk spike: build a `DhceEngine` world, render it as an `ArrayMesh`, sculpt with the
/// left mouse, and measure the per-stroke "paint + re-tessellate + upload" cost at full
/// density. The make-or-break check for the Godot stack choice.
///
/// All compute is in Rust (the `dhce-godot` GDExtension); C# only uploads buffers and runs
/// the camera. The engine is a GDExtension class, so it is driven via `Call(...)` — C# has
/// no static binding for it.
[GlobalClass]
public partial class CartographerSpike : Node3D
{
    [Export] public float Width = 6000f;
    [Export] public float Height = 6000f;
    [Export] public float Spacing = 12f;     // smaller ⇒ more regions (lower to stress-test toward ~445k)
    [Export] public int Seed = 12345;
    [Export] public int Octaves = 6;
    [Export] public float Exaggeration = 900f;
    [Export] public float BrushRadius = 350f;
    [Export] public float BrushStrength = 0.06f;

    private GodotObject _engine;
    private MeshInstance3D _terrain;
    private readonly ArrayMesh _mesh = new();
    private int _dabs;
    private double _accumMs;

    public override void _Ready()
    {
        _engine = ClassDB.Instantiate("DhceEngine").AsGodotObject();
        if (_engine == null)
        {
            GD.PrintErr("DhceEngine not found — enable the dhce GDExtension (addons/dhce/dhce.gdextension).");
            return;
        }

        _engine.Call("build", Width, Height, Spacing, (float)Seed, Octaves);
        GD.Print($"[DHCE] regions={_engine.Call("region_count")} triangles={_engine.Call("triangle_count")}");

        var mat = new StandardMaterial3D
        {
            VertexColorUseAsAlbedo = true,
            Roughness = 0.95f,
        };
        _terrain = new MeshInstance3D { Mesh = _mesh, MaterialOverride = mat };
        AddChild(_terrain);
        Retessellate();

        // Self-contained scene: spawn a sun and the orbit camera centered on the map.
        var sun = new DirectionalLight3D { RotationDegrees = new Vector3(-50, -40, 0) };
        AddChild(sun);

        var cam = new OrbitCamera
        {
            Target = new Vector3(Width * 0.5f, 0f, Height * 0.5f),
            Distance = Mathf.Max(Width, Height) * 0.9f,
        };
        AddChild(cam);
        cam.Current = true;
    }

    /// Re-pack the whole surface in Rust and rebuild the ArrayMesh. The N0 question is
    /// whether this is smooth at full density; if it stutters, the fallback is a
    /// `RenderingServer` partial vertex update over the touched regions (the engine
    /// returns them from paint_*).
    private void Retessellate()
    {
        _engine.Call("tessellate", Exaggeration);
        var positions = _engine.Call("surface_positions").As<Vector3[]>();
        if (positions.Length == 0) return;
        var normals = _engine.Call("surface_normals").As<Vector3[]>();
        var colors = _engine.Call("surface_colors").As<Color[]>();
        var indices = _engine.Call("surface_indices").As<int[]>();

        var arrays = new Godot.Collections.Array();
        arrays.Resize((int)Mesh.ArrayType.Max);
        arrays[(int)Mesh.ArrayType.Vertex] = positions;
        arrays[(int)Mesh.ArrayType.Normal] = normals;
        arrays[(int)Mesh.ArrayType.Color] = colors;
        arrays[(int)Mesh.ArrayType.Index] = indices;

        _mesh.ClearSurfaces();
        _mesh.AddSurfaceFromArrays(Mesh.PrimitiveType.Triangles, arrays);
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
        Retessellate();
        _accumMs += (Time.GetTicksUsec() - t0) / 1000.0;
        if (++_dabs % 30 == 0)
            GD.Print($"[DHCE] paint+retess avg {_accumMs / _dabs:0.0} ms over {_dabs} dabs");
    }
}
