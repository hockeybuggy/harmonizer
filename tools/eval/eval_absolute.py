# /// script
# dependencies = ["librosa", "numpy", "scipy", "soundfile"]
# ///

import argparse
import csv
import subprocess
from pathlib import Path

import librosa
import numpy as np
import soundfile as sf
from scipy import signal

RATE = 48_000
TRACK_RATE = 12_000
FRAME = 512
HOP = 120
HOP_MS = 1000 * HOP / TRACK_RATE
QUALITIES = ("low", "balanced", "high")
NOTES = "C3,E3,G3"


def render(binary, source, destination, quality, notes, expression):
    subprocess.run(
        [
            str(binary.resolve()),
            "--input",
            str(source),
            "--output",
            str(destination),
            "--mode",
            "absolute",
            "--quality",
            quality,
            "--notes",
            notes,
            "--spread",
            "0",
            "--expression",
            str(expression),
        ],
        check=True,
        stdout=subprocess.DEVNULL,
    )


def read(path):
    audio, sr = sf.read(path, dtype="float32", always_2d=True)
    if sr != RATE:
        raise ValueError(f"{path}: expected {RATE} Hz, got {sr}")
    return audio.mean(axis=1)


def pitch(audio):
    down = signal.resample_poly(audio, 1, RATE // TRACK_RATE)
    return librosa.pyin(
        down,
        sr=TRACK_RATE,
        fmin=50,
        fmax=1000,
        frame_length=FRAME,
        hop_length=HOP,
    )[0]


def features(audio):
    _, _, spectrum = signal.stft(audio, RATE, nperseg=960, noverlap=480)
    magnitude = np.abs(spectrum)
    positive = np.maximum(np.diff(magnitude, axis=1, prepend=magnitude[:, :1]), 0)
    flux = np.linalg.norm(positive, axis=0)
    low_filter = signal.butter(4, 100, fs=RATE, output="sos")
    low = signal.sosfiltfilt(low_filter, audio)
    return np.mean(low**2), flux


def vibrato_depth(reference):
    cents = 1200 * np.log2(reference)
    depth = np.zeros_like(cents)
    band = signal.butter(
        2, (4, 8.5), btype="bandpass", fs=TRACK_RATE / HOP, output="sos"
    )
    boundaries = (
        np.flatnonzero(~np.isfinite(np.diff(cents)) | (np.abs(np.diff(cents)) > 100))
        + 1
    )
    for indices in np.split(np.arange(len(cents)), boundaries):
        if len(indices) >= 40:
            oscillation = signal.sosfiltfilt(band, cents[indices])
            depth[indices] = np.abs(signal.hilbert(oscillation))
    return np.minimum(depth, 100)


def onset_metrics(voiced, accurate, leaked):
    boundaries = np.flatnonzero(np.diff(voiced, prepend=False, append=False))
    leakage_ms, target_ms = [], []
    missed = 0
    for start, end in boundaries.reshape(-1, 2):
        if (end - start) * HOP_MS < 250:
            continue
        stop = min(end, start + round(300 / HOP_MS))
        leakage_ms.append(HOP_MS * np.count_nonzero(leaked[start:stop]))
        good = accurate[start:end]
        hits = np.flatnonzero(good[:-1] & good[1:])
        if hits.size:
            target_ms.append(HOP_MS * int(hits[0]))
        else:
            missed += 1
    return {
        "onset_notes": len(leakage_ms),
        "onset_leak_ms_median": percentile(leakage_ms, 50),
        "onset_leak_ms_p90": percentile(leakage_ms, 90),
        "onset_leak_ms_max": max(leakage_ms, default=None),
        "onset_target_ms_median": percentile(target_ms, 50),
        "onset_target_ms_p90": percentile(target_ms, 90),
        "onset_target_misses": missed,
    }


def percentile(values, q):
    return round(float(np.percentile(values, q)), 1) if values else None


def frame_power(audio):
    hop = RATE * HOP // TRACK_RATE
    padded = np.pad(audio, (hop // 2, hop - hop // 2))
    frames = np.lib.stride_tricks.sliding_window_view(padded, hop)[::hop]
    return np.mean(frames**2, axis=1)


def consonant_ratio(voiced, source_power, output_power):
    energetic = (~voiced) & (source_power > 1e-4 * source_power.max())
    if not energetic.any():
        return None
    return round(
        float(np.sqrt(output_power[energetic].sum() / source_power[energetic].sum())), 4
    )


def metrics(reference, output, tolerance, in_features, out_features, target):
    voiced = np.isfinite(reference)
    error = np.abs(1200 * np.log2(output / target))
    accurate = voiced & (error <= tolerance)
    # When input and target agree, pitch alone cannot tell leakage from retuning.
    distinguishable = voiced & (np.abs(1200 * np.log2(reference / target)) > 60)
    leaked = distinguishable & (np.abs(1200 * np.log2(output / reference)) <= 30)
    low_in, flux_in = in_features
    low_out, flux_out = out_features
    spike = flux_out > np.maximum(2 * flux_in, np.quantile(flux_in, 0.95))
    return {
        "voiced_frames": int(voiced.sum()),
        "accurate_frames": int(accurate.sum()),
        "strict_accurate_frames": int((voiced & (error <= 15)).sum()),
        "leakage_frames": int(leaked.sum()),
        "distinct_frames": int(distinguishable.sum()),
        "below_100_hz_db": round(
            10 * np.log10(max(low_out, 1e-20) / max(low_in, 1e-20)), 2
        ),
        "flux_spikes_pct": round(100 * spike.mean(), 2),
        **onset_metrics(voiced, accurate, leaked & ~accurate),
    }


def evaluate(args):
    args.output.mkdir(parents=True, exist_ok=True)
    pitch_root = args.output.parent / "reference"
    pitch_root.mkdir(exist_ok=True)
    rows = []
    for source in sorted(args.corpus.glob("*.wav")):
        if args.only and not source.stem.startswith(tuple(args.only)):
            continue
        if args.exclude and source.stem.startswith(tuple(args.exclude)):
            continue
        audio = read(source)
        cache = (
            pitch_root / f"{source.stem}-{len(audio)}-{source.stat().st_mtime_ns}.npy"
        )
        if cache.exists():
            reference = np.load(cache)
        else:
            reference = pitch(audio)
            np.save(cache, reference)
        depth = vibrato_depth(reference)
        tolerance = 15 + args.expression / 100 * depth
        in_features = features(audio)
        source_power = frame_power(audio)
        print(source.stem, flush=True)
        for quality in args.quality or QUALITIES:
            destination = args.output / f"{source.stem}-{quality}-a2.wav"
            render(args.binary, source, destination, quality, "A2", args.expression)
            output = read(destination)[: len(audio)]
            out_pitch = pitch(output)
            np.savez(
                args.output / f"{source.stem}-{quality}-pitch.npz",
                reference=reference,
                output=out_pitch,
                vibrato_depth=depth,
            )
            rows.append(
                {
                    "file": source.stem,
                    "quality": quality,
                    "expression_pct": args.expression,
                    **metrics(
                        reference,
                        out_pitch,
                        tolerance,
                        in_features,
                        features(output),
                        220,
                    ),
                    "consonant_rms_ratio": consonant_ratio(
                        np.isfinite(reference), source_power, frame_power(output)
                    ),
                }
            )
            if args.chords and source.stem.startswith(tuple(args.chords)):
                render(
                    args.binary,
                    source,
                    args.output / f"{source.stem}-{quality}-chord.wav",
                    quality,
                    NOTES,
                    args.expression,
                )
    if not rows:
        raise SystemExit(f"No matching WAV files in {args.corpus}")
    with (args.output / "metrics.csv").open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


def main():
    parser = argparse.ArgumentParser(
        description="Render and measure absolute-mode retuning."
    )
    parser.add_argument("corpus", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument(
        "--binary", type=Path, default=Path("target/release/harmonizer")
    )
    parser.add_argument(
        "--only", action="append", help="limit inputs to this filename prefix"
    )
    parser.add_argument("--exclude", action="append", help="skip this filename prefix")
    parser.add_argument("--quality", choices=QUALITIES, action="append")
    parser.add_argument(
        "--chords", action="append", help="also render a chord for this filename prefix"
    )
    parser.add_argument(
        "--expression", type=float, default=100, help="expression percentage (0–100)"
    )
    args = parser.parse_args()
    if not 0 <= args.expression <= 100:
        parser.error("--expression must be in 0..=100")
    evaluate(args)


if __name__ == "__main__":
    main()
