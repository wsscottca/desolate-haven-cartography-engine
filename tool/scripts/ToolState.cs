using Godot;

namespace DesolateHaven.Cartography;

/// The authoring tools, in toolbar order. Shortcuts 1–7 map to these (see ToolUi).
/// (Named ToolKind so the `Tool` name is free for CartographerSpike's ToolState property.)
/// `Biome` stamps a Region preset's whole trait bundle; `Trait` paints a single trait.
public enum ToolKind { Raise, Carve, Level, Crest, River, Flood, Biome, Trait, Region, Territory, RegionSelect, Cave, Transition, Smooth, Flatten, Grab, Erode, Zone }

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
    /// Brush size as a fraction of the camera→cursor distance, so the brush keeps a constant apparent
    /// size as you zoom (DhcePlugin recomputes `RadiusM` each dab from this × distance). The editor
    /// dock exposes 6 discrete levels (`SizeFractions`, via `SetSizeLevel`) as progressively larger
    /// icons; the runtime ToolUi sets `RadiusFraction` directly from its own slider.
    public static readonly float[] SizeFractions = { 0.02f, 0.05f, 0.10f, 0.18f, 0.30f, 0.45f };
    public int SizeLevel = 2;            // index of the active size icon (mirrors RadiusFraction)
    public float RadiusFraction = 0.10f; // = SizeFractions[SizeLevel]; settable for the runtime ToolUi
    /// Absolute brush radius in METRES; when > 0 it overrides the zoom-coupled `RadiusFraction` so the
    /// author can shape very large or very small areas at an exact size (dock number picker, 0.2 m–2 km).
    /// 0 = auto (use `RadiusFraction` × camera distance, the size icons).
    public float RadiusOverrideM = 0f;
    public float RadiusM = 350f;         // effective radius (m); recomputed each dab (override, or fraction × distance)
    public float StrengthM = 50f;        // sculpt step in METRES (→ normalized via exaggeration)

    /// Pick one of the 6 size levels (used by the dock's brush-size icons).
    public void SetSizeLevel(int level)
    {
        SizeLevel = Mathf.Clamp(level, 0, SizeFractions.Length - 1);
        RadiusFraction = SizeFractions[SizeLevel];
    }
    public int BiomeId = 1;            // 1..=14 Region preset for the Biome (stamp) tool
    public int RegionId = 1;           // 1..=14 named place for the Region (assign) tool
    public int TraitId = 6;            // engine trait id for the Trait tool (6 = vegetation)
    public float TraitValue = 1f;      // target value the Trait tool paints (scalar 0..1, or enum idx)
    public int LiquidKind = 0;         // 0 water, 1 lava (River + Flood)
    public float CourseIntensity = 0.05f; // small: course water/carve gains are large in the core
    public float FloodAmount = 0.04f;     // small per dab; the stroke settles on release
    public float BlendWidthM = 200f;      // transition-brush band width (m) → blend_brush
    public float GrabDeltaNorm = 0f;      // Grab: vertical drag delta this dab (normalized elev; plugin-set)
    public float FlattenTargetNorm = 0f;  // Flatten: target plane height (normalized elev; plugin-set at stroke start)

    // --- Zone Edit tool: pick a whole zone, then edit it as one. The selection (a cell-id list) lives
    // here so the viewport (which sets it) and the dock (whose op buttons read it) share one source. ---
    /// How the Zone tool turns a click/drag into a selection. Smart traces by elevation contrast;
    /// Water floods a lake; Contiguous takes the same-region area; Polygon draws an outline.
    public enum ZoneSelectMode { Smart, Water, Contiguous, Polygon }
    public ZoneSelectMode ZoneSelect = ZoneSelectMode.Smart;
    public float ZoneTolerance = 0.04f;    // Smart-select edge tolerance (normalized elevation step)
    public float ZoneOffsetM = 60f;        // Drop/Raise step (m), converted to normalized via exaggeration
    public float ZoneLevelWeight = 1.0f;   // Level blend (0..1; 1 = flat to mean)
    public int ZoneSmoothIters = 4;        // Smooth / feather Jacobi passes
    public float ZoneSmoothWeight = 0.5f;  // Smooth / feather relax weight (0..1)
    public float ZoneFeatherWidthM = 400f; // Feather band width (m) inward from the zone border
    public float ZoneFillOffsetM = 0f;     // Fill height above the rim (m)
    public int ZoneFillKind = 0;           // 0 water, 1 lava
    /// The current zone selection (core cell ids); empty when nothing is selected.
    public int[] ZoneSelection = System.Array.Empty<int>();
    /// Grade is a two-click gesture: armed by the dock, the plugin captures the low point then the high.
    public bool ZoneGradeArmed = false;
    public bool ZoneGradeHasLow = false;
    public Vector3 ZoneGradeLow;
    public bool HasZoneSelection => ZoneSelection != null && ZoneSelection.Length > 0;

    /// Apply the active tool at world-ground point `hit` (Godot XZ plane → core x,y). Returns
    /// which surfaces changed. `exaggeration` converts the metre sculpt step to the core's
    /// normalized-elevation step (on-screen height = normalized × exaggeration). `engine` is the
    /// DhceEngine; all calls go through Variant marshalling.
    public EditResult Apply(GodotObject engine, Vector3 hit, float exaggeration, bool invert = false)
    {
        double x = hit.X, z = hit.Z, r = RadiusM;
        double s = StrengthM / Mathf.Max(exaggeration, 1f); // metres → normalized elevation
        double sw = Mathf.Clamp(StrengthM / 100f, 0.02f, 1f); // smooth/flatten blend weight (0..1) per dab
        // 3D-sphere brush (directional fix): bite a sphere centred on the hit, not a vertical column.
        // hit.Y is the surface height under the cursor (= normalized elev × exaggeration).
        engine.Call("set_brush_sphere", (double)hit.Y, (double)exaggeration);
        switch (Active)
        {
            // Ctrl-invert swaps Raise↔Carve and Smooth↔Roughen.
            case ToolKind.Raise: engine.Call("paint_terrain", x, z, r, s, invert ? 1 : 0); return EditResult.Terrain;
            case ToolKind.Carve: engine.Call("paint_terrain", x, z, r, s, invert ? 0 : 1); return EditResult.Terrain;
            case ToolKind.Level: engine.Call("paint_terrain", x, z, r, s, 2); return EditResult.Terrain;
            case ToolKind.Crest: engine.Call("paint_terrain", x, z, r, s, 3); return EditResult.Terrain;
            case ToolKind.Smooth: engine.Call(invert ? "roughen_terrain" : "smooth_terrain", x, z, r, invert ? s : sw); return EditResult.Terrain;
            case ToolKind.Flatten: engine.Call("flatten_terrain", x, z, r, sw, (double)FlattenTargetNorm); return EditResult.Terrain;
            case ToolKind.Grab: engine.Call("grab_terrain", x, z, r, (double)GrabDeltaNorm); return EditResult.Terrain;
            case ToolKind.Erode: engine.Call("erode_brush", x, z, r); return EditResult.Terrain;
            case ToolKind.River: engine.Call("paint_course", x, z, r, (double)CourseIntensity, LiquidKind);
                             return EditResult.Terrain | EditResult.Liquid;
            case ToolKind.Flood: engine.Call("paint_liquid", x, z, r, (double)FloodAmount, LiquidKind);
                             return EditResult.Liquid;
            case ToolKind.Biome: engine.Call("paint_region_traits", x, z, r, BiomeId); return EditResult.Terrain;
            case ToolKind.Trait: engine.Call("paint_trait", x, z, r, TraitId, (double)TraitValue); return EditResult.Terrain;
            case ToolKind.Region: engine.Call("paint_region", x, z, r, RegionId); return EditResult.Terrain;
            case ToolKind.Transition: engine.Call("blend_brush", x, z, r, (double)BlendWidthM); return EditResult.Terrain;
            // Zone Edit is click-to-select + dock-button ops (not a left-drag stroke), so the brush path is a no-op.
            case ToolKind.Zone: return EditResult.None;
            default: return EditResult.None;
        }
    }
}
