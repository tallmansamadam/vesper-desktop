"""
scenes.py - Scene presets for LIFX lights.

A "scene" is just data: edit, add, or remove entries in SCENES freely.
Each scene applies a coordinated look across ALL discovered lights and turns
them on. Three kinds:

    "white"   - same colour temperature on every light
    "color"   - same single RGB colour on every light
    "palette" - cycle a list of RGB colours across the lights, so each bulb
                gets a different (but coordinated) colour for a layered look

Fields:
    name    : button label
    kind    : "white" | "color" | "palette"
    pct     : brightness 0..100
    kelvin  : (white only) colour temperature 2500..9000
    rgb     : (color only) (r, g, b) 0..255
    colors  : (palette only) list of (r, g, b)
    art     : OPTIONAL explicit gradient stops for the tile artwork; if omitted
              it is derived from the scene's colours by tile_stops().

The GUI renders each scene as a gradient-art tile (see art.py + tile_stops).
"""

import random

from lifx_lan import rgb_to_hsbk

# Approximate RGB tints for white colour temperatures, used for tile art.
_WHITE_TINTS = {
    2700: (255, 197, 143),
    3200: (255, 214, 170),
    4800: (255, 241, 224),
    6500: (210, 230, 255),
    8000: (205, 226, 255),
}

SCENES = [
    # --- whites -------------------------------------------------------------
    {"name": "Relax",      "kind": "white", "pct": 28,  "kelvin": 2700},
    {"name": "Reading",    "kind": "white", "pct": 70,  "kelvin": 3200},
    {"name": "Focus",      "kind": "white", "pct": 100, "kelvin": 4800},
    {"name": "Energize",   "kind": "white", "pct": 100, "kelvin": 6500},

    # --- warm single-colour moods ------------------------------------------
    {"name": "Candlelight", "kind": "color", "pct": 18, "rgb": (255, 110, 20)},
    {"name": "Nightlight",  "kind": "color", "pct": 3,  "rgb": (255, 80, 10)},
    {"name": "Cinema",      "kind": "color", "pct": 12, "rgb": (45, 0, 95)},

    # --- multi-colour palettes (each bulb a different hue) ------------------
    {"name": "Fireplace",  "kind": "palette", "pct": 35,
     "colors": [(255, 70, 0), (255, 120, 20), (190, 35, 0)]},
    {"name": "Sunset",     "kind": "palette", "pct": 55,
     "colors": [(255, 94, 0), (255, 40, 70), (150, 0, 130)]},
    {"name": "Golden Hour", "kind": "palette", "pct": 60,
     "colors": [(255, 175, 45), (255, 110, 30), (220, 60, 40)]},
    {"name": "Ocean",      "kind": "palette", "pct": 60,
     "colors": [(0, 80, 220), (0, 170, 190), (0, 210, 120)]},
    {"name": "Lagoon",     "kind": "palette", "pct": 60,
     "colors": [(0, 175, 150), (60, 195, 100), (20, 135, 120), (110, 180, 70)]},
    {"name": "Deep Sea",   "kind": "palette", "pct": 42,
     "colors": [(0, 40, 120), (0, 90, 150), (0, 140, 140)]},
    {"name": "Forest",     "kind": "palette", "pct": 52,
     "colors": [(30, 170, 40), (120, 190, 0), (0, 150, 95)]},
    {"name": "Aurora",     "kind": "palette", "pct": 65,
     "colors": [(0, 255, 150), (60, 120, 255), (170, 0, 255)]},
    {"name": "Galaxy",     "kind": "palette", "pct": 55,
     "colors": [(40, 0, 120), (120, 0, 200), (220, 0, 160)]},
    {"name": "Ultraviolet", "kind": "palette", "pct": 60,
     "colors": [(80, 0, 200), (150, 0, 255), (220, 40, 255)]},
    {"name": "Berry",      "kind": "palette", "pct": 60,
     "colors": [(200, 0, 90), (150, 0, 140), (90, 0, 160)]},
    {"name": "Neon",       "kind": "palette", "pct": 85,
     "colors": [(255, 0, 130), (0, 200, 255), (175, 0, 255)]},
    {"name": "Citrus",     "kind": "palette", "pct": 75,
     "colors": [(255, 200, 0), (255, 120, 0), (180, 220, 0)]},
    {"name": "Tropical",   "kind": "palette", "pct": 75,
     "colors": [(255, 90, 0), (255, 0, 120), (0, 200, 180)]},
    {"name": "Rainbow",    "kind": "palette", "pct": 80,
     "colors": [(255, 0, 0), (255, 140, 0), (255, 230, 0),
                (0, 200, 60), (0, 120, 255), (150, 0, 255)]},
    {"name": "Party",      "kind": "palette", "pct": 82,
     "colors": [(255, 0, 80), (0, 120, 255), (150, 0, 255),
                (0, 220, 80), (255, 150, 0)]},

    # --- expanded set -------------------------------------------------------
    {"name": "Sunrise",   "kind": "palette", "pct": 60,
     "colors": [(255, 160, 90), (255, 110, 150), (255, 205, 120)]},
    {"name": "Sakura",    "kind": "palette", "pct": 60,
     "colors": [(255, 170, 205), (255, 120, 175), (255, 205, 225)]},
    {"name": "Lavender",  "kind": "palette", "pct": 50,
     "colors": [(180, 150, 255), (205, 170, 240), (150, 115, 230)]},
    {"name": "Mint",      "kind": "palette", "pct": 60,
     "colors": [(120, 235, 200), (80, 220, 185), (170, 245, 215)]},
    {"name": "Storm",     "kind": "palette", "pct": 55,
     "colors": [(40, 60, 110), (70, 130, 210), (120, 205, 255)]},
    {"name": "Lava",      "kind": "palette", "pct": 55,
     "colors": [(255, 55, 0), (190, 15, 0), (255, 120, 25)]},
    {"name": "Moonlight", "kind": "white",   "pct": 28, "kelvin": 8000},
    {"name": "Twilight",  "kind": "palette", "pct": 50,
     "colors": [(60, 45, 135), (120, 70, 185), (45, 65, 150)]},
    {"name": "Autumn",    "kind": "palette", "pct": 55,
     "colors": [(220, 90, 20), (200, 40, 20), (240, 170, 40)]},
    {"name": "Snow",      "kind": "palette", "pct": 55,
     "colors": [(180, 210, 255), (140, 180, 235), (220, 235, 255)]},
]


# --------------------------------------------------------------------------
# Colour helpers for tile artwork
# --------------------------------------------------------------------------

def _mul(c, f):
    return tuple(max(0, min(255, int(ch * f))) for ch in c)


def _tint(c, f):
    return tuple(max(0, min(255, int(ch + (255 - ch) * f))) for ch in c)


def tile_stops(scene):
    """Return the list of RGB gradient stops used to render a scene's tile."""
    if "art" in scene:
        return scene["art"]
    if scene["kind"] == "palette":
        cols = scene["colors"]
        # Lead with a darkened first colour so the gradient has depth.
        return [_mul(cols[0], 0.55)] + list(cols)
    if scene["kind"] == "color":
        base = scene["rgb"]
    else:  # white
        base = _WHITE_TINTS.get(scene["kelvin"], (255, 230, 200))
    return [_mul(base, 0.45), base, _tint(base, 0.4)]


def swatch(scene):
    """A single representative colour for a scene (for fallbacks)."""
    stops = tile_stops(scene)
    return stops[len(stops) // 2]


# --------------------------------------------------------------------------
# Photo search queries (used by fetch_photos.py to pull tile imagery)
# --------------------------------------------------------------------------

# Topic terms for anime-category image search (see fetch_photos.py / Wallhaven).
QUERIES = {
    "Relax":       "lofi",
    "Reading":     "books room library",
    "Focus":       "studying",
    "Energize":    "office",
    "Candlelight": "candle fire dark",
    "Nightlight":  "starry night sky",
    "Cinema":      "purple dark",
    "Fireplace":   "fireplace",
    "Sunset":      "sunset scenery",
    "Golden Hour": "golden sunset field",
    "Ocean":       "underwater sunlight",
    "Lagoon":      "tropical water",
    "Deep Sea":    "underwater ocean blue",
    "Forest":      "forest green nature",
    "Aurora":      "aurora night sky",
    "Galaxy":      "galaxy stars space",
    "Ultraviolet": "purple neon glow",
    "Berry":       "pink flowers scenery",
    "Neon":        "neon city night",
    "Citrus":      "orange sunset sky",
    "Tropical":    "beach summer sunset",
    "Rainbow":     "rainbow colorful sky",
    "Party":       "fireworks festival night",
    "Sunrise":     "sunrise",
    "Sakura":      "sakura",
    "Lavender":    "lavender",
    "Mint":        "mint green",
    "Storm":       "lightning storm",
    "Lava":        "lava",
    "Moonlight":   "moonlight",
    "Twilight":    "dusk sky",
    "Autumn":      "autumn leaves",
    "Snow":        "snow winter",
}


def scene_query(scene):
    return QUERIES.get(scene["name"], scene["name"])


# --------------------------------------------------------------------------
# Audio ambience per scene (keys map to loops generated by ambience.py)
# --------------------------------------------------------------------------

AMBIENCE = {
    "Relax": "warm",         "Reading": "cafe",       "Focus": "rain",
    "Energize": "birds",     "Candlelight": "fire",   "Nightlight": "night",
    "Cinema": "drone",       "Fireplace": "fire",     "Sunset": "seagulls",
    "Golden Hour": "cicadas", "Ocean": "ocean",       "Lagoon": "stream",
    "Deep Sea": "underwater", "Forest": "birds",      "Aurora": "shimmer",
    "Galaxy": "deep_space",  "Ultraviolet": "drone",  "Berry": "chimes",
    "Neon": "city",          "Citrus": "cicadas",     "Tropical": "seagulls",
    "Rainbow": "club",       "Party": "crowd",        "Sunrise": "birds",
    "Sakura": "chimes",      "Lavender": "warm",      "Mint": "wind",
    "Storm": "storm",        "Lava": "fire",          "Moonlight": "night",
    "Twilight": "night",     "Autumn": "leaves",      "Snow": "wind",
}

# Synth simulacrum to use when a real recording (<key>.ogg) is unavailable.
FALLBACK = {
    "birds": "forest", "crowd": "club", "city": "rain", "cicadas": "night",
    "stream": "ocean", "seagulls": "ocean", "cafe": "warm", "chimes": "shimmer",
}


def scene_ambience(scene):
    return AMBIENCE.get(scene["name"], "warm")


# --------------------------------------------------------------------------
# Applying a scene to lights
# --------------------------------------------------------------------------

def _set_rgb_at(lan, light, r, g, b, pct, duration_ms):
    hue, sat, _bri, _k = rgb_to_hsbk(r, g, b)
    bri = int(max(0, min(100, pct)) / 100 * 65535)
    lan.set_color(light, hue, sat, bri, light.hsbk[3], duration_ms)


def _assignment(colors, n, shuffle):
    """Spread a palette across n lights, shuffled so re-applying re-mixes it."""
    out = []
    while len(out) < n:
        chunk = list(colors)
        if shuffle:
            random.shuffle(chunk)
        out.extend(chunk)
    return out[:n]


def apply_scene(lan, lights, scene, power_on=True, duration_ms=800, shuffle=True):
    """Apply a scene dict across the given lights (and turn them on).

    For palette scenes the colour-to-bulb assignment is shuffled on every call,
    so clicking the same scene again re-mixes which light shows which colour.
    """
    kind = scene["kind"]
    pct = scene.get("pct", 100)
    assignment = (_assignment(scene["colors"], len(lights), shuffle)
                  if kind == "palette" else None)
    for i, light in enumerate(lights):
        if kind == "white":
            lan.set_white(light, pct, scene["kelvin"], duration_ms)
        elif kind == "color":
            _set_rgb_at(lan, light, *scene["rgb"], pct, duration_ms)
        elif kind == "palette":
            _set_rgb_at(lan, light, *assignment[i], pct, duration_ms)
        if power_on:
            lan.set_power(light, True, duration_ms)
