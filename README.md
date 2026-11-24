# Harmonizer

A realtime polyphonic harmonizer, driven by MIDI. Hold a note and the incoming
audio is re-pitched to it; hold a chord and you get one voice per note.

- **Absolute mode** tracks the pitch of a monophonic input (voice, solo
  instrument) and resynthesises it at exactly each held note.
- **Relative mode** transposes any input by the interval between each held
  note and a reference note (C3 by default), using a phase vocoder.

Both modes preserve formants, with a knob to shift them. A quality setting
trades latency against smoothness.

Runs as a VST3 plugin or as an offline command-line renderer.

## Building

```sh
cargo xtask bundle harmonizer-plugin --release
cp -R target/bundled/Harmonizer.vst3 ~/Library/Audio/Plug-Ins/VST3/
```
