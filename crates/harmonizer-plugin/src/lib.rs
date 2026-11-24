use harmonizer_dsp::{Harmonizer, Mode, Params as DspParams, Quality};
use nice_plug::prelude::*;
use std::sync::Arc;

mod params;

pub use params::HarmonizerParams;

pub struct HarmonizerPlugin {
    params: Arc<HarmonizerParams>,
    harmonizer: Option<Harmonizer>,
    reported_latency: u32,
}

impl Default for HarmonizerPlugin {
    fn default() -> Self {
        Self {
            params: Arc::new(HarmonizerParams::default()),
            harmonizer: None,
            reported_latency: 0,
        }
    }
}

impl HarmonizerPlugin {
    fn dsp_params(&self) -> DspParams {
        let p = &self.params;
        DspParams {
            mode: match p.mode.value() {
                params::ModeParam::Absolute => Mode::Absolute,
                params::ModeParam::Relative => Mode::Relative,
            },
            quality: match p.quality.value() {
                params::QualityParam::LowLatency => Quality::LowLatency,
                params::QualityParam::Balanced => Quality::Balanced,
                params::QualityParam::HighQuality => Quality::HighQuality,
            },
            wet_gain: p.wet.value(),
            dry_gain: p.dry.value(),
            formant_semitones: p.formant.value(),
            reference_note: p.reference_note.value() as f32,
            pitch_bend_range: p.pitch_bend_range.value() as f32,
            attack_ms: p.attack.value(),
            release_ms: p.release.value(),
            velocity_sensitivity: p.velocity_sensitivity.value(),
            stereo_spread: p.stereo_spread.value(),
        }
    }
}

impl Plugin for HarmonizerPlugin {
    const NAME: &'static str = "Harmonizer";
    const VENDOR: &'static str = "hockeybuggy";
    const URL: &'static str = "https://github.com/hockeybuggy/harmonizer";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;

    type Editor = ();
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn activate(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl ActivateContext<Self>,
    ) -> bool {
        let mut harmonizer = Harmonizer::new(
            buffer_config.sample_rate,
            buffer_config.max_buffer_size as usize,
        );
        harmonizer.set_params(self.dsp_params());
        harmonizer.reset();
        self.reported_latency = harmonizer.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.harmonizer = Some(harmonizer);
        true
    }

    fn reset(&mut self) {
        if let Some(harmonizer) = &mut self.harmonizer {
            harmonizer.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let params = self.dsp_params();
        let Some(harmonizer) = &mut self.harmonizer else {
            return ProcessStatus::Normal;
        };
        harmonizer.set_params(params);
        let latency = harmonizer.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
        }

        let num_samples = buffer.samples();
        let channels = buffer.as_slice();
        let mut next_event = context.next_event();
        let mut block_start = 0;
        while block_start < num_samples {
            while let Some(event) = next_event {
                if event.timing() as usize > block_start {
                    break;
                }
                handle_event(harmonizer, event);
                next_event = context.next_event();
            }
            let block_end = next_event
                .map(|e| (e.timing() as usize).min(num_samples))
                .unwrap_or(num_samples);

            match channels {
                [mono] => harmonizer.process(&mut [&mut mono[block_start..block_end]]),
                [left, right, ..] => harmonizer.process(&mut [
                    &mut left[block_start..block_end],
                    &mut right[block_start..block_end],
                ]),
                [] => {}
            }
            block_start = block_end;
        }

        ProcessStatus::Normal
    }
}

fn handle_event(harmonizer: &mut Harmonizer, event: NoteEvent<()>) {
    match event {
        NoteEvent::NoteOn { key, velocity, .. } => {
            harmonizer.note_on(key.number_or_middle_c(), velocity)
        }
        NoteEvent::NoteOff { key, .. } | NoteEvent::Choke { key, .. } => match key.number() {
            Some(note) => harmonizer.note_off(note),
            None => harmonizer.release_all(),
        },
        NoteEvent::MidiPitchBend { value, .. } => harmonizer.set_pitch_bend(value * 2.0 - 1.0),
        NoteEvent::MidiCC { cc, .. }
            if cc == control_change::ALL_NOTES_OFF || cc == control_change::ALL_SOUND_OFF =>
        {
            harmonizer.release_all()
        }
        _ => {}
    }
}

impl Vst3Plugin for HarmonizerPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"HarmonizerHckbgy";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
}

nice_export_vst3!(HarmonizerPlugin);
