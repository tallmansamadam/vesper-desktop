"""
ambience.py - Procedurally synthesized ambient loops (Python stdlib only).

Generates seamless ~12s WAV loops into audio/, one per ambience key, built from
noise and oscillators. No external audio libraries, no downloads, no licensing
concerns. The GUI plays the matching loop for a scene via winsound.

Build/refresh:  python ambience.py            (skips existing)
                python ambience.py --force     (rebuild all)
"""

import array
import math
import os
import random
import sys
import wave

SR = 22050
DUR = 12.0
N = int(SR * DUR)
AUDIO_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "audio")


# --------------------------------------------------------------------------
# Building blocks
# --------------------------------------------------------------------------

def _white(n=N):
    return [random.uniform(-1, 1) for _ in range(n)]


def _lowpass(x, a):
    y = [0.0] * len(x)
    p = 0.0
    for i, v in enumerate(x):
        p += a * (v - p)
        y[i] = p
    return y


def _highpass(x, a):
    lp = _lowpass(x, a)
    return [x[i] - lp[i] for i in range(len(x))]


def _brown(n=N, leak=0.997, step=0.04):
    y = [0.0] * n
    p = 0.0
    for i in range(n):
        p = p * leak + random.uniform(-1, 1) * step
        y[i] = p
    return y


def _normalize(x, peak=0.85):
    m = max(1e-9, max(abs(v) for v in x))
    g = peak / m
    return [v * g for v in x]


def _loopify(x, fade=0.5):
    """Crossfade the tail into the head so the WAV loops seamlessly."""
    nf = int(fade * SR)
    n = len(x)
    for i in range(nf):
        w = i / nf
        x[i] = x[i] * w + x[n - nf + i] * (1 - w)
    return x[:n - nf]


# --------------------------------------------------------------------------
# Generators (each returns a list of N floats)
# --------------------------------------------------------------------------

def gen_ocean():
    b = _lowpass(_brown(), 0.05)
    hiss = _highpass(_white(), 0.5)
    out = [0.0] * N
    for i in range(N):
        lfo = 0.5 + 0.5 * math.sin(2 * math.pi * 0.09 * i / SR)
        out[i] = b[i] * (0.35 + 0.65 * lfo) + hiss[i] * 0.12 * max(0.0, lfo - 0.55) * 2.2
    return out


def gen_underwater():
    """Very deep submerged: an overwhelming all-pad wall. No drips, no noise."""
    out = [0.0] * N
    freqs = [32.7, 49.0, 65.4, 82.4, 98.0, 130.8]   # low C chord cluster
    voices = 3
    for i in range(N):
        t = i / SR
        s = 0.0
        for fi, f in enumerate(freqs):
            for vv in range(voices):
                det = 1 + 0.0035 * math.sin(2 * math.pi * (0.018 + 0.006 * fi + 0.004 * vv) * t + fi * 1.3 + vv)
                amp = 0.6 + 0.4 * math.sin(2 * math.pi * (0.02 + 0.013 * fi) * t + vv)
                s += amp * math.sin(2 * math.pi * f * det * t)
        out[i] = s / (len(freqs) * voices)
    # slow moving lowpass = "pressure" sweep, keeps it muffled and enveloping
    swept = [0.0] * N
    p = 0.0
    for i in range(N):
        t = i / SR
        a = 0.06 + 0.05 * (0.5 + 0.5 * math.sin(2 * math.pi * 0.015 * t))
        p += a * (out[i] - p)
        swept[i] = p * (0.85 + 0.15 * math.sin(2 * math.pi * 0.025 * t))
    return swept


def gen_rain():
    base = [v * 0.5 for v in _highpass(_white(), 0.45)]
    for _ in range(int(DUR * 55)):
        s = random.randint(0, N - 1)
        amp = random.uniform(0.2, 0.6)
        for k in range(70):
            if s + k < N:
                base[s + k] += amp * math.exp(-k / 14) * random.uniform(-1, 1)
    return base


def gen_storm():
    base = gen_rain()
    rumble = _lowpass(_brown(), 0.01)
    t = int(random.uniform(2, 4) * SR)
    while t < N:
        L = int(random.uniform(1.6, 3.2) * SR)
        amp = random.uniform(0.6, 1.0)
        for k in range(L):
            if t + k < N:
                env = math.sin(math.pi * k / L) ** 2
                base[t + k] += rumble[(t + k) % N] * amp * env * 3.0
        t += L + int(random.uniform(4, 8) * SR)
    return base


def gen_fire():
    out = [v * 0.8 for v in _lowpass(_brown(), 0.07)]
    for _ in range(int(DUR * 30)):
        s = random.randint(0, N - 1)
        amp = random.uniform(0.3, 0.95)
        L = random.randint(15, 130)
        for k in range(L):
            if s + k < N:
                out[s + k] += amp * math.exp(-k / (L * 0.3)) * random.uniform(-1, 1)
    return out


def gen_wind():
    w = _lowpass(_white(), 0.02)
    out = [0.0] * N
    for i in range(N):
        t = i / SR
        lfo = 0.5 + 0.5 * math.sin(2 * math.pi * 0.05 * t + 1.5 * math.sin(2 * math.pi * 0.013 * t))
        out[i] = w[i] * (0.2 + 0.8 * lfo)
    return out


def gen_forest():
    base = [v * 0.4 for v in _lowpass(_white(), 0.02)]
    for _ in range(int(DUR * 2.2)):
        pos = random.randint(0, N - 1)
        for _c in range(random.randint(2, 5)):
            L = random.randint(int(0.04 * SR), int(0.11 * SR))
            f0 = random.uniform(2200, 3600)
            f1 = f0 * random.uniform(1.2, 1.8)
            for k in range(L):
                if pos + k < N:
                    t = k / SR
                    f = f0 + (f1 - f0) * k / L
                    base[pos + k] += 0.2 * math.sin(math.pi * k / L) * math.sin(2 * math.pi * f * t)
            pos += L + random.randint(int(0.05 * SR), int(0.16 * SR))
            if pos >= N:
                break
    return base


def gen_night():
    out = [0.06 * math.sin(2 * math.pi * 70 * i / SR) for i in range(N)]
    for _ in range(int(DUR * 6)):
        s = random.randint(0, N - 1)
        L = random.randint(int(0.2 * SR), int(0.6 * SR))
        f = random.uniform(4200, 5200)
        trill = random.uniform(18, 28)
        for k in range(L):
            if s + k < N:
                t = k / SR
                amp = max(0.0, math.sin(2 * math.pi * trill * t)) ** 2
                env = math.exp(-((k - L / 2) ** 2) / (2 * (L / 4) ** 2))
                out[s + k] += 0.12 * amp * env * math.sin(2 * math.pi * f * t)
    return out


def _pad(freqs, amp=0.4, move=0.04, det=0.002):
    out = [0.0] * N
    for i in range(N):
        t = i / SR
        s = 0.0
        for fi, f in enumerate(freqs):
            d = 1 + det * math.sin(2 * math.pi * 0.07 * t + fi)
            a = 0.5 + 0.5 * math.sin(2 * math.pi * (move + 0.01 * fi) * t + fi)
            s += a * math.sin(2 * math.pi * f * d * t)
        out[i] = s / len(freqs) * amp
    return out


def gen_warm():
    return _pad([110, 165, 220, 277], amp=0.5, move=0.04)


def gen_shimmer():
    return _pad([220, 330, 440, 550, 660], amp=0.4, move=0.03)


def gen_bright():
    base = gen_shimmer()
    for i in range(N):
        t = i / SR
        pulse = max(0.0, math.sin(2 * math.pi * 2.0 * t)) ** 3
        base[i] = base[i] * 0.7 + 0.18 * pulse * math.sin(2 * math.pi * 120 * t)
    return base


def gen_deep_space():
    out = [0.0] * N
    freqs = [55, 82.5, 110, 138.6]
    for i in range(N):
        t = i / SR
        s = 0.0
        for fi, f in enumerate(freqs):
            a = 0.5 + 0.5 * math.sin(2 * math.pi * (0.02 + 0.007 * fi) * t + fi * 1.7)
            d = 1 + 0.004 * math.sin(2 * math.pi * 0.05 * t + fi)
            s += a * math.sin(2 * math.pi * f * d * t)
        out[i] = s / len(freqs) * 0.5
    noise = _lowpass(_white(), 0.008)
    for i in range(N):
        out[i] += noise[i] * 0.15
    return out


def gen_drone():
    """Dark cinematic wall - swarming detuned low voices + masking bed, sustained."""
    out = [0.0] * N
    freqs = [49.0, 65.4, 98.0, 130.8]
    voices = 3
    for i in range(N):
        t = i / SR
        s = 0.0
        for fi, f in enumerate(freqs):
            for vv in range(voices):
                det = 1 + 0.004 * math.sin(2 * math.pi * (0.02 + 0.007 * fi + 0.003 * vv) * t + fi + vv)
                s += math.sin(2 * math.pi * f * det * t)
        out[i] = s / (len(freqs) * voices) * 0.7
    bed = _lowpass(_white(), 0.12)              # dark broadband bed to mask noise
    for i in range(N):
        t = i / SR
        out[i] = (out[i] * 0.85 + bed[i] * 0.25) * (0.85 + 0.15 * math.sin(2 * math.pi * 0.02 * t))
    return out


def gen_leaves():
    base = [v * 0.5 for v in _lowpass(_white(), 0.03)]
    for i in range(N):
        t = i / SR
        base[i] *= 0.4 + 0.6 * max(0.0, math.sin(2 * math.pi * 0.07 * t))
    for _ in range(int(DUR * 120)):
        s = random.randint(0, N - 1)
        for k in range(8):
            if s + k < N:
                base[s + k] += 0.25 * math.exp(-k / 3) * random.uniform(-1, 1)
    return base


def gen_club():
    out = [0.0] * N
    bpm = 120
    beat = 60.0 / bpm
    hat = _highpass(_white(), 0.6)
    t = 0.0
    while t < DUR:                              # four-on-the-floor kick
        s = int(t * SR)
        for k in range(int(0.18 * SR)):
            if s + k < N:
                env = math.exp(-k / (0.10 * SR))
                f = 120 * math.exp(-k / (0.03 * SR)) + 45
                out[s + k] += 0.9 * env * math.sin(2 * math.pi * f * k / SR)
        t += beat
    t = beat / 2
    while t < DUR:                              # offbeat hats
        s = int(t * SR)
        for k in range(int(0.05 * SR)):
            if s + k < N:
                out[s + k] += hat[s + k] * 0.3 * math.exp(-k / (0.02 * SR))
        t += beat
    for i in range(N):                          # pulsing sub-bass
        tt = i / SR
        out[i] += 0.15 * math.sin(2 * math.pi * 55 * tt) * (0.5 + 0.5 * math.sin(2 * math.pi * (bpm / 60) * tt))
    return out


GENERATORS = {
    "ocean": gen_ocean, "underwater": gen_underwater, "rain": gen_rain,
    "storm": gen_storm, "fire": gen_fire, "wind": gen_wind,
    "forest": gen_forest, "night": gen_night, "warm": gen_warm,
    "shimmer": gen_shimmer, "bright": gen_bright, "deep_space": gen_deep_space,
    "drone": gen_drone, "leaves": gen_leaves, "club": gen_club,
}


# --------------------------------------------------------------------------
# Build
# --------------------------------------------------------------------------

def _write(path, samples):
    samples = _normalize(_loopify(samples))
    with wave.open(path, "w") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        data = array.array("h", [int(max(-1.0, min(1.0, s)) * 32767) for s in samples])
        w.writeframes(data.tobytes())


def build(force=False):
    os.makedirs(AUDIO_DIR, exist_ok=True)
    for key, gen in GENERATORS.items():
        path = os.path.join(AUDIO_DIR, key + ".wav")
        if os.path.exists(path) and not force:
            print("skip", key)
            continue
        random.seed(sum(ord(c) for c in key))   # deterministic per key
        _write(path, gen())
        print("OK  ", key)
    print(f"\nDone: {len(GENERATORS)} ambiences in {AUDIO_DIR}")


if __name__ == "__main__":
    build(force="--force" in sys.argv)
