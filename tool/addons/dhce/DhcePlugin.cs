#if TOOLS
using Godot;

namespace DesolateHaven.Cartography;

/// In-editor entry point for the DHCE authoring tool — the editor-plugin reshell (ADR 0005).
/// Editor-only (`#if TOOLS`): hosts the `DhceDock` (full tool panel, R4), streams the scene's
/// `DhceWorld` around the editor camera each frame, and — when a `DhceWorld` is selected — forwards
/// 3D viewport input (`_Forward3DGuiInput`) into a brush stroke: ray from the editor camera →
/// `raycast_terrain` → `ToolState.Apply` (which sets the 3D-sphere brush, R3). Persistence is R5.
[Tool]
public partial class DhcePlugin : EditorPlugin
{
    private DhceDock _dock;
    private readonly ToolState _tool = new();   // shared by the dock and the viewport picking
    private DhceWorld _edited;                   // the selected/edited DhceWorld (drives picking)
    private bool _painting;                      // a left-drag stroke is in progress

    public override void _EnterTree()
    {
        _dock = new DhceDock();
        _dock.Init(_tool);
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
        _dock?.Bind(world);
        _dock?.SimTick();
        if (world == null || !world.GenDone) return;
        var cam = GetEditorCamera();
        if (cam != null) world.UpdateStreaming(GroundFocus(cam, world));
    }

    // --- viewport picking: only active while a DhceWorld is selected ---

    public override bool _Handles(GodotObject @object) => @object is DhceWorld;

    public override void _Edit(GodotObject @object)
    {
        _edited = @object as DhceWorld;
        _painting = false;
    }

    public override void _MakeVisible(bool visible)
    {
        if (!visible) { _edited = null; _painting = false; }
    }

    /// Turn a left-drag in the 3D viewport into a brush stroke on the selected world. Uses the editor
    /// camera's own ray so the brush lands under the cursor from any angle; the 3D-sphere falloff
    /// (set inside `ToolState.Apply`) keeps it biting the surface, not a vertical column.
    public override int _Forward3DGuiInput(Camera3D camera, InputEvent @event)
    {
        int pass = (int)EditorPlugin.AfterGuiInput.Pass;
        int stop = (int)EditorPlugin.AfterGuiInput.Stop;

        var world = _edited;
        if (world == null || !world.GenDone || world.Engine == null || camera == null)
            return pass;

        Vector2 mouse;
        if (@event is InputEventMouseButton mb)
        {
            if (mb.ButtonIndex != MouseButton.Left) return pass;
            if (!mb.Pressed)
            {
                bool was = _painting;
                _painting = false;
                return was ? stop : pass;
            }
            mouse = mb.Position;
        }
        else if (@event is InputEventMouseMotion mm)
        {
            if (!_painting || !mm.ButtonMask.HasFlag(MouseButtonMask.Left)) return pass;
            mouse = mm.Position;
        }
        else return pass;

        Vector3 origin = camera.ProjectRayOrigin(mouse);
        Vector3 dir = camera.ProjectRayNormal(mouse);
        var hits = world.Engine.Call("raycast_terrain", origin, dir, (double)world.Exaggeration).As<Vector3[]>();
        if (hits.Length == 0)
        {
            _painting = false; // missed terrain → let the editor select/navigate normally
            return pass;
        }
        _painting = true;
        EditResult res = _tool.Apply(world.Engine, hits[0], world.Exaggeration);
        if ((res & EditResult.Terrain) != 0) world.RepaintDirtyTerrain();
        if ((res & EditResult.Liquid) != 0) world.RebuildLiquid();
        return stop;
    }

    // --- helpers ---

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
