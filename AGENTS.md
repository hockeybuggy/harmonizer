# Harmonizer

A realtime, MIDI-driven polyphonic harmonizer written in Rust. Each held MIDI
note gets a voice that re-pitches the incoming audio to that note. It ships as
a VST3 plugin (for Ableton Live) and an offline CLI.

## Layout

- `crates/harmonizer-dsp` — all DSP. No plugin-framework dependencies.
  - `harmonizer.rs` — `Harmonizer` facade: voice allocation, envelopes, dry
    delay, wet/dry mix, engine selection.
  - `engine.rs` — the `Engine` trait both re-pitching engines implement.
  - `psola.rs` + `pitch.rs` — absolute mode: pitch tracking and PSOLA.
  - `vocoder.rs` — relative mode: phase vocoder with formant preservation.
- `crates/harmonizer-plugin` — VST3 wrapper (nice-plug) and egui editor.
- `crates/harmonizer-cli` — offline renderer (`harmonizer` binary).
- `crates/vst3-smoke` — a minimal VST3 host that loads the built bundle,
  drives both plugin classes like a DAW, and fails on any defect.
- `xtask` — `cargo xtask bundle harmonizer-plugin --release` builds
  `target/bundled/Harmonizer.vst3`.
- `vendor/nice-plug` — nice-plug 0.4.2 with a VST3 bus-index fix, wired in
  through `[patch.crates-io]` (see `vendor/README.md`).

## Rules

- Realtime safety: `Engine::process`, `Harmonizer::process` and everything they
  call must not allocate, lock, or do unbounded work. Allocate in constructors.
  `crates/harmonizer-dsp/tests/realtime.rs` enforces this with
  `assert_no_alloc`; keep it passing.
- Engines must produce identical output regardless of how the input is split
  into blocks (hosts send anything from 1 to thousands of samples).
- Latency reported by `Engine::latency()` must be exact: the dry signal is
  delayed by it and the host compensates for it.
- Comments only for non-obvious *why*. No narrative or section-header comments.
- Canadian spelling in prose and identifiers (colour, behaviour, centre).
- Tests: cover behaviour with measurable assertions (detected pitch of the
  output, latency alignment, block-size invariance). No trivial tests.

## Commands

```sh
cargo test -p harmonizer-dsp
cargo clippy --workspace --all-targets
cargo xtask bundle harmonizer-plugin --release
cargo run --release -p vst3-smoke -- target/bundled/Harmonizer.vst3
cargo run --release -p harmonizer-cli -- --help
cargo test -p harmonizer-plugin -- --ignored  # writes target/editor-*.png
```

`cargo xtask bundle` builds the outermost directory with a `Cargo.toml`, so
in a worktree under `.worktrees/` it bundles the main checkout's code into the
main checkout's `target/bundled`. Bundle from the main checkout.
