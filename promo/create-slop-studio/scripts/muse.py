"""Turns the Meta Muse renders (hologram on black) into transparent, cropped pictures for laying over footage.

    python -X utf8 scripts/muse.py <in.png> <out.png>

Light on black becomes alpha: alpha is the brightest channel, colour is divided back out, so the glow keeps its
colour over any background. All three poses share one crop, so swapping between them doesn't move her.
"""
import sys

import numpy as np
from PIL import Image

CROP = (380, 24, 1300, 768)  # x0, y0, x1, y1 in the 1376x768 renders (the pointing arm reaches x=1265)

src, dst = sys.argv[1], sys.argv[2]
im = Image.open(src).convert("RGB")
if im.size != (1376, 768):
    im = im.resize((1376, 768), Image.LANCZOS)
im = im.crop(CROP)
a = np.asarray(im).astype(np.float32) / 255.0
alpha = a.max(axis=2)
# a little lift so the faint scan-line gaps stay see-through but the body reads solid
alpha = np.clip((alpha - 0.03) / 0.97, 0, 1)
alpha = np.clip(alpha * 1.25, 0, 1) ** 0.85
rgb = np.where(alpha[..., None] > 0.001, a / np.maximum(alpha[..., None], 0.001), 0)
rgb = np.clip(rgb, 0, 1)
out = np.dstack([rgb, alpha])
Image.fromarray((out * 255).astype(np.uint8), "RGBA").save(dst)
print(dst, im.size)
