using Godot;

namespace DesolateHaven.Cartography;

/// The world sun: a Godot DirectionalLight3D with a visible, draggable sphere in the sky. Drag
/// the sphere to move the sun (which sets the light direction); Brightness / Warmth / Hue shape
/// its colour through Godot's own light properties. Exposes the horizontal light direction
/// (`MapDir`) so the minimap knows which faces to shade — the minimap has no separate lighting.
public partial class WorldSun : DirectionalLight3D
{
    private Vector3 _center;
    private float _radius = 1000f;
    private Vector3 _dirToSun = new Vector3(0.55f, 0.7f, 0.45f).Normalized();
    private MeshInstance3D _disc;
    private StandardMaterial3D _discMat;

    public float Brightness = 1.0f;
    public float Warmth = 0.5f; // 0 cool, 1 warm
    public float Hue = 0.0f;    // -0.5..0.5 tint

    public Vector3 Center => _center;
    public float Radius => _radius;
    public Vector3 SunWorldPos => _center + _dirToSun * _radius;

    /// Horizontal direction toward the sun (core ground = Godot XZ) for the minimap hill-shade.
    public Vector2 MapDir
    {
        get
        {
            var h = new Vector2(_dirToSun.X, _dirToSun.Z);
            return h.Length() > 0.001f ? h.Normalized() : new Vector2(0.7f, 0.7f);
        }
    }

    public override void _Ready()
    {
        ShadowEnabled = false;
        _discMat = new StandardMaterial3D();
        _discMat.Set("shading_mode", 0); // unshaded
        _disc = new MeshInstance3D
        {
            Mesh = new SphereMesh { Radius = 1f, Height = 2f },
            MaterialOverride = _discMat,
            TopLevel = true, // sit in world space, independent of the light's rotation
        };
        AddChild(_disc);
        ApplyColor();
        ApplyDir();
    }

    public void Configure(Vector3 center, float radius)
    {
        _center = center;
        _radius = Mathf.Max(radius, 1f);
        ApplyDir();
    }

    public void SetDirToSun(Vector3 dir)
    {
        if (dir.Length() < 1e-4f) return;
        _dirToSun = dir.Normalized();
        ApplyDir();
    }

    public void SetBrightness(float v) { Brightness = v; ApplyColor(); }
    public void SetWarmth(float v) { Warmth = v; ApplyColor(); }
    public void SetHue(float v) { Hue = v; ApplyColor(); }

    private void ApplyDir()
    {
        if (_disc == null) return;
        GlobalPosition = _center;
        Vector3 travel = -_dirToSun; // light travels from the sun toward the centre
        Vector3 up = Mathf.Abs(travel.Dot(Vector3.Up)) > 0.98f ? Vector3.Right : Vector3.Up;
        LookAt(_center + travel, up); // forward (-Z) = travel direction
        _disc.GlobalPosition = SunWorldPos;
        _disc.Scale = Vector3.One * (_radius * 0.03f);
    }

    private void ApplyColor()
    {
        Color cool = new Color(0.78f, 0.86f, 1.0f);
        Color warm = new Color(1.0f, 0.80f, 0.52f);
        Color c = cool.Lerp(warm, Mathf.Clamp(Warmth, 0f, 1f));
        if (Mathf.Abs(Hue) > 0.001f) c = Color.FromHsv(Mathf.PosMod(c.H + Hue, 1f), c.S, c.V);
        LightColor = c;
        LightEnergy = Mathf.Max(Brightness, 0f);
        if (_discMat != null)
        {
            _discMat.AlbedoColor = c;
            _discMat.EmissionEnabled = true;
            _discMat.Emission = c;
            _discMat.EmissionEnergyMultiplier = 2.5f;
        }
    }
}
