"""
gen_store.py - Google Play listing graphics, generated from the same mark as
the app icon so the store page and the launcher agree.

Outputs (into ../../lifx_mobile/store/):
  icon_512.png            Play "app icon"      512x512, opaque
  feature_graphic.png     Play "feature graphic" 1024x500

Run:  python branding/gen_store.py
"""

import os

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

import gen_logo as G

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.abspath(os.path.join(HERE, "..", "..", "lifx_mobile", "store"))

ACCENT = (0x4c, 0xc2, 0xff)
TEXT = (0xec, 0xee, 0xf2)
MUTED = (0x8b, 0x93, 0xa3)


def _font(name, size):
    for path in (f"C:\\Windows\\Fonts\\{name}", name):
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    return ImageFont.load_default()


def feature_graphic(w=1024, h=500):
    """Dial on the left, wordmark + tagline on the right, on the app's bg."""
    img = Image.new("RGB", (w, h), G.BG)

    # The mark, with its light spilling into the surrounding space.
    mark_px = 380
    mark = G.render(mark_px, opaque=True).convert("RGB")
    mx, my = 70, (h - mark_px) // 2

    # Radiant wash behind/around the dial, in the mark's own spectrum.
    glow = Image.new("RGB", (w, h), (0, 0, 0))
    cx, cy = mx + mark_px / 2, my + mark_px / 2
    for radius, color, dim in ((mark_px * 1.15, (60, 30, 120), 0.55),
                               (mark_px * 0.78, (140, 60, 40), 0.45),
                               (mark_px * 0.52, (255, 176, 80), 0.30)):
        glow = ImageChops.screen(
            glow, G._dim(G._radial(max(w, h), cx, cy, radius, color, power=2.0)
                         .crop((0, 0, w, h)), dim))
    glow = glow.filter(ImageFilter.GaussianBlur(radius=26))
    img = ImageChops.screen(img, glow)

    # Composite only the mark's *light*: subtracting its flat background
    # first stops the paste box from showing as a brighter rectangle when
    # screened onto the wash.
    mark_light = ImageChops.subtract(mark, Image.new("RGB", mark.size, G.BG))
    mask = Image.new("L", (mark_px, mark_px), 0)
    ImageDraw.Draw(mask).ellipse([0, 0, mark_px - 1, mark_px - 1], fill=255)
    mask = mask.filter(ImageFilter.GaussianBlur(radius=10))
    img.paste(ImageChops.screen(img.crop((mx, my, mx + mark_px, my + mark_px)),
                                mark_light), (mx, my), mask)

    d = ImageDraw.Draw(img)
    tx = mx + mark_px + 66
    f_name = _font("bahnschrift.ttf", 108)
    f_tag = _font("bahnschrift.ttf", 34)

    # "Ves" white + "per" accent, matching the in-app header.
    d.text((tx, 176), "Ves", font=f_name, fill=TEXT)
    ves_w = d.textlength("Ves", font=f_name)
    d.text((tx + ves_w, 176), "per", font=f_name, fill=ACCENT)

    d.text((tx + 4, 306), "Smart light control, all on your", font=f_tag, fill=MUTED)
    d.text((tx + 4, 348), "own network.", font=f_tag, fill=MUTED)
    return img


def main():
    os.makedirs(OUT, exist_ok=True)

    G.render(512, opaque=True).save(os.path.join(OUT, "icon_512.png"))
    print("wrote icon_512.png")

    feature_graphic().save(os.path.join(OUT, "feature_graphic.png"))
    print("wrote feature_graphic.png")
    print("->", OUT)


if __name__ == "__main__":
    main()
