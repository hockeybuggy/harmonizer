# Harmonizer

A realtime polyphonic harmonizer, driven by MIDI. Hold a note and the incoming
audio is re-pitched to it; hold a chord and you get one voice per note.

- **Absolute mode** tracks the pitch of a monophonic input (voice, solo
  instrument) and resynthesises it at exactly each held note, using pitch
  detection and PSOLA. The singer's own pitch doesn't matter: a held A3 always
  comes out at A3.
- **Relative mode** transposes any input — chords, pads, drums — by the
  interval between each held note and a reference note (C3 by default), using
  a phase vocoder.

Both modes preserve formants, so voices don't turn into chipmunks, and have a
**Formant** knob to shift them for character. **Quality** trades latency for
smoothness; the latency is reported to the host, which compensates for it.

Runs as a VST3 plugin or as an offline command-line renderer.

## Building and installing

```sh
cargo xtask bundle harmonizer-plugin --release
cp -R target/bundled/Harmonizer.vst3 ~/Library/Audio/Plug-Ins/VST3/
```

In Live, enable **Use VST3 Plug-in System Folders** under Settings → Plug-Ins
and rescan. The bundle contains two plugins with the same sound and controls:

- **Harmonizer** — an audio effect. Put it on the audio track you want
  harmonized.
- **Harmonizer Instrument** — an instrument that takes its audio through a
  sidechain input. Use it if your host won't send MIDI to an audio effect.

## Using it in Ableton Live

### Audio effect (Harmonizer)

1. Put **Harmonizer** on the audio track (e.g. your vocal).
2. Create a MIDI track for the notes. Set its **MIDI To** to the vocal track,
   and the second chooser to **Harmonizer**.
3. Arm the MIDI track (or set its monitor to **In**) and play or record notes.

### Instrument (Harmonizer Instrument)

1. Put **Harmonizer Instrument** on a MIDI track.
2. In the device's title bar, open the sidechain section and pick the audio
   track you want harmonized as the source.
3. Play or record notes on that MIDI track. Mute the source track, or turn
   **Dry** up on the harmonizer, depending on whether you want the original
   to be heard.

### Tips

- With **Dry** at −∞ (the default) you only hear the harmony voices; raise it to
  blend the original in at the same latency.
- **Low latency** is for playing live through monitors; **High quality** is for
  mixing, where Live's delay compensation hides the latency.
- **Spread** pans voices by pitch, low notes left and high notes right.
- Pitch bend moves every voice by up to **Bend range** semitones.

## Offline rendering

```sh
cargo run --release -p harmonizer-cli -- \
  --input vocal.wav --output harmony.wav --notes C3,E3,G3
cargo run --release -p harmonizer-cli -- \
  --input vocal.wav --output harmony.wav --midi chords.mid --mode relative
```

Run with `--help` for every option. Note names use Live's convention: C3 is
MIDI note 60.

## Development

```sh
cargo test -p harmonizer-dsp
cargo clippy --workspace --all-targets
```

`crates/harmonizer-dsp` holds all the DSP with no plugin-framework
dependencies; `crates/harmonizer-plugin` wraps it with
[nice-plug](https://codeberg.org/RustAudio/nice-plug); `crates/harmonizer-cli`
drives it offline the way a host would.
