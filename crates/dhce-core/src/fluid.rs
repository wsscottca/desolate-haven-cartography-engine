//! Hydraulic liquid simulation.
//!
//! Height-field shallow-water / virtual-pipes over the mesh, plus priority-flood for
//! basin fills. Each cell carries a liquid column per [`crate::liquids::LiquidType`];
//! flow is driven by hydraulic head (terrain + liquid surface). Fixed-step and
//! mass-conserving so it settles to a stable equilibrium and mirrors in Godot.
//!
//! Phase 3.
