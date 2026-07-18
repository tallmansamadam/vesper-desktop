"""
gen_anim.py - Animate the Vesper mark: the dial sweeps up, its arc filling
with spectrum colour as the pointer travels, the core igniting, and the whole
space around it flooding with radiant light in the current hue.

Renders a looping GIF (sweep up -> hold -> sweep back) plus a filmstrip
contact sheet for review. Shares its geometry and light model with
gen_logo.py, so the animation and the icon are literally the same mark.

Run:  python branding/gen_anim.py
"""

import math
import os

from PIL import Image, ImageChops, ImageDraw, ImageFilter

import gen_logo as G

HERE = os.path.dirname(os.path.abspath(__file__))
SS = 2                              # supersample (lighter than the icon's 4x)


def _ease_out(p):
    return 1 - (1 - p) ** 3


def _ease_in_out(p):
    return 3 * p * p - 2 * p * p * p


def frame(size, p):
    """One frame of the sweep. p in 0..1 = dial position (0 = off, 1 = full)."""
    s = size * SS
    cx = cy = s / 2
    R = s * 0.335
    arc_w = int(s * 0.058)
    bbox = [cx - R, cy - R, cx + R, cy + R]
    face_r = s * 0.262
    swept = G.ARC_SWEEP * p
    hue_now = G._hue_at(p)

    # --- the space around the dial, flooded with the current hue ----------
    body = Image.new("RGB", (s, s), G.BG)
    if p > 0.01:
        space = Image.new("RGB", (s, s), (0, 0, 0))
        sd = ImageDraw.Draw(space)
        # a wide, soft wash of the live colour reaching the frame edges
        steps = 26
        rad = s * (0.30 + 0.34 * p)
        for i in range(steps, 0, -1):
            f = (1 - i / steps) ** 2.0 * p
            r = rad * i / steps
            sd.ellipse([cx - r, cy - r, cx + r, cy + r],
                       fill=(int(hue_now[0] * f), int(hue_now[1] * f),
                             int(hue_now[2] * f)))
        space = G._dim(space.filter(ImageFilter.GaussianBlur(radius=s * 0.06)),
                       0.85)
        body = ImageChops.screen(body, space)

    ImageDraw.Draw(body).ellipse(
        [cx - face_r, cy - face_r, cx + face_r, cy + face_r], fill=G.FACE)

    layers = []

    # --- rays + halo, revealed only as far as the pointer has travelled ---
    if p > 0.02:
        rays = Image.new("RGB", (s, s), (0, 0, 0))
        rd = ImageDraw.Draw(rays)
        count = max(2, int(96 * p))
        for i in range(count):
            t = (i / max(1, count - 1)) * p
            a = math.radians(G.ARC_START + G.ARC_SWEEP * t)
            rd.line([(cx + R * 0.52 * math.cos(a), cy + R * 0.52 * math.sin(a)),
                     (cx + R * 0.95 * math.cos(a), cy + R * 0.95 * math.sin(a))],
                    fill=G._hue_at(t), width=int(s * 0.016))
        layers.append(G._dim(
            rays.filter(ImageFilter.GaussianBlur(radius=s * 0.03)), 0.75))

        halo = Image.new("RGB", (s, s), (0, 0, 0))
        hd = ImageDraw.Draw(halo)
        steps = max(2, int(260 * p))
        for i in range(steps):
            t = (i / max(1, steps - 1)) * p
            a0 = G.ARC_START + swept * i / steps
            a1 = G.ARC_START + swept * (i + 1) / steps + 1.2
            hd.arc(bbox, start=a0, end=a1, fill=G._hue_at(t),
                   width=int(arc_w * 1.9))
        layers.append(halo.filter(ImageFilter.GaussianBlur(radius=s * 0.042)))

    # --- core ignites as the dial rises -----------------------------------
    k = _ease_out(min(1.0, p * 1.15))
    layers.append(G._dim(
        G._radial(s, cx, cy, s * 0.20, (255, 176, 80), power=2.4), 0.25 + 0.75 * k))
    layers.append(G._dim(
        G._radial(s, cx, cy, s * 0.105, (255, 236, 190), power=1.1), k))
    layers.append(G._dim(
        G._radial(s, cx, cy, s * 0.055, (255, 255, 255), power=0.4), k))

    lit = body
    for lay in layers:
        lit = ImageChops.screen(lit, lay)
    img = lit.convert("RGBA")

    # --- crisp arc, filled to the pointer ---------------------------------
    if p > 0.02:
        arc = Image.new("RGBA", (s, s), (0, 0, 0, 0))
        ad = ImageDraw.Draw(arc)
        steps = max(2, int(260 * p))
        for i in range(steps):
            t = (i / max(1, steps - 1)) * p
            a0 = G.ARC_START + swept * i / steps
            a1 = G.ARC_START + swept * (i + 1) / steps + 1.2
            ad.arc(bbox, start=a0, end=a1, fill=G._hue_at(t), width=arc_w)
        rr = R - arc_w / 2
        for ang, t in ((G.ARC_START, 0.0), (G.ARC_START + swept, p)):
            ex = cx + rr * math.cos(math.radians(ang))
            ey = cy + rr * math.sin(math.radians(ang))
            ad.ellipse([ex - arc_w / 2, ey - arc_w / 2,
                        ex + arc_w / 2, ey + arc_w / 2],
                       fill=G._hue_at(t) + (255,))
        img = Image.alpha_composite(img, arc)

    # --- the nib: colour is deposited where the pointer meets the ring -----
    # Drawn additively, under the pointer, so the band reads as being laid
    # down by the tip rather than merely appearing beside it.
    a = math.radians(G.ARC_START + swept)
    if p > 0.01:
        nx, ny = cx + R * math.cos(a), cy + R * math.sin(a)
        spark = Image.new("RGB", (s, s), (0, 0, 0))
        for rad, col, dim in ((s * 0.085, hue_now, 0.85),
                              (s * 0.030, (255, 255, 255), 0.95)):
            spark = ImageChops.screen(
                spark, G._dim(G._radial(s, nx, ny, rad, col, power=1.2), dim))
        img = Image.alpha_composite(
            ImageChops.screen(img.convert("RGB"), spark).convert("RGBA"),
            Image.new("RGBA", (s, s), (0, 0, 0, 0)))

    # --- pointer rides the sweep, its tip riding the band it draws --------
    d = ImageDraw.Draw(img)
    p1 = (cx + s * 0.145 * math.cos(a), cy + s * 0.145 * math.sin(a))
    p2 = (cx + s * 0.302 * math.cos(a), cy + s * 0.302 * math.sin(a))
    pw = int(s * 0.034)
    d.line([p1, p2], fill=(255, 255, 255, 255), width=pw)
    for pt in (p1, p2):
        d.ellipse([pt[0] - pw / 2, pt[1] - pw / 2,
                   pt[0] + pw / 2, pt[1] + pw / 2], fill=(255, 255, 255, 255))

    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, s - 1, s - 1],
                                           radius=int(s * 0.225), fill=255)
    img.putalpha(mask)
    return img.resize((size, size), Image.LANCZOS)


def main():
    size = 240
    up, hold, down = 26, 8, 14
    ps = ([_ease_out(i / (up - 1)) for i in range(up)]
          + [1.0] * hold
          + [1.0 - _ease_in_out(i / (down - 1)) for i in range(down)])

    frames = []
    for i, p in enumerate(ps):
        f = frame(size, p)
        flat = Image.new("RGB", f.size, (0, 0, 0))
        flat.paste(f, (0, 0), f)
        frames.append(flat)
        if i % 12 == 0:
            print(f"  frame {i+1}/{len(ps)}")

    gif = os.path.join(HERE, "vesper_sweep.gif")
    frames[0].save(gif, save_all=True, append_images=frames[1:],
                   duration=45, loop=0, optimize=True)
    print("wrote", gif, f"({len(frames)} frames)")

    # filmstrip for review
    picks = [0, 6, 12, 18, 25, 33]
    strip = Image.new("RGB", (size * len(picks) + 10 * (len(picks) + 1),
                              size + 20), (30, 30, 34))
    for i, idx in enumerate(picks):
        strip.paste(frames[min(idx, len(frames) - 1)],
                    (10 + i * (size + 10), 10))
    sheet = os.path.join(HERE, "_filmstrip.png")
    strip.save(sheet)
    print("wrote", sheet)


if __name__ == "__main__":
    main()
