"""
Classify the canon region map (assets/canon-map-regions.jpg) into a DHCE region-id raster.

This map is a de-iconed, region-labelled version of the canon world: each of the 14 regions is a
distinct flat colour zone with its name typed on it. We turn it into a per-cell region-id raster.

Approach (robust to same-coloured regions, no straight-line hacks):
  * COASTLINE = border-connected flood of the teal sea (the artist's real silhouette).
  * Each region has a hand-placed CENTROID (read off the labelled map) — see REGIONS.
  * We SAMPLE each region's actual colour (median Lab in a window at its centroid, skipping the
    white text + sea), then assign every LAND pixel to the region with the lowest
    `Wc·colour_distance² + Wp·position_distance²`. Distinct-colour regions are split by colour;
    regions that share a colour (the greys, browns, greens, pales) are split by position. No HSV
    class buckets, no per-region spatial fix-ups.
  * The TEXT labels are masked (near-white) so they're assigned by position only (→ their own
    region), then a median filter cleans the residue.
  * The GREAT LAKE (id 3) is the inland (non-ocean) water of the central channel.

Two sources, two jobs:
  * REGION ids   ← canon-map-regions.jpg (clean, labelled)   → src/canon_region_map.bin
  * COLOUR/albedo ← canon-map-base.jpg   (the full art)      → src/canon_color_map.bin
    (the 3D terrain is draped with the full 4K canon-map-base.jpg at runtime; this raster only
     feeds the small overview minimap.)

Outputs:
  * src/canon_region_map.bin                  — GRID_W×GRID_H u8 region ids (row 0 = north)
  * src/canon_color_map.bin                   — CW×CH×3 u8 canon RGB (minimap albedo)
  * ../../assets/canon-region-classified.png  — per-pixel classification at working res (verify vs canon)
  * ../../assets/canon-region-grid.png        — the final region grid in accent colours
  * ../../assets/canon-color-grid.png         — the colour raster (the canon art at grid res)
"""
import os
import numpy as np
from PIL import Image
from scipy import ndimage

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
REGION_SRC = os.path.join(REPO, "assets", "canon-map-regions.jpg")  # region ids (clean, labelled)
COLOR_SRC = os.path.join(REPO, "assets", "canon-map-base.jpg")      # albedo/minimap (full art)
BIN = os.path.join(HERE, "..", "src", "canon_region_map.bin")
CBIN = os.path.join(HERE, "..", "src", "canon_color_map.bin")
ASSETS = os.path.join(REPO, "assets")

GRID_W = 1536       # REGION raster width  (4:3; region identity — large areas, no need for 4K)
GRID_H = 1152       # REGION raster height
CW = 1024           # COLOUR raster width  — minimap only (3D drape uses the full 4K JPG as a texture)
CH = 768            # COLOUR raster height
DOWN = 1            # classify at full 4K (thin painted/texture borders survive intact)

# Hybrid classify tuning. Interiors are labelled by colour+position (so each region lands correctly,
# nothing leaks); only a band around the borders is left for the watershed to snap to the real colour
# edge. W_POSITION keeps same-coloured neighbours apart; BAND_FRAC is the contested-border half-width
# (fraction of image width); EDGE_BLUR denoises JPEG blocks before the colour-edge gradient.
W_COLOUR = 1.0
W_POSITION = 9.0
BAND_FRAC = 0.008
GL_RADIUS = 0.14
W_TEXTURE = 0.45    # weight of the texture-energy edges (stipple/smooth) vs colour edges in the relief
TEX_WIN = 9         # window (px) for the local-luminance-stddev texture measure

# RegionKeyHex accents (id -> RGB), for the preview PNGs. id 0 = ocean. (Matches DhceWorld.RegionKeyHex.)
ACCENT = {
    0: (0x0c, 0x1c, 0x36), 1: (0xc7, 0xa2, 0x4b), 2: (0x2e, 0x5c, 0x9e), 3: (0x19, 0xc3, 0xc3),
    4: (0x5a, 0x9a, 0x4a), 5: (0xe6, 0xd2, 0x4a), 6: (0x9a, 0xa0, 0xaa), 7: (0x6e, 0x5a, 0x82),
    8: (0x6f, 0xa9, 0xce), 9: (0x2e, 0x8c, 0x8c), 10: (0xb2, 0x3a, 0x2e), 11: (0xd6, 0x88, 0x3a),
    12: (0x3a, 0x2e, 0x4a), 13: (0xc7, 0x7f, 0xa8), 14: (0x6e, 0x7a, 0x4b),
}

# Region centroids in canon-normalized coords (px = x/W east, py = y/H south), read off the labelled
# canon-map-regions.jpg. Great Lake (3) is handled as inland water, but keep a centroid for fallback.
REGIONS = [
    (1,  0.50, 0.10, "Jagged Mountains"),
    (2,  0.45, 0.47, "Sacred Forest"),       # was "Sacred Woods Plateau"
    (3,  0.55, 0.45, "Great Lake"),          # inland water of the central channel
    (4,  0.65, 0.36, "Temperate Forest"),    # center-right green band, right of the lake
    (5,  0.79, 0.59, "Rolling Plains"),      # was "Open Plains"
    (6,  0.52, 0.73, "Underdeep"),
    (7,  0.31, 0.44, "Deep Wood"),
    (8,  0.27, 0.26, "Frozen Reaches"),
    (9,  0.13, 0.21, "Lost Isles"),
    (10, 0.34, 0.85, "Blisterwood"),
    (11, 0.55, 0.92, "Volcanic Scape"),
    (12, 0.25, 0.64, "Blight Ruins"),
    (13, 0.82, 0.85, "Scattered Isles"),
    (14, 0.82, 0.21, "Marsh Bog"),
]


def main():
    im = Image.open(REGION_SRC).convert("RGB")
    if DOWN > 1:
        im = im.resize((im.width // DOWN, im.height // DOWN), Image.BILINEAR)
    W, H = im.size
    rgb = np.asarray(im).astype(np.float32)
    ys, xs = np.mgrid[0:H, 0:W]
    px = xs / W
    py = ys / H

    # --- mask the typed white text, then inpaint each text pixel from the nearest non-text colour, so
    # the labels leave NO spurious colour edges and the region fills right over where the word was. ---
    lab0 = np.asarray(im.convert("LAB")).astype(np.float32)
    chroma0 = np.sqrt((lab0[..., 1] - 128.0) ** 2 + (lab0[..., 2] - 128.0) ** 2)
    white = (lab0[..., 0] > 235.0) & (chroma0 < 12.0)
    if white.any():
        _, (iy, ix) = ndimage.distance_transform_edt(white, return_indices=True)
        rgb = rgb[iy, ix]  # every text pixel ← nearest non-text colour (non-text pixels unchanged)
    filled = Image.fromarray(rgb.astype(np.uint8), "RGB")
    irgb = np.asarray(filled).astype(np.int32)

    # --- ocean: border-connected flood of the teal sea (on the inpainted image) ---
    border = np.concatenate([irgb[0], irgb[-1], irgb[:, 0], irgb[:, -1]]).reshape(-1, 3)
    ocean_col = np.median(border, axis=0)
    dist = np.sqrt(((irgb - ocean_col) ** 2).sum(2))
    bright = irgb.mean(axis=2)
    # Sea = teal-coloured AND not near-black, so the dark volcanic massif in the south isn't flooded
    # as ocean just because it's a dark colour the flood could connect to from the edge.
    ocean_like = (dist < 42) & (bright > 55)
    seed = np.zeros((H, W), bool)
    seed[0, :] = seed[-1, :] = seed[:, 0] = seed[:, -1] = True
    seed &= ocean_like
    ocean = ndimage.binary_propagation(seed, mask=ocean_like)

    # --- Great Lake (id 3): carve the central blue pool out of the sea and seed it as its own region. ---
    b_dom = (irgb[..., 2] > irgb[..., 0] + 12) & (irgb[..., 2] > irgb[..., 1] + 6) & (irgb[..., 2] > 70)
    glx, gly = REGIONS[2][1], REGIONS[2][2]
    gl_water = b_dom & (((px - glx) ** 2 + (py - gly) ** 2) < GL_RADIUS ** 2)
    ocean &= ~gl_water

    # --- watershed relief: COLOUR edges + TEXTURE edges, so borders between same-coloured but
    # different-textured regions (the greens, the browns) are real ridges too. Median denoise keeps the
    # painted borders crisp (unlike a gaussian blur). ---
    lab_s = np.asarray(filled.convert("LAB")).astype(np.float32)     # sharp, for colour matching
    lab_b = ndimage.median_filter(lab_s, size=(3, 3, 1))            # edge-preserving JPEG denoise

    def norm(a):
        a = a - a.min()
        return a / max(float(a.max()), 1e-6)

    cgrad = np.zeros((H, W), np.float32)                            # colour-edge magnitude (Lab)
    for c in range(3):
        cgrad += np.hypot(ndimage.sobel(lab_b[..., c], axis=1), ndimage.sobel(lab_b[..., c], axis=0))
    Lf = lab_b[..., 0]                                              # texture energy = local luminance stddev
    lm = ndimage.uniform_filter(Lf, size=TEX_WIN)
    tex = np.sqrt(np.maximum(ndimage.uniform_filter(Lf * Lf, size=TEX_WIN) - lm * lm, 0.0))
    tgrad = np.hypot(ndimage.sobel(tex, axis=1), ndimage.sobel(tex, axis=0))  # texture-transition edges
    grad = norm(norm(cgrad) + W_TEXTURE * norm(tgrad))
    grad = (grad * 255.0).astype(np.uint8)

    # --- step 1: confident interior label by colour+position. Sample each region's colour (median Lab
    # at its centroid, skipping sea + any residual bright pixels), then assign every land pixel to the
    # lowest `Wc·colour² + Wp·position²`. This places every region correctly with no leakage. ---
    land = (~ocean) & (~gl_water)
    win = max(2, int(0.012 * W))
    reg_lab = np.zeros((len(REGIONS), 3), np.float32)
    for i, (rid, cx, cy, _label) in enumerate(REGIONS):
        gx, gy = int(cx * W), int(cy * H)
        x0, x1 = max(0, gx - win), min(W, gx + win + 1)
        y0, y1 = max(0, gy - win), min(H, gy + win + 1)
        patch = lab_s[y0:y1, x0:x1].reshape(-1, 3)
        m = land[y0:y1, x0:x1].reshape(-1)
        reg_lab[i] = np.median(patch[m] if m.any() else patch, axis=0)
    ly, lx = np.where(land)
    plab = lab_s[ly, lx] / 255.0                       # (N,3)
    ppx, ppy = px[ly, lx], py[ly, lx]                  # (N,)
    # Running argmin over regions (memory-safe at 4K — no N×K matrix).
    best_cost = np.full(ly.shape[0], np.inf, np.float32)
    best_id = np.zeros(ly.shape[0], np.uint8)
    for i, (rid, cx, cy, _label) in enumerate(REGIONS):
        cd = ((plab - reg_lab[i] / 255.0) ** 2).sum(1)
        pd = (ppx - cx) ** 2 + (ppy - cy) ** 2
        cost = (W_COLOUR * cd + W_POSITION * pd).astype(np.float32)
        m = cost < best_cost
        best_cost[m] = cost[m]
        best_id[m] = rid
    seed = np.zeros((H, W), np.int32)
    seed[ly, lx] = best_id

    # --- step 2: keep interiors as fixed markers, blank a band around every label border, and let the
    # watershed re-decide ONLY that band — snapping each border to the real colour edge (exact edges)
    # where one exists, without any region leaking past its confident interior. ---
    OCEAN_MARK = 100
    diff = np.zeros((H, W), bool)
    diff[:, :-1] |= seed[:, :-1] != seed[:, 1:]
    diff[:, 1:] |= seed[:, :-1] != seed[:, 1:]
    diff[:-1, :] |= seed[:-1, :] != seed[1:, :]
    diff[1:, :] |= seed[:-1, :] != seed[1:, :]
    band = ndimage.binary_dilation(diff, iterations=max(1, int(BAND_FRAC * W)))
    markers = seed.copy()
    markers[band & land] = 0          # only the land border band is re-decided
    markers[ocean] = OCEAN_MARK       # sea stays fixed (land can't flood it)
    markers[gl_water] = 3
    for rid, cx, cy, _label in REGIONS:  # safety: a region wholly inside the band keeps a centroid seed
        if rid == 3 or (markers == rid).any():
            continue
        gx, gy = int(cx * W), int(cy * H)
        markers[gy, gx] = rid

    labels = ndimage.watershed_ift(grad, markers).astype(np.int32)
    reg = np.where(labels == OCEAN_MARK, 0, labels).astype(np.uint8)
    reg[ocean] = 0  # re-assert the sea

    # any land pixel the watershed left unassigned → nearest assigned region (rare safety net)
    holes = (reg == 0) & (~ocean)
    if holes.any():
        _, (hy, hx) = ndimage.distance_transform_edt(reg == 0, return_indices=True)
        filled_reg = reg[hy, hx]
        reg[holes] = filled_reg[holes]
        reg[ocean] = 0

    # --- tidy: drop tiny disconnected fragments (stray specks, thin leaks like a pink tendril in the
    # north) and reassign each to its surrounding majority. Sea-locked islets fall to sea; land tendrils
    # join their neighbouring region. The big region bodies (centroid component) are always kept. ---
    minsize = int(0.0003 * H * W)
    drop = np.zeros((H, W), bool)
    for rid, cx, cy, _label in REGIONS:
        mask = reg == rid
        if not mask.any():
            continue
        lbl, n = ndimage.label(mask)
        if n <= 1:
            continue
        counts = np.bincount(lbl.ravel())
        cyi, cxi = int(cy * H), int(cx * W)
        keep_cc = lbl[cyi, cxi] if (0 <= cyi < H and 0 <= cxi < W) else 0
        small = np.where(counts < minsize)[0]
        small = small[(small != 0) & (small != keep_cc)]
        if small.size:
            drop |= np.isin(lbl, small)
    if drop.any():
        reg[drop] = 0
        settled = reg.astype(np.int32)
        settled[ocean] = 200  # sentinel so sea is a valid reassignment target
        _, (iy, ix) = ndimage.distance_transform_edt(settled == 0, return_indices=True)
        nn = settled[iy, ix]
        d = (reg == 0) & (~ocean)
        reg[d] = nn[d].astype(np.uint8)
        reg[reg == 200] = 0
        reg[ocean] = 0

    # --- previews + final raster (region ids by nearest-neighbour so ids never blend) ---
    save_accent(reg, os.path.join(ASSETS, "canon-region-classified.png"))
    reg_img = Image.fromarray(reg, "L").resize((GRID_W, GRID_H), Image.NEAREST)
    grid = np.asarray(reg_img, np.uint8)
    save_accent(grid, os.path.join(ASSETS, "canon-region-grid.png"))

    # colour raster: the actual canon ART (icons + everything), pixel-for-pixel — the minimap albedo.
    color = Image.open(COLOR_SRC).convert("RGB")
    if color.size != (CW, CH):
        color = color.resize((CW, CH), Image.LANCZOS)
    carr = np.asarray(color, np.uint8)
    Image.fromarray(carr, "RGB").save(os.path.join(ASSETS, "canon-color-grid.png"))

    with open(os.path.abspath(BIN), "wb") as f:
        f.write(grid.tobytes())
    with open(os.path.abspath(CBIN), "wb") as f:
        f.write(carr.tobytes())
    counts = {int(i): int((grid == i).sum()) for i in range(15)}
    print("grid region cell counts:", counts)
    missing = [i for i in range(1, 15) if counts[i] == 0]
    print("MISSING regions:", missing if missing else "none")
    print("wrote", os.path.abspath(BIN), grid.shape)
    print("wrote", os.path.abspath(CBIN), carr.shape)


def save_accent(reg, path):
    out = np.zeros((*reg.shape, 3), np.uint8)
    for i, col in ACCENT.items():
        out[reg == i] = col
    Image.fromarray(out, "RGB").save(path)


if __name__ == "__main__":
    main()
