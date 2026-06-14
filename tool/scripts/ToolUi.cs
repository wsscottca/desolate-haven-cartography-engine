using Godot;

namespace DesolateHaven.Cartography;

/// On-screen authoring overlay (runtime tool, F5 — not an editor dock). One left-side panel
/// with sections: Tools, Brush, World, Physics, and a status line. Mutates the shared ToolState
/// and calls back into CartographerSpike for regenerate / repaint / liquid. The panel is a
/// PanelContainer (MouseFilter = Stop), so clicks on it are consumed before terrain painting.
public partial class ToolUi : CanvasLayer
{
    /// Set by CartographerSpike before AddChild; the UI's single dependency.
    public CartographerSpike Root;

    private readonly ButtonGroup _toolGroup = new ButtonGroup();
    private Label _status;
    private OptionButton _biome, _liquidKind;
    private static readonly string[] BiomeNames =
    {
        "Jagged Mountains", "Sacred Woods Plateau", "Great Lake", "Temperate Forest",
        "Open Plains", "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles",
        "Blisterwood", "Volcanic Scape", "Blight Ruins", "Scattered Isles", "Marsh Bog",
    };

    public override void _Ready()
    {
        var panel = new PanelContainer();
        panel.SetAnchorsAndOffsetsPreset(Control.LayoutPreset.TopLeft);
        panel.Position = new Vector2(8, 8);
        AddChild(panel);

        var col = new VBoxContainer { CustomMinimumSize = new Vector2(240, 0) };
        panel.AddChild(col);

        Section(col, "TOOLS");
        AddToolButton(col, "Raise", ToolKind.Raise);
        AddToolButton(col, "Carve", ToolKind.Carve);
        AddToolButton(col, "Level", ToolKind.Level);
        AddToolButton(col, "Crest", ToolKind.Crest);
        AddToolButton(col, "River", ToolKind.River);
        AddToolButton(col, "Flood", ToolKind.Flood);
        AddToolButton(col, "Biome", ToolKind.Biome);

        Section(col, "BRUSH");
        Slider(col, "Radius (m)", 10, 2000, 5, Root.Tool.RadiusM, v => Root.Tool.RadiusM = (float)v);
        Slider(col, "Strength (m)", 1, 400, 1, Root.Tool.StrengthM, v => Root.Tool.StrengthM = (float)v);

        _biome = new OptionButton();
        for (int i = 0; i < BiomeNames.Length; i++) _biome.AddItem($"{i + 1}. {BiomeNames[i]}", i + 1);
        _biome.Selected = 0;
        _biome.ItemSelected += idx => Root.Tool.BiomeId = _biome.GetItemId((int)idx);
        Labeled(col, "Biome", _biome);

        _liquidKind = new OptionButton();
        _liquidKind.AddItem("Water", 0);
        _liquidKind.AddItem("Lava", 1);
        _liquidKind.Selected = 0;
        _liquidKind.ItemSelected += idx => Root.Tool.LiquidKind = (int)idx;
        Labeled(col, "Liquid", _liquidKind);

        _status = new Label { Text = "ready", AutowrapMode = TextServer.AutowrapMode.WordSmart };
        col.AddChild(new HSeparator());
        col.AddChild(_status);
    }

    public void SetStatus(string text) => _status.Text = text;

    // --- builder helpers (DRY) ---

    private void Section(Container parent, string title)
    {
        parent.AddChild(new HSeparator());
        parent.AddChild(new Label { Text = title });
    }

    private void AddToolButton(Container parent, string text, ToolKind tool)
    {
        var b = new Button { Text = text, ToggleMode = true, ButtonGroup = _toolGroup };
        b.Pressed += () => Root.Tool.Active = tool;
        if (tool == Root.Tool.Active) b.ButtonPressed = true; // pre-select the default tool
        parent.AddChild(b);
    }

    /// A labelled HSlider row; calls `onChange` on every move. Returns the slider for callers
    /// that need to drive it (e.g. keyboard brush-size shortcuts in Task 6).
    private HSlider Slider(Container parent, string label, double min, double max, double step,
                           double val, System.Action<double> onChange)
    {
        var lbl = new Label { Text = $"{label}: {val:0.###}" };
        parent.AddChild(lbl);
        var s = new HSlider { MinValue = min, MaxValue = max, Step = step, Value = val };
        s.ValueChanged += v => { lbl.Text = $"{label}: {v:0.###}"; onChange(v); };
        parent.AddChild(s);
        return s;
    }

    private void Labeled(Container parent, string label, Control control)
    {
        parent.AddChild(new Label { Text = label });
        parent.AddChild(control);
    }
}
