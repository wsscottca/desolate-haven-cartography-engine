//! GDExtension adapter (Phase 8).
//!
//! Wraps `dhce-core` for the Godot `desolate-haven` game so the game generates the
//! same terrain / biomes / liquids / scatter as the browser authoring tool. Stub
//! until Phase 8 adds the `godot` (gdext) dependency and the extension entry point.

/// Re-exported so the native build links the shared core even before the gdext glue lands.
pub use dhce_core::VERSION as CORE_VERSION;
