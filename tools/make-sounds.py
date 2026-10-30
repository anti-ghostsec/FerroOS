#!/usr/bin/env python3
"""Makes FerroOS's system sounds from nothing but sine waves and noise.

    python3 tools/make-sounds.py      writes heavy/sounds/*.wav

Original sounds in the spirit of a 1995 desktop (a warm chime at startup, a
low tone for errors, and a ding; emptying the Recycle Bin is silent). All are soft and quiet;
none is a copy of anyone's. Same output every run, so
the files in the repository can be checked against this script.
"""

import math
import os
import struct
import wave

RATE = 22050
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "heavy", "sounds")


def bell(freq, t, decay, partials=((1.0, 1.0), (2.0, 0.35), (3.01, 0.15), (4.2, 0.07))):
    """A struck bell: a few partials, the higher ones dying faster."""
    return sum(a * math.sin(2 * math.pi * freq * m * t) * math.exp(-t * decay * m) for m, a in partials)


def pad(freq, t, attack):
    """A soft held note: two slightly detuned sines, faded in."""
    env = min(1.0, t / attack)
    return env * (math.sin(2 * math.pi * freq * t) + 0.6 * math.sin(2 * math.pi * freq * 1.003 * t + 0.5))


def echo(samples, delay_s=0.11, feedback=0.35, mix=0.3):
    """A small room: one feedback delay, enough to soften the edges."""
    d = int(delay_s * RATE)
    out = list(samples)
    for i in range(d, len(out)):
        out[i] += out[i - d] * feedback
    return [s * (1 - mix) + o * mix for s, o in zip(samples, out)]


def startup():
    # Four notes rising (C, G, E, D an octave up), each joining the last.
    notes = [(261.63, 0.0), (392.00, 0.28), (329.63, 0.56), (587.33, 0.84)]
    length = 3.6
    out = []
    for i in range(int(length * RATE)):
        t = i / RATE
        s = 0.0
        for f, start in notes:
            if t >= start:
                u = t - start
                s += 0.5 * pad(f, u, 0.25) * math.exp(-u * 0.9) + 0.35 * bell(f * 2, u, 2.5)
        fade = min(1.0, (length - t) / 0.8)
        out.append(s * fade)
    return echo(out)


def error():
    # Two low tones close together, the second a fourth under the first.
    out = []
    for i in range(int(0.7 * RATE)):
        t = i / RATE
        soft = ((1.0, 1.0), (2.0, 0.12))
        s = bell(293.66, t, 5.0, soft)
        if t > 0.12:
            s += 0.9 * bell(220.0, t - 0.12, 4.5, soft)
        out.append(s)
    return out


def ding():
    # A low, round ding (G above middle C), not a high ping.
    return echo([bell(392.0, i / RATE, 4.5, ((1.0, 1.0), (2.0, 0.15))) for i in range(int(0.8 * RATE))], 0.07, 0.25, 0.2)



def write(name, samples, loud):
    # Each sound fades in over 25 ms, so none starts with a click or a snap.
    fade = int(0.025 * RATE)
    samples = [s * min(1.0, i / fade) for i, s in enumerate(samples)]
    peak = max(abs(s) for s in samples) or 1.0
    frames = b"".join(struct.pack("<h", int(s / peak * loud * 32767)) for s in samples)
    with wave.open(os.path.join(OUT, name + ".wav"), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(frames)


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for name, make, loud in [("startup", startup, 0.4), ("error", error, 0.3), ("ding", ding, 0.3)]:
        write(name, make(), loud)
        print("wrote", name + ".wav")
