use std::path::Path;

use anyhow::{Context, Result, bail};
use midly::{Format, MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};

const DEFAULT_US_PER_QUARTER: u32 = 500_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiEventKind {
    NoteOn { note: u8, velocity: f32 },
    NoteOff { note: u8 },
    PitchBend(f32),
    Sustain(bool),
    ReleaseAll,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiEvent {
    pub sample: usize,
    pub kind: MidiEventKind,
}

#[derive(Clone, Copy)]
struct TempoSegment {
    start_tick: u64,
    start_seconds: f64,
    us_per_quarter: u32,
}

struct TempoMap {
    ticks_per_quarter: u64,
    segments: Vec<TempoSegment>,
}

impl TempoMap {
    fn build(ticks_per_quarter: u64, mut changes: Vec<(u64, usize, usize, u32)>) -> Result<Self> {
        changes.sort_unstable_by_key(|&(tick, track, index, _)| (tick, track, index));
        let mut segments = vec![TempoSegment {
            start_tick: 0,
            start_seconds: 0.0,
            us_per_quarter: DEFAULT_US_PER_QUARTER,
        }];

        for (tick, _, _, us_per_quarter) in changes {
            if us_per_quarter == 0 {
                bail!("MIDI file contains a zero tempo meta event");
            }
            let last = *segments.last().expect("seeded with tick 0");
            if tick == last.start_tick {
                segments.last_mut().expect("non-empty").us_per_quarter = us_per_quarter;
            } else {
                let elapsed = ticks_to_seconds(
                    tick - last.start_tick,
                    last.us_per_quarter,
                    ticks_per_quarter,
                );
                segments.push(TempoSegment {
                    start_tick: tick,
                    start_seconds: last.start_seconds + elapsed,
                    us_per_quarter,
                });
            }
        }

        Ok(Self {
            ticks_per_quarter,
            segments,
        })
    }

    fn tick_to_seconds(&self, tick: u64) -> f64 {
        let idx = self
            .segments
            .partition_point(|seg| seg.start_tick <= tick)
            .saturating_sub(1);
        let seg = self.segments[idx];
        seg.start_seconds
            + ticks_to_seconds(
                tick - seg.start_tick,
                seg.us_per_quarter,
                self.ticks_per_quarter,
            )
    }
}

fn ticks_to_seconds(ticks: u64, us_per_quarter: u32, ticks_per_quarter: u64) -> f64 {
    (ticks as f64) * (f64::from(us_per_quarter) / 1_000_000.0) / (ticks_per_quarter as f64)
}

fn normalise_pitch_bend(raw: u16) -> f32 {
    // 14-bit MIDI pitch bend centres at 8192 (0x2000), spanning 0..=16383.
    if raw >= 8192 {
        (f32::from(raw - 8192) / 8191.0).clamp(0.0, 1.0)
    } else {
        (-(f32::from(8192 - raw) / 8192.0)).clamp(-1.0, 0.0)
    }
}

pub fn parse_midi_bytes(bytes: &[u8], sample_rate: u32) -> Result<Vec<MidiEvent>> {
    if sample_rate == 0 {
        bail!("sample rate must be positive");
    }

    let smf = Smf::parse(bytes).context("failed to parse standard MIDI file")?;
    match smf.header.format {
        Format::SingleTrack | Format::Parallel => {}
        Format::Sequential => {
            bail!("unsupported MIDI format 2 (sequential tracks); expected format 0 or 1");
        }
    }

    let ticks_per_quarter = match smf.header.timing {
        Timing::Metrical(tpq) => {
            let value = u64::from(tpq.as_int());
            if value == 0 {
                bail!("MIDI file has zero ticks per quarter note");
            }
            value
        }
        Timing::Timecode(..) => {
            bail!("SMPTE timecode MIDI files are not supported; expected metrical (PPQ) timing");
        }
    };

    let mut tempo_changes = Vec::new();
    let mut raw_events = Vec::new();

    for (track_idx, track) in smf.tracks.iter().enumerate() {
        let mut tick = 0_u64;
        for (event_idx, event) in track.iter().enumerate() {
            tick += u64::from(event.delta.as_int());
            match event.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(us)) => {
                    tempo_changes.push((tick, track_idx, event_idx, us.as_int()));
                }
                TrackEventKind::Midi { message, .. } => {
                    let kind = match message {
                        MidiMessage::NoteOn { key, vel } => {
                            let vel_u8 = vel.as_int();
                            if vel_u8 == 0 {
                                Some(MidiEventKind::NoteOff { note: key.as_int() })
                            } else {
                                Some(MidiEventKind::NoteOn {
                                    note: key.as_int(),
                                    velocity: f32::from(vel_u8) / 127.0,
                                })
                            }
                        }
                        MidiMessage::NoteOff { key, .. } => {
                            Some(MidiEventKind::NoteOff { note: key.as_int() })
                        }
                        MidiMessage::PitchBend { bend } => Some(MidiEventKind::PitchBend(
                            normalise_pitch_bend(bend.0.as_int()),
                        )),
                        MidiMessage::Controller { controller, value }
                            if controller.as_int() == 64 =>
                        {
                            Some(MidiEventKind::Sustain(value.as_int() >= 64))
                        }
                        MidiMessage::Controller { controller, .. }
                            if matches!(controller.as_int(), 120 | 123) =>
                        {
                            Some(MidiEventKind::ReleaseAll)
                        }
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        raw_events.push((tick, track_idx, event_idx, kind));
                    }
                }
                _ => {}
            }
        }
    }

    let tempo_map = TempoMap::build(ticks_per_quarter, tempo_changes)?;
    raw_events.sort_unstable_by_key(|&(tick, track, index, _)| (tick, track, index));

    let sr = f64::from(sample_rate);
    let events = raw_events
        .into_iter()
        .map(|(tick, _, _, kind)| {
            let seconds = tempo_map.tick_to_seconds(tick);
            let sample = (seconds * sr).round() as usize;
            MidiEvent { sample, kind }
        })
        .collect();
    Ok(events)
}

pub fn load_midi_file(path: &Path, sample_rate: u32) -> Result<Vec<MidiEvent>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read MIDI file {}", path.display()))?;
    parse_midi_bytes(&bytes, sample_rate)
        .with_context(|| format!("failed to parse MIDI file {}", path.display()))
}

#[cfg(test)]
mod tests {
    use midly::num::{u4, u7, u14, u15, u24, u28};
    use midly::{
        Format, Fps, Header, MetaMessage, MidiMessage, PitchBend, Smf, Timing, TrackEvent,
        TrackEventKind,
    };

    use super::*;

    fn encode_smf(smf: &Smf<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        smf.write(&mut bytes).expect("encode SMF");
        bytes
    }

    #[test]
    fn converts_ticks_to_samples_across_mid_file_tempo_change() {
        let track = vec![
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
            },
            TrackEvent {
                delta: u28::new(240),
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
                kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(1_000_000))),
            },
            TrackEvent {
                delta: u28::new(480),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(60),
                        vel: u7::new(0),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            },
        ];
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))),
            tracks: vec![track],
        };

        let events = parse_midi_bytes(&encode_smf(&smf), 48_000).unwrap();
        assert_eq!(
            events,
            vec![
                MidiEvent {
                    sample: 12_000,
                    kind: MidiEventKind::NoteOn {
                        note: 60,
                        velocity: 1.0,
                    },
                },
                MidiEvent {
                    sample: 72_000,
                    kind: MidiEventKind::NoteOff { note: 60 },
                },
            ]
        );
    }

    #[test]
    fn merges_format_1_tracks_and_reads_tempo_changes_from_any_track() {
        let track0 = vec![
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(60),
                        vel: u7::new(100),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(960),
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
        ];
        let track1 = vec![
            TrackEvent {
                delta: u28::new(480),
                kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(1_000_000))),
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(5),
                    message: MidiMessage::NoteOn {
                        key: u7::new(64),
                        vel: u7::new(127),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            },
        ];
        let smf = Smf {
            header: Header::new(Format::Parallel, Timing::Metrical(u15::new(480))),
            tracks: vec![track0, track1],
        };

        let events = parse_midi_bytes(&encode_smf(&smf), 48_000).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].sample, 0);
        assert_eq!(events[1].sample, 24_000);
        assert_eq!(
            events[1].kind,
            MidiEventKind::NoteOn {
                note: 64,
                velocity: 1.0,
            }
        );
        assert_eq!(events[2].sample, 72_000);
        assert_eq!(events[2].kind, MidiEventKind::NoteOff { note: 60 });
    }

    #[test]
    fn handles_pitch_bend_and_all_notes_off_controllers() {
        let track = vec![
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::PitchBend {
                        bend: PitchBend(u14::new(0)),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(1),
                    message: MidiMessage::PitchBend {
                        bend: PitchBend(u14::new(8192)),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(2),
                    message: MidiMessage::PitchBend {
                        bend: PitchBend(u14::new(16383)),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::Controller {
                        controller: u7::new(123),
                        value: u7::new(0),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::Controller {
                        controller: u7::new(120),
                        value: u7::new(0),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            },
        ];
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))),
            tracks: vec![track],
        };

        let events = parse_midi_bytes(&encode_smf(&smf), 48_000).unwrap();
        assert_eq!(
            events,
            vec![
                MidiEvent {
                    sample: 0,
                    kind: MidiEventKind::PitchBend(-1.0),
                },
                MidiEvent {
                    sample: 0,
                    kind: MidiEventKind::PitchBend(0.0),
                },
                MidiEvent {
                    sample: 0,
                    kind: MidiEventKind::PitchBend(1.0),
                },
                MidiEvent {
                    sample: 0,
                    kind: MidiEventKind::ReleaseAll,
                },
                MidiEvent {
                    sample: 0,
                    kind: MidiEventKind::ReleaseAll,
                },
            ]
        );
    }

    #[test]
    fn rejects_smpte_timecode_and_format_2_midi() {
        let smpte = Smf {
            header: Header::new(Format::SingleTrack, Timing::Timecode(Fps::Fps25, 40)),
            tracks: vec![vec![TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            }]],
        };
        let err = parse_midi_bytes(&encode_smf(&smpte), 48_000).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("smpte"));

        let seq = Smf {
            header: Header::new(Format::Sequential, Timing::Metrical(u15::new(480))),
            tracks: vec![vec![TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            }]],
        };
        assert!(parse_midi_bytes(&encode_smf(&seq), 48_000).is_err());
    }

    #[test]
    fn parses_sustain_pedal_cc64_threshold() {
        let track = vec![
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::Controller {
                        controller: u7::new(64),
                        value: u7::new(63),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(240),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::Controller {
                        controller: u7::new(64),
                        value: u7::new(64),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(240),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::Controller {
                        controller: u7::new(64),
                        value: u7::new(0),
                    },
                },
            },
            TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            },
        ];
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))),
            tracks: vec![track],
        };

        let events = parse_midi_bytes(&encode_smf(&smf), 48_000).unwrap();
        assert_eq!(
            events,
            vec![
                MidiEvent {
                    sample: 0,
                    kind: MidiEventKind::Sustain(false),
                },
                MidiEvent {
                    sample: 12_000,
                    kind: MidiEventKind::Sustain(true),
                },
                MidiEvent {
                    sample: 24_000,
                    kind: MidiEventKind::Sustain(false),
                },
            ]
        );
    }
}
