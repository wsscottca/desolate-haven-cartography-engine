//! Deterministic decoration placement.
//!
//! Given a seed + biome grid + elevation + per-biome [`crate::biomes::BiomeDecor`],
//! emits instance sets (position, species, scale, rotation) for rocks/trees and
//! anchor points for region labels — identical in the browser tool and in Godot.
//!
//! Phase 7.
