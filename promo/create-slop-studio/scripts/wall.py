"""The desktop wallpaper, darkened and blurred, as the backdrop of the end card: python -X utf8 scripts/wall.py <out.jpg>"""
import sys
import winreg

from PIL import Image, ImageEnhance, ImageFilter

with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Control Panel\Desktop") as key:
    path = winreg.QueryValueEx(key, "WallPaper")[0]
im = Image.open(path).convert("RGB")
scale = max(1920 / im.width, 1080 / im.height)
im = im.resize((round(im.width * scale), round(im.height * scale)), Image.LANCZOS)
l, t = (im.width - 1920) // 2, (im.height - 1080) // 2
im = im.crop((l, t, l + 1920, t + 1080)).filter(ImageFilter.GaussianBlur(26))
im = ImageEnhance.Brightness(im).enhance(0.42)
im.save(sys.argv[1], quality=90)
print(sys.argv[1])
