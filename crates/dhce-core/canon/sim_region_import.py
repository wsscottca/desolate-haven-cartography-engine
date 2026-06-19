"""Simulate DhceWorld.ApplyRegionMap (per-pixel classify + windowed majority vote) against the
hand-painted region map, to validate region/liquid classification before importing in Godot.
Run: python sim_region_import.py [path-to-image]"""
import sys
import numpy as np
from PIL import Image

REGION_HEX = [
    "0c1c36", "c7a24b", "2e5c9e", "19c3c3", "5a9a4a", "e6d24a", "9aa0aa", "6e5a82",
    "6fa9ce", "2e8c8c", "b23a2e", "d6883a", "3a2e4a", "c77fa8", "8a857c",
]
REGION_NAME = [
    "Ocean", "Jagged Mtns", "Sacred Woods", "Great Lake", "Temperate Forest", "Open Plains",
    "Underdeep", "Deep Wood", "Frozen Reaches", "Lost Isles", "Blisterwood", "Volcanic Scape",
    "Blight Ruins", "Scattered Isles", "Marsh & Bog",
]
LIQUID_HEX = ["2c94a3", "f2591a", "3d4d2e", "c6e2ee"]
LIQUID_NAME = ["Water", "Lava", "Marsh", "Ice"]


def hex_rgb(h):
    return np.array([int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)], dtype=np.float32) / 255.0


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "../../../assets/canon-region-color-map.jpg"
    img = Image.open(path).convert("RGBA")
    MAX_EDGE = 1280
    if max(img.size) > MAX_EDGE:
        s = MAX_EDGE / max(img.size)
        img = img.resize((round(img.size[0] * s), round(img.size[1] * s)), Image.BILINEAR)
    W, H = img.size
    arr = np.asarray(img, dtype=np.float32) / 255.0  # H,W,4
    rgb = arr[:, :, :3]
    alpha = arr[:, :, 3]

    reg = np.stack([hex_rgb(h) for h in REGION_HEX])   # 15,3
    liq = np.stack([hex_rgb(h) for h in LIQUID_HEX])    # 4,3

    # nearest region + nearest liquid per pixel (squared distance)
    dreg = ((rgb[:, :, None, :] - reg[None, None, :, :]) ** 2).sum(-1)  # H,W,15
    dliq = ((rgb[:, :, None, :] - liq[None, None, :, :]) ** 2).sum(-1)  # H,W,4
    reg_cls = dreg.argmin(-1).astype(np.int32)         # H,W
    liq_cls = dliq.argmin(-1).astype(np.int32)
    is_liq = (dliq.min(-1) < dreg.min(-1)) & (alpha >= 0.5)
    reg_cls[alpha < 0.5] = 0  # transparent -> ocean

    cols = min(W, 256)
    rows = min(H, 256)
    half = max(4, round(max(W / cols, H / rows)))

    ids = np.zeros((rows, cols), np.int32)
    liquids = np.zeros((rows, cols), np.int32)
    for gy in range(rows):
        cy = min(int((gy + 0.5) * H / rows), H - 1)
        y0, y1 = max(cy - half, 0), min(cy + half, H - 1)
        for gx in range(cols):
            cx = min(int((gx + 0.5) * W / cols), W - 1)
            x0, x1 = max(cx - half, 0), min(cx + half, W - 1)
            wl = is_liq[y0:y1 + 1, x0:x1 + 1]
            wr = reg_cls[y0:y1 + 1, x0:x1 + 1]
            wk = liq_cls[y0:y1 + 1, x0:x1 + 1]
            n_tot = wl.size
            n_liq = int(wl.sum())
            if n_liq * 2 >= n_tot and n_tot > 0:
                liquids[gy, gx] = int(np.bincount(wk[wl], minlength=4).argmax()) + 1
                land = wr[~wl]
                if land.size > 0:
                    ids[gy, gx] = int(np.bincount(land, minlength=15).argmax())
                else:
                    ids[gy, gx] = int(reg_cls[cy, cx])
            else:
                land = wr[~wl]
                ids[gy, gx] = int(np.bincount(land, minlength=15).argmax()) if land.size else 0

    print(f"image {W}x{H} -> grid {cols}x{rows}, vote half={half}px ({2*half+1}px window)")
    print("\nREGION cells:")
    rc = np.bincount(ids.ravel(), minlength=15)
    for i in range(15):
        if rc[i]:
            print(f"  {i:2d} {REGION_NAME[i]:18s} {rc[i]:6d}  ({100*rc[i]/ids.size:4.1f}%)")
    print("\nLIQUID cells:")
    lc = np.bincount(liquids.ravel(), minlength=5)
    print(f"  none {lc[0]:6d}  ({100*lc[0]/liquids.size:4.1f}%)")
    for k in range(4):
        if lc[k + 1]:
            print(f"  {LIQUID_NAME[k]:6s} {lc[k+1]:6d}  ({100*lc[k+1]/liquids.size:4.1f}%)")

    # render a preview: region accent, liquid overlaid
    out = np.zeros((rows, cols, 3), np.uint8)
    for i in range(15):
        out[ids == i] = (reg[i] * 255).astype(np.uint8)
    for k in range(4):
        m = liquids == (k + 1)
        out[m] = (liq[k] * 255 * 0.85 + out[m] * 0.15).astype(np.uint8)
    Image.fromarray(out, "RGB").resize((cols * 2, rows * 2), Image.NEAREST).save("sim_region_import_out.png")
    print("\nwrote sim_region_import_out.png")


if __name__ == "__main__":
    main()
