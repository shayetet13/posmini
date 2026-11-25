"""Draws the app icons (home-screen icon of the installed app) into frontend/public/static.

    python frontend/tools/make_icons.py        (needs Pillow)

A white receipt on the brand orange. 'maskable' icons keep the picture inside the safe zone
(the OS may crop them to a circle or squircle); the others have their own rounded corners.
"""
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "public" / "static"
TOP, BOTTOM = (228, 105, 58), (188, 76, 28)  # --brand gradient (E4693A -> BC4C1C)
INK = (217, 95, 43)
SUPER = 4


def gradient(size):
    img = Image.new("RGB", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            t = (x + y) / (2 * size - 2)
            px[x, y] = tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM))
    return img


def receipt(draw, size, scale):
    """The receipt glyph, centred; `scale` is the share of the icon its height may use."""
    h = size * scale
    w = h * 0.74
    left, top = (size - w) / 2, (size - h) / 2
    teeth = 6
    tooth = w / teeth
    notch = h * 0.07
    points = [(left, top), (left + w, top), (left + w, top + h - notch)]
    for i in range(teeth):  # zigzag bottom edge, right to left
        points.append((left + w - tooth * (i + 0.5), top + h))
        points.append((left + w - tooth * (i + 1), top + h - notch))
    draw.polygon(points, fill=(255, 255, 255))
    pad = w * 0.14
    bar = h * 0.07
    # a thick "queue number" bar, then item lines
    y = top + h * 0.14
    draw.rounded_rectangle([left + pad, y, left + w - pad, y + bar * 2.3], radius=bar, fill=INK)
    for i, shrink in enumerate((0, 0.18, 0)):
        y = top + h * 0.40 + i * h * 0.15
        draw.rounded_rectangle([left + pad, y, left + w - pad - w * shrink, y + bar], radius=bar / 2, fill=(214, 200, 184))
    y = top + h * 0.40 + 3 * h * 0.15 - h * 0.02
    draw.rounded_rectangle([left + pad, y, left + w * 0.62, y + bar * 1.4], radius=bar / 2, fill=INK)


def make(size, name, *, maskable=False, rounded=True):
    big = size * SUPER
    img = gradient(big)
    draw = ImageDraw.Draw(img)
    receipt(draw, big, 0.50 if maskable else 0.62)
    img = img.resize((size, size), Image.LANCZOS)
    if rounded and not maskable:
        mask = Image.new("L", (big, big), 0)
        ImageDraw.Draw(mask).rounded_rectangle([0, 0, big - 1, big - 1], radius=int(big * 0.22), fill=255)
        rgba = img.convert("RGBA")
        rgba.putalpha(mask.resize((size, size), Image.LANCZOS))
        img = rgba
    img.save(OUT / name, optimize=True)
    print(f"{name}  {size}x{size}")


if __name__ == "__main__":
    make(192, "icon-192.png")
    make(512, "icon-512.png")
    make(512, "icon-maskable-512.png", maskable=True)
    make(180, "apple-touch-icon.png", rounded=False)  # iOS rounds the corners itself
    make(48, "favicon.png")
