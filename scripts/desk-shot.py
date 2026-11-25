"""Put an in-app screenshot of bro on the desktop, as it looks with Acrylic on.

The desktop app's own captures (`BRO_GUI_SCRIPT="...;shot:x.png"`) hold only what bro drew: with a see-through
material the PNG keeps its alpha and nothing is behind it. This puts the desktop wallpaper there, blurred and
tinted the way Windows draws Acrylic under a window, and gives the window its rounded corners, edge and shadow.

Take the capture with a see-through material first, e.g.
    BRO_GUI_SCRIPT="wait:2500;theme:github-dark;material:acrylic,70;wait:1500;key:alt+n;wait:900;shot:C:/tmp/a.png;exit"
then
    python -X utf8 scripts/desk-shot.py C:/tmp/a.png C:/tmp/a-desk.png

Options: --wallpaper <picture> (default: the one Windows shows now), --margin <px of desktop around the window>,
--at <x>,<y> (the window's top-left on the wallpaper; default: centred), --blur <px>.
Needs Pillow.
"""

import argparse
import random
import sys

from PIL import Image, ImageChops, ImageDraw, ImageEnhance, ImageFilter, ImageStat

CORNER = 8  # Windows 11 rounds a window's corners by this much


def current_wallpaper():
    """The picture Windows shows on the desktop now."""
    import winreg

    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Control Panel\Desktop") as key:
        return winreg.QueryValueEx(key, "WallPaper")[0]


def cover(picture, w, h):
    """`picture` scaled to cover at least w x h (never shrunk below it)."""
    scale = max(w / picture.width, h / picture.height, 1.0)
    if scale > 1.0:
        picture = picture.resize((round(picture.width * scale), round(picture.height * scale)), Image.LANCZOS)
    return picture


def acrylic(behind, light, blur):
    """What Windows draws under an Acrylic window: the desktop blurred, pulled toward the system's dark or light
    tint, with a little grain."""
    out = behind.filter(ImageFilter.GaussianBlur(blur))
    out = ImageEnhance.Color(out).enhance(1.15)
    tint = Image.new("RGB", out.size, (243, 243, 243) if light else (32, 32, 32))
    out = Image.blend(out, tint, 0.6 if light else 0.45)
    rng = random.Random(7)
    grain = Image.frombytes("L", out.size, bytes(rng.randrange(118, 139) for _ in range(out.width * out.height)))
    return ImageChops.add(out, Image.merge("RGB", (grain, grain, grain)), scale=1.0, offset=-128)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("shot")
    ap.add_argument("out", nargs="?")
    ap.add_argument("--wallpaper")
    ap.add_argument("--margin", type=int, default=72)
    ap.add_argument("--at")
    ap.add_argument("--blur", type=float, default=42.0)
    a = ap.parse_args()

    shot = Image.open(a.shot).convert("RGBA")
    out_path = a.out or a.shot.rsplit(".", 1)[0] + "-desk.png"
    if shot.getextrema()[3][0] == 255:
        print("note: this capture has no see-through pixels; take it with material:acrylic,<tint> to see Acrylic", file=sys.stderr)

    w, h = shot.size
    cw, ch = w + 2 * a.margin, h + 2 * a.margin
    wall = cover(Image.open(a.wallpaper or current_wallpaper()).convert("RGB"), cw, ch)
    if a.at:
        x, y = (int(v) for v in a.at.split(","))
        left, top = x - a.margin, y - a.margin
    else:
        left, top = (wall.width - cw) // 2, (wall.height - ch) // 2
    left = max(0, min(left, wall.width - cw))
    top = max(0, min(top, wall.height - ch))
    desk = wall.crop((left, top, left + cw, top + ch))

    # light or dark Acrylic, from what bro drew
    light = ImageStat.Stat(shot.convert("L")).mean[0] > 140

    box = (a.margin, a.margin, a.margin + w, a.margin + h)
    window = acrylic(desk.crop(box), light, a.blur).convert("RGBA")
    window.alpha_composite(shot)
    shape = Image.new("L", (w, h), 0)
    ImageDraw.Draw(shape).rounded_rectangle((0, 0, w - 1, h - 1), CORNER, fill=255)

    # the window's shadow on the desktop, then the window, then its hairline edge
    canvas = desk.convert("RGBA")
    shadow = Image.new("L", (cw, ch), 0)
    ImageDraw.Draw(shadow).rounded_rectangle((box[0], box[1] + 14, box[2] - 1, box[3] + 13), CORNER, fill=150)
    shadow = shadow.filter(ImageFilter.GaussianBlur(26))
    canvas.paste(Image.new("RGBA", (cw, ch), (0, 0, 0, 255)), (0, 0), shadow)
    canvas.paste(window, box[:2], shape)
    edge = Image.new("RGBA", (cw, ch), (0, 0, 0, 0))
    ImageDraw.Draw(edge).rounded_rectangle((box[0], box[1], box[2] - 1, box[3] - 1), CORNER, outline=(0, 0, 0, 60) if light else (255, 255, 255, 46))
    canvas.alpha_composite(edge)
    canvas.convert("RGB").save(out_path)
    print(out_path)


if __name__ == "__main__":
    main()
