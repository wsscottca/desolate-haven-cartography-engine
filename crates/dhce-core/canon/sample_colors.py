import numpy as np
from PIL import Image

img = Image.open("../../../assets/canon-region-color-map.jpg").convert("RGB")
W, H = img.size
full = np.asarray(img)
print(f"full {W}x{H}")


def patch(name, x0, y0, x1, y1):
    p = full[int(y0 * H):int(y1 * H), int(x0 * W):int(x1 * W)].reshape(-1, 3)
    med = np.median(p, axis=0).astype(int)
    print(f"  {name:16s} median #{med[0]:02x}{med[1]:02x}{med[2]:02x} ({med[0]:3d},{med[1]:3d},{med[2]:3d})")


print("\nWATER candidates (lake + channels):")
patch("lake center", 0.46, 0.44, 0.54, 0.52)
patch("lake N", 0.47, 0.36, 0.53, 0.42)
patch("channel W", 0.28, 0.46, 0.33, 0.50)
patch("channel SW", 0.30, 0.62, 0.35, 0.68)
patch("channel S", 0.40, 0.82, 0.45, 0.88)
patch("far-left water", 0.00, 0.50, 0.04, 0.58)

print("\nOCEAN:")
patch("ocean R-edge", 0.97, 0.45, 1.00, 0.55)
patch("ocean BR", 0.95, 0.96, 1.00, 1.00)

print("\nMARSH (NE olive):")
patch("marsh NE", 0.74, 0.10, 0.82, 0.18)
patch("marsh NE-2", 0.66, 0.16, 0.72, 0.22)

# What does the lake azure classify to under the CURRENT keys?
def rgb(h): return np.array([int(h[i:i+2], 16) for i in (0, 2, 4)], float)
keys = {"L:water#38858c": "38858c", "R:GreatLake#19c3c3": "19c3c3", "R:LostIsles#2e8c8c": "2e8c8c",
        "R:Frozen#6fa9ce": "6fa9ce", "R:Sacred#2e5c9e": "2e5c9e", "R:ocean#0c1c36": "0c1c36"}
lake = np.array([47, 127, 159], float)
print("\nLake #2d82a3 distance (d2/255^2) to current keys:")
for k, v in sorted(keys.items(), key=lambda kv: ((rgb(kv[1]) - lake) ** 2).sum()):
    print(f"  {k:22s} {(((rgb(v)-lake)**2).sum())/(255**2):.4f}")
