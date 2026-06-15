#if TOOLS
using Godot;

namespace DesolateHaven.Cartography;

/// In-editor entry point for the DHCE authoring tool — the editor-plugin reshell (ADR 0005).
/// Editor-only (`#if TOOLS`): a dock with a Generate button, and per-frame streaming of the scene's
/// `DhceWorld` around the editor viewport camera. R2; later stages add viewport picking + the
/// 3D-sphere brush (R3), the full tool dock (R4), and `DhceWorldState` persistence (R5).
[Tool]
public partial class DhcePlugin : EditorPlugin
{
    private Control _dock;
    private Label _status;

    public override void _EnterTree()
    {
        _dock = BuildDock();
        AddControlToDock(DockSlot.RightUl, _dock);
        SetProcess(true);
    }

    public override void _ExitTree()
    {
        if (_dock != null)
        {
            RemoveControlFromDocks(_dock);
            _dock.QueueFree();
            _dock = null;
        }
    }

    public override void _Process(double delta)
    {
        var world = FindWorld();
        if (world == null || !world.GenDone) return;
        var cam = GetEditorCamera();
        if (cam != null) world.UpdateStreaming(GroundFocus(cam, world));
    }

    private Control BuildDock()
    {
        var root = new VBoxContainer { Name = "DHCE" };
        var title = new Label { Text = "DHCE Cartographer" };
        title.AddThemeFontSizeOverride("font_size", 16);
        root.AddChild(title);

        var gen = new Button { Text = "Generate" };
        gen.Pressed += OnGenerate;
        root.AddChild(gen);

        _status = new Label
        {
            Text = "Add a DhceWorld node to the scene, then Generate.",
            AutowrapMode = TextServer.AutowrapMode.WordSmart,
        };
        root.AddChild(_status);
        return root;
    }

    private void OnGenerate()
    {
        var world = FindWorld();
        if (world == null)
        {
            _status.Text = "No DhceWorld in the scene. Add one (Add Node → DhceWorld), then Generate.";
            return;
        }
        _status.Text = "Generating… (the editor pauses for a few seconds)";
        world.Generate();
        _status.Text = $"Generated a {world.WorldSizeKm:0} km world. Fly the viewport to stream it in.";
    }

    /// The DhceWorld in the currently-edited scene (root or first descendant), or null.
    private static DhceWorld FindWorld()
    {
        var root = EditorInterface.Singleton.GetEditedSceneRoot();
        if (root == null) return null;
        return root as DhceWorld ?? FindDescendant(root);
    }

    private static DhceWorld FindDescendant(Node n)
    {
        foreach (var c in n.GetChildren())
        {
            if (c is DhceWorld dw) return dw;
            var found = FindDescendant(c);
            if (found != null) return found;
        }
        return null;
    }

    private static Camera3D GetEditorCamera()
    {
        var vp = EditorInterface.Singleton.GetEditorViewport3D(0);
        return vp?.GetCamera3D();
    }

    /// Project the editor camera's forward ray to the ground (Y = 0), clamped to world bounds — the
    /// streaming focus. Falls back to the camera's XZ when it isn't looking down.
    private static Vector3 GroundFocus(Camera3D cam, DhceWorld world)
    {
        Vector3 o = cam.GlobalPosition;
        Vector3 d = -cam.GlobalTransform.Basis.Z; // forward
        float t = d.Y < -1e-3f ? -o.Y / d.Y : 0f;
        Vector3 g = t > 0f ? o + d * t : o;
        return new Vector3(
            Mathf.Clamp(g.X, 0f, world.WorldWidthM),
            0f,
            Mathf.Clamp(g.Z, 0f, world.WorldHeightM));
    }
}
#endif
