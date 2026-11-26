# Using Harmonizer in Ableton Live

Harmonizer needs audio to re-pitch and MIDI notes to tell it what pitches to
make. With **Dry** at −∞ (the default), it's silent until both arrive.

## Install on macOS

Install [Rust](https://rustup.rs/), then build and copy the plugin bundle from
the repo root:

```sh
cargo xtask bundle harmonizer-plugin --release
mkdir -p ~/Library/Audio/Plug-Ins/VST3
cp -R target/bundled/Harmonizer.vst3 ~/Library/Audio/Plug-Ins/VST3/
```

In Live, enable **Use VST3 Plug-in System Folders** under **Settings → Plug-Ins**
and rescan. The bundle contains two devices with the same sound and controls.
Pick one based on how you want to route MIDI.

## Route audio and MIDI

### Harmonizer (audio effect)

1. Put **Harmonizer** on the audio track you want to harmonize. If you're
   singing live, select your microphone and set **Monitor** to **In** (or
   **Auto** and arm the track). Use headphones to avoid feedback.
2. On a separate MIDI track, set **MIDI To** to that audio track. In the
   chooser underneath, select **Harmonizer**.
3. Arm the MIDI track and play notes, or launch a MIDI clip on it.

### Harmonizer Instrument (sidechain)

Use the instrument if your host won't send MIDI to an audio effect.

1. Put **Harmonizer Instrument** on a MIDI track. Arm it to play live, or
   launch a MIDI clip.
2. In the device's **Sidechain** menu, select the audio track you want to
   harmonize.
3. Play a clip on that audio track, or select a live input and enable
   monitoring. Raise **Dry** if you want the original in the plugin's mix.

## Shape the sound

- **Absolute** follows a single pitched source, such as a voice or a solo
  instrument. **Expression** controls how much vibrato and pitch movement
  carry through: 100% keeps it, while 0% locks the voices to the MIDI notes.
- **Relative** re-pitches the whole input by the interval from **Reference**
  (C3 by default) to each held note. Use it for chords, pads, and drums.
- **Formant** changes the character of the voices without changing the notes.
  At 0 semitones, it preserves the input formants.
- **Quality** trades latency for smoothness. Use **Low** for live monitoring
  and try **High** for mixing, where Live compensates for the reported latency.
- **Dry** blends in the original, aligned to the harmonized voices. **Spread**
  pans lower voices left and higher ones right. Leave some headroom: even
  though chords are level-balanced, peaks can still add up.
- **Bend range** sets how far the pitch wheel bends the notes. The sustain
  pedal holds voices after you release their keys.

Open the plugin window (the wrench icon) to see the input level, detected pitch
(or reference note in Relative mode), and sounding voices. You can also unfold
Live's device with the triangle in its title bar to adjust, automate, or MIDI-map
parameters without opening the plugin window.

## If it's silent

| What the plugin says | What to check |
| --- | --- |
| **No audio coming in** or **No sidechain audio** | Check the audio source, track monitoring, and sidechain selection. |
| **Waiting for notes** | Check the MIDI track's output routing, arm state, or clip playback. |
| **No clear pitch** | Feed Absolute mode a single pitched voice. Switch to Relative mode for chords, drums, or other polyphonic input. |

[Back to the README](../README.md)
