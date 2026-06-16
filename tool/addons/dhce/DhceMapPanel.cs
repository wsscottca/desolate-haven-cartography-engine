#if TOOLS
using Godot;

namespace DesolateHaven.Cartography;

/// Viewport overlay docked under the minimap, styled with the game theme: the surface readouts
/// (emergent biome / named region / temp-moist) and the VIEW (map-layer) selector with its
/// contextual temperature/moisture paint slider. Fed by the plugin each frame.
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

    public void Init(ToolState tool, DhceMinimap minimap)
    {
        _tool = tool;
        _minimap = minimap;
        Theme = DhceUi.Theme;
        CustomMinimumSize = new Vector2(232, 0);

        var col = new VBoxContainer { SizeFlagsHorizontal = SizeFlags.ExpandFill };
        AddChild(col);

        _biome = DhceUi.Dim("Biome: —"); col.AddChild(_biome);
        _region = DhceUi.Dim("Region: —"); col.AddChild(_region);
        _trait = DhceUi.Dim("Temp/Moist: —"); col.AddChild(_trait);

        _view = new OptionButton();
        for (int i = 0; i < ViewNames.Length; i++) _view.AddItem(ViewNames[i], i);
        _view.Selected = 0;
        _view.ItemSelected += idx => SelectView((int)idx);
        col.AddChild(_view);
        _viewHint = DhceUi.Dim(""); col.AddChild(_viewHint);
        _viewBrushLabel = DhceUi.Dim("Paint value: 0.5"); _viewBrushLabel.Visible = false; col.AddChild(_viewBrushLabel);
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

    // Hug content so the panel sizes to the readouts + View.
    public void HugContent()
    {
        var m = GetCombinedMinimumSize();
        if (!Size.IsEqualApprox(m)) Size = m;
    }

    public void SetBiomeReadout(string label)
    {
        if (_biome != null) _biome.Text = string.IsNullOrEmpty(label) ? "Biome: —" : $"Biome: {label}";
    }

    public void SetRegionReadout(long id)
    {
        if (_region == null) return;
        _region.Text = id < 0 ? "Region: —"
            : id == 0 ? "Region: unassigned"
            : id <= BiomeNames.Length ? $"Region: {BiomeNames[(int)id - 1]}"
            : "Region: ?";
    }

    public void SetTraitReadout(double temperature, double moisture)
    {
        if (_trait == null) return;
        _trait.Text = double.IsNaN(temperature) ? "Temp/Moist: —"
            : $"Temp/Moist: {temperature:0.00} / {moisture:0.00}";
    }

    private void SelectView(int mode)
    {
        _world?.SetViewMode(mode);
        _minimap?.Refresh();
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
}
#endif
