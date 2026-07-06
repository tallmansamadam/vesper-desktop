"""
effects.py - Live light animations and screen-sync colour computation.

Each effect is a frame function: fn(lan, lights, t, state) -> None, called ~N
times/sec by the GUI's LightDriver thread. Colours are sent with
lan.stream_color (single-send, short transition) so the bulbs animate smoothly
without flooding the network. `state` is a per-run dict the driver seeds (e.g.
"base" = each bulb's colour when the effect started).
"""

import colorsys
import math
import random

from PIL import ImageGrab


def _hsbk(h, s, v, kelvin=3500):
    return (int(h * 65535) & 0xFFFF,
            int(max(0.0, min(1.0, s)) * 65535),
            int(max(0.0, min(1.0, v)) * 65535),
            kelvin)


# --------------------------------------------------------------------------
# Light effects
# --------------------------------------------------------------------------

def breathe(lan, lights, t, st):
    """Smoothly fade every bulb's brightness up and down around its colour."""
    m = 0.12 + 0.88 * (0.5 - 0.5 * math.cos(2 * math.pi * t / 6.0))
    for light in lights:
        h, s, b, k = st["base"][light.target]
        lan.stream_color(light, h, s, int(b * m), k, duration_ms=260)


def flow(lan, lights, t, st):
    """Rainbow hue that flows through the spectrum, offset per bulb."""
    n = max(1, len(lights))
    for i, light in enumerate(lights):
        hue = (t * 0.05 + i / n) % 1.0
        lan.stream_color(light, *_hsbk(hue, 1.0, 0.85), duration_ms=320)


def flicker(lan, lights, t, st):
    """Warm candle/fire flicker (re-rolls on a speed-scaled interval)."""
    if t - st.get("ft", -1.0) < 0.09:
        return                                  # gate by effect-time so SPEED applies
    st["ft"] = t
    for light in lights:
        v = random.uniform(0.45, 1.0)
        hue = random.uniform(0.03, 0.09)
        s = random.uniform(0.7, 0.95)
        lan.stream_color(light, *_hsbk(hue, s, v, 1800), duration_ms=110)


def strobe(lan, lights, t, st):
    """Hard on/off strobe on each bulb's current colour."""
    on = int(t * 8) % 2 == 0
    for light in lights:
        h, s, _b, k = st["base"][light.target]
        lan.stream_color(light, h, s, 65535 if on else 0, k, duration_ms=30)


EFFECTS = {"breathe": breathe, "flow": flow, "flicker": flicker, "strobe": strobe}
NEEDS_BASE = {"breathe", "strobe"}


# --------------------------------------------------------------------------
# Screen sync
# --------------------------------------------------------------------------

def sample_screen():
    """Dominant vivid (hue, sat, val) of the current screen contents."""
    img = ImageGrab.grab().convert("RGB").resize((48, 27))
    data = img.tobytes()
    xs = ys = ss = vs = ws = 0.0
    for i in range(0, len(data), 3):
        h, s, v = colorsys.rgb_to_hsv(data[i] / 255, data[i + 1] / 255, data[i + 2] / 255)
        w = s * v + 0.02                       # weight toward saturated pixels
        a = 2 * math.pi * h
        xs += math.cos(a) * w
        ys += math.sin(a) * w
        ss += s * w
        vs += v * w
        ws += w
    if ws <= 0:
        return 0.0, 0.0, 0.0
    hue = (math.atan2(ys, xs) / (2 * math.pi)) % 1.0
    return hue, ss / ws, vs / ws


def _push(lan, lights, hue, sat, val, st):
    key = (round(hue, 2), round(sat, 2), round(val, 2))
    if key == st.get("last"):
        return                                  # skip tiny changes, save traffic
    st["last"] = key
    h, s, b, k = _hsbk(hue, sat, val)
    for light in lights:
        lan.stream_color(light, h, s, b, k, duration_ms=350)


def screen_complement(lan, lights, t, st):
    """Bulbs show the opposite hue to the screen, for contrast."""
    hue, sat, val = sample_screen()
    hue = (hue + 0.5) % 1.0
    _push(lan, lights, hue,
          min(1.0, sat * 1.25 + 0.2),
          max(0.25, min(1.0, val * 1.1 + 0.1)), st)


def screen_augment(lan, lights, t, st):
    """Bulbs push the screen's dominant colour further - boosted vibrancy."""
    hue, sat, val = sample_screen()
    _push(lan, lights, hue,
          min(1.0, sat * 1.7 + 0.15),
          max(0.3, min(1.0, val * 1.15 + 0.15)), st)


SYNC = {"complement": screen_complement, "augment": screen_augment}
