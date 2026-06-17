using Godot;

namespace DesolateHaven.Cartography;

/// Compact, regenerate-from-this snapshot of an authored world (reshell R5). Holds the generation
/// params plus every per-cell authored field; the tool (and the game) rebuild the *identical* world
/// by running the shared core's `build(params)` and then overwriting these fields — no mesh bake.
/// Saved as a binary `.res` (external to the `.tscn`, which only stores the reference). At full
/// density the field arrays are tens of MB; the binary saver compresses them.
///
/// Known v1 limitation: `shape_delta` and the per-trait painted *base* are not stored — re-running
/// Apply shaping / Blend borders after a load re-bases once from the loaded (already-shaped/blended)
/// state. One-time and non-destructive.
[Tool]
[GlobalClass]
public partial class DhceWorldState : Resource
{
    // Generation params (the deterministic seed of the whole world).
    [Export] public int Seed = 12345;
    [Export] public float WorldSizeKm = 20f;
    [Export] public float SpacingM = 12f;
    [Export] public int Octaves = 6;
    [Export] public float TerrainHeightKm = 2.4f;
    [Export] public float ChunkSizeM = 256f;
    [Export] public float BaseBlendM = 1800f;         // width regions' base-elevation trunk blends (softer steps)
    [Export] public float SeaLevel = 0f; // water level (normalized elevation); default set at generate
    [Export] public float LapseRate = 0.6f;           // climate: temperature drop per unit elevation
    [Export] public float OrographicStrength = 0.45f; // climate: windward-wet / lee-dry moisture pull
    [Export] public float WindDeg = 0f;               // prevailing wind direction (degrees)

    // Authored per-cell fields (restored after a deterministic build).
    [Export] public float[] Elevation = System.Array.Empty<float>();
    [Export] public byte[] Biome = System.Array.Empty<byte>();
    [Export] public byte[] BiomeLocked = System.Array.Empty<byte>();
    [Export] public byte[] Region = System.Array.Empty<byte>(); // named-Region (place) membership
    [Export] public float[] LiquidDepth = System.Array.Empty<float>();
    [Export] public byte[] LiquidKind = System.Array.Empty<byte>();
    [Export] public byte[] CourseMask = System.Array.Empty<byte>();

    // Trait fields (trait ids 0..7; enums 6/7 stored as whole-number floats).
    [Export] public float[] Jaggedness = System.Array.Empty<float>();
    [Export] public float[] Relief = System.Array.Empty<float>();
    [Export] public float[] FoothillFalloff = System.Array.Empty<float>();
    [Export] public float[] Erosion = System.Array.Empty<float>();
    [Export] public float[] Temperature = System.Array.Empty<float>();
    [Export] public float[] Moisture = System.Array.Empty<float>();
    [Export] public float[] Vegetation = System.Array.Empty<float>();
    [Export] public float[] PaletteFamily = System.Array.Empty<float>();

    // Shared tables: 7 base palettes (7×6×3) + per-Region landform dials (15×4) + per-Region lake
    // thresholds (15) + per-Region river thresholds (15).
    [Export] public float[] BasePalettes = System.Array.Empty<float>();
    [Export] public float[] RegionLandform = System.Array.Empty<float>();
    [Export] public float[] RegionLakeDepth = System.Array.Empty<float>();
    [Export] public float[] RegionRiverThreshold = System.Array.Empty<float>();
}
