#if TOOLS
using Godot;
using System.Collections.Generic;

namespace DesolateHaven.Cartography;

/// Slices an authored `DhceWorld` into per-Region **level scenes** (reshell #2; see
/// docs/specs/dhce-region-slicing.md). Each assigned Region → a self-contained `.tscn`
/// (terrain mesh + trimesh collider + water + scatter proxies) the game loads directly. Also writes
/// the single editable master `DhceWorldState` and a `regions.json` adjacency manifest for later
/// gates. Baking is required because the core regenerates the *whole* world from params, not a slice.
public static class DhceLevelSlicer
{
    // The 14 canon places (Region ids 1..14), matching the core roster + the dock picker.
    private static readonly string[] RegionNames =
    {
        "Jagged Mountains", "Sacred Forest", "Great Lake", "Temperate Forest",
        "Rolling Plains", "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles",
        "Blisterwood", "Volcanic Scape", "Blight Ruins", "Scattered Isles", "Marsh & Bog",
    };

    private static readonly Color WaterColor = new(0.22f, 0.52f, 0.55f, 0.6f); // teal, matches canon ocean
    private static readonly Color LavaColor = new(0.95f, 0.35f, 0.10f, 0.9f);
    private static readonly Color MarshColor = new(0.239f, 0.302f, 0.180f, 0.8f); // murky bog water (#3D4D2E)
    private static readonly Color IceColor = new(0.776f, 0.886f, 0.933f, 0.9f);   // frozen water / ice (#C6E2EE)

    /// Liquid-kind id (0 water, 1 lava, 2 marsh, 3 ice) → baked water-surface colour.
    private static Color LiquidColor(float kind) => Mathf.RoundToInt(kind) switch
    {
        1 => LavaColor,
        2 => MarshColor,
        3 => IceColor,
        _ => WaterColor,
    };
    private static readonly Color TreeColor = new(0.25f, 0.45f, 0.18f);
    private static readonly Color RockColor = new(0.55f, 0.52f, 0.48f);

    /// Slice into `<exportDir>/levels/`. Returns a status line.
    public static string Slice(DhceWorld world, string exportDir)
    {
        var engine = world?.Engine;
        if (engine == null || !world.GenDone) return "Generate a world first.";

        string levelsDir = NormalizeDir(exportDir) + "/levels";
        var mk = DirAccess.MakeDirRecursiveAbsolute(levelsDir);
        if (mk != Error.Ok) return $"Can't create {levelsDir}: {mk}";

        float exag = world.Exaggeration;
        byte[] region = engine.Call("region_export").As<byte[]>();
        var present = new SortedSet<int>();
        int unassigned = 0;
        foreach (var b in region) { if (b == 0) unassigned++; else present.Add(b); }
        if (present.Count == 0) return "No Regions assigned yet — paint Regions, then slice.";

        int built = 0;
        foreach (int id in present)
        {
            var root = BuildRegionScene(world, engine, id, exag);
            var packed = new PackedScene();
            if (packed.Pack(root) == Error.Ok)
            {
                string path = $"{levelsDir}/{SafeName(id)}.tscn";
                if (ResourceSaver.Save(packed, path) == Error.Ok) built++;
            }
            root.QueueFree();
        }

        // Editable master state (regenerate-from-this; the one source for re-editing + re-slicing).
        var master = world.Save();
        if (master != null) ResourceSaver.Save(master, $"{levelsDir}/world_master.res", ResourceSaver.SaverFlags.Compress);

        WriteManifest(engine, levelsDir, present);
        return $"Sliced {built}/{present.Count} regions → {levelsDir}  ({unassigned} cells unassigned)";
    }

    private static Node3D BuildRegionScene(DhceWorld world, GodotObject engine, int id, float exag)
    {
        var root = new Node3D { Name = SafeName(id) };
        root.SetMeta("region_id", id);
        root.SetMeta("region_name", RegionName(id));

        // Terrain mesh + trimesh static collider.
        engine.Call("slice_region_terrain", id, (double)exag);
        var pos = engine.Call("region_positions").As<Vector3[]>();
        if (pos.Length > 0)
        {
            var mesh = BuildMesh(pos, engine.Call("region_normals").As<Vector3[]>(),
                                 engine.Call("region_colors").As<Color[]>(), engine.Call("region_indices").As<int[]>());
            var mi = new MeshInstance3D { Name = "Terrain", Mesh = mesh, MaterialOverride = TerrainMat() };
            Adopt(root, mi);

            var body = new StaticBody3D { Name = "Collider" };
            var shape = new CollisionShape3D { Shape = mesh.CreateTrimeshShape() };
            Adopt(root, body);
            body.AddChild(shape);
            shape.Owner = root;
        }

        // Water surface (if any wet cells in the Region).
        engine.Call("slice_region_liquid", id, (double)exag);
        var wpos = engine.Call("region_liquid_positions").As<Vector3[]>();
        if (wpos.Length > 0)
        {
            var types = engine.Call("region_liquid_types").As<float[]>();
            var wcol = new Color[wpos.Length];
            for (int k = 0; k < wpos.Length; k++) wcol[k] = k < types.Length ? LiquidColor(types[k]) : WaterColor;
            var wmesh = BuildMesh(wpos, engine.Call("region_liquid_normals").As<Vector3[]>(), wcol, engine.Call("region_liquid_indices").As<int[]>());
            Adopt(root, new MeshInstance3D { Name = "Water", Mesh = wmesh, MaterialOverride = WaterMat() });
        }

        // Scatter: bake the authored library (per-slot MultiMesh with the real mesh) if present,
        // else the legacy biome proxy markers.
        var lib = world.Scatter;
        if (lib != null && lib.Slots.Count > 0)
        {
            engine.Call("tessellate_region_scatter_rules", id, lib.ToRulesFlat(), (double)exag, (double)world.Seed);
            foreach (var node in BuildScatterMeshes(lib, engine.Call("scatter_data").As<float[]>(), engine.Call("scatter_count").As<int>(), int.MaxValue, preferProxy: false, embed: true))
                Adopt(root, node);
        }
        else
        {
            engine.Call("tessellate_region_scatter", id, (double)exag, 0.5, (double)world.Seed);
            int scount = engine.Call("scatter_count").As<int>();
            if (scount > 0) Adopt(root, new MultiMeshInstance3D { Name = "ScatterProxy", Multimesh = BuildScatter(engine.Call("scatter_data").As<float[]>(), scount) });
        }

        BakeCaves(world, engine, root, id, exag);
        return root;
    }

    /// Carve the authored volumetric features whose centre lands in this Region into a Surface-Nets
    /// mesh (+ trimesh collider), embedded in the level scene. v1 note: the carved patch overlays the
    /// flat region terrain at the opening (no hole cut yet) — a later refinement.
    private static void BakeCaves(DhceWorld world, GodotObject engine, Node3D root, int id, float exag)
    {
        if (world.Caves == null || world.Caves.Count == 0) return;
        var mine = new List<Vector4>();
        foreach (Vector4 c in world.Caves)
            if ((int)engine.Call("region_id_at", c.X, c.Z).AsInt64() == id) mine.Add(c);
        if (mine.Count == 0) return;

        var flat = new float[mine.Count * 4];
        for (int i = 0; i < mine.Count; i++) { flat[i * 4] = mine[i].X; flat[i * 4 + 1] = mine[i].Y; flat[i * 4 + 2] = mine[i].Z; flat[i * 4 + 3] = mine[i].W; }
        engine.Call("tessellate_caves", flat, (double)exag, 3.0);
        var pos = engine.Call("cave_positions").As<Vector3[]>();
        if (pos.Length == 0) return;
        var norm = engine.Call("cave_normals").As<Vector3[]>();
        var idx = engine.Call("cave_indices").As<int[]>();
        var col = new Color[pos.Length];
        for (int k = 0; k < pos.Length; k++) col[k] = new Color(0.40f, 0.38f, 0.36f); // rock
        var mesh = BuildMesh(pos, norm, col, idx);

        Adopt(root, new MeshInstance3D { Name = "Caves", Mesh = mesh, MaterialOverride = CaveMat() });
        var body = new StaticBody3D { Name = "CaveCollider" };
        Adopt(root, body);
        var shape = new CollisionShape3D { Shape = mesh.CreateTrimeshShape() };
        body.AddChild(shape);
        shape.Owner = root;
    }

    private static StandardMaterial3D CaveMat()
    {
        var m = new StandardMaterial3D { VertexColorUseAsAlbedo = true, Roughness = 0.95f };
        m.Set("cull_mode", 2); // CULL_DISABLED — Surface-Nets winding isn't guaranteed outward
        return m;
    }

    // --- mesh/material/multimesh helpers ---

    private static ArrayMesh BuildMesh(Vector3[] positions, Vector3[] normals, Color[] colors, int[] indices)
    {
        var arrays = new Godot.Collections.Array();
        arrays.Resize((int)Mesh.ArrayType.Max);
        arrays[(int)Mesh.ArrayType.Vertex] = positions;
        arrays[(int)Mesh.ArrayType.Normal] = normals;
        arrays[(int)Mesh.ArrayType.Color] = colors;
        arrays[(int)Mesh.ArrayType.Index] = indices;
        var am = new ArrayMesh();
        am.AddSurfaceFromArrays(Mesh.PrimitiveType.Triangles, arrays);
        return am;
    }

    private static MultiMesh BuildScatter(float[] data, int count)
    {
        var mm = new MultiMesh
        {
            TransformFormat = MultiMesh.TransformFormatEnum.Transform3D,
            UseColors = true,
            Mesh = new BoxMesh { Size = Vector3.One },
            InstanceCount = count,
        };
        for (int k = 0; k < count; k++)
        {
            // Flat [x, y(core-ground), z(height), scale, species]; Godot is Y-up: (x, height, y).
            float x = data[k * 5], gy = data[k * 5 + 1], h = data[k * 5 + 2], scale = data[k * 5 + 3], species = data[k * 5 + 4];
            var basis = new Basis(Quaternion.Identity).Scaled(Vector3.One * Mathf.Max(scale, 0.5f));
            mm.SetInstanceTransform(k, new Transform3D(basis, new Vector3(x, h, gy)));
            mm.SetInstanceColor(k, species > 0.5f ? RockColor : TreeColor);
        }
        return mm;
    }

    private const int InstanceBakeCap = 6000; // per-slot safety cap when baking individual instances

    /// Build scatter nodes per slot from packed `[x, y(core), z(height), scale, slot]` data.
    /// `preferProxy` picks the slot's low-poly proxy (editor preview) over its real mesh (bake).
    /// `embed` (bake) duplicates the mesh so it loses its `res://` path and **saves inline** in the
    /// level `.tscn` — self-contained for the game, no tool asset needed — and honours a slot's
    /// `BakeAsInstances` (individual `MeshInstance3D`s, capped) vs one instanced `MultiMesh`. The
    /// source `MeshPath` is recorded as `mesh_path` meta so the game can swap to its own asset/LOD.
    /// `cap` limits total instances (editor preview). Shared by the slicer bake + the editor preview.
    public static List<Node3D> BuildScatterMeshes(DhceScatterLibrary lib, float[] data, int count, int cap, bool preferProxy, bool embed)
    {
        var nodes = new List<Node3D>();
        if (count <= 0 || lib == null) return nodes;
        int use = Mathf.Min(count, cap);
        var bySlot = new Dictionary<int, List<Transform3D>>();
        for (int k = 0; k < use; k++)
        {
            float x = data[k * 5], gy = data[k * 5 + 1], h = data[k * 5 + 2], scale = data[k * 5 + 3];
            int slot = (int)data[k * 5 + 4];
            var basis = new Basis(Quaternion.Identity).Scaled(Vector3.One * Mathf.Max(scale, 0.1f));
            if (!bySlot.TryGetValue(slot, out var list)) { list = new List<Transform3D>(); bySlot[slot] = list; }
            list.Add(new Transform3D(basis, new Vector3(x, h, gy)));
        }
        foreach (var kv in bySlot)
        {
            var slot = kv.Key >= 0 && kv.Key < lib.Slots.Count ? lib.Slots[kv.Key] : null;
            string path = slot == null ? "" : (preferProxy && !string.IsNullOrEmpty(slot.ProxyMeshPath)) ? slot.ProxyMeshPath : slot.MeshPath;
            Mesh mesh = LoadMesh(path) ?? (preferProxy ? null : LoadMesh(slot?.ProxyMeshPath)) ?? FallbackProxyMesh();
            if (embed && mesh != null) mesh = (Mesh)mesh.Duplicate(true); // embed inline in the saved scene
            string name = Sanitize(slot?.Name ?? kv.Key.ToString());

            if (embed && slot != null && slot.BakeAsInstances)
            {
                int n = Mathf.Min(kv.Value.Count, InstanceBakeCap);
                for (int k = 0; k < n; k++)
                {
                    var mi = new MeshInstance3D { Name = $"Scatter_{name}_{k}", Mesh = mesh, Transform = kv.Value[k] };
                    if (!string.IsNullOrEmpty(slot.MeshPath)) mi.SetMeta("mesh_path", slot.MeshPath);
                    ApplyLod(mi, slot);
                    nodes.Add(mi);
                }
            }
            else
            {
                var mm = new MultiMesh { TransformFormat = MultiMesh.TransformFormatEnum.Transform3D, Mesh = mesh, InstanceCount = kv.Value.Count };
                for (int k = 0; k < kv.Value.Count; k++) mm.SetInstanceTransform(k, kv.Value[k]);
                var mmi = new MultiMeshInstance3D { Name = $"Scatter_{name}", Multimesh = mm };
                if (slot != null && !string.IsNullOrEmpty(slot.MeshPath)) mmi.SetMeta("mesh_path", slot.MeshPath);
                ApplyLod(mmi, slot);
                nodes.Add(mmi);
            }
        }
        return nodes;
    }

    /// Load a `Mesh` from a path: a Mesh resource directly, or the first MeshInstance3D's mesh in a
    /// PackedScene (`.glb`/`.gltf`). Null if the path is empty or unresolvable in this project.
    public static Mesh LoadMesh(string path)
    {
        if (string.IsNullOrEmpty(path) || !ResourceLoader.Exists(path)) return null;
        var res = ResourceLoader.Load(path);
        if (res is Mesh m) return m;
        if (res is PackedScene ps)
        {
            var inst = ps.Instantiate();
            var found = FindMesh(inst);
            inst.QueueFree();
            return found;
        }
        return null;
    }

    private static Mesh FindMesh(Node n)
    {
        if (n is MeshInstance3D mi && mi.Mesh != null) return mi.Mesh;
        foreach (var c in n.GetChildren()) { var f = FindMesh(c); if (f != null) return f; }
        return null;
    }

    private static BoxMesh FallbackProxyMesh() => new BoxMesh { Size = Vector3.One };

    /// Distance-cull a scatter node past the slot's `VisibilityEndM` (0 = never) — scatter LOD.
    private static void ApplyLod(GeometryInstance3D node, DhceScatterSlot slot)
    {
        if (slot == null || slot.VisibilityEndM <= 0f) return;
        node.VisibilityRangeEnd = slot.VisibilityEndM;
        node.VisibilityRangeEndMargin = slot.VisibilityEndM * 0.1f;
        node.VisibilityRangeFadeMode = GeometryInstance3D.VisibilityRangeFadeModeEnum.Self;
    }

    private static string Sanitize(string s)
    {
        var sb = new System.Text.StringBuilder();
        foreach (char c in s) sb.Append(char.IsLetterOrDigit(c) ? c : '_');
        return sb.Length == 0 ? "slot" : sb.ToString();
    }

    private static StandardMaterial3D TerrainMat()
    {
        var m = new StandardMaterial3D { VertexColorUseAsAlbedo = true, Roughness = 1.0f, Metallic = 0.0f };
        m.Set("cull_mode", 2); // CULL_DISABLED (Y-up remap flips winding)
        return m;
    }

    private static StandardMaterial3D WaterMat()
    {
        var m = new StandardMaterial3D { VertexColorUseAsAlbedo = true, Roughness = 0.1f };
        m.Set("transparency", 1);
        m.Set("cull_mode", 2);
        return m;
    }

    // Add `child` to `root` and set its owner so PackedScene.Pack serializes it.
    private static void Adopt(Node root, Node child) { root.AddChild(child); child.Owner = root; }

    // --- manifest ---

    private static void WriteManifest(GodotObject engine, string levelsDir, SortedSet<int> present)
    {
        int[] adj = engine.Call("region_adjacency").As<int[]>(); // flat [a, b, sharedPairs, ...]
        var regions = new Godot.Collections.Array();
        foreach (int id in present)
        {
            var neighbors = new Godot.Collections.Array();
            for (int i = 0; i + 2 < adj.Length; i += 3)
            {
                int a = adj[i], b = adj[i + 1], shared = adj[i + 2];
                int other = a == id ? b : (b == id ? a : 0);
                if (other == 0) continue;
                neighbors.Add(new Godot.Collections.Dictionary { { "id", other }, { "name", RegionName(other) }, { "sharedEdges", shared } });
            }
            regions.Add(new Godot.Collections.Dictionary
            {
                { "id", id }, { "name", RegionName(id) },
                { "cells", engine.Call("region_cell_count", id).As<int>() },
                { "scene", $"{SafeName(id)}.tscn" }, { "neighbors", neighbors },
            });
        }
        using var f = FileAccess.Open($"{levelsDir}/regions.json", FileAccess.ModeFlags.Write);
        f?.StoreString(Json.Stringify(regions, "  "));
    }

    // --- naming / paths ---

    private static string RegionName(int id) => id >= 1 && id <= RegionNames.Length ? RegionNames[id - 1] : $"Region{id}";

    private static string SafeName(int id)
    {
        var sb = new System.Text.StringBuilder();
        foreach (char c in RegionName(id)) sb.Append(char.IsLetterOrDigit(c) ? c : '_');
        return sb.ToString();
    }

    private static string NormalizeDir(string dir)
    {
        dir = dir?.Trim() ?? "";
        if (dir.StartsWith("res://") || dir.StartsWith("user://")) return ProjectSettings.GlobalizePath(dir);
        return string.IsNullOrEmpty(dir) ? DefaultExportDir() : System.IO.Path.GetFullPath(dir);
    }

    /// A **tool-local** export folder by default (`res://exports`). Levels bake self-contained (meshes
    /// embedded), so the owner can copy them into the game project by hand — the tool never writes into
    /// another project. Change the dock path field to point elsewhere when you choose to.
    public static string DefaultExportDir() => ProjectSettings.GlobalizePath("res://exports");
}
#endif
