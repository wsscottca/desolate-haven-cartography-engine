using Godot;

namespace DesolateHaven.Cartography;

/// Camera for the cartography tool, matching Godot's 3D editor viewport navigation so the
/// transition is muscle-memory:
///   - **Middle-drag** orbits around the point under the view.
///   - **Shift + middle-drag** pans.
///   - **Wheel** dollies toward/away from the ground (in freelook it adjusts fly speed).
///   - **Right-drag** is freelook: mouse looks around (cursor captured) and **WASD** flies,
///     **Q/E** drop/rise, **Shift** moves faster.
/// Left mouse is deliberately untouched — it's the sculpt tool. Y-up: the terrain lies in the
/// XZ plane, elevation on +Y. `FocusPoint` (the camera→ground intersection) is what the
/// streamer uses to decide which tiles to mesh.
[GlobalClass]
public partial class OrbitCamera : Camera3D
{
    [Export] public float Yaw = 0.0f;       // radians around +Y
    [Export] public float Pitch = -0.9f;    // radians; negative looks down at the ground
    [Export] public float MoveSpeed = 4000f; // freelook fly speed, world units/sec
    [Export] public float FastMultiplier = 4f;
    [Export] public float OrbitSpeed = 0.01f;
    [Export] public float LookSpeed = 0.005f;
    [Export] public float PanSpeed = 1.0f;
    [Export] public float ZoomStep = 0.12f;  // fraction of distance-to-ground per wheel notch

    private const float PitchLimit = 1.55f;   // just shy of straight up/down (avoid gimbal flip)

    private bool _freelook;
    private Vector3 _pivot;                    // orbit pivot, captured on middle-press
    private float _pivotDist;

    public override void _Ready()
    {
        Near = 1f;
        Far = 200000f;
        ApplyRotation();
    }

    /// Position the camera looking down at `center` from `distance` away — the default overview.
    public void FrameOverhead(Vector3 center, float distance)
    {
        Yaw = 0f;
        Pitch = -0.9f;
        ApplyRotation();
        Position = center - Forward() * distance;
    }

    /// Ground-plane (Y=0) point the camera is looking at; falls back to the camera's footprint
    /// when it looks flat or up. The streamer meshes tiles around this point.
    public Vector3 FocusPoint
    {
        get
        {
            Vector3 pos = GlobalPosition;
            Vector3 fwd = Forward();
            if (fwd.Y < -1e-4f)
            {
                float t = -pos.Y / fwd.Y;
                return pos + fwd * t;
            }
            return new Vector3(pos.X, 0f, pos.Z);
        }
    }

    public override void _Process(double delta)
    {
        if (!_freelook) return;

        float speed = MoveSpeed * (float)delta;
        if (Input.IsKeyPressed(Key.Shift)) speed *= FastMultiplier;

        Vector3 move = Vector3.Zero;
        if (Input.IsPhysicalKeyPressed(Key.W)) move += Forward();
        if (Input.IsPhysicalKeyPressed(Key.S)) move -= Forward();
        if (Input.IsPhysicalKeyPressed(Key.D)) move += Right();
        if (Input.IsPhysicalKeyPressed(Key.A)) move -= Right();
        if (Input.IsPhysicalKeyPressed(Key.E)) move += Vector3.Up;
        if (Input.IsPhysicalKeyPressed(Key.Q)) move -= Vector3.Up;

        if (move != Vector3.Zero)
            Position += move.Normalized() * speed;
    }

    public override void _UnhandledInput(InputEvent e)
    {
        if (e is InputEventMouseButton mb)
        {
            switch (mb.ButtonIndex)
            {
                case MouseButton.Right:
                    SetFreelook(mb.Pressed);
                    break;
                case MouseButton.Middle when mb.Pressed:
                    CapturePivot();
                    break;
                case MouseButton.WheelUp when mb.Pressed:
                    if (_freelook) MoveSpeed *= 1.1f; else Dolly(1f);
                    break;
                case MouseButton.WheelDown when mb.Pressed:
                    if (_freelook) MoveSpeed /= 1.1f; else Dolly(-1f);
                    break;
            }
        }
        else if (e is InputEventMouseMotion mm)
        {
            if (_freelook)
            {
                Yaw -= mm.Relative.X * LookSpeed;
                Pitch = Mathf.Clamp(Pitch - mm.Relative.Y * LookSpeed, -PitchLimit, PitchLimit);
                ApplyRotation();
            }
            else if ((mm.ButtonMask & MouseButtonMask.Middle) != 0)
            {
                if (Input.IsKeyPressed(Key.Shift)) Pan(mm.Relative);
                else Orbit(mm.Relative);
            }
        }
    }

    private void SetFreelook(bool on)
    {
        _freelook = on;
        // Capture the cursor while flying (like the editor) for smooth, unbounded mouse-look.
        Input.MouseMode = on ? Input.MouseModeEnum.Captured : Input.MouseModeEnum.Visible;
    }

    /// Capture the point the camera orbits about: where the view ray meets the ground (or a
    /// sensible distance ahead when looking flat).
    private void CapturePivot()
    {
        Vector3 fwd = Forward();
        _pivotDist = (fwd.Y < -1e-4f) ? (-Position.Y / fwd.Y) : Mathf.Max(Position.Y, 1000f);
        _pivot = Position + fwd * _pivotDist;
    }

    private void Orbit(Vector2 rel)
    {
        Yaw -= rel.X * OrbitSpeed;
        Pitch = Mathf.Clamp(Pitch - rel.Y * OrbitSpeed, -PitchLimit, PitchLimit);
        ApplyRotation();
        Position = _pivot - Forward() * _pivotDist; // keep the pivot fixed on screen
    }

    private void Pan(Vector2 rel)
    {
        // Scale by height so a drag covers a consistent fraction of the view at any altitude.
        float scale = PanSpeed * Mathf.Max(Mathf.Abs(Position.Y), 100f) / 1000f;
        Position -= (Right() * rel.X - Up() * rel.Y) * scale;
    }

    private void Dolly(float dir)
    {
        Vector3 fwd = Forward();
        float distToGround = (fwd.Y < -1e-4f) ? (-Position.Y / fwd.Y) : Mathf.Max(Position.Y, 1000f);
        Position += fwd * (distToGround * ZoomStep * dir);
    }

    private void ApplyRotation() => Basis = Basis.FromEuler(new Vector3(Pitch, Yaw, 0f));

    private Vector3 Forward() => -Basis.Z;
    private Vector3 Right() => Basis.X;
    private Vector3 Up() => Basis.Y;
}
