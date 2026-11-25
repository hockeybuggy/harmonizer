"""Render and measure relative mode with NumPy, SciPy and SoundFile.

uv run --with numpy --with scipy --with soundfile python3 tools/eval/eval_relative.py \
    /path/to/corpus target/eval/relative/before

Renders are latency-compensated. Pre-echo is the median power in the 20 ms
before each input onset, in dB relative to input power in the next 25 ms.
Rise time is the local 10–90% RMS rise on the same input-anchored windows.
Unmatched peak power measures energy in peaks over 25 cents from any expected
shifted peak; it is a proxy for leakage, not a perceptual score.

To compare sustained vowels with directly synthesised references, first run:
    cargo test -p harmonizer-dsp --release --test vocoder write_phase_vowels -- --ignored
Then add --vowels target/eval/phase/vowels to the evaluator command. It writes
vowels.csv and the reference and shifted WAVs alongside the corpus results.
Vowel spectra are compared at equal RMS. Adjacent-partial phase coherence
measures stability over time; agreement also penalises dispersion across
partials, ignoring a common time offset. Both are best at 1.
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
SEMITONES = {"plus4": 4, "minus5": -5}
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
                    if name in SEMITONES:
                        error, unmatched = pitch_metrics(x, y, SEMITONES[name])
                        row.update(pitch_error_cents=error, unmatched_pct=unmatched)
                    writer.writerow(row)


def vowel_metrics(output, ideal, frequency):
    start = RATE // 4
    width = 3 * RATE // 25
    hop = RATE // 50
    window = signal.windows.hann(width, sym=False)
    harmonics = np.arange(1, int(4800 / frequency) + 1)
    radians = 2 * np.pi * frequency / RATE
    basis = np.exp(-1j * radians * np.arange(width)[:, None] * harmonics)

    def describe(x):
        x = np.asarray(x[start:-start], dtype=np.float64)
        rms = np.sqrt(power(x))
        if rms == 0:
            raise ValueError("Sustained-vowel measurement received silence")
        x = x / rms
        frames = np.lib.stride_tricks.sliding_window_view(x, width)[::hop] * window
        partials = frames @ basis
        partials *= np.exp(-1j * radians * np.arange(len(frames))[:, None] * hop * harmonics)
        adjacent = partials[:, 1:] * partials[:, :-1].conj()
        unit = adjacent / np.maximum(np.abs(adjacent), 1e-15)
        return (
            20 * np.log10(np.max(np.abs(x))),
            np.mean(x**4),
            unit.mean(axis=0),
            np.abs(adjacent).mean(axis=0),
            np.abs(np.fft.rfft(frames, axis=1)),
        )

    crest, kurtosis, phase, _, spectrum = describe(output)
    ideal_crest, ideal_kurtosis, ideal_phase, weights, ideal_spectrum = describe(ideal)
    # A time shift rotates all adjacent pairs equally; measure their spread, not that offset.
    agreement = phase * ideal_phase.conj()
    return {
        "crest_db": crest,
        "ideal_crest_db": ideal_crest,
        "kurtosis": kurtosis,
        "ideal_kurtosis": ideal_kurtosis,
        "phase_coherence": np.average(np.abs(phase), weights=weights),
        "phase_agreement": np.abs(np.average(agreement, weights=weights)),
        "spectral_convergence": np.linalg.norm(spectrum - ideal_spectrum) / np.linalg.norm(ideal_spectrum),
    }


def evaluate_vowels(args):
    fields = ("f0", "semitones", "quality", "crest_db", "ideal_crest_db", "kurtosis",
              "ideal_kurtosis", "phase_coherence", "phase_agreement", "spectral_convergence")
    args.output.mkdir(parents=True, exist_ok=True)
    with (args.output / "vowels.csv").open("w", newline="") as file:
        writer = csv.DictWriter(file, fieldnames=fields)
        writer.writeheader()
        for f0 in (110, 150, 220):
            source = args.output / f"phase-vowel-{f0}-input.wav"
            samples = np.fromfile(args.vowels / f"{f0}-+0.f32le", dtype="<f4")
            sf.write(source, samples, RATE, subtype="FLOAT")
            for name, shift in SEMITONES.items():
                if name not in args.renders:
                    continue
                ideal = np.fromfile(args.vowels / f"{f0}-{shift:+}.f32le", dtype="<f4")
                sf.write(args.output / f"phase-vowel-{f0}-{name}-ideal.wav", ideal, RATE, subtype="FLOAT")
                frequency = f0 * 2 ** (shift / 12)
                for quality in args.qualities:
                    destination = args.output / f"phase-vowel-{f0}-{quality}-{name}.wav"
                    render(args.cli, source, destination, quality, SHIFTS[name])
                    output = load(destination)[: len(ideal)]
                    if len(output) != len(ideal):
                        raise ValueError(f"Render shorter than ideal: {destination}")
                    metrics = vowel_metrics(output, ideal, frequency)
                    writer.writerow(dict(f0=f0, semitones=shift, quality=quality,
                                         **{key: f"{value:.5f}" for key, value in metrics.items()}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cli", type=Path, default=Path("target/release/harmonizer"))
    parser.add_argument("--files", nargs="+", default=["*.wav"])
    parser.add_argument("--qualities", nargs="+", choices=QUALITIES, default=QUALITIES)
    parser.add_argument("--renders", nargs="+", choices=SHIFTS, default=list(SHIFTS))
    parser.add_argument("--vowels", type=Path, help="directory of raw Rust test vowel fixtures")
    args = parser.parse_args()
    evaluate(args)
    if args.vowels:
        evaluate_vowels(args)


if __name__ == "__main__":
    main()
