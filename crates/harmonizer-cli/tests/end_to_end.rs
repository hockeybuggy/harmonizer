use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "harmonizer-cli-e2e-{}-{}-{label}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_sine_wav(path: &Path, sample_rate: u32, frames: usize, channels: u16) -> Vec<Vec<f32>> {
    let mut data = vec![vec![0.0_f32; frames]; channels as usize];
    for (ch, channel) in data.iter_mut().enumerate() {
        let freq = if ch == 0 { 220.0 } else { 330.0 };
        for (i, sample) in channel.iter_mut().enumerate() {
            *sample = 0.5 * (i as f32 * freq * std::f32::consts::TAU / sample_rate as f32).sin();
        }
    }
    let mut writer = WavWriter::create(
        path,
        WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..frames {
        for ch in &data {
            writer.write_sample(ch[i]).unwrap();
        }
    }
    writer.finalize().unwrap();
    data
}

fn read_float_wav(path: &Path) -> (WavSpec, Vec<Vec<f32>>) {
    let mut reader = WavReader::open(path).unwrap();
    let spec = reader.spec();
    let ch_count = spec.channels as usize;
    let mut channels = vec![Vec::new(); ch_count];
    for (i, sample) in reader.samples::<f32>().enumerate() {
        channels[i % ch_count].push(sample.unwrap());
    }
    (spec, channels)
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

fn write_empty_midi(path: &Path) {
    let smf = Smf {
        header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))),
        tracks: vec![vec![TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        }]],
    };
    let mut bytes = Vec::new();
    smf.write(&mut bytes).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn notes_flag_produces_non_silent_output() {
    let dir = TempDir::new("notes");
    let input_path = dir.path().join("in.wav");
    let output_path = dir.path().join("out.wav");
    write_sine_wav(&input_path, 48_000, 12_000, 1);

    let output = Command::new(env!("CARGO_BIN_EXE_harmonizer"))
        .args([
            "--input",
            input_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--notes",
            "C3,E3,G3",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.trim().is_empty(), "expected summary on stderr");

    let (spec, channels) = read_float_wav(&output_path);
    assert_eq!(spec.sample_rate, 48_000);
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.sample_format, SampleFormat::Float);
    assert_eq!(spec.bits_per_sample, 32);
    assert!(rms(&channels[0][..12_000]) > 0.05);
}

#[test]
fn midi_without_notes_and_default_dry_is_silent() {
    let dir = TempDir::new("silent");
    let input_path = dir.path().join("in.wav");
    let midi_path = dir.path().join("empty.mid");
    let output_path = dir.path().join("out.wav");
    write_sine_wav(&input_path, 48_000, 12_000, 1);
    write_empty_midi(&midi_path);

    let output = Command::new(env!("CARGO_BIN_EXE_harmonizer"))
        .args([
            "--input",
            input_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--midi",
            midi_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_, channels) = read_float_wav(&output_path);
    assert!(rms(&channels[0]) < 1e-6);
}

#[test]
fn dry_only_latency_compensated_matches_input_sample_for_sample() {
    let dir = TempDir::new("dry-match");
    let input_path = dir.path().join("in.wav");
    let output_path = dir.path().join("out.wav");
    let input = write_sine_wav(&input_path, 48_000, 8_000, 2);

    let output = Command::new(env!("CARGO_BIN_EXE_harmonizer"))
        .args([
            "--input",
            input_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--notes",
            "60",
            "--dry-db",
            "0",
            "--wet-db",
            "-inf",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (spec, channels) = read_float_wav(&output_path);
    assert_eq!(spec.channels, 2);
    assert!(channels[0].len() >= input[0].len());
    for ch in 0..2 {
        assert_eq!(&channels[ch][..input[ch].len()], &input[ch][..]);
        assert!(channels[ch][input[ch].len()..].iter().all(|&s| s == 0.0));
    }
}

#[test]
fn no_latency_compensation_preserves_leading_latency() {
    let dir = TempDir::new("raw-latency");
    let input_path = dir.path().join("in.wav");
    let output_path = dir.path().join("out.wav");
    let input = write_sine_wav(&input_path, 48_000, 4_000, 1);

    let output = Command::new(env!("CARGO_BIN_EXE_harmonizer"))
        .args([
            "--input",
            input_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--notes",
            "60",
            "--mode",
            "relative",
            "--quality",
            "balanced",
            "--dry-db",
            "0",
            "--wet-db",
            "-inf",
            "--no-latency-compensation",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_, channels) = read_float_wav(&output_path);
    let mut h = harmonizer_dsp::Harmonizer::new(48_000.0, 256);
    h.set_params(harmonizer_dsp::Params {
        mode: harmonizer_dsp::Mode::Relative,
        quality: harmonizer_dsp::Quality::Balanced,
        ..Default::default()
    });
    let vocoder_latency = h.latency();
    assert!(vocoder_latency > 0);
    assert!(channels[0][..vocoder_latency].iter().all(|&s| s == 0.0));
    assert_eq!(
        &channels[0][vocoder_latency..vocoder_latency + input[0].len()],
        &input[0][..]
    );
}

#[test]
fn midi_file_triggers_note_and_silences_after_release() {
    let dir = TempDir::new("midi-notes");
    let input_path = dir.path().join("in.wav");
    let midi_path = dir.path().join("notes.mid");
    let output_path = dir.path().join("out.wav");
    write_sine_wav(&input_path, 48_000, 48_000, 1);

    let smf = Smf {
        header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))),
        tracks: vec![vec![
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(60),
                        vel: u7::new(127),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(240),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOff {
                        key: u7::new(60),
                        vel: u7::new(0),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            },
        ]],
    };
    let mut bytes = Vec::new();
    smf.write(&mut bytes).unwrap();
    std::fs::write(&midi_path, bytes).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_harmonizer"))
        .args([
            "--input",
            input_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--midi",
            midi_path.to_str().unwrap(),
            "--release-ms",
            "20",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_, channels) = read_float_wav(&output_path);
    assert!(rms(&channels[0][2_000..10_000]) > 0.05);
    assert!(rms(&channels[0][24_000..40_000]) < 1e-6);
}
