using Godot;

namespace DesolateHaven.Cartography;

/// The authoring tools, in toolbar order. Shortcuts 1–7 map to these (see ToolUi).
/// (Named ToolKind so the `Tool` name is free for CartographerSpike's ToolState property.)
/// `Biome` stamps a Region preset's whole trait bundle; `Trait` paints a single trait.
public enum ToolKind { Raise, Carve, Level, Crest, River, Flood, Biome, Trait }

/// What a stroke changed, so the caller knows which render surface(s) to refresh.
[System.Flags]
public enum EditResult { None = 0, Terrain = 1, Liquid = 2 }

/// Active tool + brush parameters, and the single place that maps a stroke to a DhceEngine
/// call. Kept apart from rendering (CartographerSpike) and UI (ToolUi): the UI mutates these
/// fields; CartographerSpike calls Apply() on left-drag. Radius/strength are metres / normalized
/// elevation (1 Godot unit = 1 m); the core's brush ops take metres.
public sealed class ToolState
{
    public ToolKind Active = ToolKind.Raise;
    public float RadiusFraction = 0.12f; // brush radius as a fraction of the camera→cursor distance
    public float RadiusM = 350f;         // effective radius (m); recomputed each dab from the fraction
    public float StrengthM = 50f;        // sculpt step in METRES (→ normalized via exaggeration)
    public int BiomeId = 1;            // 1..=14 Region preset for the Biome (stamp) tool
    public int TraitId = 6;            // engine trait id for the Trait tool (6 = vegetation)
    public float TraitValue = 1f;      // target value the Trait tool paints (scalar 0..1, or enum idx)
    public int LiquidKind = 0;         // 0 water, 1 lava (River + Flood)
    public float CourseIntensity = 0.05f; // small: course water/carve gains are large in the core
    public float FloodAmount = 0.04f;     // small per dab; the stroke settles on release

    /// Apply the active tool at world-ground point `hit` (Godot XZ plane → core x,y). Returns
    /// which surfaces changed. `exaggeration` converts the metre sculpt step to the core's
    /// normalized-elevation step (on-screen height = normalized × exaggeration). `engine` is the
    /// DhceEngine; all calls go through Variant marshalling.
    public EditResult Apply(GodotObject engine, Vector3 hit, float exaggeration)
    {
        double x = hit.X, z = hit.Z, r = RadiusM;
        double s = StrengthM / Mathf.Max(exaggeration, 1f); // metres → normalized elevation
        // 3D-sphere brush (directional fix): bite a sphere centred on the hit, not a vertical column.
        // hit.Y is the surface height under the cursor (= normalized elev × exaggeration).
        engine.Call("set_brush_sphere", (double)hit.Y, (double)exaggeration);
        switch (Active)
        {
            case ToolKind.Raise: engine.Call("paint_terrain", x, z, r, s, 0); return EditResult.Terrain;
            case ToolKind.Carve: engine.Call("paint_terrain", x, z, r, s, 1); return EditResult.Terrain;
            case ToolKind.Level: engine.Call("paint_terrain", x, z, r, s, 2); return EditResult.Terrain;
            case ToolKind.Crest: engine.Call("paint_terrain", x, z, r, s, 3); return EditResult.Terrain;
            case ToolKind.River: engine.Call("paint_course", x, z, r, (double)CourseIntensity, LiquidKind);
                             return EditResult.Terrain | EditResult.Liquid;
            case ToolKind.Flood: engine.Call("paint_liquid", x, z, r, (double)FloodAmount, LiquidKind);
                             return EditResult.Liquid;
            case ToolKind.Biome: engine.Call("paint_region_traits", x, z, r, BiomeId); return EditResult.Terrain;
            case ToolKind.Trait: engine.Call("paint_trait", x, z, r, TraitId, (double)TraitValue); return EditResult.Terrain;
            default: return EditResult.None;
        }
    }
}
