#!/usr/bin/env python3
"""Synthesise every sound of the clean tier.

Each sample is made here from noise, decaying sines and filters, modelled on
what makes the sound (urethane wheels on a surface, a wooden deck struck on
the ground, a truck dragged along a rail); nothing is read from a recording
or a game. Seeded, so the output is reproducible. Writes 48 kHz mono 16-bit
WAVs into crates/skate-game/clean-audio/ with its sounds.json and
CREDITS.json.

Loops are built in the frequency domain, so they repeat without a seam, and
none has a narrow peak (a whistle or a squeak) in it: `check` measures that.

The app is built with whatever is in clean-audio/: this script puts the
synthesised set there, `restore_recordings.py` the recorded one (which the
app ships with for now).

Usage: scripts/cleanroom/synth_sounds.py [--check]
"""
import json
import sys
import wave
from pathlib import Path

import numpy as np

RATE = 48000
OUT = Path(__file__).resolve().parents[2] / "crates/skate-game/clean-audio"
LOOP_SECONDS = 3.0
#: Loops are mastered quiet so they sit under the hits (the driver restores them).
LOOP_RMS_DB = -24.0
HIT_PEAK_DB = -4.0


def seeded(name):
    return np.random.default_rng(abs(hash_name(name)))


def hash_name(name):
    # (Python's own hash is salted per run)
    h = 2166136261
    for c in name.encode():
        h = ((h ^ c) * 16777619) & 0xFFFFFFFF
    return h


def db(x):
    return 10 ** (x / 20)


def seconds(n):
    return np.arange(n) / RATE


# ---------------------------------------------------------------- building blocks


def shaped_noise(rng, n, shape):
    """Noise with the spectrum `shape(frequencies)`; periodic over its length."""
    spectrum = np.fft.rfft(rng.standard_normal(n))
    out = np.fft.irfft(spectrum * shape(np.fft.rfftfreq(n, 1 / RATE)), n)
    return out / (np.sqrt(np.mean(out**2)) + 1e-12)


def bandpass(lo, hi, slope=2.0):
    """A band between `lo` and `hi` Hz with gentle skirts (`slope` orders)."""
    return lambda f: 1 / np.sqrt((1 + (lo / np.maximum(f, 1e-3)) ** (2 * slope)) * (1 + (f / hi) ** (2 * slope)))


def bump(centre, q, gain):
    """A broad resonance: `gain` times more at `centre`, `q` wide."""
    return lambda f: 1 + (gain - 1) / (1 + (q * (f / centre - centre / np.maximum(f, 1e-3))) ** 2)


def times(*shapes):
    return lambda f: np.prod([s(f) for s in shapes], axis=0)


def wobble(rng, n, rate_hz, depth, power=1.0):
    """A positive, periodic gain about 1 that wanders at up to `rate_hz`."""
    slow = shaped_noise(rng, n, lambda f: (f < rate_hz) * (f > 0.3))
    slow = np.abs(slow) ** power if power != 1.0 else slow
    slow = (slow - slow.mean()) / (slow.std() + 1e-12)
    return np.maximum(1 + depth * slow, 0.0)


def grit(rng, n, per_second, lo, hi, loop=True):
    """Sparse clicks (grains under a wheel, a rail's pits), band-limited."""
    hits = np.zeros(n)
    at = rng.integers(0, n, int(per_second * n / RATE))
    # (all about as loud: one tick that stands out is heard every time the loop comes round)
    hits[at] = rng.uniform(0.5, 1.0, at.size) * rng.choice([-1, 1], at.size)
    spectrum = np.fft.rfft(hits) * bandpass(lo, hi)(np.fft.rfftfreq(n, 1 / RATE))
    out = np.fft.irfft(spectrum, n)
    return out / (np.sqrt(np.mean(out**2)) + 1e-12)


def modes(n, partials, start=0.0):
    """A struck body: decaying sines (frequency Hz, decay seconds, level)."""
    t = seconds(n) - start
    on = t >= 0
    out = np.zeros(n)
    for freq, decay, level, phase in partials:
        out += on * level * np.sin(2 * np.pi * freq * t + phase) * np.exp(-np.maximum(t, 0) / decay)
    return out


def burst(rng, n, lo, hi, decay, start=0.0, attack=0.0005):
    """A noise burst (a slap, a scuff) in a band, starting at `start` seconds."""
    spectrum = np.fft.rfft(rng.standard_normal(2 * n)) * bandpass(lo, hi)(np.fft.rfftfreq(2 * n, 1 / RATE))
    noise = np.fft.irfft(spectrum, 2 * n)[:n]
    noise /= np.sqrt(np.mean(noise**2)) + 1e-12
    t = seconds(n) - start
    env = (t >= 0) * np.minimum(np.maximum(t, 0) / attack, 1.0) * np.exp(-np.maximum(t, 0) / decay)
    return noise * env


def struck(rng, n, table, start=0.0, spread=0.06, level=1.0):
    """`table` (frequency, decay, level) as partials, each a little off so no two hits match."""
    partials = [
        (f * (1 + rng.uniform(-spread, spread)), d * rng.uniform(0.8, 1.25), a * level * rng.uniform(0.7, 1.3), rng.uniform(0, 2 * np.pi))
        for f, d, a in table
    ]
    return modes(n, partials, start)


def fade_out(x, tail=0.012):
    k = int(tail * RATE)
    x[-k:] *= np.linspace(1, 0, k)
    return x


# ---------------------------------------------------------------- loops

# A wooden deck and its trucks: what rings when the board is knocked.
DECK = [(410, 0.045, 1.0), (760, 0.034, 0.8), (1180, 0.024, 0.6), (1750, 0.017, 0.45), (2600, 0.011, 0.3), (3900, 0.007, 0.2)]


def roll(name, low, high, rumble, peaks=(), grains=(0, 0, 0, 0.0), flutter=(30.0, 0.12)):
    """Wheels rolling: a band of noise from the surface, the bearings' hiss, the ground's grain."""
    rng = seeded(name)
    n = int(LOOP_SECONDS * RATE)
    surface = shaped_noise(rng, n, times(bandpass(low, high, 2.0), *[bump(*p) for p in peaks]))
    body = shaped_noise(rng, n, bandpass(45, 160, 2.0)) * rumble
    hiss = shaped_noise(rng, n, bandpass(2200, 4500, 2.0)) * 0.035
    per_second, lo, hi, level = grains
    grain = grit(rng, n, per_second, lo, hi) * level if per_second else 0.0
    return (surface + body + hiss + grain) * wobble(rng, n, 2.5, 0.10) * wobble(rng, n, *flutter)


def scrape(name, low, high, peaks, chatter_hz, chatter, body, grains):
    """Something dragged: band noise broken up by the stick and slip of the two surfaces."""
    rng = seeded(name)
    n = int(LOOP_SECONDS * RATE)
    carrier = shaped_noise(rng, n, times(bandpass(low, high, 3.0), *[bump(*p) for p in peaks]))
    rough = wobble(rng, n, chatter_hz, chatter, power=1.5)
    under = shaped_noise(rng, n, bandpass(120, 420, 2.0)) * body
    per_second, lo, hi, level = grains
    grain = grit(rng, n, per_second, lo, hi) * level if per_second else 0.0
    return (carrier * rough + under + grain) * wobble(rng, n, 2.0, 0.12)


LOOPS = {
    # smooth concrete: mostly below 1 kHz, a little grain
    "roll_smooth": lambda: roll("roll_smooth", 140, 900, 0.5, grains=(140, 900, 3000, 0.07)),
    # asphalt: lower, coarser, the stones ticking under the wheels
    "roll_rough": lambda: roll("roll_rough", 110, 1500, 0.8, grains=(170, 700, 4200, 0.30), flutter=(55.0, 0.25)),
    # a wooden ramp: the sheet under the wheels booms, the top goes
    "roll_wood": lambda: roll("roll_wood", 120, 620, 0.9, peaks=[(170, 2.5, 2.6), (300, 2.5, 2.2), (470, 3.0, 1.6)], flutter=(11.0, 0.15)),
    # sheet metal: brighter, a broad ring to it
    "roll_metal": lambda: roll("roll_metal", 260, 2800, 0.4, peaks=[(850, 4.0, 2.2), (1800, 5.0, 2.0), (2600, 5.0, 1.5)], grains=(40, 1200, 5000, 0.10)),
    # a truck along a steel rail or coping
    "grind_metal": lambda: scrape("grind_metal", 600, 4200, [(1250, 5.0, 2.4), (2500, 6.0, 2.0), (3700, 6.0, 1.5)], 90.0, 0.45, 0.3, (60, 1500, 6000, 0.12)),
    # a truck along a concrete ledge: lower, crunchier
    "grind_concrete": lambda: scrape("grind_concrete", 280, 3200, [(620, 2.5, 2.0), (1500, 3.0, 1.6)], 45.0, 0.6, 0.45, (220, 500, 3800, 0.4)),
    # wheels pushed sideways: urethane scrubbing, chattering as it grips and lets go
    "powerslide": lambda: scrape("powerslide", 380, 2100, [(900, 2.0, 1.8)], 60.0, 0.7, 0.2, (0, 0, 0, 0.0)),
}

# ---------------------------------------------------------------- hits


def pop(i):
    """The tail struck on the ground: a click, the deck's own ring, a low knock."""
    rng = seeded(f"pop_{i}")
    n = int(0.26 * RATE)
    x = struck(rng, n, DECK) + burst(rng, n, 1800, 9000, 0.0025) * 1.6
    x += modes(n, [(rng.uniform(95, 125), 0.04, 1.1, 0.0)])
    return fade_out(x)


def land(i):
    """Four wheels coming down, the back ones a moment after the front: thumps, the deck, a slap."""
    rng = seeded(f"land_impact_{i}")
    n = int(0.42 * RATE)
    x = np.zeros(n)
    gap = rng.uniform(0.014, 0.034)
    for start, level in [(0.0, 1.0), (gap, rng.uniform(0.7, 1.0))]:
        x += modes(n, [(rng.uniform(68, 105), 0.075, 1.6 * level, 0.0)], start)
        x += struck(rng, n, [(300, 0.045, 0.9), (520, 0.035, 0.7), (900, 0.022, 0.4)], start, level=level)
        x += burst(rng, n, 500, 2600, 0.022, start) * 1.1 * level
    # the wheels rolling away
    x += burst(rng, n, 160, 900, 0.11, gap, attack=0.02) * 0.25
    return fade_out(x)


def catch(i):
    """Feet meeting the griptape: a soft slap and a scuff."""
    rng = seeded(f"catch_{i}")
    n = int(0.2 * RATE)
    x = burst(rng, n, 280, 1800, 0.02) + struck(rng, n, [(240, 0.03, 0.7), (430, 0.022, 0.5)])
    x += burst(rng, n, 2200, 6000, 0.028, rng.uniform(0.004, 0.012), attack=0.004) * 0.35
    return fade_out(x)


def flip(i):
    """The board turning over in the air: the shoe flicking off, then a whoosh that flutters with the spin."""
    rng = seeded(f"flip_{i}")
    length = [0.30, 0.26, 0.34][i - 1]
    n = int(length * RATE)
    t = seconds(n) / length
    x = np.zeros(n)
    # the whoosh climbs and falls: bands that come and go in turn
    for centre, peak_at in [(450, 0.25), (800, 0.4), (1400, 0.52), (2100, 0.6), (1100, 0.8)]:
        band = burst(rng, n, centre * 0.75, centre * 1.35, 10.0, attack=0.0001)
        x += band * np.exp(-(((t - peak_at) / 0.2) ** 2)) * (700 / centre) ** 0.3
    spin = rng.uniform(19, 27)
    x *= 1 - 0.38 * (0.5 + 0.5 * np.sin(2 * np.pi * spin * seconds(n) + rng.uniform(0, 6)))
    x += burst(rng, n, 2500, 8000, 0.012) * 0.5
    return fade_out(x * np.sin(np.pi * np.clip(t, 0, 1)) ** 0.5, 0.03)


def push(i):
    """A foot pushing off the ground: it lands, then scrapes back."""
    rng = seeded(f"push_{i}")
    length = [0.46, 0.36, 0.5][i - 1]
    n = int(length * RATE)
    t = seconds(n)
    x = modes(n, [(rng.uniform(85, 110), 0.03, 0.9, 0.0)]) + burst(rng, n, 400, 2200, 0.018) * 0.6
    scrape_env = np.clip((t - 0.03) / 0.04, 0, 1) * np.exp(-np.maximum(t - 0.07, 0) / (length * 0.3))
    rough = 1 - 0.5 * np.abs(np.sin(2 * np.pi * rng.uniform(45, 70) * t + rng.uniform(0, 6)))
    x += burst(rng, n, 450, 3600, 10.0, attack=0.0001) * scrape_env * rough * 0.8
    return fade_out(x, 0.03)


def footstep(i):
    """A shoe on the ground: the heel, then the sole."""
    rng = seeded(f"footstep_{i}")
    n = int(0.22 * RATE)
    toe = rng.uniform(0.022, 0.04)
    x = modes(n, [(rng.uniform(92, 130), 0.035, 1.0, 0.0)]) + burst(rng, n, 450, 2400, 0.016) * 0.7
    x += burst(rng, n, 700, 3800, 0.02, toe, attack=0.003) * 0.4 + modes(n, [(rng.uniform(140, 180), 0.02, 0.35, 0.0)], toe)
    return fade_out(x)


def bail(i):
    """A fall: the body going down, the board bouncing away and coming to rest."""
    rng = seeded(f"bail_{i}")
    length = [0.9, 0.62][i - 1]
    n = int(length * RATE)
    x = modes(n, [(rng.uniform(58, 82), 0.09, 1.8, 0.0)]) + burst(rng, n, 90, 520, 0.07) * 1.4
    x += burst(rng, n, 600, 3000, 0.05, 0.02, attack=0.01) * 0.35
    # the board: knocks coming closer together and quieter, as a thing bouncing does
    at, gap, level = rng.uniform(0.07, 0.12), rng.uniform(0.11, 0.15), 0.9
    while at < length - 0.08 and level > 0.08:
        x += struck(rng, n, DECK, at, spread=0.1, level=level) + burst(rng, n, 1500, 7000, 0.003, at) * level
        at += gap
        gap *= rng.uniform(0.62, 0.78)
        level *= rng.uniform(0.55, 0.72)
        if gap < 0.018:
            break
    return fade_out(x, 0.04)


def chime():
    """A score banked: two soft bell notes, a fifth apart, the second just after the first."""
    n = int(0.9 * RATE)
    x = np.zeros(n)
    for note, start, level in [(1046.5, 0.0, 1.0), (1568.0, 0.07, 0.8)]:
        x += modes(n, [(note, 0.30, level, 0.0), (note * 2.76, 0.12, 0.22 * level, 0.0), (note * 5.4, 0.05, 0.07 * level, 0.0)], start)
        x += modes(n, [(note * 1.003, 0.30, 0.3 * level, 1.0)], start)  # a slow beat, as a bell has
    t = seconds(n)
    return fade_out(x * np.minimum(t / 0.004, 1.0), 0.05)


HITS = {
    **{f"pop_{i}": (lambda i=i: pop(i)) for i in range(1, 5)},
    **{f"land_impact_{i}": (lambda i=i: land(i)) for i in range(1, 5)},
    **{f"catch_{i}": (lambda i=i: catch(i)) for i in range(1, 4)},
    **{f"flip_{i}": (lambda i=i: flip(i)) for i in range(1, 4)},
    **{f"push_{i}": (lambda i=i: push(i)) for i in range(1, 4)},
    **{f"footstep_{i}": (lambda i=i: footstep(i)) for i in range(1, 5)},
    **{f"bail_{i}": (lambda i=i: bail(i)) for i in range(1, 3)},
    "chime": chime,
}
#: Quieter kinds, against the loudest hits (a pop, a landing).
HIT_LEVEL_DB = {"catch": -4.0, "flip": -3.0, "push": -7.0, "footstep": -3.0, "chime": -2.0}

# ---------------------------------------------------------------- output


def master(name, x):
    x = x - x.mean()
    if name in LOOPS:
        x = x * (db(LOOP_RMS_DB) / np.sqrt(np.mean(x**2)))
        # round the few peaks that stick out, rather than clip them
        return np.tanh(x / db(-7.0)) * db(-7.0)
    kind = name.rsplit("_", 1)[0] if name[-1].isdigit() else name
    return x * (db(HIT_PEAK_DB + HIT_LEVEL_DB.get(kind, 0.0)) / np.max(np.abs(x)))


def tonality(x):
    """How far the strongest narrow band stands above its surroundings, dB: a whistle is 20 or more."""
    spectrum = np.abs(np.fft.rfft(x * np.hanning(x.size))) ** 2
    freqs = np.fft.rfftfreq(x.size, 1 / RATE)
    keep = (freqs > 150) & (freqs < 9000)
    narrow = np.convolve(spectrum, np.ones(9) / 9, "same")
    wide = np.convolve(spectrum, np.ones(1501) / 1501, "same")
    return 10 * np.log10(np.max(narrow[keep] / (wide[keep] + 1e-20)))


def check(name, x):
    problems = []
    if np.max(np.abs(x)) > 0.99:
        problems.append("clips")
    if name in LOOPS:
        if tonality(x) > 12.0:
            problems.append(f"a narrow peak ({tonality(x):.1f} dB)")
        seam = abs(x[0] - x[-1])
        if seam > 4 * np.percentile(np.abs(np.diff(x)), 99):
            problems.append(f"a seam ({seam:.3f})")
        # no moment much louder than the rest: it would mark every turn of the loop
        k = int(0.02 * RATE)
        loud = np.sqrt(np.convolve(x**2, np.ones(k) / k, "same"))
        if loud.max() > 3.2 * np.median(loud):
            problems.append(f"a moment that stands out ({loud.max() / np.median(loud):.1f}x)")
    elif abs(x[-1]) > 0.01 or abs(x[0]) > 0.2:
        problems.append("doesn't start or end quietly")
    return problems


def main():
    made, failed = {}, False
    for name, make in {**LOOPS, **HITS}.items():
        x = master(name, make())
        problems = check(name, x)
        failed |= bool(problems)
        spectrum = np.abs(np.fft.rfft(x)) ** 2
        centroid = np.sum(np.fft.rfftfreq(x.size, 1 / RATE) * spectrum) / np.sum(spectrum)
        print(
            f"{name:16s} {x.size / RATE:4.2f} s  peak {20 * np.log10(np.max(np.abs(x))):6.1f} dB  "
            f"rms {20 * np.log10(np.sqrt(np.mean(x**2))):6.1f} dB  centre {centroid:5.0f} Hz  {'; '.join(problems)}"
        )
        made[name] = x
    if failed:
        sys.exit("some sounds failed their checks: nothing written")
    if "--check" in sys.argv:
        return
    for name, x in made.items():
        with wave.open(str(OUT / f"{name}.wav"), "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(RATE)
            w.writeframes(np.round(x * 32767).astype("<i2").tobytes())
    names = sorted(made)
    (OUT / "sounds.json").write_text(
        json.dumps({"rate": RATE, "sounds": {n: {"file": f"{n}.wav", "loop": n in LOOPS, "gain": 1.0} for n in names}}, indent=1) + "\n"
    )
    credit = {
        "url": "scripts/cleanroom/synth_sounds.py",
        "author": "SK8 AR (original synthesis)",
        "license": "Original work, generated from noise and sine synthesis; no recorded or game audio",
    }
    (OUT / "CREDITS.json").write_text(json.dumps([{"sound": n, **credit} for n in names], indent=1) + "\n")
    print(f"synthesised {len(made)} sounds into {OUT}")


if __name__ == "__main__":
    main()
