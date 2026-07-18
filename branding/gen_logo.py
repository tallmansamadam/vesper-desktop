"""
gen_logo.py - Generate the Vesper logo and every app icon size from code.

The mark: the app's signature rotary dial, lit from within. Its 270-degree
value arc sweeps the full hue spectrum, that spectrum washes across the dial
face as a blurred conic glow, and a white-hot core burns at the centre -
the evening star, held on a dial.

Light is composited additively (screen blending) rather than painted on top,
so the colour genuinely illuminates the face instead of sitting beside it.
Everything is geometry + a hue sweep at 4x supersampling: original by
construction, regenerable at any size.

Outputs:
  branding/vesper_1024.png            master, transparent, rounded-square bg
  branding/vesper_icon_256.png        desktop window icon (Tk iconphoto)
  ../lifx_mobile android mipmaps      ic_launcher.png at all densities
  ../lifx_mobile iOS AppIcon set      every size in Contents.json (opaque)

Run:  python branding/gen_logo.py
"""

import colorsys
import json
import math
import os

from PIL import Image, ImageChops, ImageDraw, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
MOBILE = os.path.normpath(os.path.join(HERE, "..", "..", "lifx_mobile"))

BG = (9, 10, 14)
FACE = (18, 20, 27)
SS = 4                                      # supersample factor

# Hue sweep: red -> orange -> yellow -> green -> cyan -> blue -> violet
HUE_FROM, HUE_TO = 0.02, 0.80
ARC_START, ARC_SWEEP = 135.0, 270.0         # PIL: y down, clockwise from 3 o'clock


def _hue_at(t):
    """RGB tuple for position t (0..1) along the arc's hue sweep."""
    h = HUE_FROM + (HUE_TO - HUE_FROM) * t
    r, g, b = colorsys.hsv_to_rgb(h % 1.0, 0.95, 1.0)
    return int(r * 255), int(g * 255), int(b * 255)


def _dim(img, factor):
    """Scale an RGB layer's intensity (how much light it contributes)."""
    return img.point(lambda v: int(v * factor))


def _spectrum_arc(draw, bbox, width, steps=260):
    """The value arc as a hue sweep, segment by segment (no gaps)."""
    for i in range(steps):
        a0 = ARC_START + ARC_SWEEP * i / steps
        a1 = ARC_START + ARC_SWEEP * (i + 1) / steps + 1.2
        draw.arc(bbox, start=a0, end=a1, fill=_hue_at(i / (steps - 1)),
                 width=width)


def _rays(s, cx, cy, r_in, r_out, width, count=96):
    """Spectrum beams firing from the core out to the ring."""
    layer = Image.new("RGB", (s, s), (0, 0, 0))
    d = ImageDraw.Draw(layer)
    for i in range(count):
        t = i / (count - 1)
        a = math.radians(ARC_START + ARC_SWEEP * t)
        d.line([(cx + r_in * math.cos(a), cy + r_in * math.sin(a)),
                (cx + r_out * math.cos(a), cy + r_out * math.sin(a))],
               fill=_hue_at(t), width=width)
    return layer


def _radial(s, cx, cy, radius, color, power=1.0, steps=40):
    """A soft radial light bloom on black (for additive compositing)."""
    layer = Image.new("RGB", (s, s), (0, 0, 0))
    d = ImageDraw.Draw(layer)
    for i in range(steps, 0, -1):
        f = (1 - i / steps) ** power
        r = radius * i / steps
        d.ellipse([cx - r, cy - r, cx + r, cy + r],
                  fill=(int(color[0] * f), int(color[1] * f), int(color[2] * f)))
    return layer


def render(size, opaque=False):
    """Render the mark at `size` px. opaque=True fills the square (iOS)."""
    s = size * SS
    cx = cy = s / 2
    R = s * 0.335                           # arc radius
    arc_w = int(s * 0.058)
    bbox = [cx - R, cy - R, cx + R, cy + R]
    face_r = s * 0.262

    # --- body: dark square + dark dial face -------------------------------
    body = Image.new("RGB", (s, s), BG)
    bd = ImageDraw.Draw(body)
    bd.ellipse([cx - face_r, cy - face_r, cx + face_r, cy + face_r], fill=FACE)

    # --- light layers (black = no light; screened in additively) ----------
    # 1. Rays: spectrum beams lighting the outer half of the face. Held off
    #    the centre so a dark gap remains for the core to read against.
    rays = _rays(s, cx, cy, R * 0.52, R * 0.95, width=int(s * 0.016))
    rays = _dim(rays.filter(ImageFilter.GaussianBlur(radius=s * 0.03)), 0.75)

    # 2. Ring bloom: the arc itself, thick and blurred - the halo.
    halo = Image.new("RGB", (s, s), (0, 0, 0))
    _spectrum_arc(ImageDraw.Draw(halo), bbox, width=int(arc_w * 1.9))
    halo = halo.filter(ImageFilter.GaussianBlur(radius=s * 0.042))

    # 3. Core, in three stages: a wide warm falloff, a soft inner bloom, and
    #    a small white-hot centre. The stacking is what makes it read as a
    #    lamp rather than a pale disc.
    warm = _radial(s, cx, cy, s * 0.20, (255, 176, 80), power=2.4)
    soft = _radial(s, cx, cy, s * 0.105, (255, 236, 190), power=1.1)
    hot = _radial(s, cx, cy, s * 0.055, (255, 255, 255), power=0.4)

    lit = body
    for layer in (rays, halo, warm, soft, hot):
        lit = ImageChops.screen(lit, layer)

    img = lit.convert("RGBA")

    # --- crisp spectrum arc on top ----------------------------------------
    arc = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    ad = ImageDraw.Draw(arc)
    _spectrum_arc(ad, bbox, width=arc_w)
    for ang, t in ((ARC_START, 0.0), (ARC_START + ARC_SWEEP, 1.0)):
        rr = R - arc_w / 2
        ex = cx + rr * math.cos(math.radians(ang))
        ey = cy + rr * math.sin(math.radians(ang))
        ad.ellipse([ex - arc_w / 2, ey - arc_w / 2,
                    ex + arc_w / 2, ey + arc_w / 2], fill=_hue_at(t) + (255,))
    img = Image.alpha_composite(img, arc)

    # --- pointer, with its own small glow ---------------------------------
    ang = math.radians(311)
    r1, r2 = s * 0.145, s * 0.233
    p1 = (cx + r1 * math.cos(ang), cy + r1 * math.sin(ang))
    p2 = (cx + r2 * math.cos(ang), cy + r2 * math.sin(ang))
    pw = int(s * 0.034)

    pglow = Image.new("RGB", (s, s), (0, 0, 0))
    pd = ImageDraw.Draw(pglow)
    pd.line([p1, p2], fill=(180, 190, 210), width=int(pw * 2.2))
    pglow = pglow.filter(ImageFilter.GaussianBlur(radius=s * 0.02))
    img = Image.alpha_composite(
        ImageChops.screen(img.convert("RGB"), pglow).convert("RGBA"),
        Image.new("RGBA", (s, s), (0, 0, 0, 0)))

    d = ImageDraw.Draw(img)
    d.line([p1, p2], fill=(255, 255, 255, 255), width=pw)
    for p in (p1, p2):
        d.ellipse([p[0] - pw / 2, p[1] - pw / 2, p[0] + pw / 2, p[1] + pw / 2],
                  fill=(255, 255, 255, 255))

    # --- shape mask -------------------------------------------------------
    if not opaque:
        mask = Image.new("L", (s, s), 0)
        ImageDraw.Draw(mask).rounded_rectangle(
            [0, 0, s - 1, s - 1], radius=int(s * 0.225), fill=255)
        img.putalpha(mask)

    out = img.resize((size, size), Image.LANCZOS)
    if opaque:
        rgb = Image.new("RGB", out.size, BG)
        rgb.paste(out, mask=out.split()[3])
        return rgb
    return out


def main():
    os.makedirs(HERE, exist_ok=True)

    render(1024).save(os.path.join(HERE, "vesper_1024.png"))
    render(256).save(os.path.join(HERE, "vesper_icon_256.png"))
    print("branding masters written")

    android_res = os.path.join(MOBILE, "android", "app", "src", "main", "res")
    for dpi, px in [("mdpi", 48), ("hdpi", 72), ("xhdpi", 96),
                    ("xxhdpi", 144), ("xxxhdpi", 192)]:
        p = os.path.join(android_res, f"mipmap-{dpi}", "ic_launcher.png")
        if os.path.isdir(os.path.dirname(p)):
            render(px).save(p)
            print("android", dpi, px)

    iconset = os.path.join(MOBILE, "ios", "Runner", "Assets.xcassets",
                           "AppIcon.appiconset")
    cj = os.path.join(iconset, "Contents.json")
    if os.path.exists(cj):
        with open(cj, encoding="utf-8") as fh:
            spec = json.load(fh)
        for entry in spec.get("images", []):
            fn = entry.get("filename")
            if not fn:
                continue
            px = round(float(entry.get("size", "0x0").split("x")[0])
                       * float(entry.get("scale", "1x").rstrip("x")))
            render(px, opaque=True).save(os.path.join(iconset, fn))
            print("ios", fn, px)

    print("done")


if __name__ == "__main__":
    main()
