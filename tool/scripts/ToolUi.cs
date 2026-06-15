using Godot;
using System.Collections.Generic;

namespace DesolateHaven.Cartography;

/// On-screen authoring overlay (runtime tool, F5 — not an editor dock), styled to the Desolate
/// Haven Visual Style Guide and laid out like the "Sundered Vale cartographer's table" mockup:
/// a left dock (title, Terrain|Biomes tabs, brush-size dots, tool/world sections) and a right
/// panel (physics). Mutates the shared ToolState and calls back into CartographerSpike. Panels
/// are PanelContainers (MouseFilter = Stop), so clicks on them never paint terrain.
public partial class ToolUi : CanvasLayer
{
    public CartographerSpike Root;

    private readonly ButtonGroup _toolGroup = new ButtonGroup();
    private readonly ButtonGroup _sizeGroup = new ButtonGroup();
    private readonly ButtonGroup _biomeGroup = new ButtonGroup();
    private readonly List<Button> _toolButtons = new();
    private readonly List<Button> _sizeDots = new();
    private Label _status;
    private VBoxContainer _terrainTab, _biomesTab;
    private Button _tabTerrain, _tabBiomes;
    private OptionButton _liquidKind;
    private SpinBox _seed, _oct, _size, _spacing;
    private ToolKind _lastTerrainTool = ToolKind.Raise;

    private static readonly float[] BrushFractions = { 0.03f, 0.06f, 0.12f, 0.25f, 0.5f };
    private static readonly int[] DotFontSizes = { 9, 12, 16, 20, 25 };

    private double _simFlow = 0.45, _simEvap = 0.001;
    private int _simSubsteps = 10, _simTick;
    private CheckButton _simulate;
    private const int SimEveryNFrames = 6;

    private static readonly string[] BiomeNames =
    {
        "Jagged Mountains", "Sacred Woods Plateau", "Great Lake", "Temperate Forest",
        "Open Plains", "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles",
        "Blisterwood", "Volcanic Scape", "Blight Ruins", "Scattered Isles", "Marsh & Bog",
    };

    public override void _Ready()
    {
        var rootCtl = new Control { MouseFilter = Control.MouseFilterEnum.Ignore, Theme = ToolTheme.Build() };
        rootCtl.SetAnchorsAndOffsetsPreset(Control.LayoutPreset.FullRect);
        AddChild(rootCtl);

        BuildLeftDock(rootCtl);
        BuildRightPanel(rootCtl);

        Root.Tool.RadiusFraction = BrushFractions[2];
        SetTab(true);
    }

    // --- left dock ---------------------------------------------------------------------------

    private void BuildLeftDock(Control parent)
    {
        var dock = new PanelContainer { Position = new Vector2(10, 10) };
        dock.CustomMinimumSize = new Vector2(252, 0);
        parent.AddChild(dock);
        var col = new VBoxContainer();
        dock.AddChild(col);

        var brand = new Label { Text = "DESOLATE HAVEN" };
        brand.AddThemeColorOverride("font_color", ToolTheme.Gold);
        brand.AddThemeFontSizeOverride("font_size", 11);
        col.AddChild(brand);
        var title = new Label { Text = "The Loom" };
        title.AddThemeColorOverride("font_color", ToolTheme.Ink);
        title.AddThemeFontSizeOverride("font_size", 30);
        if (ToolTheme.Display != null) title.AddThemeFontOverride("font", ToolTheme.Display);
        col.AddChild(title);
        var sub = new Label { Text = "cartographer's table" };
        sub.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        col.AddChild(sub);

        var tabs = new HBoxContainer();
        _tabTerrain = TabButton("Terrain", () => SetTab(true));
        _tabBiomes = TabButton("Biomes", () => SetTab(false));
        tabs.AddChild(_tabTerrain);
        tabs.AddChild(_tabBiomes);
        col.AddChild(tabs);

        col.AddChild(ToolTheme.Header("BRUSH SIZE"));
        var dots = new HBoxContainer();
        for (int i = 0; i < BrushFractions.Length; i++) dots.AddChild(SizeDot(i));
        col.AddChild(dots);

        _terrainTab = new VBoxContainer();
        col.AddChild(_terrainTab);
        BuildTerrainTab(_terrainTab);

        _biomesTab = new VBoxContainer { Visible = false };
        col.AddChild(_biomesTab);
        BuildBiomesTab(_biomesTab);

        col.AddChild(new HSeparator());
        _status = new Label { Text = "ready", AutowrapMode = TextServer.AutowrapMode.WordSmart };
        _status.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        col.AddChild(_status);
    }

    private void BuildTerrainTab(VBoxContainer col)
    {
        col.AddChild(ToolTheme.Header("TERRAIN TOOLS"));
        var grid = new GridContainer { Columns = 2 };
        col.AddChild(grid);
        AddToolButton(grid, "Raise", ToolKind.Raise);
        AddToolButton(grid, "Carve", ToolKind.Carve);
        AddToolButton(grid, "Level", ToolKind.Level);
        AddToolButton(grid, "Crest", ToolKind.Crest);
        AddToolButton(grid, "River", ToolKind.River);
        AddToolButton(grid, "Flood", ToolKind.Flood);

        var streams = GhostButton("Generate Streams");
        streams.Pressed += () =>
        {
            Root.Engine.Call("generate_streams", 0.5, 1.0);
            Root.RepaintDirtyTerrain();
            Root.RebuildLiquid();
        };
        col.AddChild(streams);

        col.AddChild(ToolTheme.Header("BRUSH"));
        Slider(col, "Strength (m)", 1, 400, 1, Root.Tool.StrengthM, v => Root.Tool.StrengthM = (float)v);
        _liquidKind = new OptionButton();
        _liquidKind.AddItem("Water", 0);
        _liquidKind.AddItem("Lava", 1);
        _liquidKind.Selected = 0;
        _liquidKind.ItemSelected += idx => Root.Tool.LiquidKind = (int)idx;
        col.AddChild(_liquidKind);

        col.AddChild(ToolTheme.Header("WORLD"));
        _seed = SpinRow(col, "Seed", 0, 999999, 1, Root.Seed);
        _oct = SpinRow(col, "Octaves", 1, 12, 1, Root.Octaves);
        _size = SpinRow(col, "Size (km)", 1, 60, 1, Root.WorldSizeKm);
        _spacing = SpinRow(col, "Spacing (m)", 4, 60, 1, Root.SpacingM);
        Slider(col, "Height (km)", 0.1, 10, 0.1, Root.TerrainHeightKm, v => Root.SetTerrainHeight((float)v));
        var regen = GhostButton("Regenerate (discards edits)");
        regen.Pressed += () =>
        {
            Root.Seed = (int)_seed.Value;
            Root.Octaves = (int)_oct.Value;
            Root.WorldSizeKm = (float)_size.Value;
            Root.SpacingM = (float)_spacing.Value;
            Root.Regenerate();
        };
        col.AddChild(regen);
    }

    private void BuildBiomesTab(VBoxContainer col)
    {
        col.AddChild(ToolTheme.Header("BIOME LAYER"));
        var note = new Label
        {
            Text = "Pick a biome, then paint. Territory + Select tools land next.",
            AutowrapMode = TextServer.AutowrapMode.WordSmart,
        };
        note.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        col.AddChild(note);

        var grid = new GridContainer { Columns = 2 };
        col.AddChild(grid);
        for (int id = 1; id <= BiomeNames.Length; id++) grid.AddChild(SwatchButton(id, BiomeNames[id - 1]));
    }

    // --- right panel (physics) ---------------------------------------------------------------

    private void BuildRightPanel(Control parent)
    {
        var panel = new PanelContainer
        {
            AnchorLeft = 1, AnchorRight = 1, AnchorTop = 0, AnchorBottom = 0,
            GrowHorizontal = Control.GrowDirection.Begin,
            OffsetLeft = -262, OffsetRight = -10, OffsetTop = 10,
        };
        panel.CustomMinimumSize = new Vector2(252, 0);
        parent.AddChild(panel);
        var col = new VBoxContainer();
        panel.AddChild(col);

        col.AddChild(ToolTheme.Header("PHYSICS"));
        Slider(col, "Sea level", -1.0, 1.0, 0.01, 0.0, v => { Root.Engine.Call("set_sea_level", v); Root.RebuildLiquid(); });
        Slider(col, "Flow rate", 0.0, 0.5, 0.01, _simFlow, v => _simFlow = v);
        Slider(col, "Evaporation", 0.0, 0.02, 0.0005, _simEvap, v => _simEvap = v);
        Slider(col, "Substeps", 1, 40, 1, _simSubsteps, v => _simSubsteps = (int)v);

        var rain = GhostButton("Rain");
        rain.Pressed += () => { Root.Engine.Call("rain", 0.05); Root.RebuildLiquid(); };
        col.AddChild(rain);
        var settle = GhostButton("Settle (1 step)");
        settle.Pressed += () => { Root.Engine.Call("step_fluid", _simFlow, _simEvap, _simSubsteps); Root.RebuildLiquid(); };
        col.AddChild(settle);
        var clear = GhostButton("Clear liquid");
        clear.Pressed += () => { Root.Engine.Call("clear_liquid"); Root.RebuildLiquid(); };
        col.AddChild(clear);
        _simulate = new CheckButton { Text = "Simulate" };
        col.AddChild(_simulate);
    }

    public override void _Process(double delta)
    {
        if (_simulate == null || !_simulate.ButtonPressed) return;
        if (++_simTick < SimEveryNFrames) return;
        _simTick = 0;
        Root.Engine.Call("step_fluid", _simFlow, _simEvap, _simSubsteps);
        Root.RebuildLiquid();
    }

    public override void _UnhandledInput(InputEvent e)
    {
        if (e is not InputEventKey { Pressed: true, Echo: false } k) return;
        switch (k.Keycode)
        {
            case Key.Key1: case Key.Key2: case Key.Key3:
            case Key.Key4: case Key.Key5: case Key.Key6:
                int ti = (int)k.Keycode - (int)Key.Key1;
                if (ti < _toolButtons.Count) { SetTab(true); _toolButtons[ti].ButtonPressed = true; }
                break;
            case Key.Bracketleft: SetBrushSize(BrushSizeIndex() - 1); break;
            case Key.Bracketright: SetBrushSize(BrushSizeIndex() + 1); break;
            case Key.B: SetTab(!_terrainTab.Visible); break;
        }
    }

    public void SetStatus(string text) => _status.Text = text;

    // --- helpers -----------------------------------------------------------------------------

    private void SetTab(bool terrain)
    {
        _terrainTab.Visible = terrain;
        _biomesTab.Visible = !terrain;
        _tabTerrain.ButtonPressed = terrain;
        _tabBiomes.ButtonPressed = !terrain;
        if (terrain)
        {
            Root.Tool.Active = _lastTerrainTool;
            foreach (var b in _toolButtons)
                if ((ToolKind)(int)b.GetMeta("tool") == _lastTerrainTool) b.ButtonPressed = true;
        }
        else
        {
            Root.Tool.Active = ToolKind.Biome;
        }
    }

    private Button TabButton(string text, System.Action onPress)
    {
        var b = new Button { Text = text, ToggleMode = true, SizeFlagsHorizontal = Control.SizeFlags.ExpandFill };
        b.Pressed += onPress;
        return b;
    }

    private Button SizeDot(int i)
    {
        var b = new Button { Text = "●", ToggleMode = true, ButtonGroup = _sizeGroup, TooltipText = $"{BrushFractions[i] * 100:0}% of view" };
        b.AddThemeFontSizeOverride("font_size", DotFontSizes[i]);
        b.CustomMinimumSize = new Vector2(36, 34);
        b.ButtonPressed = i == 2;
        b.Pressed += () => Root.Tool.RadiusFraction = BrushFractions[i];
        _sizeDots.Add(b);
        return b;
    }

    private void SetBrushSize(int idx)
    {
        idx = Mathf.Clamp(idx, 0, _sizeDots.Count - 1);
        _sizeDots[idx].ButtonPressed = true; // fires Pressed → sets RadiusM
    }

    private int BrushSizeIndex()
    {
        for (int i = 0; i < _sizeDots.Count; i++) if (_sizeDots[i].ButtonPressed) return i;
        return 2;
    }

    private void AddToolButton(Container parent, string text, ToolKind tool)
    {
        var b = new Button
        {
            Text = text, ToggleMode = true, ButtonGroup = _toolGroup,
            SizeFlagsHorizontal = Control.SizeFlags.ExpandFill,
        };
        b.SetMeta("tool", (int)tool);
        if (ToolTheme.Display != null) b.AddThemeFontOverride("font", ToolTheme.Display);
        b.Pressed += () => { Root.Tool.Active = tool; _lastTerrainTool = tool; };
        if (tool == Root.Tool.Active) b.ButtonPressed = true;
        parent.AddChild(b);
        _toolButtons.Add(b);
    }

    private Button SwatchButton(int id, string name)
    {
        var b = new Button { Text = name, ToggleMode = true, ButtonGroup = _biomeGroup };
        Color c = ReadBiomeColor(id);
        var sb = new StyleBoxFlat { BgColor = new Color(c, 0.9f), BorderColor = ToolTheme.Rule };
        sb.SetBorderWidthAll(1);
        sb.SetCornerRadiusAll(5);
        sb.SetContentMarginAll(6);
        var hov = (StyleBoxFlat)sb.Duplicate();
        hov.BorderColor = ToolTheme.Gold;
        hov.SetBorderWidthAll(2);
        b.AddThemeStyleboxOverride("normal", sb);
        b.AddThemeStyleboxOverride("hover", hov);
        b.AddThemeStyleboxOverride("pressed", hov);
        float lum = c.R * 0.299f + c.G * 0.587f + c.B * 0.114f;
        Color txt = lum > 0.5f ? ToolTheme.Bg : ToolTheme.Ink;
        b.AddThemeColorOverride("font_color", txt);
        b.AddThemeColorOverride("font_hover_color", txt);
        b.AddThemeColorOverride("font_pressed_color", txt);
        b.Pressed += () => { Root.Tool.Active = ToolKind.Biome; Root.Tool.BiomeId = id; };
        return b;
    }

    private Color ReadBiomeColor(int id)
    {
        var a = Root.Engine.Call("biome_color_of", id).As<float[]>();
        return a.Length >= 3 ? new Color(a[0], a[1], a[2]) : new Color(0.5f, 0.5f, 0.5f);
    }

    private Button GhostButton(string text)
    {
        var b = new Button { Text = text, SizeFlagsHorizontal = Control.SizeFlags.ExpandFill };
        if (ToolTheme.Display != null) b.AddThemeFontOverride("font", ToolTheme.Display);
        return b;
    }

    private HSlider Slider(Container parent, string label, double min, double max, double step,
                           double val, System.Action<double> onChange)
    {
        var lbl = new Label { Text = $"{label}: {val:0.###}" };
        lbl.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        parent.AddChild(lbl);
        var s = new HSlider { MinValue = min, MaxValue = max, Step = step, Value = val };
        s.ValueChanged += v => { lbl.Text = $"{label}: {v:0.###}"; onChange(v); };
        parent.AddChild(s);
        return s;
    }

    private SpinBox SpinRow(Container parent, string label, double min, double max, double step, double val)
    {
        var lbl = new Label { Text = label };
        lbl.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        parent.AddChild(lbl);
        var sb = new SpinBox { MinValue = min, MaxValue = max, Step = step, Value = val };
        parent.AddChild(sb);
        return sb;
    }
}
