using Godot;

namespace DesolateHaven.Cartography;

/// The authoring tools, in toolbar order. Shortcuts 1–7 map to these (see ToolUi).
public enum Tool { Raise, Carve, Level, Crest, River, Flood, Biome }

/// What a stroke changed, so the caller knows which render surface(s) to refresh.
[System.Flags]
public enum EditResult { None = 0, Terrain = 1, Liquid = 2 }

/// Active tool + brush parameters, and the single place that maps a stroke to a DhceEngine
/// call. Kept apart from rendering (CartographerSpike) and UI (ToolUi): the UI mutates these
/// fields; CartographerSpike calls Apply() on left-drag. Radius/strength are metres / normalized
/// elevation (1 Godot unit = 1 m); the core's brush ops take metres.
public sealed class ToolState
{
    public Tool Active = Tool.Raise;
    public float RadiusM = 350f;       // brush footprint radius, metres
    public float Strength = 0.06f;     // sculpt step (normalized elevation)
    public int BiomeId = 1;            // 1..=14 for the Biome tool
    public int LiquidKind = 0;         // 0 water, 1 lava (River + Flood)
    public float CourseIntensity = 0.5f;
    public float FloodAmount = 0.25f;

    /// Apply the active tool at world-ground point `hit` (Godot XZ plane → core x,y). Returns
    /// which surfaces changed. `engine` is the DhceEngine; all calls go through Variant marshalling.
    public EditResult Apply(GodotObject engine, Vector3 hit)
    {
        double x = hit.X, z = hit.Z, r = RadiusM, s = Strength;
        switch (Active)
        {
            case Tool.Raise: engine.Call("paint_terrain", x, z, r, s, 0); return EditResult.Terrain;
            case Tool.Carve: engine.Call("paint_terrain", x, z, r, s, 1); return EditResult.Terrain;
            case Tool.Level: engine.Call("paint_terrain", x, z, r, s, 2); return EditResult.Terrain;
            case Tool.Crest: engine.Call("paint_terrain", x, z, r, s, 3); return EditResult.Terrain;
            case Tool.River: engine.Call("paint_course", x, z, r, (double)CourseIntensity, LiquidKind);
                             return EditResult.Terrain | EditResult.Liquid;
            case Tool.Flood: engine.Call("paint_liquid", x, z, r, (double)FloodAmount, LiquidKind);
                             return EditResult.Liquid;
            case Tool.Biome: engine.Call("paint_biome", x, z, r, BiomeId); return EditResult.Terrain;
            default: return EditResult.None;
        }
    }
}
