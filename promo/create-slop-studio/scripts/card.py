"""A labelled stand-in card for a clip that doesn't exist yet: python -X utf8 scripts/card.py <out.png> "LINE ONE\nline two" """
import sys

from PIL import Image, ImageDraw, ImageFont

out, text = sys.argv[1], sys.argv[2].replace("\\n", "\n")
im = Image.new("RGB", (1920, 1080), (14, 16, 22))
d = ImageDraw.Draw(im)
try:
    big = ImageFont.truetype("C:/Windows/Fonts/arialbd.ttf", 96)
    small = ImageFont.truetype("C:/Windows/Fonts/arial.ttf", 52)
except OSError:
    big = small = ImageFont.load_default()
lines = text.split("\n")
y = 380
for i, line in enumerate(lines):
    font = big if i == 0 else small
    w = d.textlength(line, font=font)
    d.text(((1920 - w) / 2, y), line, font=font, fill=(255, 95, 168) if i == 0 else (200, 204, 212))
    y += 140 if i == 0 else 80
im.save(out)
