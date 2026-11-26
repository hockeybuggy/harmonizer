# Developing Harmonizer

- [DSP](../crates/harmonizer-dsp/src/lib.rs) — voice allocation, pitch tracking and PSOLA for Absolute mode, and a phase vocoder for Relative mode. No plugin-framework dependencies.
- [VST3 plugin](../crates/harmonizer-plugin/src/lib.rs) — the nice-plug wrapper and egui editor.
- [CLI](../crates/harmonizer-cli/src/lib.rs) — the offline WAV renderer and MIDI reader.

From the repo root:

```sh
cargo test -p harmonizer-dsp -p harmonizer-cli
cargo clippy --workspace --all-targets
```

On macOS, validate both plugin variants with the [VST3 smoke host](../crates/vst3-smoke/README.md):

```sh
cargo xtask bundle harmonizer-plugin --release
cargo run --release -p vst3-smoke -- target/bundled/Harmonizer.vst3
```

Run the bundler from a regular checkout. From a nested Git worktree it picks
the outermost `Cargo.toml` and builds the main checkout instead. Omit
`--release` from the bundle command to let nice-plug catch allocations in
the audio callback in a debug build.

With a GPU, regenerate editor screenshots under `target/editor-*.png`:

```sh
cargo test -p harmonizer-plugin --lib render_editor_snapshots -- --ignored
```

[Back to the README](../README.md)
