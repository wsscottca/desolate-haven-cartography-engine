//! Canon colour raster — the **pixel-for-pixel albedo base**. `canon/classify_canon.py` downscales
//! the canon map (`assets/canon-map-base.jpg`, 4K) into a `CANON_W × CANON_H × 3` RGB grid
//! (row 0 = north, RGB row-major), baked here at build time. The terrain's per-cell albedo
//! ([`crate::world::World`]'s `cell_color`) samples this at each unpainted cell's normalized
//! `(u, v)` so a fresh Generate is draped with the actual canon art — a paintable base the author
//! sculpts and paints over. Like the region raster it is sampled by `(u, v)`, so it is decoupled
//! from the world's metres and matches the canon's 4:3 aspect regardless of world size.

const CANON_W: usize = 1024; // feeds the overview minimap only; the 3D terrain is draped with the full
const CANON_H: usize = 768; //  4K canon JPG as a GPU texture (crisp at any zoom/LOD) — see DhceWorld.cs
static CANON_COLOR: &[u8] = include_bytes!("canon_color_map.bin");

/// Whether the baked colour raster is present and the expected size (it ships via `include_bytes!`,
/// so this is only false if the `.bin` wasn't regenerated after a dimension change).
pub fn has_canon_color() -> bool {
    CANON_COLOR.len() >= CANON_W * CANON_H * 3
}

/// Canon albedo at normalized `(u, v)` (`u` west→east, `v` north→south, row 0 = north) as
/// `[r, g, b]` in `0..=1`. The raw sRGB channels are handed back as plain `0..1` to match the
/// palette convention (`regions`' palette colours are likewise stored as plain `0..1`). Returns
/// `None` if the baked raster is missing/short, so callers fall back to the procedural palette.
pub fn canon_color_at(u: f64, v: f64) -> Option<[f32; 3]> {
    if !has_canon_color() {
        return None;
    }
    let gx = ((u.clamp(0.0, 1.0) * CANON_W as f64) as usize).min(CANON_W - 1);
    let gy = ((v.clamp(0.0, 1.0) * CANON_H as f64) as usize).min(CANON_H - 1);
    let i = (gy * CANON_W + gx) * 3;
    Some([
        CANON_COLOR[i] as f32 / 255.0,
        CANON_COLOR[i + 1] as f32 / 255.0,
        CANON_COLOR[i + 2] as f32 / 255.0,
    ])
}
