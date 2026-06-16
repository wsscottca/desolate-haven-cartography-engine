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
    private DhceMinimap _minimap;                // overview map, floated in the 3D viewport's top-right corner
    private DhceMapPanel _mapPanel;              // readouts + VIEW selector, docked under the minimap
    private readonly ToolState _tool = new();   // shared by the dock and the viewport picking
    private DhceWorld _edited;                   // the selected/edited DhceWorld (drives picking)
    private bool _painting;                      // a left-drag stroke is in progress
    private bool _needRepaint, _needLiquid;      // coalesce brush-stroke re-tessellation to once/frame
    private MeshInstance3D _gizmo;               // brush-footprint ring under the cursor (ephemeral)
    private readonly List<Vector3> _polyVerts = new(); // in-progress Territory polygon (ground points)
    private MeshInstance3D _polyLine;            // polygon outline overlay (ephemeral)
    private Node3D _caveOverlay;                 // translucent carve-sphere gizmos (ephemeral)
    private int _caveOverlayCount = -1;          // cave count the overlay was built for
    private Vector3? _lastBrushHit;              // last terrain hit (core space) — a streaming anchor

    public override void _EnterTree()
    {
        _dock = new DhceDock();
        _dock.Init(_tool);
        AddControlToDock(DockSlot.RightUl, _dock);
        _minimap = new DhceMinimap();
        AddMinimapOverlay(_minimap);
        _dock.SetMinimap(_minimap); // the dock drives Bind/Refresh; the plugin owns its placement
        _mapPanel = new DhceMapPanel();
        _mapPanel.Init(_tool, _minimap);
        AddMapPanelOverlay(_mapPanel);
        SetProcess(true);
    }

    public override void _ExitTree()
    {
        FreeGizmo();
        ClearPolygon();
        FreeCaveOverlay();
        if (_minimap != null && GodotObject.IsInstanceValid(_minimap))
        {
            _minimap.GetParent()?.RemoveChild(_minimap);
            _minimap.QueueFree();
        }
        _minimap = null;
        if (_mapPanel != null && GodotObject.IsInstanceValid(_mapPanel))
        {
            _mapPanel.GetParent()?.RemoveChild(_mapPanel);
            _mapPanel.QueueFree();
        }
        _mapPanel = null;
        if (_dock != null)
        {
            RemoveControlFromDocks(_dock);
            _dock.QueueFree();
            _dock = null;
        }
    }

    /// Float the overview map in the 3D viewport's top-right corner. Added as a child of the editor's
    /// 3D SubViewport so it renders over the view (the SubViewportContainer forwards GUI input, so the
    /// minimap's own wheel-zoom / drag-pan keep working); anchored top-right with a small margin.
    private static void AddMinimapOverlay(DhceMinimap mm)
    {
        var vp = EditorInterface.Singleton.GetEditorViewport3D(0);
        if (vp == null) return;
        vp.AddChild(mm);
        const float size = 232f, margin = 12f;
        mm.AnchorLeft = 1f; mm.AnchorRight = 1f; mm.AnchorTop = 0f; mm.AnchorBottom = 0f;
        mm.OffsetLeft = -(size + margin); mm.OffsetRight = -margin;
        mm.OffsetTop = margin; mm.OffsetBottom = margin + size;
    }

    /// Float the readouts + VIEW panel directly beneath the minimap, same width and right margin.
    private static void AddMapPanelOverlay(DhceMapPanel p)
    {
        var vp = EditorInterface.Singleton.GetEditorViewport3D(0);
        if (vp == null) return;
        vp.AddChild(p);
        const float size = 232f, margin = 12f, gap = 8f, height = 210f;
        p.AnchorLeft = 1f; p.AnchorRight = 1f; p.AnchorTop = 0f; p.AnchorBottom = 0f;
        p.OffsetLeft = -(size + margin); p.OffsetRight = -margin;
        p.OffsetTop = margin + size + gap; p.OffsetBottom = margin + size + gap + height;
    }

    public override void _Process(double delta)
    {
        // The 3D viewport may not exist yet at _EnterTree; attach the overlays the first frame it does.
        if (_minimap != null && GodotObject.IsInstanceValid(_minimap) && _minimap.GetParent() == null)
            AddMinimapOverlay(_minimap);
        if (_mapPanel != null && GodotObject.IsInstanceValid(_mapPanel) && _mapPanel.GetParent() == null)
            AddMapPanelOverlay(_mapPanel);

        var world = FindWorld();
        _dock?.Bind(world);
        _mapPanel?.SetWorld(world); // so the VIEW switch can recolour the current world
        _dock?.SimTick();
        _dock?.FlushDeferred(); // apply coalesced palette edits once per frame
        // Flush the coalesced brush-stroke re-tessellation once per frame.
        if (_edited != null && _edited.GenDone)
        {
            if (_needRepaint) { _edited.RepaintDirtyTerrain(); _needRepaint = false; }
            if (_needLiquid) { _edited.RebuildLiquid(); _needLiquid = false; }
            // Progressive rain runs every frame while the Rain tool is active; any other tool stops it.
            if (_tool.Active == ToolKind.Rain) _edited.StepRain(delta);
            else if (_edited.RainActive) _edited.StopRain();
        }
        if (world == null || !world.GenDone) return;
        var cam = GetEditorCamera();
        if (cam != null)
        {
            Vector3 ClampCore(Vector3 p) => new Vector3(
                Mathf.Clamp(p.X, 0f, world.WorldWidthM), 0f,
                Mathf.Clamp(p.Z, 0f, world.WorldHeightM));
            Vector3 ground = GroundFocus(cam);                       // editor-space point the camera looks at
            Vector3 look = ClampCore(world.ToCore(ground));          // look ring + the map marker
            Vector3 camFoot = ClampCore(world.ToCore(cam.GlobalPosition)); // footprint: keeps ground under us loaded
            Vector3 brush = _lastBrushHit.HasValue ? ClampCore(_lastBrushHit.Value) : look;
            world.UpdateStreaming(camFoot, look, brush);
            _dock?.SetMapFocus(look);
            // Scale bar: the world span the viewport covers at the focus depth (editor-space distance).
            float dist = cam.GlobalPosition.DistanceTo(ground);
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
            // Ray into core space: shift the origin by the centring offset; the render-root is
            // translation-only so the direction is unchanged. Hits come back in core space.
            Vector3 origin = world.ToCore(camera.ProjectRayOrigin(mouse));
            Vector3 dir = camera.ProjectRayNormal(mouse);
            var hits = world.Engine.Call("raycast_terrain", origin, dir, (double)world.Exaggeration).As<Vector3[]>();
            onTerrain = hits.Length > 0;
            if (onTerrain) { hit = hits[0]; _lastBrushHit = hit; }
            _mapPanel?.SetBiomeReadout(onTerrain ? world.Engine.Call("biome_label_at", hit.X, hit.Z).AsString() : null);
            _mapPanel?.SetRegionReadout(onTerrain ? world.Engine.Call("region_id_at", hit.X, hit.Z).AsInt64() : -1);
            _mapPanel?.SetTraitReadout(
                onTerrain ? world.Engine.Call("trait_at", hit.X, hit.Z, 4).AsDouble() : double.NaN,
                onTerrain ? world.Engine.Call("trait_at", hit.X, hit.Z, 5).AsDouble() : double.NaN);
            // Zoom-coupled brush: radius = fraction of the camera→cursor distance, so the ring keeps a
            // constant on-screen size at any zoom. hit is core space → convert to editor space to measure.
            if (onTerrain)
                _tool.RadiusM = Mathf.Max(_tool.RadiusFraction * camera.GlobalPosition.DistanceTo(world.ToEditor(hit)), 1f);
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

            // Rain tool: a click starts progressive rainfall over the area (a drifting cloud rains down).
            if (_tool.Active == ToolKind.Rain)
            {
                world.StartRain(hit, _tool.RadiusM, _tool.RainRate);
                _painting = true;
                _dock?.SetStatus("raining — drag to move the cloud, switch tools to stop");
                return stop;
            }
            _painting = true;
        }
        else // motion
        {
            var mm = (InputEventMouseMotion)@event;
            if (_tool.Active == ToolKind.RegionSelect) return pass; // Select is click-only
            if (_tool.Active == ToolKind.Rain) // drag relocates the rain area
            {
                if (_painting && mm.ButtonMask.HasFlag(MouseButtonMask.Left) && onTerrain) world.MoveRain(hit, _tool.RadiusM);
                return stop;
            }
            if (!_painting || !mm.ButtonMask.HasFlag(MouseButtonMask.Left) || !onTerrain) return pass;
        }

        EditResult res = _tool.Apply(world.Engine, hit, world.Exaggeration);
        // Coalesce: a fast drag dabs several times per frame; flush the re-tessellation once in
        // _Process so a chunk is rebuilt at most once a frame (the core accumulates dirty chunks).
        if ((res & EditResult.Terrain) != 0) _needRepaint = true;
        if ((res & EditResult.Liquid) != 0) _needLiquid = true;
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
        if (_polyLine == null || !GodotObject.IsInstanceValid(_polyLine) || _polyLine.GetParent() != world.RenderRoot)
        {
            FreePoly();
            _polyLine = new MeshInstance3D { CastShadow = GeometryInstance3D.ShadowCastingSetting.Off };
            world.RenderRoot.AddChild(_polyLine); // owner left null → ephemeral
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
        if (_caveOverlay == null || !GodotObject.IsInstanceValid(_caveOverlay) || _caveOverlay.GetParent() != world.RenderRoot || _caveOverlayCount != world.Caves.Count)
            RebuildCaveOverlay(world);
    }

    private void RebuildCaveOverlay(DhceWorld world)
    {
        FreeCaveOverlay();
        _caveOverlay = new Node3D { Name = "DhceCaveGizmos" };
        world.RenderRoot.AddChild(_caveOverlay); // owner left null → ephemeral
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
        if (_gizmo == null || !GodotObject.IsInstanceValid(_gizmo) || _gizmo.GetParent() != world.RenderRoot)
        {
            FreeGizmo();
            _gizmo = BuildGizmo();
            world.RenderRoot.AddChild(_gizmo); // owner left null → ephemeral, never serialized
        }
        _gizmo.Visible = show;
        if (!show) return;
        float r = Mathf.Max(_tool.RadiusM, 1f);
        _gizmo.Scale = new Vector3(r, 1f, r);
        _gizmo.Position = pos; // pos is core space; the render-root applies the centring offset
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

    /// Project the editor camera's forward ray to the ground plane (Y = 0) in editor/global space — the
    /// point the camera is looking at. Falls back to the camera's own XZ when it isn't looking down.
    /// `_Process` converts this to core space (and clamps to world bounds) for streaming.
    private static Vector3 GroundFocus(Camera3D cam)
    {
        Vector3 o = cam.GlobalPosition;
        Vector3 d = -cam.GlobalTransform.Basis.Z; // forward
        float t = d.Y < -1e-3f ? -o.Y / d.Y : 0f;
        return t > 0f ? o + d * t : o;
    }
}
#endif
