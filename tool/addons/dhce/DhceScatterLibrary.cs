using Godot;
using System.Collections.Generic;

namespace DesolateHaven.Cartography;

/// The authored scatter library (N3d): the ordered set of model slots, saved with the world (a
/// `DhceWorld` references one). A slot's index is its core rule `slot` id. `ToRulesFlat` packs the
/// rules for the deterministic core engine (`tessellate_scatter_rules` — 9 floats per slot).
[Tool]
[GlobalClass]
public partial class DhceScatterLibrary : Resource
{
    [Export] public Godot.Collections.Array<DhceScatterSlot> Slots = new();

    /// Flatten to the engine rule array: `[slot, density, scaleMin, scaleMax, elevMin, elevMax,
    /// vegMask, regionMask, biomeMask]` per slot, in slot order.
    public float[] ToRulesFlat()
    {
        var f = new List<float>(Slots.Count * 9);
        for (int i = 0; i < Slots.Count; i++)
        {
            var s = Slots[i];
            if (s == null) continue;
            f.Add(i);
            f.Add(s.Density);
            f.Add(s.ScaleMin);
            f.Add(s.ScaleMax);
            f.Add(s.ElevMin);
            f.Add(s.ElevMax);
            f.Add(s.VegetationMask);
            f.Add(s.RegionMask);
            f.Add(s.BiomeMask);
        }
        return f.ToArray();
    }
}
