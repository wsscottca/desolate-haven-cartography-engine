#if TOOLS
using Godot;

namespace DesolateHaven.Cartography;

/// Viewport overlay docked under the minimap, styled with the game theme: the surface readouts
/// (emergent biome / named region / temp-moist) for the point under the cursor, fed by the plugin
/// each frame. The VIEW (map-layer) selector now lives in the sidebar dock (DhceDock).
public partial class DhceMapPanel : PanelContainer
{
    private static readonly string[] BiomeNames =
    {
        "Jagged Mountains", "Sacred Forest", "Great Lake", "Temperate Forest",
        "Rolling Plains", "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles",
        "Blisterwood", "Volcanic Scape", "Blight Ruins", "Scattered Isles", "Marsh & Bog",
    };

    private Label _biome, _region, _trait;

    public void Init()
    {
        Theme = DhceUi.Theme;
        CustomMinimumSize = new Vector2(232, 0);

        var col = new VBoxContainer { SizeFlagsHorizontal = SizeFlags.ExpandFill };
        AddChild(col);

        _biome = DhceUi.Dim("Biome: —"); col.AddChild(_biome);
        _region = DhceUi.Dim("Region: —"); col.AddChild(_region);
        _trait = DhceUi.Dim("Temp/Moist: —"); col.AddChild(_trait);
    }

    // Hug content so the panel sizes to the readouts.
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
}
#endif
