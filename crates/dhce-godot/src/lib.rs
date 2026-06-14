//! GDExtension adapter exposing `dhce-core` to the Godot `desolate-haven` game.
//!
//! `DhceEngine` (a `RefCounted`) wraps the shared core so the game generates the same
//! terrain + biomes as the browser authoring tool. Determinism is guaranteed by the
//! core (same seed/inputs ⇒ identical output on wasm32 and native).
//!
//! See `docs/specs/dhce-core-contract.md` for the full contract. Pin the `godot`
//! crate version to the project's Godot 4.x build.

use dhce_core::mesh::Mesh;
use dhce_core::{biomes, elevation, geometry};
use godot::prelude::*;

struct DhceExtension;

#[gdextension]
unsafe impl ExtensionLibrary for DhceExtension {}

#[derive(GodotClass)]
#[class(base = RefCounted)]
struct DhceEngine {
    width: f64,
    height: f64,
    mesh: Option<Mesh>,
    elevation_r: Vec<f64>,
    biome_r: Vec<u8>,
    base: Base<RefCounted>,
}

#[godot_api]
impl IRefCounted for DhceEngine {
    fn init(base: Base<RefCounted>) -> Self {
        DhceEngine {
            width: 0.0,
            height: 0.0,
            mesh: None,
            elevation_r: Vec::new(),
            biome_r: Vec::new(),
            base,
        }
    }
}

#[godot_api]
impl DhceEngine {
    /// Build mesh + per-region elevation + biome classification.
    #[func]
    fn build(&mut self, width: f64, height: f64, spacing: f64, seed: f64, octaves: i64) {
        let mesh = Mesh::new(width, height, spacing, seed as u64);
        let nr = mesh.num_regions();
        self.elevation_r = elevation::assign_region_elevation(&mesh, width, height, seed as u64, octaves as u32);

        let cx = width * 0.5;
        let cy = height * 0.5;
        let max_d = 0.5 * (width * width + height * height).sqrt();
        let mut biome_r = vec![0u8; nr];
        for r in 0..nr {
            let p = mesh.pos_of_r(r);
            let dist = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt() / max_d;
            let moist = biomes::moisture_at(p[0], p[1], width, height, seed as u64);
            biome_r[r] = biomes::classify(self.elevation_r[r], moist, dist);
        }
        self.biome_r = biome_r;
        self.width = width;
        self.height = height;
        self.mesh = Some(mesh);
    }

    #[func]
    fn region_count(&self) -> i64 {
        self.mesh.as_ref().map(|m| m.num_regions() as i64).unwrap_or(0)
    }

    #[func]
    fn version(&self) -> GString {
        GString::from(dhce_core::VERSION)
    }

    /// Biome id (1..=14, 0 = none) for a region.
    #[func]
    fn biome_at(&self, region: i64) -> i64 {
        self.biome_r.get(region as usize).copied().unwrap_or(0) as i64
    }

    /// Surface vertex positions (x, y, z per vertex) at a vertical exaggeration.
    #[func]
    fn surface_positions(&self, exaggeration: f64) -> PackedFloat32Array {
        self.surface(exaggeration)
            .map(|s| PackedFloat32Array::from(s.positions.as_slice()))
            .unwrap_or_default()
    }

    /// Surface vertex normals (3 per vertex).
    #[func]
    fn surface_normals(&self, exaggeration: f64) -> PackedFloat32Array {
        self.surface(exaggeration)
            .map(|s| PackedFloat32Array::from(s.normals.as_slice()))
            .unwrap_or_default()
    }

    /// Surface vertex colors (3 per vertex, from each region's biome).
    #[func]
    fn surface_colors(&self, exaggeration: f64) -> PackedFloat32Array {
        self.surface(exaggeration)
            .map(|s| PackedFloat32Array::from(s.colors.as_slice()))
            .unwrap_or_default()
    }

    /// Surface triangle indices (3 per triangle).
    #[func]
    fn surface_indices(&self, exaggeration: f64) -> PackedInt32Array {
        match self.surface(exaggeration) {
            Some(s) => {
                let v: Vec<i32> = s.indices.iter().map(|&i| i as i32).collect();
                PackedInt32Array::from(v.as_slice())
            }
            None => PackedInt32Array::new(),
        }
    }
}

impl DhceEngine {
    /// Build the render surface with per-region biome colors. Not exported to Godot.
    fn surface(&self, exaggeration: f64) -> Option<geometry::Surface> {
        let mesh = self.mesh.as_ref()?;
        let roster = biomes::roster();
        let nr = mesh.num_regions();
        let mut rc = vec![0.5f32; nr * 3];
        for r in 0..nr {
            let id = self.biome_r.get(r).copied().unwrap_or(0);
            if id >= 1 && (id as usize) <= biomes::BIOME_COUNT {
                let c = roster[(id - 1) as usize].color;
                rc[3 * r] = c[0];
                rc[3 * r + 1] = c[1];
                rc[3 * r + 2] = c[2];
            }
        }
        Some(geometry::build_surface(mesh, &self.elevation_r, exaggeration, &rc))
    }
}
