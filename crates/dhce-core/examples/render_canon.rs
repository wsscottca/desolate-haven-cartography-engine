//! Top-down canon-map preview renderer — a no-Godot iteration harness for tuning the canon
//! generator's defaults against `assets/canon-map-base.jpg`.
//!
//! Mirrors the editor's `DhceWorld.Generate()` pipeline against the core `World` and writes RGBA PNGs
//! via the same `World::minimap()` the in-editor overview uses, so the preview reads exactly like a
//! fresh **Generate** (canon-art albedo + hill-shade + contours + water/lava overlay). The world is
//! the canon's 4:3 (40 × 30 km) and the previews are rendered at that true aspect (no squash).
//!
//! Run: `cargo run -p dhce-core --example render_canon [spacing_m]`
//! Writes `<repo>/assets/canon-preview*.png`; spacing defaults to 80 m (coarse = fast).
//!
//! The PNG writer is hand-rolled (stored/uncompressed DEFLATE + CRC32 + Adler32) so the example
//! needs **no image crate** — keeping the core's dependency set (and the GDExtension build) clean.

use dhce_core::world::World;
use std::io::Write;

// --- Editor defaults (tool/addons/dhce/DhceWorld.cs) the preview mirrors so it matches Generate. --
const SEED: u64 = 12345;
const OCTAVES: u32 = 6;
const TERRAIN_HEIGHT_KM: f64 = 14.0; // full ±1.5 span = 14 km → land to +7 km, water to −1 km
const ELEV_SPAN: f64 = 3.0; // ELEV_MAX − ELEV_MIN in world.rs
const BASE_BLEND_M: f64 = 1800.0;
const SHAPE_STRENGTH: f64 = 1.0;
const RIVER_DEPTH_GAIN: f64 = 0.015;
const WORLD_W: f64 = 40_000.0; // 40 km E–W (canon 4:3)
const WORLD_H: f64 = 30_000.0; // 30 km N–S
const RENDER_W: usize = 900; // preview width px; height follows the world aspect (minimap_height)

fn main() {
    // Output is always the fixed repo asset path; only the mesh spacing is overridable.
    let out = asset_path("canon-preview.png");
    let flat_out = asset_path("canon-preview-flat.png");
    let region_out = asset_path("canon-region-preview.png");
    let spacing: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(80.0);

    let _exaggeration = TERRAIN_HEIGHT_KM * 1000.0 / ELEV_SPAN; // metres per normalized unit (relief mode)

    // --- Mirror DhceWorld.Generate() (region-guided terrain) -------------------------------------
    let mut w = World::new();
    w.set_base_blend_m(BASE_BLEND_M); // tool pushes 1800 (core default is 900)
    w.set_flat_base(false); // region-guided base: per-region base height + gradient seed the relief
    // Climate defaults (lapse 0.6, orographic 0.45, wind west→east) already match World::new().
    let t0 = std::time::Instant::now();
    w.build(WORLD_W, WORLD_H, spacing, SEED, OCTAVES);
    // Canon water model: sea level is the 0 waterline (land positive, water to −1 km). Set it, then
    // bake the per-region relief and carve rivers / pond lakes (the Great-Lake bowl fills here).
    let sea = 0.0;
    w.set_sea_level(sea);
    w.reshape_and_reflow(SHAPE_STRENGTH, RIVER_DEPTH_GAIN);
    let gen_ms = t0.elapsed().as_millis();

    // Water sanity: count seated water vs lava cells, and how many are the Great Lake (region 3) —
    // the user's "lake & rivers are dry" check. Lake/lava > 0 confirms `fill_lakes` seated them.
    let depths = w.liquid_depth_export();
    let kinds = w.liquid_kind_export();
    let (mut water, mut lava, mut lake) = (0usize, 0usize, 0usize);
    for r in 0..depths.len() {
        if depths[r] > 0.02 {
            if kinds.get(r).copied().unwrap_or(0) == 1 { lava += 1 } else { water += 1 }
            if w.biome_at(r) == 3 { lake += 1 }
        }
    }
    println!("liquid: {water} water + {lava} lava cells; Great-Lake (region 3) wet cells: {lake}");

    // True-aspect previews (4:3): width RENDER_W, height from the world's aspect.
    let rh = w.minimap_height(RENDER_W);
    // NW light (−0.7, −0.7) matches the editor minimap's default sun map-direction. The shaded
    // minimap shows relief + water/lava overlay; the flat raster shows the true terrain albedo — now
    // the draped canon art — the honest reference for matching the canon map.
    // Region-view raster (each canon place in its accent) — verifies the traced layout/silhouette.
    w.set_view_mode(5); // VIEW_REGION
    let region = w.minimap(RENDER_W, -0.7, -0.7);
    write_png(&region_out, RENDER_W, rh, &region).expect("write region preview");
    w.set_view_mode(0); // back to VIEW_NATURAL for the colour previews
    let shaded = w.minimap(RENDER_W, -0.7, -0.7);
    let flat = flat_albedo(&w, RENDER_W, rh);
    write_png(&out, RENDER_W, rh, &shaded).expect("write shaded preview");
    write_png(&flat_out, RENDER_W, rh, &flat).expect("write flat preview");
    println!(
        "render_canon: {WORLD_W:.0}×{WORLD_H:.0} m @ {spacing} m spacing, sea={sea:.3} → region+shaded+flat ({RENDER_W}×{rh}px, {gen_ms} ms)"
    );
}

/// `<repo>/assets/<file>` resolved from this crate's manifest dir (`crates/dhce-core`).
fn asset_path(file: &str) -> String {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // repo root
    p.push("assets");
    p.push(file);
    p.to_string_lossy().into_owned()
}

/// Flat top-down raster of the true terrain albedo (`World::albedo_at`) — same north-up framing as
/// `World::minimap`, but no shading/contours/liquid overlay. Off-map samples fall back to a neutral.
fn flat_albedo(w: &World, n: usize, nh: usize) -> Vec<u8> {
    let mut out = vec![0u8; n * nh * 4];
    for gy in 0..nh {
        let y = (gy as f64 + 0.5) / nh as f64 * WORLD_H;
        for gx in 0..n {
            let x = (gx as f64 + 0.5) / n as f64 * WORLD_W;
            let px = (gy * n + gx) * 4;
            match w.albedo_at(x, y) {
                Some(rgb) => {
                    out[px] = (rgb[0].clamp(0.0, 1.0) * 255.0) as u8;
                    out[px + 1] = (rgb[1].clamp(0.0, 1.0) * 255.0) as u8;
                    out[px + 2] = (rgb[2].clamp(0.0, 1.0) * 255.0) as u8;
                    out[px + 3] = 255;
                }
                None => out[px + 3] = 255, // off-map → opaque black
            }
        }
    }
    out
}

// --- Minimal zero-dependency PNG writer (8-bit RGBA) ----------------------------------------------

fn write_png(path: &str, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    assert_eq!(rgba.len(), w * h * 4, "rgba must be w*h*4 bytes");
    // Path is a compile-time constant (CARGO_MANIFEST_DIR + fixed segments) — no untrusted input.
    let file = std::fs::File::create(path)?; // nosemgrep
    let mut f = std::io::BufWriter::new(file);
    f.write_all(&[137, 80, 78, 71, 13, 10, 26, 10])?; // PNG signature

    // IHDR: width, height, bit depth 8, colour type 6 (RGBA), no compression/filter/interlace.
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_chunk(&mut f, b"IHDR", &ihdr)?;

    // Raw filtered scanlines: each row prefixed with filter byte 0 (None).
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for row in 0..h {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[row * w * 4..(row + 1) * w * 4]);
    }
    write_chunk(&mut f, b"IDAT", &zlib_store(&raw))?;
    write_chunk(&mut f, b"IEND", &[])?;
    f.flush()
}

/// Wrap `data` as a zlib stream of uncompressed DEFLATE "stored" blocks (≤ 65535 bytes each).
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // zlib header (CM=deflate, 32K window, FCHECK ok)
    let mut i = 0;
    while i < data.len() || data.is_empty() {
        let len = (data.len() - i).min(0xFFFF);
        let final_block = i + len >= data.len();
        out.push(if final_block { 1 } else { 0 }); // BFINAL, BTYPE=00 (stored)
        out.extend_from_slice(&(len as u16).to_le_bytes());
        out.extend_from_slice(&(!(len as u16)).to_le_bytes());
        out.extend_from_slice(&data[i..i + len]);
        i += len;
        if final_block {
            break;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn write_chunk(f: &mut impl Write, kind: &[u8; 4], data: &[u8]) -> std::io::Result<()> {
    f.write_all(&(data.len() as u32).to_be_bytes())?;
    f.write_all(kind)?;
    f.write_all(data)?;
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    f.write_all(&crc.finish().to_be_bytes())
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// CRC-32 (IEEE, reflected) computed incrementally without a precomputed table.
struct Crc(u32);
impl Crc {
    fn new() -> Self {
        Crc(0xFFFF_FFFF)
    }
    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= byte as u32;
            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }
    fn finish(self) -> u32 {
        !self.0
    }
}
