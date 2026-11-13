#!/usr/bin/env python3
"""Puts the clean tier's recorded sounds back in crates/skate-game/clean-audio/.

The clean tier has two sets of its 31 sounds: the CC0 recordings it began
with (this script), and a set synthesised from scratch
(`synth_sounds.py`). Each script overwrites clean-audio/ with its set, so
running one or the other chooses which the app is built with.

The recordings are taken from the repository's history (the commit before
the synthesised set replaced them), and:

- resampled from 32 kHz to 48 kHz, which the HRTF rendering works at (in
  the frequency domain: a loop still meets its own start);
- `roll_smooth`, the wheels' loop on every surface, is cleaned: the
  recording has a wooden board's resonance about 530-625 Hz and a cluster
  of creaks 1.7-1.95 s into its 2.5 s, heard as a creaking board under all
  skating. Each frequency is held to 1.5 times its own usual (median)
  level over the loop, which takes the creaks out and leaves the steady
  roll, and peaks of its long-term spectrum are brought down to the
  spectrum around them.

Needs numpy. Run from anywhere in the repository.
"""
import io
import json
import subprocess
import wave
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
DIR = "crates/skate-game/clean-audio"
REV = "0e25ffb^"
RATE = 48_000


def from_history(name: str) -> bytes:
    return subprocess.run(["git", "-C", str(ROOT), "show", f"{REV}:{DIR}/{name}"], check=True, capture_output=True).stdout


def read(data: bytes):
    with wave.open(io.BytesIO(data)) as w:
        assert (w.getnchannels(), w.getsampwidth()) == (1, 2)
        return np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64) / 32768, w.getframerate()


def resample(x: np.ndarray, rate: int, looping: bool) -> np.ndarray:
    """To RATE through the spectrum; a hit is padded so its end doesn't wrap onto its start."""
    pad = 0 if looping else len(x) // 4
    x = np.concatenate([x, np.zeros(pad)])
    n = round(len(x) * RATE / rate)
    spectrum = np.fft.rfft(x)
    out = np.fft.irfft(np.concatenate([spectrum, np.zeros(n // 2 + 1 - len(spectrum))]), n) * n / len(x)
    return out[: n - round(pad * RATE / rate)] if pad else out


def steady(x: np.ndarray) -> np.ndarray:
    """A loop without what stands out of it: creaks in time, resonances in pitch."""
    size, hop = 2048, 512
    window = np.hanning(size + 1)[:-1]
    # Frames taken round the loop, so its end runs on into its start.
    at = (np.arange(0, len(x), hop)[:, None] + np.arange(size)[None, :]) % len(x)
    frames = np.fft.rfft(x[at] * window, axis=1)
    level = np.abs(frames) + 1e-12
    usual = np.median(level, axis=0)
    held = 1.5 * usual * np.tanh(level / (1.5 * usual))
    # Peaks of the long-term spectrum, down to the spectrum about them (a third of an octave each way).
    spectrum = (held**2).mean(axis=0)
    freqs = np.fft.rfftfreq(size, 1 / RATE)
    about = np.array([np.exp(np.mean(np.log(spectrum[(freqs >= f / 1.26) & (freqs <= f * 1.26)] + 1e-24))) if f > 0 else spectrum[0] for f in freqs])
    trim = np.sqrt(np.minimum(1.0, about / spectrum))
    shaped = np.fft.irfft(frames * (held / level) * trim, size, axis=1) * window
    out = np.zeros(len(x))
    weight = np.zeros(len(x))
    np.add.at(out, at, shaped)
    np.add.at(weight, at, np.broadcast_to(window**2, at.shape))
    out /= weight
    return out * np.sqrt(np.mean(x**2) / np.mean(out**2))


def write(path: Path, x: np.ndarray):
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(np.clip(np.round(x * 32768), -32768, 32767).astype(np.int16).tobytes())


def main():
    out = ROOT / DIR
    sounds = json.loads(from_history("sounds.json"))
    for name, entry in sounds["sounds"].items():
        x, rate = read(from_history(entry["file"]))
        peak = np.max(np.abs(x))
        x = resample(x, rate, entry["loop"])
        if name == "roll_smooth":
            x = steady(x)
        # (resampling can overshoot a clipped peak)
        x *= min(1.0, max(peak, 1e-9) / np.max(np.abs(x)))
        write(out / entry["file"], x)
    sounds["rate"] = RATE
    (out / "sounds.json").write_text(json.dumps(sounds, indent=1) + "\n")
    credits = json.loads(from_history("CREDITS.json"))
    for credit in credits:
        if credit["sound"] == "roll_smooth":
            credit["changes"] = "Creaks and a board's resonance taken out (scripts/cleanroom/restore_recordings.py)"
    (out / "CREDITS.json").write_text(json.dumps(credits, indent=1) + "\n")
    print(f"{len(sounds['sounds'])} recorded sounds in {out} at {RATE} Hz")


if __name__ == "__main__":
    main()
