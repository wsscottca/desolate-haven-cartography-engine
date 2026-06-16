#if TOOLS
using Godot;

namespace DesolateHaven.Cartography;

/// Viewport overlay docked directly under the minimap: the live readouts for the surface under the
/// cursor (emergent biome, named region, temperature/moisture) plus the VIEW selector — which
/// recolours both the 3D view and the overview map — and its contextual temperature/moisture paint
/// slider. Kept out of the dock so all map-related info reads together with the overview. Fed by the
/// plugin each frame: `SetWorld` (for the view switch) and the per-hover `Set*Readout` calls.
public partial class DhceMapPanel : PanelContainer
{
    private static readonly string[] ViewNames = { "Natural", "Temperature", "Moisture", "Elevation", "Biome", "Region" };
    private static readonly string[] BiomeNames =
    {
        "Jagged Mountains", "Sacred Woods Plateau", "Great Lake", "Temperate Forest",
        "Open Plains", "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles",
        "Blisterwood", "Volcanic Scape", "Blight Ruins", "Scattered Isles", "Marsh & Bog",
    };

    private ToolState _tool;
    private DhceMinimap _minimap;
    private DhceWorld _world;

    private Label _biome, _region, _trait, _viewHint, _viewBrushLabel;
    private OptionButton _view;
    private HSlider _viewBrushSlider;

    /// Build the panel once; the plugin passes the shared ToolState + the minimap, then calls
    /// `SetWorld` each frame and `Set*Readout` from the hover raycast.
    public void Init(ToolState tool, DhceMinimap minimap)
    {
        _tool = tool;
        _minimap = minimap;
        MouseFilter = MouseFilterEnum.Ignore; // empty panel area never blocks viewport navigation
        CustomMinimumSize = new Vector2(232, 0);

        var col = new VBoxContainer { SizeFlagsHorizontal = SizeFlags.ExpandFill };
        AddChild(col);

        _biome = Dim("Biome: —"); col.AddChild(_biome);
        _region = Dim("Region: —"); col.AddChild(_region);
        _trait = Dim("Temp/Moist: —"); col.AddChild(_trait);

        col.AddChild(new HSeparator());
        _view = new OptionButton();
        for (int i = 0; i < ViewNames.Length; i++) _view.AddItem(ViewNames[i], i);
        _view.Selected = 0;
        _view.ItemSelected += idx => SelectView((int)idx);
        col.AddChild(_view);
        _viewHint = Dim(""); col.AddChild(_viewHint);
        _viewBrushLabel = Dim("Paint value: 0.5"); _viewBrushLabel.Visible = false; col.AddChild(_viewBrushLabel);
        _viewBrushSlider = new HSlider { MinValue = 0, MaxValue = 1, Step = 0.01, Value = 0.5, Visible = false };
        _viewBrushSlider.ValueChanged += v =>
        {
            _viewBrushLabel.Text = $"Paint value: {v:0.##}";
            _tool.TraitValue = (float)v;
            _tool.Active = ToolKind.Trait;
        };
        col.AddChild(_viewBrushSlider);
    }

    /// Track the scene's current DhceWorld so the VIEW switch can recolour it. Cheap; called per frame.
    public void SetWorld(DhceWorld world) => _world = world;

    /// Live emergent-biome descriptor under the cursor (fed by the plugin's hover raycast).
    public void SetBiomeReadout(string label)
    {
        if (_biome != null) _biome.Text = string.IsNullOrEmpty(label) ? "Biome: —" : $"Biome: {label}";
    }

    /// Live named-Region under the cursor (`-1` off-map, `0` unassigned, else a place id 1..14).
    public void SetRegionReadout(long id)
    {
        if (_region == null) return;
        _region.Text = id < 0 ? "Region: —"
            : id == 0 ? "Region: unassigned"
            : id <= BiomeNames.Length ? $"Region: {BiomeNames[(int)id - 1]}"
            : "Region: ?";
    }

    /// Live numeric trait inspector (temperature / moisture) under the cursor; NaN ⇒ off-map.
    public void SetTraitReadout(double temperature, double moisture)
    {
        if (_trait == null) return;
        _trait.Text = double.IsNaN(temperature) ? "Temp/Moist: —"
            : $"Temp/Moist: {temperature:0.00} / {moisture:0.00}";
    }

    private void SelectView(int mode)
    {
        _world?.SetViewMode(mode);
        _minimap?.Refresh(); // the core recolours its cache to the view; the overview tracks it
        bool paintable = mode == 1 || mode == 2; // Temperature / Moisture
        _viewBrushSlider.Visible = paintable;
        _viewBrushLabel.Visible = paintable;
        if (paintable)
        {
            _tool.TraitId = mode == 1 ? 4 : 5;
            _tool.TraitValue = (float)_viewBrushSlider.Value;
            _tool.Active = ToolKind.Trait;
            _viewHint.Text = $"Left-drag the terrain to paint {ViewNames[mode]}.";
        }
        else
        {
            _viewHint.Text = mode switch
            {
                3 => "Use the sculpt tools to edit elevation.",
                4 => "Use the Region swatches to stamp.",
                5 => "Paint with the Region tool to assign named places.",
                _ => "",
            };
        }
    }

    private static Label Dim(string text)
    {
        var l = new Label { Text = text, AutowrapMode = TextServer.AutowrapMode.WordSmart };
        l.Modulate = new Color(1, 1, 1, 0.65f);
        return l;
    }
}
#endif
