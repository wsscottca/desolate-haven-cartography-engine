using Godot;

namespace DesolateHaven.Cartography;

/// Top-down overview map: a zoomable, pannable topographic + biome image (rendered by the core's
/// minimap), with a marker at the camera focus. Wheel zooms, drag pans, click flies the 3D camera
/// there. Carries the whole-map view so the 3D scene only needs near-detail render distance.
public partial class MinimapPanel : Control
{
    public CartographerSpike Root;

    private const int N = 384;        // minimap render resolution
    private ImageTexture _tex;
    private float _zoom = 1f;          // 1 = whole map
    private Vector2 _center = new Vector2(0.5f, 0.5f); // view centre, normalized
    private bool _dragging;
    private Vector2 _dragLast;
    private float _moved;

    public override void _Ready()
    {
        CustomMinimumSize = new Vector2(232, 232);
        TooltipText = "Wheel: zoom · drag: pan · click: fly there";
    }

    /// Re-render the minimap image from the core (call after gen and after edits).
    public void Refresh()
    {
        if (Root?.Engine == null) return;
        var bytes = Root.Engine.Call("minimap", N).As<byte[]>();
        if (bytes.Length != N * N * 4) return;
        var img = Image.CreateFromData(N, N, false, Image.Format.Rgba8, bytes);
        _tex = ImageTexture.CreateFromImage(img);
        QueueRedraw();
    }

    public override void _Draw()
    {
        var size = Size;
        DrawRect(new Rect2(Vector2.Zero, size), ToolTheme.Bg2);
        if (_tex == null)
        {
            DrawRect(new Rect2(Vector2.Zero, size), ToolTheme.Rule, false, 1);
            return;
        }

        float ext = 1f / _zoom;                       // visible fraction of the map
        Vector2 origin = _center - new Vector2(ext, ext) * 0.5f; // top-left of view, normalized
        var src = new Rect2(origin.X * N, origin.Y * N, ext * N, ext * N);
        DrawTextureRectRegion(_tex, new Rect2(Vector2.Zero, size), src);

        // Camera-focus marker.
        if (Root != null)
        {
            Vector3 f = Root.CameraFocus;
            var norm = new Vector2(f.X / Mathf.Max(Root.WorldWidthM, 1f), f.Z / Mathf.Max(Root.WorldHeightM, 1f));
            Vector2 p = (norm - origin) * _zoom * size;
            if (p.X >= 0 && p.Y >= 0 && p.X <= size.X && p.Y <= size.Y)
            {
                DrawCircle(p, 5f, ToolTheme.Gold);
                DrawCircle(p, 2.5f, ToolTheme.Bg);
            }
        }

        DrawRect(new Rect2(Vector2.Zero, size), ToolTheme.Gold, false, 1);
    }

    public override void _GuiInput(InputEvent e)
    {
        if (e is InputEventMouseButton mb)
        {
            if (mb.ButtonIndex == MouseButton.WheelUp && mb.Pressed) { SetZoom(_zoom * 1.2f); AcceptEvent(); }
            else if (mb.ButtonIndex == MouseButton.WheelDown && mb.Pressed) { SetZoom(_zoom / 1.2f); AcceptEvent(); }
            else if (mb.ButtonIndex == MouseButton.Left)
            {
                if (mb.Pressed) { _dragging = true; _dragLast = mb.Position; _moved = 0f; }
                else { _dragging = false; if (_moved < 6f) FlyToControlPos(mb.Position); }
                AcceptEvent();
            }
        }
        else if (e is InputEventMouseMotion mm && _dragging)
        {
            Vector2 d = mm.Position - _dragLast;
            _dragLast = mm.Position;
            _moved += d.Length();
            _center -= d / Size / _zoom; // drag right → view moves left
            ClampCenter();
            QueueRedraw();
            AcceptEvent();
        }
    }

    public override void _Process(double delta)
    {
        if (_tex != null) QueueRedraw(); // keep the camera marker live as you fly
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
        _center.X = Mathf.Clamp(_center.X, half, 1f - half);
        _center.Y = Mathf.Clamp(_center.Y, half, 1f - half);
    }

    private void FlyToControlPos(Vector2 pos)
    {
        if (Root == null) return;
        float ext = 1f / _zoom;
        Vector2 origin = _center - new Vector2(ext, ext) * 0.5f;
        Vector2 norm = origin + pos / Size * ext;
        Root.FlyTo(norm.X * Root.WorldWidthM, norm.Y * Root.WorldHeightM);
    }
}
