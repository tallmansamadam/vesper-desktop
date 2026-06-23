"""
art.py - Procedural gradient artwork, generated with the Python stdlib only.

Tk's PhotoImage can decode PNG from a base64 string (Tk 8.6+), so we synthesize
a PNG entirely in Python (struct + zlib) and hand Tk the base64. No Pillow, no
downloads, no image files on disk - each scene's "picture" is a multi-stop
diagonal gradient built from that scene's own colour palette.
"""

import base64
import math
import struct
import zlib


def _lerp(a, b, t):
    return (a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t)


def _stop_color(stops, t):
    """Sample a list of RGB stops at position t in 0..1."""
    if t <= 0:
        return stops[0]
    if t >= 1:
        return stops[-1]
    n = len(stops) - 1
    s = t * n
    i = int(s)
    if i >= n:
        i = n - 1
    return _lerp(stops[i], stops[i + 1], s - i)


def _encode_png(width, height, raw_with_filters):
    def chunk(typ, data):
        body = typ + data
        return (struct.pack(">I", len(data)) + body
                + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF))

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)  # RGB, 8-bit
    idat = zlib.compress(raw_with_filters, 9)
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def gradient_png_b64(width, height, stops, angle=55,
                     vignette=0.16, bottom_shade=0.55):
    """Render a diagonal multi-stop gradient and return base64 PNG text.

    angle        : gradient direction in degrees (0 = horizontal).
    vignette     : 0..1, darkens the corners for depth.
    bottom_shade : 0..1, darkens the lower part so overlaid text stays legible.
    """
    if len(stops) == 1:
        stops = [stops[0], stops[0]]
    rad = math.radians(angle)
    dx, dy = math.cos(rad), math.sin(rad)
    projs = [0.0, (width - 1) * dx, (height - 1) * dy,
             (width - 1) * dx + (height - 1) * dy]
    pmin = min(projs)
    span = (max(projs) - pmin) or 1.0

    lut = [_stop_color(stops, i / 255.0) for i in range(256)]
    cx, cy = (width - 1) / 2.0, (height - 1) / 2.0
    maxd = math.hypot(cx, cy) or 1.0

    raw = bytearray()
    append = raw.append
    for y in range(height):
        append(0)                       # PNG row filter type 0 (none)
        fy = y / (height - 1) if height > 1 else 0.0
        shade = 1.0 - bottom_shade * ((fy - 0.5) / 0.5) if fy > 0.5 else 1.0
        ydy = y * dy
        for x in range(width):
            t = ((x * dx + ydy) - pmin) / span
            ti = int(t * 255)
            ti = 0 if ti < 0 else 255 if ti > 255 else ti
            r, g, b = lut[ti]
            if vignette:
                d = math.hypot(x - cx, y - cy) / maxd
                f = shade * (1.0 - vignette * d * d)
            else:
                f = shade
            append(int(r * f) & 0xFF)
            append(int(g * f) & 0xFF)
            append(int(b * f) & 0xFF)

    return base64.b64encode(_encode_png(width, height, bytes(raw))).decode("ascii")
