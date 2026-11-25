"""Contact sheets of rendered frames, for checking the story by eye.

    python scripts/sheet.py out/stills out/sheets [cols] [thumb width] [rows]
"""
import sys
from pathlib import Path

from PIL import Image, ImageDraw

src = Path(sys.argv[1])
out = Path(sys.argv[2])
cols = int(sys.argv[3]) if len(sys.argv) > 3 else 3
width = int(sys.argv[4]) if len(sys.argv) > 4 else 640
rows = int(sys.argv[5]) if len(sys.argv) > 5 else 3
out.mkdir(parents=True, exist_ok=True)
for old in out.glob("sheet-*.jpg"):
    old.unlink()

files = sorted(src.glob("f*.jpg"))
per = cols * rows
for n in range(0, len(files), per):
    batch = files[n : n + per]
    thumbs = []
    for f in batch:
        im = Image.open(f).convert("RGB")
        im = im.resize((width, round(im.height * width / im.width)), Image.LANCZOS)
        ImageDraw.Draw(im).text((8, 6), f.stem, fill=(255, 220, 120))
        thumbs.append(im)
    h = thumbs[0].height
    sheet = Image.new("RGB", (cols * width, ((len(thumbs) + cols - 1) // cols) * h), (0, 0, 0))
    for i, im in enumerate(thumbs):
        sheet.paste(im, ((i % cols) * width, (i // cols) * h))
    name = out / f"sheet-{n // per + 1:02d}.jpg"
    sheet.save(name, quality=88)
    print(name)
