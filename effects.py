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

# Every effect works from st["base"] - each bulb's colour/brightness when the
# effect started (i.e. the presently selected scene) - and treats that
# brightness as the ceiling. Effects never impose their own palette.

def breathe(lan, lights, t, st):
    """Fade brightness up to each bulb's set level and back down, keeping colour."""
    m = 0.12 + 0.88 * (0.5 - 0.5 * math.cos(2 * math.pi * t / 6.0))
    for light in lights:
        h, s, b, k = st["base"][light.target]
        lan.stream_color(light, h, s, int(b * m), k, duration_ms=260)


def _lerp_hsbk(a, b, f):
    dh = ((b[0] - a[0] + 32768) % 65536) - 32768        # shortest hue path
    hue = (a[0] + int(dh * f)) & 0xFFFF
    return (hue, int(a[1] + (b[1] - a[1]) * f),
            int(a[2] + (b[2] - a[2]) * f), int(a[3] + (b[3] - a[3]) * f))


def flow(lan, lights, t, st):
    """Flow the scene's own colours across the bulbs (no full-wheel cycling).

    Each bulb eases between the colours currently present across the lights, so
    the effect stays within the selected scene/theme instead of the rainbow.
    """
    palette = [st["base"][light.target] for light in lights]
    n = len(palette)
    if n == 0:
        return
    for i, light in enumerate(lights):
        p = t * 0.2 + i
        i0 = int(p) % n
        c = _lerp_hsbk(palette[i0], palette[(i0 + 1) % n], p - int(p))
        lan.stream_color(light, c[0], c[1], c[2], c[3], duration_ms=300)


def flicker(lan, lights, t, st):
    """Flicker each bulb's current colour (brightness never above its set level)."""
    if t - st.get("ft", -1.0) < 0.09:
        return                                  # gate by effect-time so SPEED applies
    st["ft"] = t
    for light in lights:
        h, s, b, k = st["base"][light.target]
        lan.stream_color(light, h, s, int(b * random.uniform(0.45, 1.0)), k,
                         duration_ms=110)


def strobe(lan, lights, t, st):
    """Hard on/off strobe on each bulb's current colour and brightness."""
    on = int(t * 8) % 2 == 0
    for light in lights:
        h, s, b, k = st["base"][light.target]
        lan.stream_color(light, h, s, b if on else 0, k, duration_ms=30)


EFFECTS = {"breathe": breathe, "flow": flow, "flicker": flicker, "strobe": strobe}
NEEDS_BASE = set(EFFECTS)                        # all effects use current colours


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


def _push(lan, lights, hue, sat, val, st, ease=0.4):
    # Ease the target toward the sampled colour (exponential smoothing) so screen
    # motion flows instead of jumping. `ease` per frame: higher = snappier/less
    # lag, lower = smoother. Hue eases along the shortest path around the wheel.
    ph, ps, pv = st.get("h"), st.get("s"), st.get("v")
    if ph is None:
        sh, ss, sv = hue, sat, val
    else:
        dh = ((hue - ph + 0.5) % 1.0) - 0.5
        sh = (ph + dh * ease) % 1.0
        ss = ps + (sat - ps) * ease
        sv = pv + (val - pv) * ease
    st["h"], st["s"], st["v"] = sh, ss, sv
    key = (round(sh, 3), round(ss, 3), round(sv, 3))
    if key == st.get("last"):
        return                                  # settled - nothing to send
    st["last"] = key
    h, s, b, k = _hsbk(sh, ss, sv)
    for light in lights:
        lan.stream_color(light, h, s, b, k, duration_ms=220)


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
