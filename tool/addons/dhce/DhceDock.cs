#if TOOLS
using Godot;
using System.Collections.Generic;

namespace DesolateHaven.Cartography;

/// The in-editor authoring dock (reshell R4): the full tool panel ported from the runtime `ToolUi`,
/// wired to the **selected `DhceWorld`** and the plugin's shared `ToolState` instead of
/// `CartographerSpike`. Native editor controls (no parchment theme); one scroll of sections —
/// tools, brush, world/regen, VIEW + contextual paint, regions, trait brush, region landform,
/// shaping, transitions, palette, physics. Lighting is the scene's own (WorldEnvironment /
/// DirectionalLight3D), so there's no SUN panel; the minimap + brush gizmo land in R4b.
public partial class DhceDock : ScrollContainer
{
    private ToolState _tool;          // shared with the plugin's viewport picking
    private DhceWorld _world;         // current target (the scene's DhceWorld); null until bound
    private bool _wasGenDone;         // edge-detect gen completion to reload engine-backed values

    private Label _status;
    private Label _scaleReadout;
    private OptionButton _regionPick;
    private VBoxContainer _col;          // the sidebar dock column (this ScrollContainer's content)
    private Container _panelRoot;        // the root a new Header() opens a collapsible section under
    private VBoxContainer _target;       // the container helper controls add into (a section's content)
    private readonly ButtonGroup _toolGroup = new();   // the 6 sculpt tools (one active)
    private readonly ButtonGroup _traitGroup = new();  // the 4 trait brushes (one active)
    private readonly ButtonGroup _sizeGroup = new();   // the 6 brush-size icons (one active)
    private readonly List<Button> _toolButtons = new();
    private static readonly Dictionary<string, Texture2D> _iconCache = new();

    private SpinBox _seed, _oct, _size, _spacing, _chunk;
    private OptionButton _liquidKind, _traitEnum, _palFamily, _landRegion;
    private HSlider _traitSlider;
    private Label _traitSliderLabel, _traitEnumLabel;
    private ColorPickerButton[] _palPickers;
    private SpinBox[] _landSpins;
    private CheckButton _simulate;
    private DhceMinimap _minimap;
    private LineEdit _exportPath;

    // Contextual brush-option groups — only the active tool's group is shown (see RefreshBrushOptions).
    private VBoxContainer _optSize, _optStrength, _optLiquid, _optRain, _optCave, _optBiome, _optRegion, _optTrait;

    // Scatter (N3d) model-slot editor.
    private OptionButton _slotPick;
    private LineEdit _slotName, _slotMesh, _slotProxy;
    private HSlider _slotDensity;
    private SpinBox _slotScaleMin, _slotScaleMax, _slotElevMin, _slotElevMax, _slotVisEnd;
    private CheckBox[] _slotVeg;
    private CheckButton _scatterPreview;
    private CheckBox _slotInstances;
    private bool _loadingSlot;
    private readonly Dictionary<int, Color> _palettePending = new(); // coalesce rapid ColorPicker drags
    private bool _loadingPalette, _loadingLandform;

    private double _shapeStrength = 1.0, _transitionWidthM = 200, _simFlow = 0.45, _simEvap = 0.001;
    private int _simSubsteps = 10, _simTick;
    private const int SimEveryNFrames = 6;

    private static readonly int[] TraitEngineId = { 4, 5, 6, 7 };
    private static readonly string[] LandNames = { "Jaggedness", "Relief", "Foothill falloff", "Erosion" };
    private static readonly string[] VegNames = { "Barren", "Grass", "Scrub", "Forest", "Evergreen", "Marsh", "Thorn" };
    private static readonly string[] FamilyNames = { "Verdant", "Arid", "Stone", "Ashen", "Frost", "Wetland", "Exotic" };
    private static readonly string[] SlotNames = { "Deep water", "Shallows", "Low cover", "Rock", "Cap (warm)", "Cap (snow)" };
    private static readonly string[] BiomeNames =
    {
        "Jagged Mountains", "Sacred Woods Plateau", "Great Lake", "Temperate Forest",
        "Open Plains", "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles",
        "Blisterwood", "Volcanic Scape", "Blight Ruins", "Scattered Isles", "Marsh & Bog",
    };

    /// Build the dock once — everything in this sidebar, themed + collapsible. The plugin passes the
    /// shared ToolState and calls `Bind` each frame.
    public void Init(ToolState tool)
    {
        _tool = tool;
        Name = "DHCE";
        Theme = DhceUi.Theme; // game parchment look/fonts/feel
        CustomMinimumSize = new Vector2(248, 0);
        HorizontalScrollMode = ScrollMode.Disabled;
        _col = new VBoxContainer { SizeFlagsHorizontal = SizeFlags.ExpandFill };
        AddChild(_col);
        BuildUi();
    }

    /// Point the dock at the scene's current DhceWorld; reload engine-backed values when it changes
    /// or finishes generating. Cheap when nothing changed.
    public void Bind(DhceWorld world)
    {
        bool changed = world != _world;
        _world = world;
        bool gen = world != null && world.GenDone;
        if (world != null && (changed || (gen && !_wasGenDone)))
        {
            PullWorldParams();
            if (changed) { RefreshSlots(); LoadSlot(); } // reflect the new world's scatter library
            if (gen)
            {
                LoadPaletteColors();
                LoadRegionLandform();
                _minimap?.Bind(Eng, _world.WorldWidthM, _world.WorldHeightM);
                _minimap?.Refresh();
            }
        }
        _wasGenDone = gen;
        RefreshBrushOptions(); // keep the contextual brush options in sync with the active tool
    }

    /// The overview map lives in the 3D viewport (owned + placed by the plugin); the dock just drives
    /// its data (Bind / Refresh / focus marker). Set once, right after Init.
    public void SetMinimap(DhceMinimap m) => _minimap = m;

    /// Editor-camera ground focus → the minimap marker (fed by the plugin each frame).
    public void SetMapFocus(Vector3 f) => _minimap?.SetFocus(f);

    /// Context-aware scale bar (LOD): the world span the viewport covers at the focus, unit-switched
    /// cm → m → km. Fed by the plugin each frame from the editor camera.
    public void SetViewScale(double metresAcross)
    {
        if (_scaleReadout == null) return;
        _scaleReadout.Text = metresAcross < 1.0 ? $"View: ~{metresAcross * 100.0:0} cm across"
            : metresAcross < 1000.0 ? $"View: ~{metresAcross:0} m across"
            : $"View: ~{metresAcross / 1000.0:0.0} km across";
    }

    private bool HasWorld => _world != null && _world.Engine != null;
    private GodotObject Eng => _world.Engine;

    // --- layout ---

    // Everything lives in this sidebar dock, themed + collapsible. All paint tools sit together under
    // BRUSHES; selecting a tool reveals only that brush's options below it (size / strength / liquid /
    // biome swatches / region / trait …). Non-brush controls follow in their own collapsible sections.
    private void BuildUi()
    {
        _panelRoot = _col; _target = _col;
        var title = new Label { Text = "DHCE Cartographer" };
        title.AddThemeColorOverride("font_color", ToolTheme.Gold);
        title.AddThemeFontSizeOverride("font_size", 18);
        if (ToolTheme.Display != null) title.AddThemeFontOverride("font", ToolTheme.Display);
        _target.AddChild(title);
        var gen = new Button { Text = "Generate / Regenerate" };
        gen.Pressed += OnGenerate;
        _target.AddChild(gen);
        _status = new Label { Text = "Add a DhceWorld, set params, Generate.", AutowrapMode = TextServer.AutowrapMode.WordSmart };
        _target.AddChild(_status);
        _scaleReadout = Dim("View: —"); // context-aware scale bar (LOD): viewport span at the focus
        _target.AddChild(_scaleReadout);

        Header("WORLD");
        _seed = SpinRow("Seed", 0, 999999, 1, 12345);
        _oct = SpinRow("Octaves", 1, 12, 1, 6);
        _size = SpinRow("Size (km)", 1, 60, 1, 30);
        _spacing = SpinRow("Spacing (m)", 4, 60, 1, 10);
        _chunk = SpinRow("Chunk size (m)", 32, 2048, 32, 256);
        Slider("Height (km)", 0.1, 10, 0.1, 5.0, v => _world?.SetTerrainHeight((float)v));

        Header("SAVE / LOAD", open: false);
        Button(_target, "Save world", () => { if (_world != null) SetStatus(_world.SaveToDisk()); });
        Button(_target, "Load world", () =>
        {
            if (_world == null) return;
            SetStatus(_world.LoadFromDisk());
            if (!HasWorld) return;
            PullWorldParams();
            LoadPaletteColors();
            LoadRegionLandform();
            _minimap?.Bind(Eng, _world.WorldWidthM, _world.WorldHeightM);
            _minimap?.Refresh();
        });

        BuildBrushes();        // all paint tools + their contextual options
        BuildShapingSection(); // region landform + shaping + transitions (not brushes)
        BuildPaletteEditor();  // render: per-family palette colours
        BuildPhysicsSection(); // sea level / flow / evaporation / substeps / settle / clear / simulate
        BuildScatterSection(); // model scatter (per-slot; per-biome rework is a separate phase)

        Header("EXPORT", open: false);
        _exportPath = new LineEdit { Text = DhceLevelSlicer.DefaultExportDir(), TooltipText = "Export folder; a 'levels' subfolder is created here" };
        _target.AddChild(_exportPath);
        Button(_target, "Slice into levels", () => { if (_world != null) SetStatus(DhceLevelSlicer.Slice(_world, _exportPath.Text)); });
        _target.AddChild(Dim("Bakes each assigned Region → levels/<Name>.tscn + world_master.res + regions.json."));

        SelectTrait(0, arm: false); // configure the trait-brush UI but leave the active tool at Raise
        RefreshBrushOptions();
    }

    // All paint/terraform tools in one selector; the active tool's options appear directly below it.
    private void BuildBrushes()
    {
        Header("BRUSHES");
        var sec = _target;

        var toolRow = new HFlowContainer();
        sec.AddChild(toolRow);
        AddToolButton(toolRow, "raise", "Raise", ToolKind.Raise);
        AddToolButton(toolRow, "carve", "Carve", ToolKind.Carve);
        AddToolButton(toolRow, "level", "Level", ToolKind.Level);
        AddToolButton(toolRow, "crest", "Crest", ToolKind.Crest);
        AddToolButton(toolRow, "river", "River", ToolKind.River);
        AddToolButton(toolRow, "flood", "Flood", ToolKind.Flood);
        AddToolButton(toolRow, null, "Biome", ToolKind.Biome);
        AddToolButton(toolRow, null, "Region", ToolKind.Region);
        AddToolButton(toolRow, null, "Trait", ToolKind.Trait);
        AddToolButton(toolRow, null, "Cave", ToolKind.Cave);
        AddToolButton(toolRow, null, "Rain", ToolKind.Rain);

        // SIZE — applies to every tool, always shown.
        _optSize = NewGroup(sec);
        _optSize.AddChild(Dim("Size (scales with zoom):"));
        var sizeRow = new HFlowContainer();
        _optSize.AddChild(sizeRow);
        for (int lvl = 0; lvl < ToolState.SizeFractions.Length; lvl++) AddSizeButton(sizeRow, lvl);

        // STRENGTH — sculpt tools.
        _optStrength = NewGroup(sec);
        _target = _optStrength;
        Slider("Strength (m)", 1, 400, 1, _tool.StrengthM, v => _tool.StrengthM = (float)v);
        _target = sec;

        // LIQUID — River / Flood.
        _optLiquid = NewGroup(sec);
        _target = _optLiquid;
        _liquidKind = Options(new[] { "Water", "Lava" }, 0, idx => _tool.LiquidKind = (int)idx);
        Button(_optLiquid, "Generate streams", () => { if (HasWorld) { Eng.Call("generate_streams", 0.5, 1.0); _world.RepaintDirtyTerrain(); _world.RebuildLiquid(); } });
        _target = sec;

        // RAIN.
        _optRain = NewGroup(sec);
        _target = _optRain;
        Slider("Rain rate", 0.0, 0.01, 0.0005, _tool.RainRate, v => _tool.RainRate = (float)v);
        _optRain.AddChild(Dim("Drag over terrain; a cloud drifts and rains, water pools and flows."));
        _target = sec;

        // CAVE.
        _optCave = NewGroup(sec);
        _optCave.AddChild(Dim("Drag to carve a tunnel of spheres. Baked on export / preview."));
        Button(_optCave, "Clear caves", () => { _world?.ClearCaves(); SetStatus("cleared caves"); });
        var cavePrev = new CheckButton { Text = "Preview carved caves" };
        cavePrev.Toggled += on => _world?.PreviewCaves(on);
        _optCave.AddChild(cavePrev);

        // BIOME stamp swatches.
        _optBiome = NewGroup(sec);
        _optBiome.AddChild(Dim("Pick a region preset to stamp its whole character."));
        var swatches = new GridContainer { Columns = 2 };
        _optBiome.AddChild(swatches);
        for (int id = 1; id <= BiomeNames.Length; id++) swatches.AddChild(SwatchButton(id, BiomeNames[id - 1]));

        // REGION assign: place pick + paint modes.
        _optRegion = NewGroup(sec);
        _optRegion.AddChild(Dim("Pick a place, then Brush / Polygon / Select. (VIEW → Region under the minimap shows them.)"));
        _regionPick = new OptionButton();
        for (int id = 1; id <= BiomeNames.Length; id++) _regionPick.AddItem(BiomeNames[id - 1], id);
        _regionPick.Selected = 0;
        _regionPick.ItemSelected += idx =>
        {
            _tool.RegionId = (int)_regionPick.GetItemId((int)idx);
            _tool.Active = ToolKind.Region;
            RefreshBrushOptions();
            SetStatus($"Region: {BiomeNames[_tool.RegionId - 1]}");
        };
        _optRegion.AddChild(_regionPick);
        var regionModes = new HBoxContainer();
        AddModeButton(regionModes, "brush", "Brush", ToolKind.Region);
        AddModeButton(regionModes, "polygon", "Polygon", ToolKind.Territory);
        AddModeButton(regionModes, "select", "Select", ToolKind.RegionSelect);
        _optRegion.AddChild(regionModes);

        // TRAIT brush.
        _optTrait = NewGroup(sec);
        var traitRow = new HFlowContainer();
        _optTrait.AddChild(traitRow);
        AddTraitButton(traitRow, "temperature", "Temperature", 0);
        AddTraitButton(traitRow, "moisture", "Moisture", 1);
        AddTraitButton(traitRow, "vegetation", "Vegetation", 2);
        AddTraitButton(traitRow, "palette", "Palette family", 3);
        _traitSliderLabel = Dim("Value: 1");
        _traitSlider = new HSlider { MinValue = 0, MaxValue = 1, Step = 0.01, Value = 1 };
        _traitSlider.ValueChanged += v => { _traitSliderLabel.Text = $"Value: {v:0.##}"; _tool.TraitValue = (float)v; _tool.Active = ToolKind.Trait; };
        _optTrait.AddChild(_traitSliderLabel);
        _optTrait.AddChild(_traitSlider);
        _traitEnumLabel = Dim("Type");
        _traitEnum = new OptionButton();
        _traitEnum.ItemSelected += idx => { _tool.TraitValue = (int)idx; _tool.Active = ToolKind.Trait; };
        _optTrait.AddChild(_traitEnumLabel);
        _optTrait.AddChild(_traitEnum);

        _target = sec;
    }

    /// Show only the active tool's option group (size always shows).
    private void RefreshBrushOptions()
    {
        var a = _tool.Active;
        bool sculpt = a == ToolKind.Raise || a == ToolKind.Carve || a == ToolKind.Level || a == ToolKind.Crest;
        bool liquid = a == ToolKind.River || a == ToolKind.Flood;
        bool region = a == ToolKind.Region || a == ToolKind.Territory || a == ToolKind.RegionSelect;
        if (_optStrength != null) _optStrength.Visible = sculpt;
        if (_optLiquid != null) _optLiquid.Visible = liquid;
        if (_optRain != null) _optRain.Visible = a == ToolKind.Rain;
        if (_optCave != null) _optCave.Visible = a == ToolKind.Cave;
        if (_optBiome != null) _optBiome.Visible = a == ToolKind.Biome;
        if (_optRegion != null) _optRegion.Visible = region;
        if (_optTrait != null) _optTrait.Visible = a == ToolKind.Trait;
    }

    private VBoxContainer NewGroup(Container parent)
    {
        var g = new VBoxContainer { SizeFlagsHorizontal = SizeFlags.ExpandFill };
        parent.AddChild(g);
        return g;
    }

    // Region landform dials + the shaping/transition bakes (not brushes).
    private void BuildShapingSection()
    {
        Header("REGION LANDFORM", open: false);
        _target.AddChild(Dim("Set a region's terrain character, then Apply shaping."));
        _landRegion = new OptionButton();
        for (int id = 1; id <= BiomeNames.Length; id++) _landRegion.AddItem(BiomeNames[id - 1], id);
        _landRegion.Selected = 0;
        _landRegion.ItemSelected += _ => LoadRegionLandform();
        _target.AddChild(_landRegion);
        _landSpins = new SpinBox[LandNames.Length];
        for (int i = 0; i < LandNames.Length; i++)
        {
            int idx = i;
            var sb = SpinRow(LandNames[i], 0, 1, 0.05, 0);
            sb.ValueChanged += v => OnRegionLandform(idx, v);
            _landSpins[i] = sb;
        }

        Header("SHAPING", open: false);
        _target.AddChild(Dim("Bake the landform dials into the terrain height."));
        Slider("Strength", 0, 2, 0.05, _shapeStrength, v => _shapeStrength = v);
        Button(_target, "Apply shaping", () =>
        {
            if (!HasWorld) return;
            Eng.Call("shape_terrain", _shapeStrength);
            _world.RepaintDirtyTerrain(); _world.RebuildLiquid();
            _minimap?.Refresh();
            SetStatus($"shaped @ strength {_shapeStrength:0.##}");
        });

        Header("TRANSITIONS", open: false);
        Slider("Width (m)", 0, 1200, 25, _transitionWidthM, v => _transitionWidthM = v);
        Button(_target, "Blend borders", () =>
        {
            if (!HasWorld) return;
            Eng.Call("blend_traits", _transitionWidthM);
            _world.RepaintDirtyTerrain();
            _minimap?.Refresh();
            SetStatus($"blended borders @ {_transitionWidthM:0} m");
        });
    }

    private void BuildPhysicsSection()
    {
        Header("PHYSICS", open: false);
        Slider("Sea level", -1.0, 1.0, 0.01, 0.0, v => { if (HasWorld) { Eng.Call("set_sea_level", v); _world.RebuildLiquid(); } });
        Slider("Flow rate", 0.0, 0.5, 0.01, _simFlow, v => _simFlow = v);
        Slider("Evaporation", 0.0, 0.02, 0.0005, _simEvap, v => _simEvap = v);
        Slider("Substeps", 1, 40, 1, _simSubsteps, v => _simSubsteps = (int)v);
        Button(_target, "Settle (1 step)", () => { if (HasWorld) { Eng.Call("step_fluid", _simFlow, _simEvap, _simSubsteps); _world.RebuildLiquid(); } });
        Button(_target, "Clear liquid", () => { if (HasWorld) { _world.StopRain(); Eng.Call("clear_liquid"); _world.RebuildLiquid(); } });
        _simulate = new CheckButton { Text = "Simulate" };
        _target.AddChild(_simulate);
    }

    // --- sim tick (driven by the plugin's _Process so it runs in-editor) ---

    public void SimTick()
    {
        if (_simulate == null || !_simulate.ButtonPressed || !HasWorld) return;
        if (++_simTick < SimEveryNFrames) return;
        _simTick = 0;
        if (Eng.Call("liquid_active_count").As<int>() == 0) return; // settled → idle
        Eng.Call("step_fluid", _simFlow, _simEvap, _simSubsteps);
        _world.RebuildLiquid();
    }

    // --- actions ---

    private void OnGenerate()
    {
        if (_world == null) { SetStatus("No DhceWorld in the scene. Add one (Add Node → DhceWorld)."); return; }
        _world.Seed = (int)_seed.Value;
        _world.Octaves = (int)_oct.Value;
        _world.WorldSizeKm = (float)_size.Value;
        _world.SpacingM = (float)_spacing.Value;
        _world.ChunkSizeM = (float)_chunk.Value;
        SetStatus("Generating… (the editor pauses a few seconds)");
        _world.Generate();
        _wasGenDone = false; // force a value reload on the next Bind
        SetStatus($"Generated ~{_world.WorldWidthM / 1000f:0.0} km. Select the node and left-drag to paint.");
    }

    private void PullWorldParams()
    {
        _seed.Value = _world.Seed;
        _oct.Value = _world.Octaves;
        _size.Value = _world.WorldSizeKm;
        _spacing.Value = _world.SpacingM;
        _chunk.Value = _world.ChunkSizeM;
    }

    private void SelectTrait(int dropdownIdx, bool arm = true)
    {
        int engineId = TraitEngineId[dropdownIdx];
        _tool.TraitId = engineId;
        if (arm) { _tool.Active = ToolKind.Trait; RefreshBrushOptions(); } // initial setup configures the UI without grabbing the tool
        bool isEnum = engineId == 6 || engineId == 7; // 4/5 (temp/moisture) are scalars; 6/7 are enums
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
            _tool.TraitValue = 0;
        }
        else _tool.TraitValue = (float)_traitSlider.Value;
    }

    private int SelectedLandRegion() => _landRegion.GetItemId(_landRegion.Selected);

    private void LoadRegionLandform()
    {
        if (!HasWorld || !_world.GenDone) return;
        _loadingLandform = true;
        var a = Eng.Call("biome_landform_of", SelectedLandRegion()).As<float[]>();
        for (int i = 0; i < _landSpins.Length && i < a.Length; i++) _landSpins[i].Value = a[i];
        _loadingLandform = false;
    }

    private void OnRegionLandform(int idx, double v)
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        Eng.Call("set_region_landform", id, idx, v);
        SetStatus($"{BiomeNames[id - 1]}: {LandNames[idx].ToLower()} {v:0.##} — Apply shaping to bake");
    }

    private void BuildPaletteEditor()
    {
        Header("PALETTE", open: false);
        _palFamily = Options(FamilyNames, 0, _ => LoadPaletteColors());
        _palPickers = new ColorPickerButton[SlotNames.Length];
        for (int s = 0; s < SlotNames.Length; s++)
        {
            var row = new HBoxContainer();
            var lbl = Dim(SlotNames[s]);
            lbl.SizeFlagsHorizontal = SizeFlags.ExpandFill;
            row.AddChild(lbl);
            var cp = new ColorPickerButton { CustomMinimumSize = new Vector2(46, 22) };
            int slot = s;
            cp.ColorChanged += c => OnPaletteColor(slot, c);
            _palPickers[s] = cp;
            row.AddChild(cp);
            _target.AddChild(row);
        }
    }

    private void LoadPaletteColors()
    {
        if (!HasWorld || !_world.GenDone) return;
        _loadingPalette = true;
        int fam = _palFamily.Selected;
        for (int s = 0; s < _palPickers.Length; s++)
        {
            var a = Eng.Call("base_palette_color", fam, s).As<float[]>();
            if (a.Length >= 3) _palPickers[s].Color = new Color(a[0], a[1], a[2]);
        }
        _loadingPalette = false;
    }

    private void OnPaletteColor(int slot, Color c)
    {
        // Defer: a ColorPicker drag fires many ColorChanged/frame; FlushDeferred applies once/frame
        // (one 1.7M-cell recolor + minimap render instead of many) — smooth palette editing.
        if (_loadingPalette) return;
        _palettePending[slot] = c;
    }

    /// Apply any coalesced edits (palette) — called once per frame by the plugin's _Process.
    public void FlushDeferred()
    {
        if (_palettePending.Count == 0 || !HasWorld) return;
        int fam = _palFamily.Selected;
        foreach (var kv in _palettePending)
            Eng.Call("set_base_palette_color", fam, kv.Key, (double)kv.Value.R, (double)kv.Value.G, (double)kv.Value.B);
        _palettePending.Clear();
        _world.RepaintDirtyTerrain();
        _minimap?.Refresh();
    }

    // --- control helpers ---

    public void SetStatus(string text) { if (_status != null) _status.Text = text; }

    /// Open a new collapsible section under the dock column; subsequent helper controls add into it
    /// (until the next Header). `open` sets the initial expanded state.
    private void Header(string title, bool open = true) => _target = DhceUi.Section(_panelRoot, title, open);

    /// Load an addon SVG icon (cached). Returns null before the editor has imported it — callers fall
    /// back to a text label so the button still works on the very first build.
    private static Texture2D Icon(string name)
    {
        if (_iconCache.TryGetValue(name, out var t)) return t;
        string path = $"res://addons/dhce/icons/{name}.svg";
        t = ResourceLoader.Exists(path) ? ResourceLoader.Load<Texture2D>(path) : null;
        _iconCache[name] = t;
        return t;
    }

    /// Style a control as a compact icon button: glyph centred, name in the tooltip; falls back to the
    /// text label if the icon hasn't imported yet.
    private static void StyleIcon(Button b, string iconName, string tooltip)
    {
        b.TooltipText = tooltip;
        b.CustomMinimumSize = new Vector2(36, 36);
        var icon = Icon(iconName);
        if (icon != null)
        {
            b.Icon = icon;
            b.IconAlignment = HorizontalAlignment.Center;
            b.AddThemeConstantOverride("icon_max_width", 22);
        }
        else b.Text = tooltip;
    }

    private void AddToolButton(Container parent, string iconName, string tooltip, ToolKind tool)
    {
        var b = new Button { ToggleMode = true, ButtonGroup = _toolGroup };
        if (string.IsNullOrEmpty(iconName)) { b.Text = tooltip; b.TooltipText = tooltip; b.CustomMinimumSize = new Vector2(36, 36); }
        else StyleIcon(b, iconName, tooltip);
        b.Pressed += () => { _tool.Active = tool; RefreshBrushOptions(); SetStatus(tooltip); };
        if (tool == _tool.Active) b.ButtonPressed = true;
        parent.AddChild(b);
        _toolButtons.Add(b);
    }

    private void AddTraitButton(Container parent, string iconName, string tooltip, int dropdownIdx)
    {
        var b = new Button { ToggleMode = true, ButtonGroup = _traitGroup };
        StyleIcon(b, iconName, tooltip);
        b.Pressed += () => SelectTrait(dropdownIdx);
        if (dropdownIdx == 0) b.ButtonPressed = true; // mirrors SelectTrait(0) initial config below
        parent.AddChild(b);
    }

    /// One of the 6 progressively-larger brush-size icons; selecting it sets the size level (the actual
    /// world radius is derived from the editor zoom each stroke — see ToolState.SizeFractions).
    private void AddSizeButton(Container parent, int level)
    {
        var b = new Button { ToggleMode = true, ButtonGroup = _sizeGroup };
        StyleIcon(b, $"size{level + 1}", $"Brush size {level + 1}");
        b.Pressed += () => _tool.SetSizeLevel(level);
        if (level == _tool.SizeLevel) b.ButtonPressed = true;
        parent.AddChild(b);
    }

    // --- scatter (N3d) model-slot editor ---

    private void BuildScatterSection()
    {
        Header("SCATTER (models)", open: false);
        _target.AddChild(Dim("Define model slots (mesh + placement rule). Preview is low-poly; export bakes the real mesh."));

        _slotPick = new OptionButton();
        _slotPick.ItemSelected += _ => LoadSlot();
        _target.AddChild(_slotPick);
        var slotBtns = new HBoxContainer();
        AddPlainButton(slotBtns, "Add slot", AddSlot);
        AddPlainButton(slotBtns, "Remove", RemoveSlot);
        _target.AddChild(slotBtns);

        _slotName = ScatterLine("Name", v => { var s = SelectedSlot(); if (s != null) { s.Name = v; RefreshSlotNames(); } });
        _slotMesh = ScatterLine("Mesh path (.glb/.res)", v => { var s = SelectedSlot(); if (s != null) s.MeshPath = v; });
        _slotProxy = ScatterLine("Proxy mesh (optional)", v => { var s = SelectedSlot(); if (s != null) s.ProxyMeshPath = v; });
        _slotDensity = Slider("Density", 0, 1, 0.01, 0.3, v => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.Density = (float)v; });
        _slotScaleMin = SpinRow("Scale min (m)", 0.1, 200, 0.1, 3);
        _slotScaleMin.ValueChanged += v => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.ScaleMin = (float)v; };
        _slotScaleMax = SpinRow("Scale max (m)", 0.1, 200, 0.1, 8);
        _slotScaleMax.ValueChanged += v => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.ScaleMax = (float)v; };
        _slotElevMin = SpinRow("Elev min", -2, 2, 0.02, 0.02);
        _slotElevMin.ValueChanged += v => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.ElevMin = (float)v; };
        _slotElevMax = SpinRow("Elev max", -2, 2, 0.02, 1.5);
        _slotElevMax.ValueChanged += v => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.ElevMax = (float)v; };
        _slotVisEnd = SpinRow("Cull distance (m, 0=never)", 0, 20000, 50, 0);
        _slotVisEnd.ValueChanged += v => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.VisibilityEndM = (float)v; };

        _target.AddChild(Dim("Appears on vegetation (none ticked = any):"));
        var vegGrid = new GridContainer { Columns = 2 };
        _slotVeg = new CheckBox[VegNames.Length];
        for (int i = 0; i < VegNames.Length; i++)
        {
            var cb = new CheckBox { Text = VegNames[i] };
            cb.Toggled += _ => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.VegetationMask = VegMaskFromChecks(); };
            _slotVeg[i] = cb;
            vegGrid.AddChild(cb);
        }
        _target.AddChild(vegGrid);

        _slotInstances = new CheckBox { Text = "Bake as individual instances (hand-editable)" };
        _slotInstances.Toggled += on => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.BakeAsInstances = on; };
        _target.AddChild(_slotInstances);

        _scatterPreview = new CheckButton { Text = "Preview scatter (low-poly)" };
        _scatterPreview.Toggled += on => _world?.PreviewScatter(on);
        _target.AddChild(_scatterPreview);
    }

    private DhceScatterSlot SelectedSlot()
    {
        var lib = _world?.Scatter;
        if (lib == null || _slotPick == null || _slotPick.Selected < 0 || _slotPick.Selected >= lib.Slots.Count) return null;
        return lib.Slots[_slotPick.Selected];
    }

    private int VegMaskFromChecks()
    {
        int m = 0;
        for (int i = 0; i < _slotVeg.Length; i++) if (_slotVeg[i].ButtonPressed) m |= 1 << i;
        return m;
    }

    private void AddSlot()
    {
        if (_world == null) return;
        _world.Scatter ??= new DhceScatterLibrary();
        _world.Scatter.Slots.Add(new DhceScatterSlot { Name = $"slot{_world.Scatter.Slots.Count + 1}" });
        RefreshSlots();
        _slotPick.Selected = _world.Scatter.Slots.Count - 1;
        LoadSlot();
    }

    private void RemoveSlot()
    {
        var lib = _world?.Scatter;
        if (lib == null || _slotPick.Selected < 0 || _slotPick.Selected >= lib.Slots.Count) return;
        lib.Slots.RemoveAt(_slotPick.Selected);
        RefreshSlots();
        LoadSlot();
    }

    private void RefreshSlots()
    {
        if (_slotPick == null) return;
        _slotPick.Clear();
        var lib = _world?.Scatter;
        if (lib != null) for (int i = 0; i < lib.Slots.Count; i++) _slotPick.AddItem(lib.Slots[i]?.Name ?? $"slot{i}", i);
    }

    private void RefreshSlotNames()
    {
        int sel = _slotPick.Selected;
        RefreshSlots();
        if (sel >= 0 && sel < _slotPick.ItemCount) _slotPick.Selected = sel;
    }

    private void LoadSlot()
    {
        var s = SelectedSlot();
        if (s == null) return;
        _loadingSlot = true;
        _slotName.Text = s.Name;
        _slotMesh.Text = s.MeshPath;
        _slotProxy.Text = s.ProxyMeshPath;
        _slotDensity.Value = s.Density;
        _slotScaleMin.Value = s.ScaleMin;
        _slotScaleMax.Value = s.ScaleMax;
        _slotElevMin.Value = s.ElevMin;
        _slotElevMax.Value = s.ElevMax;
        for (int i = 0; i < _slotVeg.Length; i++) _slotVeg[i].ButtonPressed = (s.VegetationMask & (1 << i)) != 0;
        _slotInstances.ButtonPressed = s.BakeAsInstances;
        _slotVisEnd.Value = s.VisibilityEndM;
        _loadingSlot = false;
    }

    private LineEdit ScatterLine(string label, System.Action<string> onChange)
    {
        _target.AddChild(Dim(label));
        var le = new LineEdit();
        le.TextChanged += t => { if (!_loadingSlot) onChange(t); };
        _target.AddChild(le);
        return le;
    }

    private static void AddPlainButton(Container parent, string text, System.Action onPress)
    {
        var b = new Button { Text = text, SizeFlagsHorizontal = SizeFlags.ExpandFill };
        b.Pressed += onPress;
        parent.AddChild(b);
    }

    private void AddModeButton(Container parent, string iconName, string tooltip, ToolKind tool)
    {
        var b = new Button();
        StyleIcon(b, iconName, tooltip);
        b.Pressed += () => { _tool.Active = tool; RefreshBrushOptions(); SetStatus($"Region: {tooltip} mode"); };
        parent.AddChild(b);
    }

    private Button SwatchButton(int id, string name)
    {
        var b = new Button { Text = name };
        b.Pressed += () => { _tool.Active = ToolKind.Biome; _tool.BiomeId = id; RefreshBrushOptions(); SetStatus($"stamp: {name}"); };
        return b;
    }

    private static Label Dim(string text)
    {
        var l = new Label { Text = text, AutowrapMode = TextServer.AutowrapMode.WordSmart };
        l.Modulate = new Color(1, 1, 1, 0.65f);
        return l;
    }

    private void Button(Container parent, string text, System.Action onPress)
    {
        var b = new Button { Text = text, SizeFlagsHorizontal = SizeFlags.ExpandFill };
        b.Pressed += onPress;
        parent.AddChild(b);
    }

    private OptionButton Options(string[] items, int selected, System.Action<long> onSelect)
    {
        var o = new OptionButton();
        for (int i = 0; i < items.Length; i++) o.AddItem(items[i], i);
        o.Selected = selected;
        o.ItemSelected += idx => onSelect(idx);
        _target.AddChild(o);
        return o;
    }

    private HSlider Slider(string label, double min, double max, double step, double val, System.Action<double> onChange)
    {
        var lbl = Dim($"{label}: {val:0.###}");
        _target.AddChild(lbl);
        var s = new HSlider { MinValue = min, MaxValue = max, Step = step, Value = val };
        s.ValueChanged += v => { lbl.Text = $"{label}: {v:0.###}"; onChange(v); };
        _target.AddChild(s);
        return s;
    }

    private SpinBox SpinRow(string label, double min, double max, double step, double val)
    {
        _target.AddChild(Dim(label));
        var sb = new SpinBox { MinValue = min, MaxValue = max, Step = step, Value = val };
        _target.AddChild(sb);
        return sb;
    }
}
#endif
