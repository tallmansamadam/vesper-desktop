"""
fetch_audio.py - Download real ambient field recordings and trim them into short
looping OGG clips for the GUI's scene audio.

Source: BBC Sound Effects (https://sound-effects.bbcrewind.co.uk) - RemArc
licence, free for personal/educational/research use, no API key. Each recording
is trimmed to ~30s with ffmpeg, loudness-normalised, and saved as
audio/<key>.ogg. The GUI prefers <key>.ogg over a synthesised <key>.wav and
falls back to a synth simulacrum (scenes.FALLBACK) when a download is missing.

Run once:  python fetch_audio.py          (skips existing)
           python fetch_audio.py --force
Needs ffmpeg on PATH + internet.
"""

import json
import os
import subprocess
import sys
import tempfile
import urllib.request

AUDIO_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "audio")
UA = {"User-Agent": "lifx-control/1.0 (personal use)"}
SEARCH = "https://sound-effects-api.bbcrewind.co.uk/api/sfx/search"
MEDIA = "https://sound-effects-media.bbcrewind.co.uk/mp3/{}.mp3"

REAL = {
    "birds":    "forest birds dawn chorus",
    "crowd":    "crowd applause cheering",
    "city":     "city street traffic",
    "cicadas":  "cicadas",
    "stream":   "stream brook babbling water",
    "seagulls": "seagulls beach",
    "cafe":     "restaurant cafe ambience",
    "chimes":   "wind chimes",
}


def search(query, n=12):
    body = json.dumps({"criteria": {"from": 0, "size": n, "query": query}}).encode()
    req = urllib.request.Request(
        SEARCH, data=body, headers={**UA, "Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=20).read()).get("results", [])


def download(url):
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read()


def make_clip(mp3_bytes, out_path, start):
    with tempfile.NamedTemporaryFile(suffix=".mp3", delete=False) as tf:
        tf.write(mp3_bytes)
        tmp = tf.name
    try:
        cmd = ["ffmpeg", "-y", "-ss", str(start), "-t", "30", "-i", tmp,
               "-af", "loudnorm=I=-18:TP=-2.0", "-ac", "1", "-ar", "44100",
               "-c:a", "libvorbis", "-q:a", "4", out_path]
        r = subprocess.run(cmd, capture_output=True)
        return r.returncode == 0 and os.path.exists(out_path)
    finally:
        try:
            os.remove(tmp)
        except OSError:
            pass


def main(force=False):
    os.makedirs(AUDIO_DIR, exist_ok=True)
    have, missing = 0, []
    for key, query in REAL.items():
        path = os.path.join(AUDIO_DIR, key + ".ogg")
        if os.path.exists(path) and not force:
            have += 1
            print("skip ", key)
            continue
        saved = False
        try:
            for r in search(query):
                dur = r.get("duration") or 0          # milliseconds
                if not (20000 <= dur <= 1200000):
                    continue
                start = 10 if dur > 40000 else 2
                try:
                    data = download(MEDIA.format(r["id"]))
                    if make_clip(data, path, start):
                        print(f"OK    {key:9} <- {query}  (id {r['id']})")
                        have += 1
                        saved = True
                        break
                except Exception:
                    continue
        except Exception as e:
            print(f"ERR   {key}: {e!r}")
        if not saved:
            print(f"MISS  {key}")
            missing.append(key)

    print(f"\nDone: {have}/{len(REAL)} real recordings.")
    if missing:
        print("Missing (will use synth simulacra):", ", ".join(missing))


if __name__ == "__main__":
    main(force="--force" in sys.argv)
