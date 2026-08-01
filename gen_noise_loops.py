"""
gen_noise_loops.py - Render the synthesised noise layers to seamless loops for
the mobile app.

The desktop generates White/Pink/Brown/Surf/Rain/Wind/Waterfall in real time
(native/src/noise.rs). Dart can't run that DSP continuously at low latency, so
mobile plays looping files instead - and these are those files, produced by the
same algorithms so the two apps sound alike.

Seamlessness matters more here than on desktop: a click at the loop point is
exactly what you'd notice while falling asleep. Each layer is rendered a
crossfade longer than its final length, then the tail is mixed back over the
head, so the join is continuous.

Output: ../lifx_mobile/assets/audio/<layer>.ogg   (needs ffmpeg)
Run:    python gen_noise_loops.py
"""

import math
import os
import shutil
import random
import struct
import subprocess
import sys
import wave

SR = 44100
SECONDS = 30.0          # loop length after the crossfade is folded in
CROSSFADE = 1.5         # seconds of overlap

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.join(HERE, "..", "lifx_mobile", "assets", "audio")
TAU = math.tau

def _ffmpeg():
    """ffmpeg from PATH, else the winget install location."""
    if shutil.which("ffmpeg"):
        return "ffmpeg"
    winget = os.path.expandvars(
        r"%LOCALAPPDATA%\Microsoft\WinGet\Packages"
        r"\Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe"
        r"\ffmpeg-8.1.1-full_build\bin\ffmpeg.exe")
    return winget if os.path.isfile(winget) else "ffmpeg"


FFMPEG = _ffmpeg()


def pink_step(b, w):
    """Paul Kellet's refined pink filter - same coefficients as noise.rs."""
    b[0] = 0.99886 * b[0] + w * 0.0555179
    b[1] = 0.99332 * b[1] + w * 0.0750759
    b[2] = 0.96900 * b[2] + w * 0.1538520
    b[3] = 0.86650 * b[3] + w * 0.3104856
    b[4] = 0.55000 * b[4] + w * 0.5329522
    b[5] = -0.7616 * b[5] - w * 0.0168980
    out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362
    b[6] = w * 0.115926
    return out * 0.11


def render(layer, n):
    """One layer, n samples, mirroring native/src/noise.rs."""
    rng = random.Random(1234)  # fixed seed, so rebuilds are reproducible
    out = [0.0] * n
    pink = [0.0] * 7
    brown = 0.0
    surf_pink = [0.0] * 7
    surf_phase = 0.0
    rain_prev = 0.0
    wind_pink = [0.0] * 7
    wind_lp = 0.0
    wind_cut = 0.0
    wind_gust = 0.0
    wf_pink = [0.0] * 7
    wf_low = 0.0
    wf_phase = 0.0

    for i in range(n):
        w = rng.uniform(-1.0, 1.0)
        if layer == "white":
            v = w * 0.35
        elif layer == "pink":
            v = pink_step(pink, w)
        elif layer == "brown":
            # leaky integrator, or it walks to the rails and clips
            brown = 0.995 * brown + 0.02 * w
            v = brown * 2.2
        elif layer == "surf":
            sp = pink_step(surf_pink, w)
            surf_phase = (surf_phase + TAU * 0.09 / SR) % TAU
            v = sp * (0.5 + 0.5 * math.sin(surf_phase)) * 1.3
        elif layer == "rain":
            v = (w - rain_prev) * 0.5
            rain_prev = w
        elif layer == "wind":
            wp = pink_step(wind_pink, w)
            wind_cut = (wind_cut + TAU * 0.05 / SR) % TAU
            cut = 0.03 + 0.05 * (0.5 + 0.5 * math.sin(wind_cut))
            wind_lp += cut * (wp - wind_lp)
            wind_gust = (wind_gust + TAU * 0.08 / SR) % TAU
            v = wind_lp * (0.35 + 0.65 * (0.5 + 0.5 * math.sin(wind_gust))) * 4.0
        elif layer == "waterfall":
            wp = pink_step(wf_pink, w)
            wf_low += 0.010 * (w - wf_low)
            wf_phase = (wf_phase + TAU * 0.13 / SR) % TAU
            v = (0.5 * w + 1.5 * wp + wf_low * 6.0) * (0.8 + 0.2 * math.sin(wf_phase)) * 0.9
        else:
            raise ValueError(layer)
        out[i] = math.tanh(v * 0.9)
    return out


def seamless(samples, fade_n):
    """Fold the tail back over the head so the loop point is continuous."""
    body = samples[:-fade_n]
    tail = samples[-fade_n:]
    for i in range(fade_n):
        t = i / fade_n
        body[i] = body[i] * t + tail[i] * (1.0 - t)
    return body


def write_wav(path, samples):
    peak = max(1e-9, max(abs(s) for s in samples))
    gain = 0.89 / peak                      # normalise, leaving headroom
    with wave.open(path, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(SR)
        f.writeframes(b"".join(
            struct.pack("<h", int(max(-1.0, min(1.0, s * gain)) * 32767))
            for s in samples))


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    fade_n = int(CROSSFADE * SR)
    total = int(SECONDS * SR) + fade_n
    for layer in ("white", "pink", "brown", "surf", "rain", "wind", "waterfall"):
        raw = render(layer, total)
        loop = seamless(raw, fade_n)
        # nz_ prefix: the ambience set already has rain/wind of its own, and
        # these are a different synthesis serving a different purpose.
        wav = os.path.join(OUT_DIR, "nz_" + layer + ".wav")
        ogg = os.path.join(OUT_DIR, "nz_" + layer + ".ogg")
        write_wav(wav, loop)
        subprocess.run([FFMPEG, "-y", "-loglevel", "error", "-i", wav,
                        "-c:a", "libvorbis", "-q:a", "4", ogg], check=True)
        os.remove(wav)
        print(f"  {layer:10s} {os.path.getsize(ogg) // 1024:4d}K")
    print("done ->", os.path.abspath(OUT_DIR))


if __name__ == "__main__":
    sys.exit(main())
