#if TOOLS
using Godot;

namespace DesolateHaven.Cartography;

/// In-editor overview map (reshell R4b): a zoomable/pannable topographic + biome image rendered by
/// the core's `minimap`, with a marker at the editor-camera ground focus. Decoupled from
/// `CartographerSpike` (the runtime `MinimapPanel`) — it takes the `DhceEngine` + world bounds and a
/// focus point. Especially useful now that 3D streaming only meshes a small render-distance ring:
/// the whole world stays visible here. Click-to-fly is dropped (no editor-camera reposition API);
/// wheel zooms, drag pans.
public partial class DhceMinimap : Control
{
    private const int N = 384; // core render resolution

    private GodotObject _engine;
    private float _widthM = 1f, _heightM = 1f;
    private ImageTexture _tex;
    private Vector3 _focus;
    private float _zoom = 1f;
    private Vector2 _viewCenter = new(0.5f, 0.5f); // normalized
    private bool _dragging;
    private Vector2 _dragLast;

    public override void _Ready()
    {
        CustomMinimumSize = new Vector2(232, 232);
        TooltipText = "Wheel: zoom · drag: pan";
    }

    /// Bind to a generated world's engine + bounds (call once gen is done).
    public void Bind(GodotObject engine, float widthM, float heightM)
    {
        _engine = engine;
        _widthM = Mathf.Max(widthM, 1f);
        _heightM = Mathf.Max(heightM, 1f);
    }

    /// Editor-camera ground focus → live marker. Cheap; only redraws when it actually moves.
    public void SetFocus(Vector3 f)
    {
        if (f == _focus) return;
        _focus = f;
        QueueRedraw();
    }

    /// Re-render the overview from the core. Call after gen and after batch edits (shape/blend/palette).
    public void Refresh()
    {
        if (_engine == null) return;
        var bytes = _engine.Call("minimap", N, -0.7, -0.7).As<byte[]>(); // fixed NW light for shading
        if (bytes.Length != N * N * 4) return;
        var img = Image.CreateFromData(N, N, false, Image.Format.Rgba8, bytes);
        _tex = ImageTexture.CreateFromImage(img);
        QueueRedraw();
    }

    public override void _Draw()
    {
        var size = Size;
        DrawRect(new Rect2(Vector2.Zero, size), new Color(0.08f, 0.08f, 0.10f));
        if (_tex == null)
        {
            DrawRect(new Rect2(Vector2.Zero, size), new Color(0.4f, 0.4f, 0.4f), false, 1);
            return;
        }

        float ext = 1f / _zoom;
        Vector2 origin = _viewCenter - new Vector2(ext, ext) * 0.5f;
        var src = new Rect2(origin.X * N, origin.Y * N, ext * N, ext * N);
        DrawTextureRectRegion(_tex, new Rect2(Vector2.Zero, size), src);

        var norm = new Vector2(_focus.X / _widthM, _focus.Z / _heightM);
        Vector2 p = (norm - origin) * _zoom * size;
        if (p.X >= 0 && p.Y >= 0 && p.X <= size.X && p.Y <= size.Y)
        {
            DrawCircle(p, 5f, new Color(0.95f, 0.8f, 0.2f));
            DrawCircle(p, 2.5f, new Color(0.08f, 0.08f, 0.10f));
        }
        DrawRect(new Rect2(Vector2.Zero, size), new Color(0.95f, 0.8f, 0.2f), false, 1);
    }

    public override void _GuiInput(InputEvent e)
    {
        if (e is InputEventMouseButton mb)
        {
            if (mb.ButtonIndex == MouseButton.WheelUp && mb.Pressed) { SetZoom(_zoom * 1.2f); AcceptEvent(); }
            else if (mb.ButtonIndex == MouseButton.WheelDown && mb.Pressed) { SetZoom(_zoom / 1.2f); AcceptEvent(); }
            else if (mb.ButtonIndex == MouseButton.Left)
            {
                _dragging = mb.Pressed;
                _dragLast = mb.Position;
                AcceptEvent();
            }
        }
        else if (e is InputEventMouseMotion mm && _dragging)
        {
            Vector2 d = mm.Position - _dragLast;
            _dragLast = mm.Position;
            _viewCenter -= d / Size / _zoom; // drag right → view moves left
            ClampCenter();
            QueueRedraw();
            AcceptEvent();
        }
    }

    private void SetZoom(float z)
    {
        _zoom = Mathf.Clamp(z, 1f, 12f);
        ClampCenter();
        QueueRedraw();
    }

    private void ClampCenter()
    {
        float half = 0.5f / _zoom;
        _viewCenter.X = Mathf.Clamp(_viewCenter.X, half, 1f - half);
        _viewCenter.Y = Mathf.Clamp(_viewCenter.Y, half, 1f - half);
    }
}
#endif
