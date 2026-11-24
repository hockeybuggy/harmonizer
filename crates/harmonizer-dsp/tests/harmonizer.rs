use harmonizer_dsp::{Harmonizer, Mode, Params, Quality};

const SAMPLE_RATE: f32 = 48_000.0;

fn sine(freq: f32, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| 0.5 * (n as f32 * freq * std::f32::consts::TAU / SAMPLE_RATE).sin())
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

fn run(harmonizer: &mut Harmonizer, input: &[f32]) -> Vec<f32> {
    let mut buffer = input.to_vec();
    harmonizer.process(&mut [&mut buffer[..]]);
    buffer
}

#[test]
fn silent_without_notes_when_dry_is_off() {
    for mode in [Mode::Absolute, Mode::Relative] {
        let mut harmonizer = Harmonizer::new(SAMPLE_RATE, 256);
        harmonizer.set_params(Params {
            mode,
            ..Params::default()
        });
        let out = run(&mut harmonizer, &sine(220.0, 24_000));
        assert!(
            rms(&out) < 1e-6,
            "{mode:?} produced output with no notes held"
        );
    }
}

#[test]
fn dry_signal_is_delayed_by_reported_latency() {
    for mode in [Mode::Absolute, Mode::Relative] {
        for quality in Quality::ALL {
            let mut harmonizer = Harmonizer::new(SAMPLE_RATE, 256);
            harmonizer.set_params(Params {
                mode,
                quality,
                dry_gain: 1.0,
                wet_gain: 0.0,
                ..Params::default()
            });
            harmonizer.reset();
            let latency = harmonizer.latency();
            let mut impulse = vec![0.0; latency + 512];
            impulse[10] = 1.0;
            let out = run(&mut harmonizer, &impulse);
            let peak = out
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap()
                .0;
            assert_eq!(peak, 10 + latency, "{mode:?} {quality:?}");
        }
    }
}

#[test]
fn held_note_produces_output_and_release_silences_it() {
    for mode in [Mode::Absolute, Mode::Relative] {
        let mut harmonizer = Harmonizer::new(SAMPLE_RATE, 256);
        harmonizer.set_params(Params {
            mode,
            ..Params::default()
        });
        let input = sine(220.0, 48_000);
        harmonizer.note_on(57, 1.0);
        let held = run(&mut harmonizer, &input);
        assert!(
            rms(&held[24_000..]) > 0.05,
            "{mode:?} held note was too quiet"
        );

        harmonizer.note_off(57);
        let released = run(&mut harmonizer, &input);
        assert!(
            rms(&released[24_000..]) < 1e-6,
            "{mode:?} kept sounding after release"
        );
        assert!(harmonizer.voices().active().next().is_none());
    }
}

#[test]
fn sustain_pedal_holds_voice_through_note_off_until_pedal_up() {
    for mode in [Mode::Absolute, Mode::Relative] {
        let mut harmonizer = Harmonizer::new(SAMPLE_RATE, 256);
        harmonizer.set_params(Params {
            mode,
            release_ms: 20.0,
            ..Params::default()
        });
        let input = sine(220.0, 24_000);

        harmonizer.set_sustain(true);
        harmonizer.note_on(57, 1.0);
        let _ = run(&mut harmonizer, &input);

        harmonizer.note_off(57);
        let sustained = run(&mut harmonizer, &input);
        assert!(
            rms(&sustained[12_000..]) > 0.05,
            "{mode:?} voice released while sustain pedal was down"
        );
        let active: Vec<_> = harmonizer.voices().active().collect();
        assert_eq!(active.len(), 1);
        assert!(!active[0].1.held && active[0].1.sustained);

        harmonizer.set_sustain(false);
        let released = run(&mut harmonizer, &input);
        assert!(
            rms(&released[12_000..]) < 1e-6,
            "{mode:?} kept sounding after sustain pedal lifted"
        );
        assert!(harmonizer.voices().active().next().is_none());
    }
}

#[test]
fn pedal_up_keeps_held_key_sounding_while_releasing_sustained_key() {
    for mode in [Mode::Absolute, Mode::Relative] {
        let mut harmonizer = Harmonizer::new(SAMPLE_RATE, 256);
        harmonizer.set_params(Params {
            mode,
            release_ms: 20.0,
            ..Params::default()
        });
        let input = sine(220.0, 24_000);

        harmonizer.set_sustain(true);
        harmonizer.note_on(57, 1.0);
        harmonizer.note_on(64, 1.0);
        let _ = run(&mut harmonizer, &input);

        harmonizer.note_off(57);
        harmonizer.set_sustain(false);
        let after = run(&mut harmonizer, &input);
        assert!(
            rms(&after[12_000..]) > 0.05,
            "{mode:?} silenced still-held key when sustain pedal lifted"
        );
        let active: Vec<_> = harmonizer.voices().active().collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].1.note, 64);
        assert!(active[0].1.held && !active[0].1.sustained);
    }
}

#[test]
fn retrigger_under_sustain_keeps_slot_and_output_continuity() {
    for mode in [Mode::Absolute, Mode::Relative] {
        let mut harmonizer = Harmonizer::new(SAMPLE_RATE, 256);
        harmonizer.set_params(Params {
            mode,
            attack_ms: 10.0,
            release_ms: 20.0,
            ..Params::default()
        });
        let input = sine(220.0, 12_000);

        harmonizer.set_sustain(true);
        harmonizer.note_on(57, 1.0);
        let _ = run(&mut harmonizer, &input);
        let original_slot = harmonizer.voices().active().next().unwrap().0;

        harmonizer.note_off(57);
        let _ = run(&mut harmonizer, &input);
        harmonizer.note_on(57, 1.0);

        let active: Vec<_> = harmonizer.voices().active().collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0, original_slot);
        assert!(
            (active[0].1.envelope - 1.0).abs() < 1e-6,
            "{mode:?} envelope dropped on retrigger under sustain"
        );

        let retriggered = run(&mut harmonizer, &sine(220.0, 2_048));
        assert!(
            rms(&retriggered[..512]) > 0.05,
            "{mode:?} dropped out immediately after retrigger under sustain"
        );
    }
}

#[test]
fn release_all_clears_sustain_pedal() {
    let mut harmonizer = Harmonizer::new(SAMPLE_RATE, 256);
    harmonizer.set_params(Params {
        release_ms: 20.0,
        ..Params::default()
    });
    let input = sine(220.0, 24_000);

    harmonizer.set_sustain(true);
    harmonizer.note_on(57, 1.0);
    let _ = run(&mut harmonizer, &input);
    harmonizer.note_off(57);
    harmonizer.release_all();
    let released = run(&mut harmonizer, &input);
    assert!(rms(&released[12_000..]) < 1e-6);
    assert!(harmonizer.voices().active().next().is_none());

    harmonizer.note_on(60, 1.0);
    let _ = run(&mut harmonizer, &input);
    harmonizer.note_off(60);
    let after = run(&mut harmonizer, &input);
    assert!(rms(&after[12_000..]) < 1e-6);
    assert!(harmonizer.voices().active().next().is_none());
}
