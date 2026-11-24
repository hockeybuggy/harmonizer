# Harmonizer

A realtime polyphonic harmonizer, driven by MIDI. Hold a note and the incoming
audio is re-pitched to it; hold a chord and you get one voice per note.

- **Absolute mode** tracks the pitch of a monophonic input (voice, solo
  instrument) and resynthesises it around each held note, using pitch detection
  and PSOLA.
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

The harmonizer needs two things: audio to re-pitch, and MIDI notes saying
which pitches to make. With **Dry** at −∞ (the default) it's silent until both
arrive. The editor says which one is missing.

### Audio effect (Harmonizer)

1. Put **Harmonizer** on the audio track you want harmonized. To sing live,
   set that track's input to your microphone and its **Monitor** to **In**
   (or **Auto**, then arm it). Use headphones, or the harmony will feed back
   through the mic.
2. Create a MIDI track for the notes. Set its **MIDI To** to the audio track,
   and the chooser underneath to **Harmonizer**.
3. Arm the MIDI track to play notes from a keyboard (or Live's computer
   keyboard), or give it a MIDI clip.

### Instrument (Harmonizer Instrument)

1. Put **Harmonizer Instrument** on a MIDI track. Arm it to play notes, or give
   it a MIDI clip.
2. In the device's **Sidechain** menu, choose the track to harmonize.
3. That track must be producing audio: a playing clip, or a microphone with
   **Monitor** on **In**. Mute it, or turn **Dry** up on the harmonizer,
   depending on whether you want to hear the original too.

### If it's silent

The line under the title explains what's missing:

- *No audio coming in* or *No sidechain audio*: nothing reaches the plugin.
  Check the track input, its Monitor setting, and the sidechain source.
- *Waiting for notes*: no MIDI has arrived. Check MIDI To, and that the MIDI
  track is armed or playing a clip.
- *No clear pitch*: Absolute mode follows a single voice. Use Relative for
  chords, drums and other polyphonic input.

The small bar left of the status line is the input level.

### Tips

- Raise **Dry** to blend the original in at the same latency.
- **Low latency** is for playing live through monitors; **High quality** is for
  mixing, where Live's delay compensation hides the latency.
- **Expression** (Absolute only) keeps vibrato and scoops around each held note:
  100% by default, 0% for fixed pitch (`--expression 0` in the CLI).
- **Spread** pans voices by pitch, low notes left and high notes right.
- Pitch bend moves every voice by up to **Bend range** semitones, and the
  sustain pedal holds voices after you let go of the keys.
- Each voice is as loud as the input, so a three-note chord peaks a few dB
  above it. Lower **Wet** if the track clips.

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
cargo test -p harmonizer-dsp -p harmonizer-cli
cargo clippy --workspace --all-targets
cargo xtask bundle harmonizer-plugin --release
cargo run --release -p vst3-smoke -- target/bundled/Harmonizer.vst3
```

`vst3-smoke` is a minimal VST3 host: it loads the bundle, checks both plugin
classes' buses, parameters, latency reporting, MIDI-driven audio, state and
editor, and exits non-zero on the first problem. Build the bundle without
`--release` to also have nice-plug abort on any allocation in the audio
callback.

`crates/harmonizer-dsp` holds all the DSP with no plugin-framework
dependencies; `crates/harmonizer-plugin` wraps it with
[nice-plug](https://codeberg.org/RustAudio/nice-plug); `crates/harmonizer-cli`
drives it offline the way a host would.
