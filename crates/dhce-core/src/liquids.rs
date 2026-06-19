//! Liquid-type registry — authored like biomes.
//!
//! Each liquid carries the physical + visual properties the fluid sim and the
//! renderer need. The set is extensible; Phase 3 wires these into the solver.

/// A liquid the Course/Flood tools can place. Stable, small, `Copy` — used as an
/// index into the registry and serialized in authored maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LiquidType {
    Water = 0,
    Lava = 1,
    Marsh = 2, // murky marsh/bog water
    Ice = 3,   // frozen water / ice
}

/// Physical + visual properties for one liquid type.
#[derive(Clone, Copy, Debug)]
pub struct LiquidProps {
    /// Relative density (affects settling order when liquids mix).
    pub density: f32,
    /// Flow rate factor; lower = more viscous (lava « water).
    pub viscosity: f32,
    /// Base surface color, linear RGB.
    pub color: [f32; 3],
    /// Emissive strength (water 0; lava glows).
    pub emissive: f32,
    /// Per-step evaporation fraction.
    pub evaporation: f32,
}

impl LiquidType {
    /// Recover a liquid type from its stored `u8` id (defaults to water).
    pub fn from_u8(id: u8) -> LiquidType {
        match id {
            1 => LiquidType::Lava,
            2 => LiquidType::Marsh,
            3 => LiquidType::Ice,
            _ => LiquidType::Water,
        }
    }

    /// Default authored properties for the built-in liquids.
    pub fn props(self) -> LiquidProps {
        match self {
            LiquidType::Water => LiquidProps {
                density: 1.0,
                viscosity: 1.0,
                color: [0.10, 0.28, 0.42],
                emissive: 0.0,
                evaporation: 0.002,
            },
            LiquidType::Lava => LiquidProps {
                density: 2.6,
                viscosity: 0.12,
                color: [0.85, 0.22, 0.06],
                emissive: 1.0,
                evaporation: 0.0,
            },
            // Murky bog water — sluggish, dark olive; colour matches the render/minimap (sRGB #3D4D2E).
            LiquidType::Marsh => LiquidProps {
                density: 1.0,
                viscosity: 0.6,
                color: [0.239, 0.302, 0.180],
                emissive: 0.0,
                evaporation: 0.001,
            },
            // Frozen water / ice — doesn't flow; pale blue-white (sRGB #C6E2EE).
            LiquidType::Ice => LiquidProps {
                density: 0.92,
                viscosity: 0.0,
                color: [0.776, 0.886, 0.933],
                emissive: 0.0,
                evaporation: 0.0,
            },
        }
    }
}
