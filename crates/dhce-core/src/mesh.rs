//! Dual half-edge mesh over a permissive Delaunay triangulation.
//!
//! Phase 1: our own clean-room reimplementation of the half-edge navigation
//! (region/triangle/side accessors) — replacing the Apache-2.0 `dual-mesh`. The
//! Delaunay step comes from a permissive crate (MIT-elect); we own the dual wrapper.
