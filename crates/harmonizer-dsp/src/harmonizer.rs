use crate::engine::{Engine, MAX_VOICES, VoiceBuffers, VoiceControl};
use crate::params::{Mode, Params, Quality};
use crate::psola::PsolaEngine;
use crate::util::{DelayLine, midi_to_hz, semitones_to_ratio};
use crate::vocoder::VocoderEngine;
use crate::voices::VoiceAllocator;

const GAIN_SMOOTHING_MS: f32 = 10.0;

/// The complete harmonizer: MIDI notes in, re-pitched voices plus dry signal out.
///
/// `process` handles one or two channels in place. Stereo input is summed to mono for the
/// voices; the dry signal keeps its stereo image. Callers that need sample-accurate notes split
/// their buffer at each event and call `note_on`/`note_off` between `process` calls.
pub struct Harmonizer {
    sample_rate: f32,
    params: Params,
    psola: Vec<PsolaEngine>,
    vocoder: Vec<VocoderEngine>,
    voices: VoiceAllocator,
    controls: [VoiceControl; MAX_VOICES],
    buffers: VoiceBuffers,
    mono: Vec<f32>,
    dry_delay: [DelayLine; 2],
    pitch_bend: f32,
    wet_gain: Smoothed,
    dry_gain: Smoothed,
}

impl Harmonizer {
    /// Allocates every engine for every mode and quality up front so switching them at runtime
    /// never allocates.
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        let psola: Vec<_> = Quality::ALL
            .iter()
            .map(|&q| PsolaEngine::new(sample_rate, q))
            .collect();
        let vocoder: Vec<_> = Quality::ALL
            .iter()
            .map(|&q| VocoderEngine::new(sample_rate, q))
            .collect();
        let max_latency = psola
            .iter()
            .map(Engine::latency)
            .chain(vocoder.iter().map(Engine::latency))
            .max()
            .unwrap_or(0);
        let params = Params::default();
        let smoothing = smoothing_coefficient(sample_rate, GAIN_SMOOTHING_MS);
        Self {
            sample_rate,
            params,
            psola,
            vocoder,
            voices: VoiceAllocator::default(),
            controls: [VoiceControl::default(); MAX_VOICES],
            buffers: VoiceBuffers::new(max_block),
            mono: vec![0.0; max_block],
            dry_delay: [DelayLine::new(max_latency), DelayLine::new(max_latency)],
            pitch_bend: 0.0,
            wet_gain: Smoothed::new(params.wet_gain, smoothing),
            dry_gain: Smoothed::new(params.dry_gain, smoothing),
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    pub fn set_params(&mut self, params: Params) {
        let engine_changed =
            params.mode != self.params.mode || params.quality != self.params.quality;
        self.params = params;
        self.wet_gain.target = params.wet_gain;
        self.dry_gain.target = params.dry_gain;
        if engine_changed {
            self.engine_mut().reset();
        }
    }

    pub fn latency(&self) -> usize {
        self.engine().latency()
    }

    pub fn detected_pitch(&self) -> Option<f32> {
        self.engine().detected_pitch()
    }

    pub fn voices(&self) -> &VoiceAllocator {
        &self.voices
    }

    pub fn note_on(&mut self, note: u8, velocity: f32) {
        let allocation = self.voices.note_on(note, velocity);
        if allocation.needs_reset {
            self.engine_mut().reset_voice(allocation.slot);
        }
    }

    pub fn note_off(&mut self, note: u8) {
        self.voices.note_off(note);
    }

    pub fn set_sustain(&mut self, down: bool) {
        self.voices.set_sustain(down);
    }

    pub fn release_all(&mut self) {
        self.voices.release_all();
    }

    /// `bend` is normalised to `-1.0..=1.0` and scaled by [`Params::pitch_bend_range`].
    pub fn set_pitch_bend(&mut self, bend: f32) {
        self.pitch_bend = bend.clamp(-1.0, 1.0);
    }

    pub fn reset(&mut self) {
        for engine in &mut self.psola {
            engine.reset();
        }
        for engine in &mut self.vocoder {
            engine.reset();
        }
        for delay in &mut self.dry_delay {
            delay.reset();
        }
        self.voices.kill_all();
        self.wet_gain.snap();
        self.dry_gain.snap();
    }

    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        let Some(len) = channels.first().map(|c| c.len()) else {
            return;
        };
        let max_block = self.buffers.max_block();
        let release_step = envelope_step(self.sample_rate, self.params.release_ms);
        let mut start = 0;
        while start < len {
            let limit = (len - start).min(max_block);
            let end = start + self.samples_until_retirement(limit, release_step);
            self.process_block(channels, start, end, release_step);
            start = end;
        }
    }

    fn samples_until_retirement(&self, limit: usize, release_step: f32) -> usize {
        let Some(mut level) = self
            .voices
            .active()
            .filter(|(_, voice)| !voice.held && !voice.sustained)
            .map(|(_, voice)| voice.envelope)
            .min_by(f32::total_cmp)
        else {
            return limit;
        };
        // Both engines share unshifted audio across voices, so retire at the same sample
        // regardless of callback size. Repeated subtraction matches the envelope's rounding.
        for samples in 1..limit {
            level -= release_step;
            if level <= 0.0 {
                return samples;
            }
        }
        limit
    }

    fn process_block(
        &mut self,
        channels: &mut [&mut [f32]],
        start: usize,
        end: usize,
        release_step: f32,
    ) {
        let len = end - start;
        let channel_count = channels.len().min(2);
        let scale = 1.0 / channel_count as f32;
        for (i, mono) in self.mono[..len].iter_mut().enumerate() {
            *mono = channels[..channel_count]
                .iter()
                .map(|c| c[start + i])
                .sum::<f32>()
                * scale;
        }

        self.update_controls();
        self.buffers.begin_block(len);
        let controls = self.controls;
        let engine: &mut dyn Engine = match self.params.mode {
            Mode::Absolute => &mut self.psola[self.params.quality.index()],
            Mode::Relative => &mut self.vocoder[self.params.quality.index()],
        };
        engine.process(&self.mono[..len], &controls, &mut self.buffers);
        let latency = engine.latency();

        let attack_step = envelope_step(self.sample_rate, self.params.attack_ms);
        let spread = self.params.stereo_spread;
        let sensitivity = self.params.velocity_sensitivity;

        for (ch, channel) in channels[..channel_count].iter_mut().enumerate() {
            let out = &mut channel[start..end];
            let delay = &mut self.dry_delay[ch];
            let mut dry = self.dry_gain;
            for sample in out.iter_mut() {
                *sample = delay.process(*sample, latency) * dry.next();
            }
            if ch + 1 == channel_count {
                self.dry_gain = dry;
            }
        }

        for slot in 0..MAX_VOICES {
            let voice = self.voices.voices()[slot];
            if !voice.active {
                continue;
            }
            let velocity_gain = 1.0 - sensitivity + sensitivity * voice.velocity;
            let pan = spread * ((voice.note as f32 - 60.0) / 24.0).clamp(-1.0, 1.0);
            let pans = if channel_count == 2 {
                [(1.0 - pan).min(1.0), (1.0 + pan).min(1.0)]
            } else {
                [1.0, 1.0]
            };
            let mut wet = self.wet_gain;
            let rendered = self.buffers.voice(slot);
            for i in 0..len {
                let level = self.voices.step_envelope(slot, attack_step, release_step)
                    * velocity_gain
                    * wet.next();
                for (ch, channel) in channels[..channel_count].iter_mut().enumerate() {
                    channel[start + i] += rendered[i] * level * pans[ch];
                }
            }
        }
        for _ in 0..len {
            self.wet_gain.next();
        }
        self.voices.retire_finished();
    }

    fn update_controls(&mut self) {
        let bend = self.pitch_bend * self.params.pitch_bend_range;
        let formant_ratio = semitones_to_ratio(self.params.formant_semitones);
        for (control, voice) in self.controls.iter_mut().zip(self.voices.voices()) {
            let note = voice.note as f32 + bend;
            *control = VoiceControl {
                active: voice.active,
                pitch: match self.params.mode {
                    Mode::Absolute => midi_to_hz(note),
                    Mode::Relative => semitones_to_ratio(note - self.params.reference_note),
                },
                formant_ratio,
            };
        }
    }

    fn engine(&self) -> &dyn Engine {
        let index = self.params.quality.index();
        match self.params.mode {
            Mode::Absolute => &self.psola[index],
            Mode::Relative => &self.vocoder[index],
        }
    }

    fn engine_mut(&mut self) -> &mut dyn Engine {
        let index = self.params.quality.index();
        match self.params.mode {
            Mode::Absolute => &mut self.psola[index],
            Mode::Relative => &mut self.vocoder[index],
        }
    }
}

fn envelope_step(sample_rate: f32, ms: f32) -> f32 {
    1.0 / (ms.max(0.1) * 0.001 * sample_rate)
}

fn smoothing_coefficient(sample_rate: f32, ms: f32) -> f32 {
    1.0 - (-1.0 / (ms * 0.001 * sample_rate)).exp()
}

#[derive(Clone, Copy)]
struct Smoothed {
    current: f32,
    target: f32,
    coefficient: f32,
}

impl Smoothed {
    fn new(value: f32, coefficient: f32) -> Self {
        Self {
            current: value,
            target: value,
            coefficient,
        }
    }

    fn next(&mut self) -> f32 {
        self.current += (self.target - self.current) * self.coefficient;
        self.current
    }

    fn snap(&mut self) {
        self.current = self.target;
    }
}
