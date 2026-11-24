use std::borrow::Cow;

use harmonizer_dsp::{Harmonizer, Params};

use crate::midi::{MidiEvent, MidiEventKind};
use crate::wav::AudioBuffer;

const TAIL_MARGIN_SECONDS: f32 = 0.05;

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub params: Params,
    pub block_size: usize,
    pub compensate_latency: bool,
}

#[derive(Clone, Debug)]
pub enum NoteSource {
    Events(Vec<MidiEvent>),
    Held { notes: Vec<u8>, velocity: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderSummary {
    pub sample_rate: u32,
    pub channels: u16,
    pub output_frames: usize,
    pub note_count: usize,
    pub latency_samples: usize,
    pub latency_compensated: bool,
}

impl RenderSummary {
    pub fn format_line(&self) -> String {
        let duration = self.output_frames as f64 / f64::from(self.sample_rate);
        let latency_ms = self.latency_samples as f64 * 1000.0 / f64::from(self.sample_rate);
        let ch_label = if self.channels == 1 { "mono" } else { "stereo" };
        let note_word = if self.note_count == 1 {
            "note"
        } else {
            "notes"
        };
        let comp_label = if self.latency_compensated {
            "compensated"
        } else {
            "raw"
        };
        format!(
            "Rendered {duration:.2}s ({} Hz, {ch_label}), {} {note_word}, latency {} samples ({latency_ms:.1} ms, {comp_label})",
            self.sample_rate, self.note_count, self.latency_samples
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlockAction<'a> {
    Process { start: usize, end: usize },
    Event(&'a MidiEvent),
}

pub fn run_blocks<F>(
    total_samples: usize,
    block_size: usize,
    events: &[MidiEvent],
    mut on_action: F,
) where
    F: FnMut(BlockAction<'_>),
{
    assert!(block_size > 0, "block_size must be positive");
    let mut event_idx = 0;
    let mut block_start = 0;

    while block_start < total_samples {
        let block_end = (block_start + block_size).min(total_samples);
        let mut cursor = block_start;

        while event_idx < events.len() && events[event_idx].sample < block_end {
            let ev = &events[event_idx];
            let ev_sample = ev.sample.max(cursor);
            if ev_sample > cursor {
                on_action(BlockAction::Process {
                    start: cursor,
                    end: ev_sample,
                });
                cursor = ev_sample;
            }
            on_action(BlockAction::Event(ev));
            event_idx += 1;
        }

        if cursor < block_end {
            on_action(BlockAction::Process {
                start: cursor,
                end: block_end,
            });
        }
        block_start = block_end;
    }
}

pub fn held_note_events(notes: &[u8], velocity: f32, release_at_sample: usize) -> Vec<MidiEvent> {
    let mut events = Vec::with_capacity(notes.len() * 2);
    for &note in notes {
        events.push(MidiEvent {
            sample: 0,
            kind: MidiEventKind::NoteOn { note, velocity },
        });
    }
    for &note in notes {
        events.push(MidiEvent {
            sample: release_at_sample,
            kind: MidiEventKind::NoteOff { note },
        });
    }
    events
}

pub fn render_audio(
    input: &AudioBuffer,
    source: &NoteSource,
    options: &RenderOptions,
) -> (AudioBuffer, RenderSummary) {
    let sample_rate_f32 = input.sample_rate as f32;
    let mut harmonizer = Harmonizer::new(sample_rate_f32, options.block_size);
    harmonizer.set_params(options.params);
    // Snap wet/dry gain smoothers to their configured initial targets before rendering sample 0.
    harmonizer.reset();

    let latency = harmonizer.latency();
    let release_samples = (options.params.release_ms * 0.001 * sample_rate_f32).ceil() as usize;
    let margin_samples = (TAIL_MARGIN_SECONDS * sample_rate_f32).ceil() as usize;
    let tail_samples = latency + release_samples + margin_samples;
    let total_samples = input.frames() + tail_samples;

    let events: Cow<'_, [MidiEvent]> = match source {
        NoteSource::Events(ev) => Cow::Borrowed(ev.as_slice()),
        NoteSource::Held { notes, velocity } => {
            // Release after the last input sample has emerged from the engine's latency buffer.
            Cow::Owned(held_note_events(notes, *velocity, input.frames() + latency))
        }
    };

    let mut channels: Vec<Vec<f32>> = input
        .channels
        .iter()
        .map(|ch| {
            let mut padded = Vec::with_capacity(total_samples);
            padded.extend_from_slice(ch);
            padded.resize(total_samples, 0.0);
            padded
        })
        .collect();

    run_blocks(
        total_samples,
        options.block_size,
        &events,
        |action| match action {
            BlockAction::Process { start, end } => match channels.as_mut_slice() {
                [mono] => harmonizer.process(&mut [&mut mono[start..end]]),
                [left, right] => {
                    harmonizer.process(&mut [&mut left[start..end], &mut right[start..end]])
                }
                _ => unreachable!("WAV input is validated to mono or stereo"),
            },
            BlockAction::Event(ev) => match ev.kind {
                MidiEventKind::NoteOn { note, velocity } => harmonizer.note_on(note, velocity),
                MidiEventKind::NoteOff { note } => harmonizer.note_off(note),
                MidiEventKind::PitchBend(bend) => harmonizer.set_pitch_bend(bend),
                MidiEventKind::Sustain(down) => harmonizer.set_sustain(down),
                MidiEventKind::ReleaseAll => harmonizer.release_all(),
            },
        },
    );

    if options.compensate_latency {
        for channel in &mut channels {
            channel.drain(..latency);
        }
    }

    let note_count = events
        .iter()
        .filter(|ev| matches!(ev.kind, MidiEventKind::NoteOn { .. }))
        .count();
    let output_frames = channels.first().map_or(0, Vec::len);

    let summary = RenderSummary {
        sample_rate: input.sample_rate,
        channels: input.channel_count(),
        output_frames,
        note_count,
        latency_samples: latency,
        latency_compensated: options.compensate_latency,
    };

    (
        AudioBuffer {
            sample_rate: input.sample_rate,
            channels,
        },
        summary,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiEventKind;

    #[test]
    fn splits_block_at_event_offset_across_block_boundaries() {
        #[derive(Debug, PartialEq)]
        enum Step {
            Process {
                block: usize,
                offset: usize,
                len: usize,
                start: usize,
                end: usize,
            },
            Event {
                block: usize,
                offset: usize,
                sample: usize,
            },
        }

        let block_size = 256;
        let events = [MidiEvent {
            sample: 300,
            kind: MidiEventKind::NoteOn {
                note: 60,
                velocity: 1.0,
            },
        }];
        let mut steps = Vec::new();
        run_blocks(600, block_size, &events, |action| match action {
            BlockAction::Process { start, end } => steps.push(Step::Process {
                block: start / block_size,
                offset: start % block_size,
                len: end - start,
                start,
                end,
            }),
            BlockAction::Event(ev) => steps.push(Step::Event {
                block: ev.sample / block_size,
                offset: ev.sample % block_size,
                sample: ev.sample,
            }),
        });

        assert_eq!(
            steps,
            vec![
                Step::Process {
                    block: 0,
                    offset: 0,
                    len: 256,
                    start: 0,
                    end: 256,
                },
                Step::Process {
                    block: 1,
                    offset: 0,
                    len: 44,
                    start: 256,
                    end: 300,
                },
                Step::Event {
                    block: 1,
                    offset: 44,
                    sample: 300,
                },
                Step::Process {
                    block: 1,
                    offset: 44,
                    len: 212,
                    start: 300,
                    end: 512,
                },
                Step::Process {
                    block: 2,
                    offset: 0,
                    len: 88,
                    start: 512,
                    end: 600,
                },
            ]
        );
    }

    #[test]
    fn handles_event_at_exact_block_boundary_without_empty_slice() {
        let events = [
            MidiEvent {
                sample: 0,
                kind: MidiEventKind::NoteOn {
                    note: 60,
                    velocity: 1.0,
                },
            },
            MidiEvent {
                sample: 256,
                kind: MidiEventKind::NoteOff { note: 60 },
            },
        ];
        let mut slices = Vec::new();
        let mut seen = Vec::new();
        run_blocks(512, 256, &events, |action| match action {
            BlockAction::Process { start, end } => slices.push((start, end)),
            BlockAction::Event(ev) => seen.push(ev.sample),
        });
        assert_eq!(slices, vec![(0, 256), (256, 512)]);
        assert_eq!(seen, vec![0, 256]);
    }
}
