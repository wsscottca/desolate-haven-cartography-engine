using Godot;

namespace DesolateHaven.Cartography;

/// One authored scatter "model slot" (N3d): a mesh + its placement rule. The tool bundles no art —
/// it records a path to the owner's `.glb`/mesh. `MeshPath` is baked at export; `ProxyMeshPath` (if
/// set) is the cheap low-poly shown while authoring (else a primitive proxy). Placement is gated by
/// elevation band + vegetation + region (see the core `ScatterRule`); the slot's index in the
/// library is its id. Masks: 0 = "any"; vegetation bits over the 7 cover types, region bits over 1..14.
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
    /// Bitmask over Region ids 1..14 (`1 << (id-1)`); 0 = any.
    [Export] public int RegionMask = 0;
}
