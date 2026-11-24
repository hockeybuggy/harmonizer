"""Render and measure relative mode with NumPy, SciPy and SoundFile.

uv run --with numpy --with scipy --with soundfile python3 tools/eval/eval_relative.py \
    /path/to/corpus target/eval/relative/before

Renders are latency-compensated. Pre-echo is the median power in the 20 ms
before each input onset, in dB relative to input power in the next 25 ms.
Rise time is the local 10–90% RMS rise on the same input-anchored windows.
Unmatched peak power measures energy in peaks over 25 cents from any expected
shifted peak; it is a proxy for leakage, not a perceptual score.
"""

import argparse
import csv
import math
from pathlib import Path
import subprocess

import numpy as np
from scipy import ndimage, signal
import soundfile as sf


RATE = 48_000
QUALITIES = ("low", "balanced", "high")
SHIFTS = {"plus4": "64", "minus5": "55", "chord": "60,64,67", "unison": "60"}


def power(x):
    return np.mean(np.square(x))


def db_ratio(numerator, denominator):
    return 10 * math.log10(max(numerator, 1e-30) / max(denominator, 1e-30))


def load(path):
    x, rate = sf.read(path)
    if rate != RATE or x.ndim != 1 or not np.all(np.isfinite(x)):
        raise ValueError(f"Expected finite 48 kHz mono audio: {path}")
    return x


def envelope(x):
    hop = RATE // 1000
    frames = x[: len(x) // hop * hop].reshape(-1, hop)
    return np.sqrt(np.mean(frames**2, axis=1))


def onsets(env):
    average = np.sqrt(np.maximum(0, ndimage.uniform_filter1d(env**2, 9)))
    before = average[:-12]
    after = average[12:]
    rise = np.maximum(0, after - 2 * before)
    peaks = signal.find_peaks(rise, height=0.04 * np.max(average), distance=45)[0] + 6
    result = []
    for peak in peaks:
        start = peak - 8
        if start < 25 or peak + 38 >= len(env):
            continue
        quiet = np.sqrt(power(env[peak - 20 : start]))
        level = average[peak + 6]
        crossing = np.flatnonzero(env[start : peak + 8] > max(quiet * 2, 0.1 * level))
        if crossing.size:
            result.append(start + crossing[0])
    return result


def onset_metrics(env, timing, reference):
    pre = []
    rise = []
    for at in timing:
        peak = at + np.argmax(env[at : at + 25])
        level = env[peak]
        pre.append(db_ratio(power(env[at - 20 : at]), power(reference[at : at + 25])))
        attack = env[at - 20 : peak + 1]
        lo = np.flatnonzero(attack >= 0.1 * level)[0]
        hi = np.flatnonzero(attack >= 0.9 * level)[0]
        rise.append(hi - lo)
    return np.median(pre), np.median(rise)


def peaks(x):
    n = len(x)
    spectrum = np.abs(np.fft.rfft(x * signal.windows.hann(n, sym=False), n * 4))
    bins = signal.find_peaks(spectrum)[0]
    hz = bins * RATE / (n * 4)
    keep = (hz >= 70) & (hz <= 3000) & (spectrum[bins] >= np.max(spectrum) * 0.015)
    return hz[keep], spectrum[bins[keep]]


def pitch_metrics(x, y, semitones):
    ratio = 2 ** (semitones / 12)
    width = RATE // 2
    errors = []
    unmatched_power = []
    windows = list(range(0, len(x) - width + 1, width // 2))
    for start in sorted(windows, key=lambda i: -power(x[i : i + width]))[:8]:
        source, strength = peaks(x[start : start + width])
        target, output_strength = peaks(y[start : start + width])
        if not source.size or not target.size:
            continue
        cents = np.abs(1200 * np.log2(target[:, None] / (source[None, :] * ratio)))
        strongest = np.argsort(strength)[-8:]
        errors.extend(np.min(cents[:, strongest], axis=0))
        unmatched = output_strength[np.min(cents, axis=1) > 25]
        unmatched_power.append(np.sum(unmatched**2) / np.sum(output_strength**2))
    if not errors:
        return "", ""
    return f"{np.median(errors):.2f}", f"{100 * np.median(unmatched_power):.2f}"


def render(cli, source, destination, quality, notes):
    result = subprocess.run([
        str(cli.resolve()), "--input", str(source), "--output", str(destination),
        "--mode", "relative", "--quality", quality, "--notes", notes,
        "--reference-note", "60", "--wet-db", "0", "--dry-db=-inf",
        "--velocity-sensitivity", "0", "--spread", "0",
    ], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"Could not render {destination}: {result.stderr.strip()}")


def evaluate(args):
    fields = ("file", "quality", "render", "onsets", "pre_db", "input_pre_db", "rise_ms",
              "input_rise_ms", "pitch_error_cents", "unmatched_pct", "null_snr_db")
    sources = sorted({path for pattern in args.files for path in args.corpus.glob(pattern)})
    if not sources:
        raise ValueError(f"No source WAV files matched {args.files} in {args.corpus}")
    args.output.mkdir(parents=True, exist_ok=True)
    with (args.output / "metrics.csv").open("w", newline="") as file:
        writer = csv.DictWriter(file, fieldnames=fields)
        writer.writeheader()
        for source in sources:
            x = load(source)
            env = envelope(x)
            timing = onsets(env)
            print(f"{source.stem}: {len(timing)} onsets", flush=True)
            if timing:
                input_pre, input_rise = onset_metrics(env, timing, env)
            for quality in args.qualities:
                for name in args.renders:
                    destination = args.output / f"{source.stem}-{quality}-{name}.wav"
                    render(args.cli, source, destination, quality, SHIFTS[name])
                    y = load(destination)[: len(x)]
                    if len(x) != len(y):
                        raise ValueError(f"Render shorter than input: {destination}")
                    row = {"file": source.stem, "quality": quality, "render": name, "onsets": len(timing)}
                    if name == "unison":
                        row["null_snr_db"] = f"{db_ratio(power(x), power(x - y)):.2f}"
                    elif timing:
                        pre, rise = onset_metrics(envelope(y), timing, env)
                        row.update(pre_db=f"{pre:.2f}", input_pre_db=f"{input_pre:.2f}",
                                   rise_ms=f"{rise:.1f}", input_rise_ms=f"{input_rise:.1f}")
                    if name in ("plus4", "minus5"):
                        error, unmatched = pitch_metrics(x, y, 4 if name == "plus4" else -5)
                        row.update(pitch_error_cents=error, unmatched_pct=unmatched)
                    writer.writerow(row)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cli", type=Path, default=Path("target/release/harmonizer"))
    parser.add_argument("--files", nargs="+", default=["*.wav"])
    parser.add_argument("--qualities", nargs="+", choices=QUALITIES, default=QUALITIES)
    parser.add_argument("--renders", nargs="+", choices=SHIFTS, default=list(SHIFTS))
    evaluate(parser.parse_args())


if __name__ == "__main__":
    main()
