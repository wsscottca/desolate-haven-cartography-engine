import numpy as np
from PIL import Image

REGION_HEX = ["0c1c36","c7a24b","2e5c9e","19c3c3","5a9a4a","e6d24a","9aa0aa","6e5a82",
              "6fa9ce","2e8c8c","b23a2e","d6883a","3a2e4a","c77fa8","8a857c"]
LIQUID_HEX = ["2c94a3","f2591a","3d4d2e","c6e2ee"]

def rgb(h): return np.array([int(h[i:i+2],16) for i in (0,2,4)], float)
reg = np.stack([rgb(h) for h in REGION_HEX]); liq = np.stack([rgb(h) for h in LIQUID_HEX])

img = Image.open("../../../assets/canon-region-color-map.jpg").convert("RGB")
W,H = img.size
full = np.asarray(img, float)

def classify(c):
    dr = ((c-reg)**2).sum(1); dl = ((c-liq)**2).sum(1)
    ri = int(dr.argmin()); li = int(dl.argmin())
    return ri, li, (dl.min() < dr.min())

# Coarse ASCII map of region ids (hex digit) over the whole image to locate the lake.
print("REGION-id map (hex digit per cell; '.'=ocean), 40x30:")
NX,NY=40,30
for gy in range(NY):
    row=""
    for gx in range(NX):
        x=int((gx+0.5)*W/NX); y=int((gy+0.5)*H/NY)
        ri,li,isl = classify(full[y,x])
        if isl: row += "wlmi"[li]            # water/lava/marsh/ice
        elif ri==0: row += "."
        else: row += "0123456789abcde"[ri]
    print(f"  {row}")
print("  legend regions: 3=GreatLake 9=LostIsles 2=Sacred 8=Frozen ; liquids: w=water m=marsh l=lava i=ice")

# Dense sample around the central lake; report water vs land(shore) shades.
print("\nLake-area samples (hex -> nearest region / liquid):")
for (x0,y0) in [(0.50,0.45),(0.50,0.40),(0.46,0.45),(0.54,0.45),(0.50,0.50),
                (0.44,0.42),(0.56,0.48),(0.48,0.37),(0.52,0.53)]:
    x=int(x0*W); y=int(y0*H); c=full[y,x]
    ri,li,isl=classify(c)
    tag = f"LIQUID:{LIQUID_HEX[li]}({['water','lava','marsh','ice'][li]})" if isl else f"REGION {ri}"
    print(f"  ({x0:.2f},{y0:.2f}) #{int(c[0]):02x}{int(c[1]):02x}{int(c[2]):02x} -> {tag}")
