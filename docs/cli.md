# Rendering audio from the command line

The `harmonizer` CLI takes a mono or stereo WAV and either a fixed set of
notes or a standard MIDI file. It writes a 32-bit float WAV at the same sample
rate and channel count, with the engine's latency compensated by default.

From the repo root, hold a C major chord over an isolated vocal:

```sh
cargo run --release -p harmonizer-cli -- \
  --input vocal.wav --output harmony.wav --notes C3,E3,G3
```

Or use a MIDI file to shift a pad by an interval for each note:

```sh
cargo run --release -p harmonizer-cli -- \
  --input pad.wav --output harmony.wav --midi chords.mid --mode relative
```

Note names follow Ableton Live's convention: **C3 is MIDI note 60**. You can
also pass note numbers, such as `--notes 60,64,67`. The Relative mode
reference defaults to C3; override it with `--reference-note`.

The output is wet-only by default. Add `--dry-db -6` to blend in the original,
or `--expression 0` in Absolute mode to keep pitches fixed to the MIDI notes.

```sh
cargo run --release -p harmonizer-cli -- --help
```

[Back to the README](../README.md)
