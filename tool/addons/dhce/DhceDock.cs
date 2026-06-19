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
    private OptionButton _viewPick;      // map-layer (VIEW) selector — moved here from DhceMapPanel
    private VBoxContainer _col;          // the sidebar dock column (this ScrollContainer's content)
    private Container _panelRoot;        // the root a new Header() opens a collapsible section under
    private VBoxContainer _target;       // the container helper controls add into (a section's content)
    private readonly ButtonGroup _toolGroup = new();   // every brush button (one active)
    private readonly ButtonGroup _sizeGroup = new();   // the 6 brush-size icons (one active)
    private readonly List<Button> _toolButtons = new();
    private static readonly Dictionary<string, Texture2D> _iconCache = new();

    private SpinBox _seed, _oct, _size, _height, _spacing, _chunk, _baseBlend;
    private SpinBox _brushRadius; // absolute brush radius (m); 0 = zoom-coupled (the size icons)
    private CheckButton _flatBase; // flat authoring base vs auto canon relief
    private OptionButton _liquidKind, _traitEnum, _palFamily, _landRegion;
    private HSlider _traitSlider;
    private Label _traitSliderLabel, _traitEnumLabel;
    private ColorPickerButton[] _palPickers;
    private SpinBox[] _landSpins;
    private SpinBox _lakeDepthSpin;   // per-Region lake-fill threshold (tied to the same region picker)
    private SpinBox _riverThreshSpin; // per-Region river threshold (tied to the same region picker)
    private SpinBox _erosionSpin;     // per-Region river-erosion strength (tied to the same region picker)
    private SpinBox _baseHeightSpin;  // per-Region base height (m); seeds the trunk — regen to apply
    private SpinBox _gradientSpin;    // per-Region gradient total rise (m)
    private SpinBox _gradientRotSpin; // per-Region gradient rotation (degrees, 0=N, CW)
    private CheckBox _gradientAnchorCheck; // pin the gradient pivot (else it follows base height)
    private SpinBox _gradientAnchorSpin;   // per-Region gradient anchor height (m), when pinned
    private CheckButton _simulate;
    private DhceMinimap _minimap;
    private LineEdit _exportPath;

    // Contextual brush-option groups — only the active tool's group is shown (see RefreshBrushOptions).
    private VBoxContainer _optSize, _optStrength, _optLiquid, _optTransition, _optCave, _optBiome, _optRegion, _optTrait;
    private VBoxContainer _optZone;                       // Zone Edit options panel
    private Label _zoneCountLabel;                        // "Selection: N cells"
    private Button _zoneUndoBtn;                          // enabled only when a zone snapshot exists
    private readonly ButtonGroup _zoneModeGroup = new();  // the zone select sub-mode buttons (one active)
    private HSlider _seaLevelSlider;
    private Label _seaLevelLabel;

    // Scatter (N3d) model-slot editor.
    private OptionButton _slotPick;
    private LineEdit _slotName, _slotMesh, _slotProxy;
    private HSlider _slotDensity;
    private SpinBox _slotScaleMin, _slotScaleMax, _slotElevMin, _slotElevMax, _slotVisEnd;
    private CheckBox[] _slotVeg;
    private CheckBox[] _slotBiome;
    private CheckButton _scatterPreview;
    private CheckBox _slotInstances;
    private bool _loadingSlot;
    private readonly Dictionary<int, Color> _palettePending = new(); // coalesce rapid ColorPicker drags
    private bool _loadingPalette, _loadingLandform;

    private double _shapeStrength = 1.0, _transitionWidthM = 200, _simFlow = 0.45, _simEvap = 0.001;
    private double _erodeTalus = 0.01, _erodeAmount = 0.5; // whole-map thermal erosion (normalized talus, shed fraction)
    private int _erodeIters = 15;
    private int _simSubsteps = 10, _simTick;
    private const int SimEveryNFrames = 6;

    private static readonly int[] TraitEngineId = { 4, 5, 6, 7 };
    private static readonly string[] ViewNames = { "Natural", "Temperature", "Moisture", "Elevation", "Biome", "Region" };
    private static readonly string[] LandNames = { "Jaggedness", "Relief", "Foothill falloff", "Erosion" };
    private static readonly string[] VegNames = { "Barren", "Grass", "Scrub", "Forest", "Evergreen", "Marsh", "Thorn" };
    private static readonly string[] FamilyNames = { "Verdant", "Arid", "Stone", "Ashen", "Frost", "Wetland", "Exotic" };
    private static readonly string[] SlotNames = { "Deep water", "Shallows", "Low cover", "Rock", "Cap (warm)", "Cap (snow)" };
    private static readonly string[] BiomeNames =
    {
        "Jagged Mountains", "Sacred Forest", "Great Lake", "Temperate Forest",
        "Rolling Plains", "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles",
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
    private bool _bindErrLogged; // gate so a failing gen-done reload logs once, not every frame

    public void Bind(DhceWorld world)
    {
        bool changed = world != _world;
        _world = world;
        bool gen = world != null && world.GenDone;
        bool reload = world != null && (changed || (gen && !_wasGenDone));
        // Latch BEFORE the reload: if anything below throws, the edge condition won't re-fire next
        // frame, so a reload bug can't turn into a per-frame Output flood (it surfaces once instead).
        _wasGenDone = gen;
        if (reload)
        {
            try
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
            catch (System.Exception ex)
            {
                if (!_bindErrLogged) { _bindErrLogged = true; GD.PrintErr("[DHCE] Bind reload failed (logged once): ", ex); }
            }
        }
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

        Header("MAP VIEW"); // map-layer selector — recolours the terrain + minimap; readouts stay under the minimap
        _viewPick = new OptionButton();
        for (int i = 0; i < ViewNames.Length; i++) _viewPick.AddItem(ViewNames[i], i);
        _viewPick.Selected = 0;
        _viewPick.ItemSelected += idx => { _world?.SetViewMode((int)idx); _minimap?.Refresh(); };
        _target.AddChild(_viewPick);

        Header("WORLD");
        _seed = SpinRow("Seed", 0, 999999, 1, 12345);
        _oct = SpinRow("Octaves", 1, 12, 1, 6);
        _size = SpinRow("Width (km)", 1, 80, 1, 40);   // E–W; canon map is 4:3 (40×30)
        _height = SpinRow("Height (km)", 1, 80, 1, 30); // N–S
        _spacing = SpinRow("Spacing (m)", 4, 60, 1, 10);
        _chunk = SpinRow("Chunk size (m)", 32, 2048, 32, 256);
        _baseBlend = SpinRow("Base blend (m)", 0, 6000, 50, 1800); // softer per-region base-elevation steps; regen to apply
        _flatBase = new CheckButton
        {
            Text = "Flat base (sculpt all relief)", ButtonPressed = true,
            TooltipText = "On: a flat plate draped with the canon art, sea level = plate (indent to add water; sculpt all relief).\nOff: auto canon relief + rivers/lakes. Regenerate to apply.",
        };
        _target.AddChild(_flatBase);
        Slider("Terrain height (km)", 0.1, 10, 0.1, 5.0, v => _world?.SetTerrainHeight((float)v));

        BuildRegionMapSection(); // optional region-coloured PNG → overrides the built-in canon layout
        BuildBrushes();        // all paint tools + their contextual options
        BuildShapingSection(); // region landform + shaping + transitions (not brushes)
        BuildClimateSection(); // physical climate: lapse (temp) + orographic (moisture) + wind
        BuildPaletteEditor();  // render: per-family palette colours
        BuildPhysicsSection(); // sea level / flow / evaporation / substeps / settle / clear / simulate
        BuildScatterSection(); // model scatter (per-slot; per-biome rework is a separate phase)

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
        AddToolButton(toolRow, "raise", "Raise (Ctrl: lower)", ToolKind.Raise);
        AddToolButton(toolRow, "carve", "Carve (Ctrl: raise)", ToolKind.Carve);
        AddToolButton(toolRow, "smooth", "Smooth (Ctrl: roughen)", ToolKind.Smooth);
        AddToolButton(toolRow, "level", "Level", ToolKind.Level);
        AddToolButton(toolRow, "flatten", "Flatten to plane", ToolKind.Flatten);
        AddToolButton(toolRow, "crest", "Crest", ToolKind.Crest);
        AddToolButton(toolRow, "grab", "Grab (drag up/down)", ToolKind.Grab);
        AddToolButton(toolRow, "erode", "Erode (thermal)", ToolKind.Erode);
        AddToolButton(toolRow, "river", "River", ToolKind.River);
        AddToolButton(toolRow, "flood", "Flood", ToolKind.Flood);
        AddToolButton(toolRow, "biome", "Biome", ToolKind.Biome);
        AddToolButton(toolRow, "region", "Region", ToolKind.Region);
        // Trait brushes, promoted to top-level — each arms ToolKind.Trait via SelectTrait (sets TraitId + options).
        AddTraitBrushButton(toolRow, "temperature", "Temperature", 0);
        AddTraitBrushButton(toolRow, "moisture", "Moisture", 1);
        AddTraitBrushButton(toolRow, "vegetation", "Vegetation", 2);
        AddTraitBrushButton(toolRow, "palette", "Palette family", 3);
        AddToolButton(toolRow, "cave", "Cave", ToolKind.Cave);
        AddToolButton(toolRow, "transition", "Transition", ToolKind.Transition);
        AddToolButton(toolRow, "zone", "Zone Edit", ToolKind.Zone);

        // SIZE — applies to every tool, always shown.
        _optSize = NewGroup(sec);
        _optSize.AddChild(Dim("Size (scales with zoom):"));
        var sizeRow = new HFlowContainer();
        _optSize.AddChild(sizeRow);
        for (int lvl = 0; lvl < ToolState.SizeFractions.Length; lvl++) AddSizeButton(sizeRow, lvl);
        // Absolute-radius override: type an exact brush radius in metres to shape very large or very
        // small areas regardless of zoom. 0 = auto (the zoom-coupled icons above); else 0.2 m … 2 km.
        _optSize.AddChild(Dim("Radius (m) — 0 = auto, else 0.2 … 2000:"));
        _brushRadius = new SpinBox
        {
            MinValue = 0, MaxValue = 2000, Step = 0.1, Value = 0,
            TooltipText = "Absolute brush radius in metres (0.2–2000). 0 = zoom-coupled (size icons).",
        };
        _brushRadius.ValueChanged += v => _tool.RadiusOverrideM = (float)v;
        _optSize.AddChild(_brushRadius);

        // STRENGTH — sculpt tools.
        _optStrength = NewGroup(sec);
        _target = _optStrength;
        Slider("Strength (m)", 1, 400, 1, _tool.StrengthM, v => _tool.StrengthM = (float)v);
        _target = sec;

        // LIQUID — River / Flood.
        _optLiquid = NewGroup(sec);
        _target = _optLiquid;
        _liquidKind = Options(new[] { "Water", "Lava", "Marsh", "Ice" }, 0, idx => _tool.LiquidKind = (int)idx);
        Button(_optLiquid, "Generate streams", () => { if (HasWorld) { Eng.Call("generate_streams", 0.5, 1.0); _world.RepaintDirtyTerrain(); _world.RebuildLiquid(); } });
        _target = sec;

        // TRANSITION brush — local blend across a biome seam (the global "Blend borders" bake stays).
        _optTransition = NewGroup(sec);
        _target = _optTransition;
        Slider("Width (m)", 0, 1200, 25, _tool.BlendWidthM, v => _tool.BlendWidthM = (float)v);
        _optTransition.AddChild(Dim("Drag across a biome seam to soften just that border. Global 'Blend borders' (TRANSITIONS) still blends the whole map."));
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

        // TRAIT brush options — the four trait brush buttons live in the BRUSHES row above; this
        // group holds only their shared contextual options (scalar value slider or enum type).
        _optTrait = NewGroup(sec);
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

        BuildZoneOptions(sec);

        _target = sec;
    }

    // ZONE EDIT — select a whole zone, then apply whole-zone operations (the selection persists, so
    // you can chain: select lake → Drop → Smooth edges → Fill). Selecting happens in the viewport;
    // these buttons run the ops on the current selection (held in ToolState.ZoneSelection).
    private void BuildZoneOptions(VBoxContainer sec)
    {
        _optZone = NewGroup(sec);
        _target = _optZone;
        _optZone.AddChild(Dim("Select a zone in the viewport, then edit it as one. Shift-click adds."));

        // Selection sub-mode.
        var modeRow = new HFlowContainer();
        AddZoneModeButton(modeRow, "Smart", ToolState.ZoneSelectMode.Smart, "Grow across smooth terrain, snap at sharp contrast");
        AddZoneModeButton(modeRow, "Water", ToolState.ZoneSelectMode.Water, "Select a whole lake / water body");
        AddZoneModeButton(modeRow, "Region", ToolState.ZoneSelectMode.Contiguous, "Select the contiguous same-region area");
        AddZoneModeButton(modeRow, "Polygon", ToolState.ZoneSelectMode.Polygon, "Draw an outline (right-click closes)");
        _optZone.AddChild(modeRow);
        Slider("Smart tolerance", 0.005, 0.3, 0.005, _tool.ZoneTolerance, v => _tool.ZoneTolerance = (float)v);
        _zoneCountLabel = Dim("Selection: 0 cells");
        _optZone.AddChild(_zoneCountLabel);
        Button(_optZone, "Clear selection", ClearZoneSelectionFromDock);

        _optZone.AddChild(Dim("— Elevation —"));
        Slider("Offset (m)", 1, 2000, 1, _tool.ZoneOffsetM, v => _tool.ZoneOffsetM = (float)v);
        var offsetRow = new HBoxContainer();
        Button(offsetRow, "Drop", () => ZoneOffset(-1));
        Button(offsetRow, "Raise", () => ZoneOffset(+1));
        _optZone.AddChild(offsetRow);
        Button(_optZone, "Level (to mean)", ZoneLevel);
        Button(_optZone, "Add grade (click low → high)", ArmZoneGrade);

        _optZone.AddChild(Dim("— Smoothing —"));
        Slider("Iterations", 1, 30, 1, _tool.ZoneSmoothIters, v => _tool.ZoneSmoothIters = (int)v);
        Slider("Strength", 0.05, 1, 0.05, _tool.ZoneSmoothWeight, v => _tool.ZoneSmoothWeight = (float)v);
        Button(_optZone, "Smooth zone", ZoneSmooth);
        Slider("Feather width (m)", 50, 4000, 50, _tool.ZoneFeatherWidthM, v => _tool.ZoneFeatherWidthM = (float)v);
        Button(_optZone, "Smooth edges (feather)", ZoneFeather);

        _optZone.AddChild(Dim("— Water —"));
        Options(new[] { "Water", "Lava", "Marsh", "Ice" }, 0, idx => _tool.ZoneFillKind = (int)idx);
        Slider("Fill above rim (m)", 0, 1000, 5, _tool.ZoneFillOffsetM, v => _tool.ZoneFillOffsetM = (float)v);
        Button(_optZone, "Fill water (to rim)", ZoneFill);

        _zoneUndoBtn = new Button { Text = "Undo last zone edit", SizeFlagsHorizontal = SizeFlags.ExpandFill, Disabled = true };
        _zoneUndoBtn.Pressed += () => { _world?.UndoZone(); RefreshZoneButtons(); SetStatus("undid last zone edit"); };
        _optZone.AddChild(_zoneUndoBtn);

        _target = sec;
    }

    private void AddZoneModeButton(Container parent, string label, ToolState.ZoneSelectMode mode, string tip)
    {
        var b = new Button { Text = label, ToggleMode = true, ButtonGroup = _zoneModeGroup, TooltipText = tip };
        b.Pressed += () => { _tool.Active = ToolKind.Zone; _tool.ZoneSelect = mode; RefreshBrushOptions(); SetStatus($"zone select: {label}"); };
        if (_tool.ZoneSelect == mode) b.ButtonPressed = true;
        parent.AddChild(b);
    }

    private bool HasZoneSel => HasWorld && _tool.HasZoneSelection;

    private void ZoneOffset(int sign)
    {
        if (!HasZoneSel) { SetStatus("select a zone first"); return; }
        double delta = sign * _tool.ZoneOffsetM / Mathf.Max(_world.Exaggeration, 1f);
        _world.SnapshotForZone(true);
        var touched = Eng.Call("zone_offset", _tool.ZoneSelection, delta).As<int[]>();
        AfterZoneEdit($"{(sign < 0 ? "dropped" : "raised")} {touched.Length} cells");
    }

    private void ZoneLevel()
    {
        if (!HasZoneSel) { SetStatus("select a zone first"); return; }
        double mean = Eng.Call("zone_mean_elevation", _tool.ZoneSelection).AsDouble();
        _world.SnapshotForZone(true);
        var touched = Eng.Call("zone_level", _tool.ZoneSelection, mean, (double)_tool.ZoneLevelWeight).As<int[]>();
        AfterZoneEdit($"levelled {touched.Length} cells to mean");
    }

    private void ZoneSmooth()
    {
        if (!HasZoneSel) { SetStatus("select a zone first"); return; }
        _world.SnapshotForZone(true);
        var touched = Eng.Call("zone_smooth", _tool.ZoneSelection, (long)_tool.ZoneSmoothIters, (double)_tool.ZoneSmoothWeight).As<int[]>();
        AfterZoneEdit($"smoothed {touched.Length} cells");
    }

    private void ZoneFeather()
    {
        if (!HasZoneSel) { SetStatus("select a zone first"); return; }
        _world.SnapshotForZone(true);
        var touched = Eng.Call("zone_feather_edges", _tool.ZoneSelection, (double)_tool.ZoneFeatherWidthM,
            (double)_tool.ZoneSmoothWeight, (long)_tool.ZoneSmoothIters).As<int[]>();
        AfterZoneEdit($"feathered {touched.Length} edge cells");
    }

    private void ZoneFill()
    {
        if (!HasZoneSel) { SetStatus("select a zone first"); return; }
        double rim = Eng.Call("zone_rim_level", _tool.ZoneSelection).AsDouble();
        if (double.IsNaN(rim)) { SetStatus("zone has no rim to fill to"); return; }
        double target = rim + _tool.ZoneFillOffsetM / Mathf.Max(_world.Exaggeration, 1f);
        _world.SnapshotForZone(true);
        var touched = Eng.Call("zone_fill_liquid", _tool.ZoneSelection, target, (long)_tool.ZoneFillKind).As<int[]>();
        _world.RepaintDirtyTerrain();
        _world.RebuildLiquid();
        _world.BuildSelectionOverlay(_tool.ZoneSelection);
        RefreshZoneButtons();
        SetStatus($"filled {touched.Length} cells to the rim");
    }

    private void ArmZoneGrade()
    {
        if (!HasZoneSel) { SetStatus("select a zone first"); return; }
        _tool.ZoneGradeArmed = true; _tool.ZoneGradeHasLow = false;
        SetStatus("grade: click the LOW point, then the HIGH point (Esc cancels)");
    }

    /// Re-tessellate, re-conform the selection overlay to the new surface, and refresh the buttons.
    private void AfterZoneEdit(string msg)
    {
        _world.RepaintDirtyTerrain();
        _world.RebuildLiquid();
        _world.BuildSelectionOverlay(_tool.ZoneSelection);
        RefreshZoneButtons();
        SetStatus(msg);
    }

    private void ClearZoneSelectionFromDock()
    {
        _tool.ZoneSelection = System.Array.Empty<int>();
        _tool.ZoneGradeArmed = false; _tool.ZoneGradeHasLow = false;
        if (_world != null) { _world.ClearSelectionOverlay(); _world.DimCanonForSelection(false); }
        RefreshZoneButtons();
        SetStatus("cleared selection");
    }

    /// Refresh the zone readout (cell count) and the undo button's enabled state. Called by the plugin
    /// after a viewport selection and by the dock after each op.
    public void RefreshZoneButtons()
    {
        if (_zoneCountLabel != null) _zoneCountLabel.Text = $"Selection: {(_tool?.ZoneSelection?.Length ?? 0)} cells";
        if (_zoneUndoBtn != null) _zoneUndoBtn.Disabled = !(_world != null && _world.HasZoneUndo);
    }

    /// Show only the active tool's option group (size always shows).
    private void RefreshBrushOptions()
    {
        var a = _tool.Active;
        bool sculpt = a == ToolKind.Raise || a == ToolKind.Carve || a == ToolKind.Level || a == ToolKind.Crest
            || a == ToolKind.Smooth || a == ToolKind.Flatten || a == ToolKind.Grab || a == ToolKind.Erode;
        bool liquid = a == ToolKind.River || a == ToolKind.Flood;
        bool region = a == ToolKind.Region || a == ToolKind.Territory || a == ToolKind.RegionSelect;
        if (_optStrength != null) _optStrength.Visible = sculpt;
        if (_optLiquid != null) _optLiquid.Visible = liquid;
        if (_optTransition != null) _optTransition.Visible = a == ToolKind.Transition;
        if (_optCave != null) _optCave.Visible = a == ToolKind.Cave;
        if (_optBiome != null) _optBiome.Visible = a == ToolKind.Biome;
        if (_optRegion != null) _optRegion.Visible = region;
        if (_optTrait != null) _optTrait.Visible = a == ToolKind.Trait;
        if (_optZone != null) _optZone.Visible = a == ToolKind.Zone;
        // The zone selection persists across tool switches (for chaining), but its highlight + canon dim
        // only show while the Zone tool is active — so other tools see the normal canon drape.
        if (HasWorld)
        {
            bool zoneShown = a == ToolKind.Zone && _tool.HasZoneSelection;
            _world.SetSelectionOverlayVisible(zoneShown);
            _world.DimCanonForSelection(zoneShown);
        }
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
        // Per-region base terrain (seeds the region-guided elevation; takes effect on Generate). Base
        // height in metres; gradient is the TOTAL rise across the region from its low edge to its high
        // edge along the rotation (0° = North, clockwise). The gradient pivots about the region centre,
        // which sits at the base height (so changing base height moves the whole ramp) — tick "Pin anchor"
        // to fix the pivot at an explicit height instead.
        _target.AddChild(Dim("— Base terrain (regenerate to apply) —"));
        _baseHeightSpin = SpinRow("Base height (m)", -8000, 8000, 10, 0);
        _baseHeightSpin.ValueChanged += v => OnRegionBaseHeight(v);
        _gradientSpin = SpinRow("Gradient — total rise (m)", -8000, 8000, 10, 0);
        _gradientSpin.ValueChanged += v => OnRegionGradient(v);
        _gradientRotSpin = SpinRow("Gradient rotation (°, 0=N CW)", 0, 360, 5, 0);
        _gradientRotSpin.ValueChanged += v => OnRegionGradientRotation(v);
        _gradientAnchorCheck = new CheckBox { Text = "Pin gradient anchor (else follows base)" };
        _gradientAnchorCheck.Toggled += _ => OnRegionGradientAnchor();
        _target.AddChild(_gradientAnchorCheck);
        _gradientAnchorSpin = SpinRow("Anchor height (m)", -8000, 8000, 10, 0);
        _gradientAnchorSpin.Editable = false;
        _gradientAnchorSpin.ValueChanged += _ => OnRegionGradientAnchor();
        _target.AddChild(Dim("Applies on Generate. Generate with 'Flat authoring base' OFF (WORLD) to see it — the flat base ignores region heights. Heights clamp to the world's height scale (TerrainHeightKm)."));

        // Per-region water tiers. Lake fill depth: how deep a closed basin must be before it holds a
        // lake/pond here. River threshold: what share of the basin's peak flow a cell must carry before
        // it becomes a trunk river here. Both default from the region's moisture; lower ⇒ more water.
        _lakeDepthSpin = SpinRow("Lake fill depth", 0, 0.5, 0.005, 0);
        _lakeDepthSpin.ValueChanged += v => OnRegionLakeDepth(v);
        _riverThreshSpin = SpinRow("River threshold", 0, 0.2, 0.005, 0);
        _riverThreshSpin.ValueChanged += v => OnRegionRiverThreshold(v);
        // River erosion: how deeply this region's rivers carve their valleys (hydraulic erosion strength,
        // ×1 = neutral). Higher ⇒ deeper canyons; 0 ⇒ no incision here.
        _erosionSpin = SpinRow("River erosion", 0, 3, 0.1, 1);
        _erosionSpin.ValueChanged += v => OnRegionErosion(v);
        Button(_target, "Apply water", () =>
        {
            if (!HasWorld) return;
            _world.SetSeaLevel(_world.SeaLevelNorm); // re-flow rivers + lakes + outlets at the current thresholds
            SetStatus("Rivers + lakes re-flowed from the per-region thresholds.");
        });
        _target.AddChild(Dim("Per-region water: lake fill depth (lower ⇒ more lakes) and river threshold (lower ⇒ more rivers). Defaults track moisture. Apply water to update."));

        Header("SHAPING", open: false);
        _target.AddChild(Dim("Bake the landform dials into the terrain height, then re-flow the watershed."));
        Slider("Strength", 0, 2, 0.05, _shapeStrength, v => _shapeStrength = v);
        Button(_target, "Apply shaping", () =>
        {
            if (!HasWorld) return;
            // Reshape relief then re-flow rivers/lakes on it, in the one idempotent order.
            Eng.Call("reshape_and_reflow", _shapeStrength, (double)_world.RiverDepthGain);
            _world.RepaintDirtyTerrain(); _world.RebuildLiquid();
            _minimap?.Refresh();
            SetStatus($"shaped @ strength {_shapeStrength:0.##} — rivers + lakes re-flowed");
        });

        Header("EROSION", open: false);
        _target.AddChild(Dim("Thermal (talus) erosion — collapse steep faces into stable slopes across the whole map. Talus = max stable steepness (lower ⇒ gentler); the Erode brush does this locally."));
        Slider("Talus", 0.002, 0.05, 0.001, _erodeTalus, v => _erodeTalus = v);
        Slider("Amount", 0.1, 1.0, 0.05, _erodeAmount, v => _erodeAmount = v);
        Slider("Iterations", 1, 60, 1, _erodeIters, v => _erodeIters = (int)v);
        Button(_target, "Apply erosion", () =>
        {
            if (!HasWorld) return;
            Eng.Call("erode", (long)_erodeIters, _erodeTalus, _erodeAmount);
            _world.RepaintDirtyTerrain(); _world.RebuildLiquid();
            _minimap?.Refresh();
            SetStatus($"eroded {_erodeIters}× @ talus {_erodeTalus:0.###}");
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
        // Sea level — captured so it syncs to the generated default (1 km above the lowest basin).
        _seaLevelLabel = Dim("Sea level: 0.00");
        _target.AddChild(_seaLevelLabel);
        _seaLevelSlider = new HSlider { MinValue = -1.5, MaxValue = 1.5, Step = 0.01, Value = 0.0, SizeFlagsHorizontal = SizeFlags.ExpandFill };
        _seaLevelSlider.ValueChanged += v => { _seaLevelLabel.Text = $"Sea level: {v:0.00}"; if (HasWorld) _world.SetSeaLevel(v); };
        _target.AddChild(_seaLevelSlider);

        Slider("Flow rate", 0.0, 0.5, 0.01, _simFlow, v => _simFlow = v);
        Slider("Evaporation", 0.0, 0.02, 0.0005, _simEvap, v => _simEvap = v);
        Slider("Substeps", 1, 40, 1, _simSubsteps, v => _simSubsteps = (int)v);
        Button(_target, "Apply rainfall", () => { if (HasWorld) { _world.ApplyRainfall(); _minimap?.Refresh(); SetStatus("applied climate rainfall — Settle further if needed"); } });
        Button(_target, "Settle (1 step)", () => { if (HasWorld) { Eng.Call("step_fluid", _simFlow, _simEvap, _simSubsteps); _world.RebuildLiquid(); } });
        Button(_target, "Clear liquid", () => { if (HasWorld) { Eng.Call("clear_liquid"); _world.RebuildLiquid(); } });
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
        _world.WorldHeightKm = (float)_height.Value;
        _world.SpacingM = (float)_spacing.Value;
        _world.ChunkSizeM = (float)_chunk.Value;
        _world.BaseBlendM = (float)_baseBlend.Value;
        _world.FlatBase = _flatBase.ButtonPressed;
        SetStatus("Generating… (the editor pauses a few seconds)");
        _world.Generate();
        _wasGenDone = false; // force a value reload on the next Bind
        // Auto-select the world node so the viewport brushes are live immediately — the plugin only
        // forwards 3D input while the DhceWorld is the selected/edited node (else the cursor reads "dead").
        try { var sel = EditorInterface.Singleton.GetSelection(); sel.Clear(); sel.AddNode(_world); } catch { }
        SetStatus($"Generated ~{_world.WorldWidthM / 1000f:0.0} km — node selected; left-drag in the 3D view to paint.");
    }

    // --- region map (canon layout source) ---

    private FileDialog _regionMapDialog;

    /// REGION MAP section: import a region-coloured PNG to override the built-in canon layout, or
    /// reset to the built-in anchors. The PNG drives the generator's first pass (territories).
    private void BuildRegionMapSection()
    {
        Header("REGION MAP", open: false);
        _target.AddChild(Dim("Optional. Paint each region in its accent colour (see VIEW → Region) and any water in a liquid colour (teal=water, orange=lava, olive=marsh, pale-blue=ice) to pool a 10 m body there. Fuzzy edges, shading & contour lines are OK — it votes by majority. Transparent/navy = ocean. North = top. Blank = built-in canon layout."));
        Button(_target, "Import region map (PNG/JPG)…", OpenRegionMapDialog);
        Button(_target, "Use built-in canon layout", () =>
        {
            if (_world == null) { SetStatus("No DhceWorld in the scene."); return; }
            _world.RegionMap = null;
            OnGenerate();
            SetStatus("Region map cleared → built-in canon layout.");
        });
    }

    private void OpenRegionMapDialog()
    {
        if (_world == null) { SetStatus("No DhceWorld in the scene. Add one first."); return; }
        if (_regionMapDialog == null)
        {
            _regionMapDialog = new FileDialog
            {
                FileMode = FileDialog.FileModeEnum.OpenFile,
                Access = FileDialog.AccessEnum.Filesystem,
                Title = "Select a region-coloured map (PNG/JPG)",
            };
            _regionMapDialog.AddFilter("*.png,*.jpg,*.jpeg", "Image (PNG/JPG)");
            _regionMapDialog.FileSelected += OnRegionMapSelected;
            AddChild(_regionMapDialog);
        }
        _regionMapDialog.PopupCentered(new Vector2I(720, 520));
    }

    private void OnRegionMapSelected(string path)
    {
        if (_world == null) return;
        var img = Image.LoadFromFile(path);
        if (img == null) { SetStatus($"Could not load image: {path}"); return; }
        _world.RegionMap = ImageTexture.CreateFromImage(img);
        OnGenerate();
        SetStatus($"Region map: {System.IO.Path.GetFileName(path)} → regenerated.");
    }

    private void PullWorldParams()
    {
        _seed.Value = _world.Seed;
        _oct.Value = _world.Octaves;
        _size.Value = _world.WorldSizeKm;
        _height.Value = _world.WorldHeightKm;
        _spacing.Value = _world.SpacingM;
        _chunk.Value = _world.ChunkSizeM;
        _baseBlend.Value = _world.BaseBlendM;
        _flatBase.ButtonPressed = _world.FlatBase;
        if (_seaLevelSlider != null)
        {
            _seaLevelSlider.SetValueNoSignal(_world.SeaLevelNorm);
            if (_seaLevelLabel != null) _seaLevelLabel.Text = $"Sea level: {_world.SeaLevelNorm:0.00}";
        }
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
        if (_lakeDepthSpin != null)
            _lakeDepthSpin.Value = Eng.Call("region_lake_depth_of", SelectedLandRegion()).As<double>();
        if (_riverThreshSpin != null)
            _riverThreshSpin.Value = Eng.Call("river_threshold_of", SelectedLandRegion()).As<double>();
        if (_erosionSpin != null)
            _erosionSpin.Value = Eng.Call("erosion_of", SelectedLandRegion()).As<double>();
        // Base-terrain knobs (stored normalized; shown in metres via the world's height scale).
        float exag = Mathf.Max(_world.Exaggeration, 1f);
        int lr = SelectedLandRegion();
        if (_baseHeightSpin != null)
            _baseHeightSpin.Value = Eng.Call("region_base_height_of", lr).As<double>() * exag;
        if (_gradientSpin != null)
            _gradientSpin.Value = Eng.Call("region_gradient_of", lr).As<double>() * exag;
        if (_gradientRotSpin != null)
            _gradientRotSpin.Value = Eng.Call("region_gradient_rotation_of", lr).As<double>();
        if (_gradientAnchorCheck != null)
        {
            double anchorNorm = Eng.Call("region_gradient_anchor_of", lr).As<double>();
            bool pinned = !double.IsNaN(anchorNorm);
            _gradientAnchorCheck.ButtonPressed = pinned;
            if (_gradientAnchorSpin != null)
            {
                _gradientAnchorSpin.Value = (pinned ? anchorNorm : Eng.Call("region_base_height_of", lr).As<double>()) * exag;
                _gradientAnchorSpin.Editable = pinned;
            }
        }
        _loadingLandform = false;
    }

    private void OnRegionBaseHeight(double meters)
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        Eng.Call("set_region_base_height", id, meters / Mathf.Max(_world.Exaggeration, 1f));
        SetStatus($"{BiomeNames[id - 1]}: base height {meters:0} m — Generate to apply");
    }

    private void OnRegionGradient(double meters)
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        Eng.Call("set_region_gradient", id, meters / Mathf.Max(_world.Exaggeration, 1f));
        SetStatus($"{BiomeNames[id - 1]}: gradient {meters:0} m total rise — Generate to apply");
    }

    private void OnRegionGradientRotation(double deg)
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        Eng.Call("set_region_gradient_rotation", id, deg);
        SetStatus($"{BiomeNames[id - 1]}: gradient rotation {deg:0}° — Generate to apply");
    }

    private void OnRegionGradientAnchor()
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        bool pinned = _gradientAnchorCheck != null && _gradientAnchorCheck.ButtonPressed;
        if (_gradientAnchorSpin != null) _gradientAnchorSpin.Editable = pinned;
        double value = pinned ? _gradientAnchorSpin.Value / Mathf.Max(_world.Exaggeration, 1f) : double.NaN;
        Eng.Call("set_region_gradient_anchor", id, value);
        SetStatus(pinned
            ? $"{BiomeNames[id - 1]}: gradient anchor pinned at {_gradientAnchorSpin.Value:0} m — Generate to apply"
            : $"{BiomeNames[id - 1]}: gradient anchor follows base height — Generate to apply");
    }

    private void OnRegionLandform(int idx, double v)
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        Eng.Call("set_region_landform", id, idx, v);
        SetStatus($"{BiomeNames[id - 1]}: {LandNames[idx].ToLower()} {v:0.##} — Apply shaping to bake");
    }

    private void OnRegionLakeDepth(double v)
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        Eng.Call("set_region_lake_depth", id, v);
        SetStatus($"{BiomeNames[id - 1]}: lake fill depth {v:0.###} — Apply water to update");
    }

    private void OnRegionRiverThreshold(double v)
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        Eng.Call("set_river_threshold", id, v);
        SetStatus($"{BiomeNames[id - 1]}: river threshold {v:0.###} (lower ⇒ more rivers) — Apply water to update");
    }

    private void OnRegionErosion(double v)
    {
        if (_loadingLandform || !HasWorld) return;
        int id = SelectedLandRegion();
        Eng.Call("set_erosion", id, v);
        SetStatus($"{BiomeNames[id - 1]}: river erosion ×{v:0.##} (deeper valleys) — Apply shaping or water to re-erode");
    }

    // Physical climate sliders: temperature follows elevation (lapse), moisture follows the wind
    // (orographic). Each edits the param + recomputes live (no regen).
    private void BuildClimateSection()
    {
        Header("CLIMATE", open: false);
        _target.AddChild(Dim("Physical climate (recolors live): high ground gets colder (lapse); slopes facing the wind get wetter while their lee dries out (orographic)."));
        Slider("Lapse rate", 0, 1.5, 0.05, 0.6, v =>
        {
            if (_world == null) return;
            _world.LapseRate = (float)v;
            _world.RecomputeClimate();
            _minimap?.Refresh();
        });
        Slider("Orographic strength", 0, 1, 0.05, 0.45, v =>
        {
            if (_world == null) return;
            _world.OrographicStrength = (float)v;
            _world.RecomputeClimate();
            _minimap?.Refresh();
        });
        Slider("Wind direction (°)", 0, 360, 5, 0, v =>
        {
            if (_world == null) return;
            _world.WindDeg = (float)v;
            _world.RecomputeClimate();
            _minimap?.Refresh();
        });
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

    // A trait brush in the main BRUSHES row: shares _toolGroup with every other brush (mutually
    // exclusive), and on press arms ToolKind.Trait via SelectTrait (which sets TraitId + options).
    private void AddTraitBrushButton(Container parent, string iconName, string tooltip, int dropdownIdx)
    {
        var b = new Button { ToggleMode = true, ButtonGroup = _toolGroup };
        StyleIcon(b, iconName, tooltip);
        b.Pressed += () => SelectTrait(dropdownIdx);
        if (_tool.Active == ToolKind.Trait && _tool.TraitId == TraitEngineId[dropdownIdx]) b.ButtonPressed = true;
        parent.AddChild(b);
        _toolButtons.Add(b);
    }

    /// One of the 6 progressively-larger brush-size icons; selecting it sets the size level (the actual
    /// world radius is derived from the editor zoom each stroke — see ToolState.SizeFractions).
    private void AddSizeButton(Container parent, int level)
    {
        var b = new Button { ToggleMode = true, ButtonGroup = _sizeGroup };
        StyleIcon(b, $"size{level + 1}", $"Brush size {level + 1}");
        // Picking a zoom-coupled icon clears any absolute-radius override (back to auto).
        b.Pressed += () => { _tool.SetSizeLevel(level); if (_brushRadius != null) _brushRadius.Value = 0; };
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

        _target.AddChild(Dim("Appears in biomes (none ticked = any):"));
        var biomeGrid = new GridContainer { Columns = 2 };
        _slotBiome = new CheckBox[BiomeNames.Length];
        for (int i = 0; i < BiomeNames.Length; i++)
        {
            var cb = new CheckBox { Text = BiomeNames[i] };
            cb.Toggled += _ => { var s = SelectedSlot(); if (s != null && !_loadingSlot) s.BiomeMask = BiomeMaskFromChecks(); };
            _slotBiome[i] = cb;
            biomeGrid.AddChild(cb);
        }
        _target.AddChild(biomeGrid);

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

    private int BiomeMaskFromChecks()
    {
        int m = 0;
        for (int i = 0; i < _slotBiome.Length; i++) if (_slotBiome[i].ButtonPressed) m |= 1 << i;
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
        for (int i = 0; i < _slotBiome.Length; i++) _slotBiome[i].ButtonPressed = (s.BiomeMask & (1 << i)) != 0;
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
