using Godot;

namespace DesolateHaven.Cartography;

/// Orbit camera for the cartography tool: right-mouse drag orbits, wheel zooms,
/// middle-mouse drags pan. Y-up (the terrain lies in the XZ plane, elevation on +Y).
[GlobalClass]
public partial class OrbitCamera : Camera3D
{
    [Export] public Vector3 Target = Vector3.Zero;
    [Export] public float Distance = 4000f;
    [Export] public float Yaw = 0.7f;       // radians around +Y
    [Export] public float Pitch = 0.9f;     // radians above the ground plane
    [Export] public float ZoomStep = 1.1f;
    [Export] public float OrbitSpeed = 0.01f;
    [Export] public float PanSpeed = 1.0f;

    public override void _Ready()
    {
        Near = 1f;
        Far = 200000f;
        UpdateTransform();
    }

    public override void _UnhandledInput(InputEvent e)
    {
        if (e is InputEventMouseMotion mm)
        {
            if ((mm.ButtonMask & MouseButtonMask.Right) != 0)
            {
                Yaw -= mm.Relative.X * OrbitSpeed;
                Pitch = Mathf.Clamp(Pitch - mm.Relative.Y * OrbitSpeed, 0.1f, 1.35f);
                UpdateTransform();
            }
            else if ((mm.ButtonMask & MouseButtonMask.Middle) != 0)
            {
                Vector3 right = GlobalTransform.Basis.X;
                Vector3 up = GlobalTransform.Basis.Y;
                float scale = PanSpeed * (Distance / 4000f);
                Target -= (right * mm.Relative.X - up * mm.Relative.Y) * scale;
                UpdateTransform();
            }
        }
        else if (e is InputEventMouseButton mb && mb.Pressed)
        {
            if (mb.ButtonIndex == MouseButton.WheelUp) { Distance /= ZoomStep; UpdateTransform(); }
            else if (mb.ButtonIndex == MouseButton.WheelDown) { Distance *= ZoomStep; UpdateTransform(); }
        }
    }

    private void UpdateTransform()
    {
        var dir = new Vector3(
            Mathf.Cos(Pitch) * Mathf.Cos(Yaw),
            Mathf.Sin(Pitch),
            Mathf.Cos(Pitch) * Mathf.Sin(Yaw));
        LookAtFromPosition(Target + dir * Distance, Target, Vector3.Up);
    }
}
