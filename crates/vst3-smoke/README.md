# VST3 smoke host

On macOS, build the release bundle from the root of a regular checkout and run:

```sh
cargo xtask bundle harmonizer-plugin --release
cargo run -p vst3-smoke --release -- target/bundled/Harmonizer.vst3
```

The host loads both Harmonizer classes, validates the public parameter IDs and
names, and renders three seconds of stereo audio at 48 kHz. It sends MIDI on
both classes, checks silence and signal levels, automates Mode and Quality,
verifies a latency-change notification, restores a saved state, and queries
`NSView` support without opening a window. It exits non-zero on the first failed
check. It does not replace a DAW test or a general VST3 validator.

The loader executes native code from the supplied bundle; use only trusted
builds. For nested Git worktrees, note that `nice-plug-xtask` looks for the
outermost ancestor with a `Cargo.toml`, not the nearest workspace root.
