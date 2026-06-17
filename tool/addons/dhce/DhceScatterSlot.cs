using Godot;

namespace DesolateHaven.Cartography;

/// One authored scatter "model slot" (N3d): a mesh + its placement rule. The tool bundles no art —
/// it records a path to the owner's `.glb`/mesh. `MeshPath` is baked at export; `ProxyMeshPath` (if
/// set) is the cheap low-poly shown while authoring (else a primitive proxy). Placement is gated by
/// elevation band + vegetation + biome + region (see the core `ScatterRule`); the slot's index in
/// the library is its id. Masks: 0 = "any"; vegetation bits over the 7 cover types, biome bits over
/// the painted biome ids 1..14, region bits over the named-Region ids 1..14.
[Tool]
[GlobalClass]
public partial class DhceScatterSlot : Resource
{
    [Export] public string Name = "slot";
    [Export(PropertyHint.File, "*.glb,*.gltf,*.res,*.tres,*.obj")] public string MeshPath = "";
    [Export(PropertyHint.File, "*.glb,*.gltf,*.res,*.tres,*.obj")] public string ProxyMeshPath = "";

    [Export(PropertyHint.Range, "0,1,0.01")] public float Density = 0.3f;
    [Export] public float ScaleMin = 3f;
    [Export] public float ScaleMax = 8f;
    [Export] public float ElevMin = 0.02f; // normalized height; default ~land-only
    [Export] public float ElevMax = 1.5f;

    /// Bitmask over the 7 vegetation cover types (`1 << veg`); 0 = any. Edited via the dock checkboxes.
    [Export(PropertyHint.Flags, "Barren,Grass,Scrub,Forest,Evergreen,Marsh,Thorn")] public int VegetationMask = 0;
    /// Bitmask over the painted biome ids 1..14 (`1 << (id-1)`); 0 = any. Edited via the dock checkboxes.
    [Export(PropertyHint.Flags, "Jagged Mountains,Sacred Woods Plateau,Great Lake,Temperate Forest,Open Plains,Underdeep,Deep Wood,Frozen Reaches,Lost Isles,Blisterwood,Volcanic Scape,Blight Ruins,Scattered Isles,Marsh & Bog")] public int BiomeMask = 0;
    /// Bitmask over named-Region ids 1..14 (`1 << (id-1)`); 0 = any.
    [Export] public int RegionMask = 0;

    /// Bake as individual selectable `MeshInstance3D` nodes (hand-editable in the game) instead of one
    /// instanced `MultiMesh`. Heavy for dense cover — capped at export; leave off for ground cover.
    [Export] public bool BakeAsInstances = false;

    /// Distance (m) past which this slot's instances are culled (LOD); 0 = never. Keeps dense cover
    /// cheap far away. Applied to the preview + the baked nodes as a `VisibilityRangeEnd`.
    [Export] public float VisibilityEndM = 0f;
}
