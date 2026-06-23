"""
fetch_photos.py - Download one topical anime image per scene and process it into
a tile-sized PNG for the GUI.

Source: the Wallhaven API (https://wallhaven.cc) restricted to the *anime*
category and *SFW* purity, no API key required. Lightweight thumbnails are used
(plenty for a 152x84 tile). These are anime wallpapers/fan art - copyrighted,
fine for personal use in a local app, not for redistribution. Each image is
cover-cropped to the tile aspect, colour-boosted a touch, and darkened along the
bottom so the scene name stays legible.

Run once:   python fetch_photos.py
Re-running only fetches the tiles that are still missing (so it's cheap to
resume). Needs Pillow + internet. The GUI uses images/<scene>.png when present
and otherwise falls back to procedurally generated gradient art (art.py).
"""

import io
import json
import os
import random
import re
import time
import urllib.parse
import urllib.request

from PIL import Image, ImageEnhance

from scenes import SCENES, scene_query

W, H = 152, 84
OUT_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "images")
UA = {"User-Agent": "lifx-control/1.0 (personal use)"}

try:
    RESAMPLE = Image.Resampling.LANCZOS
except AttributeError:                       # older Pillow
    RESAMPLE = Image.LANCZOS


def safe_name(name):
    return re.sub(r"[^a-z0-9]+", "_", name.lower()).strip("_")


def search(query, n=12):
    """Return candidate image URLs from an anime-category, SFW Wallhaven search."""
    url = "https://wallhaven.cc/api/v1/search?" + urllib.parse.urlencode(
        {"q": query, "categories": "010", "purity": "100",
         "sorting": "relevance", "atleast": "640x360"})
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=15) as r:
        data = json.loads(r.read()).get("data", [])
    urls = []
    for it in data[:n]:
        thumbs = it.get("thumbs", {})
        urls.append(thumbs.get("large") or thumbs.get("original") or it.get("path"))
    return [u for u in urls if u]


def download(url):
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=20) as r:
        return r.read()


def process(data):
    """Cover-crop to W:H, boost colour, darken the bottom half for legibility."""
    im = Image.open(io.BytesIO(data)).convert("RGB")
    sw, sh = im.size
    scale = max(W / sw, H / sh)
    nw, nh = int(sw * scale + 0.5), int(sh * scale + 0.5)
    im = im.resize((nw, nh), RESAMPLE)
    left, top = (nw - W) // 2, (nh - H) // 2
    im = im.crop((left, top, left + W, top + H))
    im = ImageEnhance.Color(im).enhance(1.18)
    im = ImageEnhance.Contrast(im).enhance(1.05)

    px = im.load()
    cutoff = H * 0.45
    for y in range(int(cutoff), H):
        f = 1.0 - 0.62 * ((y - cutoff) / (H - cutoff))
        for x in range(W):
            r, g, b = px[x, y]
            px[x, y] = (int(r * f), int(g * f), int(b * f))
    return im


def fetch_scene(scene, randomize=False):
    """Fetch + process + save one scene's image, overwriting any existing file.

    Returns the saved path on success or None. With randomize=True the search
    candidates are shuffled, so re-rolling tends to yield a different image.
    Used by the GUI's right-click re-roll (imported lazily so Pillow isn't a
    runtime dependency unless re-roll is actually used).
    """
    os.makedirs(OUT_DIR, exist_ok=True)
    path = os.path.join(OUT_DIR, safe_name(scene["name"]) + ".png")
    urls = search(scene_query(scene))
    if randomize:
        random.shuffle(urls)
    for url in urls:
        try:
            process(download(url)).save(path)
            return path
        except Exception:
            continue
    return None


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    have, missing = 0, []
    for sc in SCENES:
        path = os.path.join(OUT_DIR, safe_name(sc["name"]) + ".png")
        if os.path.exists(path):
            have += 1
            print(f"skip  {sc['name']}")
            continue
        query = scene_query(sc)
        saved = False
        try:
            for url in search(query):
                try:
                    process(download(url)).save(path)
                    print(f"OK    {sc['name']:<12} <- {query}")
                    have += 1
                    saved = True
                    break
                except Exception:
                    continue       # try the next candidate
        except Exception as e:
            print(f"ERR   {sc['name']}: {e!r}")
        if not saved:
            print(f"MISS  {sc['name']}")
            missing.append(sc["name"])
        time.sleep(0.4)            # be polite to the API

    print(f"\nDone: {have}/{len(SCENES)} scenes have photos.")
    if missing:
        print("Missing (will use gradient art):", ", ".join(missing))


if __name__ == "__main__":
    main()
