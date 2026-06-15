#if TOOLS
using Godot;
using System.Collections.Generic;

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
    private MeshInstance3D _gizmo;               // brush-footprint ring under the cursor (ephemeral)
    private readonly List<Vector3> _polyVerts = new(); // in-progress Territory polygon (ground points)
    private MeshInstance3D _polyLine;            // polygon outline overlay (ephemeral)
    private Node3D _caveOverlay;                 // translucent carve-sphere gizmos (ephemeral)
    private int _caveOverlayCount = -1;          // cave count the overlay was built for

    public override void _EnterTree()
    {
        _dock = new DhceDock();
        _dock.Init(_tool);
        AddControlToDock(DockSlot.RightUl, _dock);
        SetProcess(true);
    }

    public override void _ExitTree()
    {
        FreeGizmo();
        ClearPolygon();
        FreeCaveOverlay();
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
        if (cam != null)
        {
            Vector3 focus = GroundFocus(cam, world);
            world.UpdateStreaming(focus);
            _dock?.SetMapFocus(focus);
            // Scale bar: the world span the viewport covers at the focus depth.
            float dist = cam.GlobalPosition.DistanceTo(focus);
            _dock?.SetViewScale(2.0 * dist * Mathf.Tan(Mathf.DegToRad(cam.Fov) * 0.5));
        }
    }

    // --- viewport picking: only active while a DhceWorld is selected ---

    public override bool _Handles(GodotObject @object) => @object is DhceWorld;

    public override void _Edit(GodotObject @object)
    {
        _edited = @object as DhceWorld;
        _painting = false;
        ClearPolygon();
        FreeCaveOverlay();
    }

    public override void _MakeVisible(bool visible)
    {
        if (!visible) { _edited = null; _painting = false; FreeGizmo(); ClearPolygon(); FreeCaveOverlay(); }
    }

    /// Route 3D viewport input to the active tool: brush stroke (sculpt/trait/region brush), polygon
    /// Territory (multi-click → `regions_in_polygon` → `assign_region`), or Select (flood-assign).
    /// All use the editor camera's own ray so edits land under the cursor from any angle.
    public override int _Forward3DGuiInput(Camera3D camera, InputEvent @event)
    {
        int pass = (int)EditorPlugin.AfterGuiInput.Pass;
        int stop = (int)EditorPlugin.AfterGuiInput.Stop;

        var world = _edited;
        if (world == null || !world.GenDone || world.Engine == null || camera == null)
            return pass;

        bool isButton = @event is InputEventMouseButton;
        bool isMotion = @event is InputEventMouseMotion;
        bool isKey = @event is InputEventKey;
        if (!isButton && !isMotion && !isKey) return pass;

        // Leaving the Territory tool mid-polygon cancels the in-progress outline.
        if (_tool.Active != ToolKind.Territory && _polyVerts.Count > 0) ClearPolygon();

        // Resolve the hovered surface point (drives readouts, the gizmo, and polygon vertices).
        Vector3 hit = Vector3.Zero;
        bool onTerrain = false;
        if (!isKey)
        {
            Vector2 mouse = isButton ? ((InputEventMouseButton)@event).Position : ((InputEventMouseMotion)@event).Position;
            Vector3 origin = camera.ProjectRayOrigin(mouse);
            Vector3 dir = camera.ProjectRayNormal(mouse);
            var hits = world.Engine.Call("raycast_terrain", origin, dir, (double)world.Exaggeration).As<Vector3[]>();
            onTerrain = hits.Length > 0;
            if (onTerrain) hit = hits[0];
            _dock?.SetBiomeReadout(onTerrain ? world.Engine.Call("biome_label_at", hit.X, hit.Z).AsString() : null);
            _dock?.SetRegionReadout(onTerrain ? world.Engine.Call("region_id_at", hit.X, hit.Z).AsInt64() : -1);
            _dock?.SetTraitReadout(
                onTerrain ? world.Engine.Call("trait_at", hit.X, hit.Z, 4).AsDouble() : double.NaN,
                onTerrain ? world.Engine.Call("trait_at", hit.X, hit.Z, 5).AsDouble() : double.NaN);
        }

        // Polygon Territory tool owns clicks; no brush gizmo while it's active.
        if (_tool.Active == ToolKind.Territory)
        {
            FreeGizmo();
            return HandlePolygon(world, @event, hit, onTerrain) ? stop : pass;
        }

        // Cave tool places volumetric carve spheres (carved into geometry at export); sphere gizmos, no ring.
        if (_tool.Active == ToolKind.Cave)
        {
            FreeGizmo();
            UpdateCaveOverlay(world, true);
            if (@event is InputEventMouseButton cb && cb.ButtonIndex == MouseButton.Left)
            {
                if (!cb.Pressed) return stop;
                if (!onTerrain) return pass;
                world.AddCave(hit, Mathf.Max(_tool.RadiusM, 1f));
                RebuildCaveOverlay(world);
                _dock?.SetStatus($"placed cave ({world.CaveCount}) — carved into geometry on export");
                return stop;
            }
            return pass;
        }
        UpdateCaveOverlay(world, false); // hide cave gizmos outside Cave mode

        UpdateGizmo(world, onTerrain ? hit : Vector3.Zero, onTerrain);
        if (isKey) return pass;

        if (isButton)
        {
            var mb = (InputEventMouseButton)@event;
            if (mb.ButtonIndex != MouseButton.Left) return pass;
            if (!mb.Pressed) { bool was = _painting; _painting = false; return was ? stop : pass; }
            if (!onTerrain) { _painting = false; return pass; } // click off terrain → let editor select

            // Select tool: one click floods the contiguous same-biome area and assigns the Region.
            if (_tool.Active == ToolKind.RegionSelect)
            {
                long cell = world.Engine.Call("region_at", hit.X, hit.Z).AsInt64();
                if (cell >= 0)
                {
                    var sel = world.Engine.Call("select_contiguous", cell).As<int[]>();
                    world.Engine.Call("assign_region", sel, _tool.RegionId);
                    world.RepaintDirtyTerrain();
                    _dock?.SetStatus($"assigned {sel.Length} cells to the Region (flood)");
                }
                return stop;
            }
            _painting = true;
        }
        else // motion
        {
            var mm = (InputEventMouseMotion)@event;
            if (_tool.Active == ToolKind.RegionSelect) return pass; // Select is click-only
            if (!_painting || !mm.ButtonMask.HasFlag(MouseButtonMask.Left) || !onTerrain) return pass;
        }

        EditResult res = _tool.Apply(world.Engine, hit, world.Exaggeration);
        if ((res & EditResult.Terrain) != 0) world.RepaintDirtyTerrain();
        if ((res & EditResult.Liquid) != 0) world.RebuildLiquid();
        return stop;
    }

    // --- Territory polygon tool: left-click places vertices, right-click closes, Esc cancels ---

    private bool HandlePolygon(DhceWorld world, InputEvent e, Vector3 hit, bool onTerrain)
    {
        if (e is InputEventKey k && k.Pressed && k.Keycode == Key.Escape)
        {
            ClearPolygon();
            _dock?.SetStatus("polygon cancelled");
            return true;
        }
        if (e is InputEventMouseMotion)
        {
            if (_polyVerts.Count > 0) UpdatePolyOverlay(world, onTerrain ? hit : (Vector3?)null);
            return false; // don't consume motion → camera navigation still works while outlining
        }
        if (e is InputEventMouseButton mb && mb.Pressed)
        {
            if (mb.ButtonIndex == MouseButton.Left)
            {
                if (!onTerrain) return false;
                _polyVerts.Add(hit);
                UpdatePolyOverlay(world, null);
                _dock?.SetStatus($"polygon: {_polyVerts.Count} pts — right-click to close, Esc to cancel");
                return true;
            }
            if (mb.ButtonIndex == MouseButton.Right && _polyVerts.Count > 0)
            {
                ClosePolygon(world);
                return true;
            }
        }
        return false;
    }

    private void ClosePolygon(DhceWorld world)
    {
        if (_polyVerts.Count >= 3)
        {
            var xs = new float[_polyVerts.Count];
            var ys = new float[_polyVerts.Count];
            for (int i = 0; i < _polyVerts.Count; i++) { xs[i] = _polyVerts[i].X; ys[i] = _polyVerts[i].Z; }
            var cells = world.Engine.Call("regions_in_polygon", xs, ys).As<int[]>();
            world.Engine.Call("assign_region", cells, _tool.RegionId);
            world.RepaintDirtyTerrain();
            _dock?.SetStatus($"assigned {cells.Length} cells to the Region (polygon)");
        }
        else _dock?.SetStatus("polygon needs at least 3 points");
        ClearPolygon();
    }

    private void UpdatePolyOverlay(DhceWorld world, Vector3? cursor)
    {
        if (_polyLine == null || !GodotObject.IsInstanceValid(_polyLine) || _polyLine.GetParent() != world)
        {
            FreePoly();
            _polyLine = new MeshInstance3D { CastShadow = GeometryInstance3D.ShadowCastingSetting.Off };
            world.AddChild(_polyLine); // owner left null → ephemeral
        }
        var mat = new StandardMaterial3D
        {
            ShadingMode = BaseMaterial3D.ShadingModeEnum.Unshaded,
            AlbedoColor = new Color(0.30f, 0.85f, 1.0f),
            NoDepthTest = true,
        };
        var im = new ImmediateMesh();
        im.SurfaceBegin(Mesh.PrimitiveType.LineStrip, mat);
        foreach (var v in _polyVerts) im.SurfaceAddVertex(v);
        if (cursor.HasValue) im.SurfaceAddVertex(cursor.Value); // rubber-band to the cursor
        if (_polyVerts.Count > 0) im.SurfaceAddVertex(_polyVerts[0]); // closing hint
        im.SurfaceEnd();
        _polyLine.Mesh = im;
    }

    private void ClearPolygon()
    {
        _polyVerts.Clear();
        FreePoly();
    }

    private void FreePoly()
    {
        if (_polyLine != null && GodotObject.IsInstanceValid(_polyLine)) _polyLine.QueueFree();
        _polyLine = null;
    }

    // --- cave carve-volume gizmos (translucent spheres; geometry baked at export) ---

    private void UpdateCaveOverlay(DhceWorld world, bool show)
    {
        if (!show) { FreeCaveOverlay(); return; }
        if (_caveOverlay == null || !GodotObject.IsInstanceValid(_caveOverlay) || _caveOverlay.GetParent() != world || _caveOverlayCount != world.Caves.Count)
            RebuildCaveOverlay(world);
    }

    private void RebuildCaveOverlay(DhceWorld world)
    {
        FreeCaveOverlay();
        _caveOverlay = new Node3D { Name = "DhceCaveGizmos" };
        world.AddChild(_caveOverlay); // owner left null → ephemeral
        var mat = new StandardMaterial3D
        {
            ShadingMode = BaseMaterial3D.ShadingModeEnum.Unshaded,
            AlbedoColor = new Color(0.95f, 0.4f, 0.2f, 0.35f),
            Transparency = BaseMaterial3D.TransparencyEnum.Alpha,
            CullMode = BaseMaterial3D.CullModeEnum.Disabled,
        };
        foreach (Vector4 c in world.Caves)
        {
            _caveOverlay.AddChild(new MeshInstance3D
            {
                Mesh = new SphereMesh { Radius = c.W, Height = c.W * 2f },
                MaterialOverride = mat,
                Position = new Vector3(c.X, c.Y, c.Z),
                CastShadow = GeometryInstance3D.ShadowCastingSetting.Off,
            });
        }
        _caveOverlayCount = world.Caves.Count;
    }

    private void FreeCaveOverlay()
    {
        if (_caveOverlay != null && GodotObject.IsInstanceValid(_caveOverlay)) _caveOverlay.QueueFree();
        _caveOverlay = null;
        _caveOverlayCount = -1;
    }

    // --- brush footprint gizmo (a flat ring laid on the surface under the cursor) ---

    private void UpdateGizmo(DhceWorld world, Vector3 pos, bool show)
    {
        if (_gizmo == null || !GodotObject.IsInstanceValid(_gizmo) || _gizmo.GetParent() != world)
        {
            FreeGizmo();
            _gizmo = BuildGizmo();
            world.AddChild(_gizmo); // owner left null → ephemeral, never serialized
        }
        _gizmo.Visible = show;
        if (!show) return;
        float r = Mathf.Max(_tool.RadiusM, 1f);
        _gizmo.Scale = new Vector3(r, 1f, r);
        _gizmo.GlobalPosition = pos;
    }

    private void FreeGizmo()
    {
        if (_gizmo != null && GodotObject.IsInstanceValid(_gizmo)) _gizmo.QueueFree();
        _gizmo = null;
    }

    private static MeshInstance3D BuildGizmo()
    {
        var mat = new StandardMaterial3D
        {
            ShadingMode = BaseMaterial3D.ShadingModeEnum.Unshaded,
            AlbedoColor = new Color(0.95f, 0.85f, 0.25f),
            NoDepthTest = true, // always visible as a cursor, even behind a ridge
        };
        var im = new ImmediateMesh();
        im.SurfaceBegin(Mesh.PrimitiveType.LineStrip, mat);
        const int seg = 48;
        for (int i = 0; i <= seg; i++)
        {
            float a = Mathf.Tau * i / seg;
            im.SurfaceAddVertex(new Vector3(Mathf.Cos(a), 0f, Mathf.Sin(a))); // unit ring in XZ
        }
        im.SurfaceEnd();
        return new MeshInstance3D { Mesh = im, CastShadow = GeometryInstance3D.ShadowCastingSetting.Off };
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
