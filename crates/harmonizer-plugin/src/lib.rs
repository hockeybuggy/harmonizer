use harmonizer_dsp::{Harmonizer, Mode, Params as DspParams, Quality};
use nice_plug::prelude::*;
use nice_plug_egui::{EguiEditor, EguiEditorState, RepaintNotifier};
use std::marker::PhantomData;
use std::sync::Arc;

mod editor;
mod params;
mod telemetry;

pub use params::HarmonizerParams;
use telemetry::{InputWatch, Telemetry};

/// The same harmonizer packaged two ways, because hosts differ in how they deliver MIDI to an
/// audio effect. `Fx` sits on the audio track and takes MIDI routed to it; `Instrument` sits on a
/// MIDI track and takes the audio through its sidechain input.
pub trait Variant: 'static + Send + Sync {
    const NAME: &'static str;
    const VST3_CLASS_ID: [u8; 16];
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory];
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout];
    const SIDECHAIN_INPUT: bool;
}

pub struct Fx;
pub struct Instrument;

impl Variant for Fx {
    const NAME: &'static str = "Harmonizer";
    const VST3_CLASS_ID: [u8; 16] = *b"HarmonizerHckbgy";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
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
    const SIDECHAIN_INPUT: bool = false;
}

impl Variant for Instrument {
    const NAME: &'static str = "Harmonizer Instrument";
    const VST3_CLASS_ID: [u8; 16] = *b"HarmonizerInstHb";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Instrument, Vst3SubCategory::PitchShift];
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        aux_input_ports: &[new_nonzero_u32(2)],
        names: PortNames {
            aux_inputs: &["Sidechain"],
            ..PortNames::const_default()
        },
        ..AudioIOLayout::const_default()
    }];
    const SIDECHAIN_INPUT: bool = true;
}

pub struct HarmonizerPlugin<V: Variant> {
    params: Arc<HarmonizerParams>,
    editor_state: Arc<EguiEditorState>,
    telemetry: Arc<Telemetry>,
    repaint: RepaintNotifier,
    harmonizer: Option<Harmonizer>,
    reported_latency: u32,
    input_watch: InputWatch,
    _variant: PhantomData<V>,
}

impl<V: Variant> Default for HarmonizerPlugin<V> {
    fn default() -> Self {
        Self {
            params: Arc::new(HarmonizerParams::default()),
            editor_state: editor::default_state(),
            telemetry: Arc::new(Telemetry::default()),
            repaint: RepaintNotifier::new(),
            harmonizer: None,
            reported_latency: 0,
            input_watch: InputWatch::default(),
            _variant: PhantomData,
        }
    }
}

impl<V: Variant> HarmonizerPlugin<V> {
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
            expression: p.expression.value(),
            pitch_bend_range: p.pitch_bend_range.value() as f32,
            attack_ms: p.attack.value(),
            release_ms: p.release.value(),
            velocity_sensitivity: p.velocity_sensitivity.value(),
            stereo_spread: p.stereo_spread.value(),
        }
    }
}

impl<V: Variant> Plugin for HarmonizerPlugin<V> {
    const NAME: &'static str = V::NAME;
    const VENDOR: &'static str = "hockeybuggy";
    const URL: &'static str = "https://github.com/hockeybuggy/harmonizer";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = V::AUDIO_IO_LAYOUTS;
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;

    type Editor = EguiEditor<editor::HarmonizerEditor>;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        editor::create(
            self.params.clone(),
            self.telemetry.clone(),
            self.editor_state.clone(),
            self.repaint.clone(),
            V::SIDECHAIN_INPUT,
        )
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
        self.reported_latency = harmonizer.latency() as u32;
        self.harmonizer = Some(harmonizer);
        self.reset();
        context.set_latency_samples(self.reported_latency);
        true
    }

    fn reset(&mut self) {
        self.input_watch = InputWatch::default();
        if let Some(harmonizer) = &mut self.harmonizer {
            harmonizer.reset();
            let input_changed = self
                .input_watch
                .publish(&self.telemetry, harmonizer.sample_rate());
            if self.telemetry.publish(harmonizer) | input_changed {
                self.repaint.request_repaint();
            }
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
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
        if V::SIDECHAIN_INPUT {
            match aux.inputs.first() {
                Some(sidechain) => {
                    let sidechain = sidechain.as_slice_immutable();
                    for (ch, out) in channels.iter_mut().enumerate() {
                        match sidechain.get(ch).or(sidechain.first()) {
                            Some(input) => out.copy_from_slice(&input[..num_samples]),
                            None => out.fill(0.0),
                        }
                    }
                }
                None => channels.iter_mut().for_each(|c| c.fill(0.0)),
            }
        }

        let input_peak = InputWatch::peak(channels);

        let mut next_event = context.next_event();
        let mut block_start = 0;
        while block_start < num_samples {
            while let Some(event) = next_event {
                if event.timing() as usize > block_start {
                    break;
                }
                if let NoteEvent::NoteOn { .. } = event {
                    self.input_watch.note_received();
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
        // Events can arrive after the last sample, e.g. a note off at the very end of a block.
        while let Some(event) = next_event {
            if let NoteEvent::NoteOn { .. } = event {
                self.input_watch.note_received();
            }
            handle_event(harmonizer, event);
            next_event = context.next_event();
        }

        let pitched = (harmonizer.params().mode == Mode::Absolute)
            .then(|| harmonizer.detected_pitch().is_some());
        self.input_watch
            .observe(input_peak, num_samples, harmonizer.sample_rate(), pitched);

        if self.editor_state.is_open() {
            let input_changed = self
                .input_watch
                .publish(&self.telemetry, harmonizer.sample_rate());
            if self.telemetry.publish(harmonizer) | input_changed {
                self.repaint.request_repaint();
            }
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
        NoteEvent::MidiCC { cc, value, .. } if cc == control_change::DAMPER_PEDAL => {
            harmonizer.set_sustain(value >= 0.5)
        }
        NoteEvent::MidiCC { cc, .. }
            if cc == control_change::ALL_NOTES_OFF || cc == control_change::ALL_SOUND_OFF =>
        {
            harmonizer.release_all()
        }
        _ => {}
    }
}

impl<V: Variant> Vst3Plugin for HarmonizerPlugin<V> {
    const VST3_CLASS_ID: [u8; 16] = V::VST3_CLASS_ID;
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = V::VST3_SUBCATEGORIES;
}

nice_export_vst3!(HarmonizerPlugin<Fx>, HarmonizerPlugin<Instrument>);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn reset_clears_the_editors_input_history() {
        let mut plugin = HarmonizerPlugin::<Fx> {
            harmonizer: Some(Harmonizer::new(48_000.0, 512)),
            ..Default::default()
        };
        plugin.input_watch.note_received();
        plugin
            .input_watch
            .observe(0.5, 144_000, 48_000.0, Some(false));
        plugin.input_watch.publish(&plugin.telemetry, 48_000.0);
        assert!(plugin.telemetry.notes_seen.load(Ordering::Relaxed));
        assert!(plugin.telemetry.input_unpitched.load(Ordering::Relaxed));

        plugin.reset();
        assert!(!plugin.telemetry.notes_seen.load(Ordering::Relaxed));
        assert!(!plugin.telemetry.input_unpitched.load(Ordering::Relaxed));
        assert_eq!(plugin.telemetry.input_meter.load(Ordering::Relaxed), 0.0);

        plugin.input_watch.publish(&plugin.telemetry, 48_000.0);
        assert!(!plugin.telemetry.notes_seen.load(Ordering::Relaxed));
    }
}
