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

    // Trait brush + base-palette editor controls.
    private OptionButton _traitPick, _traitEnum, _palFamily;
    private HSlider _traitSlider;
    private Label _traitSliderLabel, _traitEnumLabel;
    private ColorPickerButton[] _palPickers;
    private bool _loadingPalette;

    private static readonly float[] BrushFractions = { 0.03f, 0.06f, 0.12f, 0.25f, 0.5f };
    private static readonly int[] DotFontSizes = { 9, 12, 16, 20, 25 };

    // Trait dropdown order → engine trait id (0 jag,1 relief,2 foothill,3 erosion,4 temp,5 moist,
    // 6 vegetation,7 palette_family). Enum traits (6,7) use the dropdown; the rest use the slider.
    private static readonly int[] TraitEngineId = { 6, 7, 0, 1, 2, 3, 4, 5 };
    private static readonly string[] TraitNames =
        { "Vegetation", "Palette family", "Jaggedness", "Relief", "Foothill falloff", "Erosion", "Temperature", "Moisture" };
    private static readonly string[] VegNames =
        { "Barren", "Grass", "Scrub", "Forest", "Evergreen", "Marsh", "Thorn" };
    private static readonly string[] FamilyNames =
        { "Verdant", "Arid", "Stone", "Ashen", "Frost", "Wetland", "Exotic" };
    private static readonly string[] SlotNames =
        { "Deep water", "Shallows", "Low cover", "Rock", "Cap (warm)", "Cap (snow)" };

    private double _simFlow = 0.45, _simEvap = 0.001;
    private int _simSubsteps = 10, _simTick;
    private CheckButton _simulate;
    private MinimapPanel _minimap;
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
        col.AddChild(ToolTheme.Header("REGIONS"));
        var note = new Label
        {
            Text = "Stamp a region's whole character, or paint a single trait below.",
            AutowrapMode = TextServer.AutowrapMode.WordSmart,
        };
        note.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        col.AddChild(note);

        var grid = new GridContainer { Columns = 2 };
        col.AddChild(grid);
        for (int id = 1; id <= BiomeNames.Length; id++) grid.AddChild(SwatchButton(id, BiomeNames[id - 1]));

        col.AddChild(ToolTheme.Header("TRAIT BRUSH"));
        _traitPick = new OptionButton();
        for (int i = 0; i < TraitNames.Length; i++) _traitPick.AddItem(TraitNames[i], i);
        _traitPick.Selected = 0;
        _traitPick.ItemSelected += idx => SelectTrait((int)idx);
        col.AddChild(_traitPick);

        _traitSliderLabel = DimLabel("Value: 1");
        col.AddChild(_traitSliderLabel);
        _traitSlider = new HSlider { MinValue = 0, MaxValue = 1, Step = 0.01, Value = 1 };
        _traitSlider.ValueChanged += v =>
        {
            _traitSliderLabel.Text = $"Value: {v:0.##}";
            Root.Tool.TraitValue = (float)v;
            Root.Tool.Active = ToolKind.Trait;
        };
        col.AddChild(_traitSlider);

        _traitEnumLabel = DimLabel("Type");
        col.AddChild(_traitEnumLabel);
        _traitEnum = new OptionButton();
        _traitEnum.ItemSelected += idx => { Root.Tool.TraitValue = (int)idx; Root.Tool.Active = ToolKind.Trait; };
        col.AddChild(_traitEnum);

        SelectTrait(0); // default to Vegetation
    }

    /// Point the Trait brush at the trait chosen in the dropdown, swapping the scalar slider for
    /// the enum dropdown (vegetation / palette family) as appropriate.
    private void SelectTrait(int dropdownIdx)
    {
        int engineId = TraitEngineId[dropdownIdx];
        Root.Tool.TraitId = engineId;
        Root.Tool.Active = ToolKind.Trait;
        bool isEnum = engineId == 6 || engineId == 7;
        _traitSlider.Visible = !isEnum;
        _traitSliderLabel.Visible = !isEnum;
        _traitEnum.Visible = isEnum;
        _traitEnumLabel.Visible = isEnum;
        if (isEnum)
        {
            _traitEnum.Clear();
            string[] names = engineId == 6 ? VegNames : FamilyNames;
            for (int k = 0; k < names.Length; k++) _traitEnum.AddItem(names[k], k);
            _traitEnum.Selected = 0;
            Root.Tool.TraitValue = 0;
        }
        else
        {
            Root.Tool.TraitValue = (float)_traitSlider.Value;
        }
    }

    // --- right panel (physics) ---------------------------------------------------------------

    private void BuildRightPanel(Control parent)
    {
        // Full-height panel with a scroll view, so adding sections (palette editor) never pushes
        // the minimap off-screen on smaller windows.
        var panel = new PanelContainer
        {
            AnchorLeft = 1, AnchorRight = 1, AnchorTop = 0, AnchorBottom = 1,
            GrowHorizontal = Control.GrowDirection.Begin,
            OffsetLeft = -262, OffsetRight = -10, OffsetTop = 10, OffsetBottom = -10,
        };
        panel.CustomMinimumSize = new Vector2(252, 0);
        parent.AddChild(panel);
        var scroll = new ScrollContainer { HorizontalScrollMode = ScrollContainer.ScrollMode.Disabled };
        panel.AddChild(scroll);
        var col = new VBoxContainer { SizeFlagsHorizontal = Control.SizeFlags.ExpandFill };
        col.CustomMinimumSize = new Vector2(232, 0);
        scroll.AddChild(col);

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

        col.AddChild(ToolTheme.Header("SUN"));
        var sunHint = new Label { Text = "Drag the sun sphere in the sky to move it.", AutowrapMode = TextServer.AutowrapMode.WordSmart };
        sunHint.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        col.AddChild(sunHint);
        Slider(col, "Brightness", 0.0, 2.0, 0.05, 1.0, v => Root.SetSunBrightness((float)v));
        Slider(col, "Warmth", 0.0, 1.0, 0.01, 0.5, v => Root.SetSunWarmth((float)v));
        Slider(col, "Hue tint", -0.5, 0.5, 0.01, 0.0, v => Root.SetSunHue((float)v));

        BuildPaletteEditor(col);

        col.AddChild(ToolTheme.Header("MAP"));
        _minimap = new MinimapPanel { Root = Root };
        col.AddChild(_minimap);
    }

    /// Editor for the 7 shared base palettes: pick a family, then edit its 6 light→dark slots.
    /// Changes recolour the whole world (the core flags every chunk dirty).
    private void BuildPaletteEditor(VBoxContainer col)
    {
        col.AddChild(ToolTheme.Header("PALETTE"));
        _palFamily = new OptionButton();
        for (int i = 0; i < FamilyNames.Length; i++) _palFamily.AddItem(FamilyNames[i], i);
        _palFamily.Selected = 0;
        _palFamily.ItemSelected += _ => LoadPaletteColors();
        col.AddChild(_palFamily);

        _palPickers = new ColorPickerButton[SlotNames.Length];
        for (int s = 0; s < SlotNames.Length; s++)
        {
            var row = new HBoxContainer();
            var lbl = DimLabel(SlotNames[s]);
            lbl.SizeFlagsHorizontal = Control.SizeFlags.ExpandFill;
            row.AddChild(lbl);
            var cp = new ColorPickerButton { CustomMinimumSize = new Vector2(46, 22) };
            int slot = s;
            cp.ColorChanged += c => OnPaletteColor(slot, c);
            _palPickers[s] = cp;
            row.AddChild(cp);
            col.AddChild(row);
        }
        LoadPaletteColors();
    }

    private void LoadPaletteColors()
    {
        _loadingPalette = true; // setting .Color fires ColorChanged — don't write back while loading
        int fam = _palFamily.Selected;
        for (int s = 0; s < _palPickers.Length; s++)
        {
            var a = Root.Engine.Call("base_palette_color", fam, s).As<float[]>();
            if (a.Length >= 3) _palPickers[s].Color = new Color(a[0], a[1], a[2]);
        }
        _loadingPalette = false;
    }

    private void OnPaletteColor(int slot, Color c)
    {
        if (_loadingPalette) return;
        int fam = _palFamily.Selected;
        Root.Engine.Call("set_base_palette_color", fam, slot, (double)c.R, (double)c.G, (double)c.B);
        Root.RepaintDirtyTerrain();
        _minimap?.Refresh();
    }

    public void RefreshMinimap() => _minimap?.Refresh();

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

    private static Label DimLabel(string text)
    {
        var l = new Label { Text = text };
        l.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        return l;
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
