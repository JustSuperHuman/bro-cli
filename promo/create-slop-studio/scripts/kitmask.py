"""The toolkit capture with its cast grid (a fixture without portraits) blurred and darkened, so only the Toolkit
panel reads. Everything right of x=985 is untouched.

    python -X utf8 scripts/kitmask.py <in.png> <out.png>
"""
import sys

from PIL import Image, ImageEnhance, ImageFilter

im = Image.open(sys.argv[1]).convert("RGBA")
box = (0, 150, 985, im.height - 60)
crop = im.crop(box)
alpha = crop.getchannel("A")
rgb = ImageEnhance.Brightness(crop.convert("RGB").filter(ImageFilter.GaussianBlur(34))).enhance(0.45)
masked = rgb.convert("RGBA")
masked.putalpha(alpha)
im.paste(masked, box[:2])
im.save(sys.argv[2])
