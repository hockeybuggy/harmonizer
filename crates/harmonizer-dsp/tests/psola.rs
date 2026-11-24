mod support;

use std::f32::consts::TAU;

use harmonizer_dsp::psola::PsolaEngine;
use harmonizer_dsp::{Engine, Quality};
use support::*;

const SETTLE: usize = 4_800;

#[test]
#[ignore = "CPU benchmark; run with --release"]
fn eight_voice_realtime_factor() {
    let input = vowel(220.0, &VOWEL_AH, 10 * SAMPLE_RATE as usize);
    let controls = voices(
        &[130.81, 164.81, 196.0, 261.63, 293.66, 329.63, 392.0, 523.25],
        1.0,
    );
    let mut buffers = harmonizer_dsp::VoiceBuffers::new(512);
    for quality in Quality::ALL {
        let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
        let start = std::time::Instant::now();
        for block in input.chunks(buffers.max_block()) {
            buffers.begin_block(block.len());
            engine.process(block, &controls, &mut buffers);
            std::hint::black_box(&buffers);
        }
        let elapsed = start.elapsed().as_secs_f64();
        eprintln!(
            "{quality:?}: {elapsed:.3} s, {:.1}× realtime",
            10.0 / elapsed
        );
    }
}

fn noise(len: usize) -> Vec<f32> {
    let mut state = 1u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as i32 as f32 / i32::MAX as f32 * 0.3
        })
        .collect()
}

fn vibrato_vowel(len: usize) -> Vec<f32> {
    let mut phase = 0.0;
    let weights: Vec<_> = (1..=64)
        .map(|k| formant_envelope(k as f32 * 220.0, &VOWEL_AH) * 0.15)
        .collect();
    (0..len)
        .map(|n| {
            let cents = 50.0 * (TAU * 5.0 * n as f32 / SAMPLE_RATE).sin();
            phase = (phase + TAU * 220.0 * (cents / 1200.0).exp2() / SAMPLE_RATE) % TAU;
            weights
                .iter()
                .enumerate()
                .map(|(k, weight)| weight * (phase * (k + 1) as f32).sin())
                .sum()
        })
        .collect()
}

fn assert_pitch(samples: &[f32], target: f32, tolerance: f32) {
    let hz = fundamental(samples, target * 0.45, target * 2.1).expect("no output pitch");
    assert!(
        cents(hz, target).abs() < tolerance,
        "detected {hz:.2} Hz, expected {target:.2} Hz"
    );
}

#[test]
fn detector_tracks_periodic_waveforms() {
    for (quality, min_hz) in Quality::ALL.into_iter().zip([80.0, 60.0, 50.0]) {
        for hz in [min_hz, 82.0, 110.0, 220.0, 440.0, 880.0, 1000.0] {
            for (name, input) in [
                ("sine", sine(hz, SETTLE)),
                ("sawtooth", sawtooth(hz, SETTLE)),
                ("vowel", vowel(hz, &VOWEL_AH, SETTLE)),
            ] {
                let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
                render(&mut engine, &input, &voices(&[], 1.0), 256);
                let detected = engine
                    .detected_pitch()
                    .unwrap_or_else(|| panic!("{quality:?}: missed {hz} Hz {name}"));
                assert!(
                    cents(detected, hz).abs() < 10.0,
                    "{quality:?}: detected {detected} Hz on {hz} Hz {name}"
                );
            }
        }
    }
}

#[test]
fn detector_releases_on_noise_or_silence() {
    for quality in Quality::ALL {
        for input in [noise(SETTLE), vec![0.0; SETTLE]] {
            let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
            let controls = voices(&[], 1.0);
            render(&mut engine, &sine(220.0, SETTLE), &controls, 256);
            assert!(engine.detected_pitch().is_some(), "{quality:?}");
            for block in input.chunks(SETTLE / 2) {
                render(&mut engine, block, &controls, 256);
                assert_eq!(engine.detected_pitch(), None, "{quality:?}");
            }
        }
    }
}

#[test]
fn retunes_each_voice_to_its_own_target() {
    let targets = [261.63, 130.81, 349.23, 440.0];
    let input = vowel(196.0, &VOWEL_AH, 3 * SETTLE);
    for quality in Quality::ALL {
        let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
        let output = render(&mut engine, &input, &voices(&targets, 1.0), 256);
        for (channel, target) in output.iter().zip(targets) {
            assert_pitch(&channel[SETTLE..], target, 10.0);
        }
    }
}

#[test]
fn retunes_low_e_on_every_preset() {
    let target = 110.0;
    let len = 3 * SETTLE;
    for (waveform, input) in [
        ("vowel", vowel(82.0, &VOWEL_AH, len)),
        ("sawtooth", sawtooth(82.0, len)),
    ] {
        for quality in Quality::ALL {
            let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
            let output = render(&mut engine, &input, &voices(&[target], 1.0), 256);
            let steady = &output[0][SETTLE..];
            assert_pitch(steady, target, 10.0);
            let leakage = inharmonic_power(steady, target);
            assert!(leakage < 0.001, "{quality:?} {waveform}: {leakage:.5}");
            if quality == Quality::LowLatency {
                assert!(engine.latency() <= (0.016 * SAMPLE_RATE) as usize);
            }
        }
    }
}

fn inharmonic_power(samples: &[f32], target: f32) -> f32 {
    let spacing = (target * samples.len() as f32 / SAMPLE_RATE).round() as usize;
    let (mut outside, mut total) = (0.0, 0.0);
    for (bin, magnitude) in magnitude_spectrum(samples).into_iter().enumerate().skip(2) {
        let power = magnitude * magnitude;
        total += power;
        let offset = bin % spacing;
        // A Hann window spreads each bin-centred partial into both neighbouring bins.
        if offset.min(spacing - offset) > 1 {
            outside += power;
        }
    }
    outside / total
}

#[test]
fn wide_shifts_stay_on_target_harmonics() {
    let targets = [165.0, 110.0, 660.0, 990.0];
    for source in [330.0, 327.0] {
        let input = vowel(source, &VOWEL_AH, SETTLE + SAMPLE_RATE as usize);
        for quality in Quality::ALL {
            let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
            let output = render(&mut engine, &input, &voices(&targets, 1.0), 256);
            for (channel, target) in output.iter().zip(targets) {
                let steady = &channel[SETTLE..];
                assert_pitch(&steady[..2 * SETTLE], target, 10.0);
                let leakage = inharmonic_power(steady, target);
                assert!(
                    leakage < 0.0005,
                    "{quality:?} {source} -> {target}: {leakage:.5}"
                );
            }
        }
    }
}

#[test]
fn octave_and_fifth_down_preserves_quiet_grain_tails() {
    let input = vowel(330.0, &VOWEL_AH, 3 * SETTLE);
    for quality in Quality::ALL {
        let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
        let output = render(&mut engine, &input, &voices(&[110.0], 1.0), 256);
        let cycle = &output[0][SETTLE..SETTLE + 480];
        let quietest = cycle.windows(48).map(rms).fold(f32::INFINITY, f32::min);
        let floor = quietest / rms(cycle);
        assert!(floor < 0.01, "{quality:?}: {floor:.5}");
    }
}

#[test]
fn new_voice_uses_voiced_history_immediately() {
    let mut engine = PsolaEngine::new(SAMPLE_RATE, Quality::HighQuality);
    let input = sine(880.0, 2 * SETTLE);
    render(&mut engine, &input[..SETTLE], &voices(&[], 1.0), 64);
    let output = render(&mut engine, &input[SETTLE..], &voices(&[660.0], 1.0), 64);
    let early = &output[0][384..960];
    assert!(rms(early) > 0.1, "new voice dropped out");
    assert_pitch(early, 660.0, 10.0);
}

#[test]
fn retuning_removes_vibrato() {
    let input = vibrato_vowel(24_000);
    for quality in Quality::ALL {
        let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
        let output = render(&mut engine, &input, &voices(&[293.66], 1.0), 256);
        for window in output[0][SETTLE..].windows(1920).step_by(480) {
            assert_pitch(window, 293.66, 15.0);
        }
    }
}

#[test]
fn shifts_formants_independently_of_pitch() {
    let hz = 150.0;
    let ratio = (7.0f32 / 12.0).exp2();
    let input = vowel(hz, &VOWEL_AH, 4 * SETTLE);
    let centroid = |samples: &[f32]| spectral_centroid(&samples[SETTLE..], 300.0, 1600.0);
    let source = centroid(&input);
    let render_centroid = |formant_ratio| {
        let mut engine = PsolaEngine::new(SAMPLE_RATE, Quality::Balanced);
        let output = render(
            &mut engine,
            &input,
            &voices(&[hz * ratio], formant_ratio),
            256,
        );
        assert_pitch(&output[0][SETTLE..], hz * ratio, 10.0);
        centroid(&output[0])
    };
    let preserved = render_centroid(1.0);
    let raised = render_centroid(ratio);
    assert!((preserved - source).abs() < 30.0, "{source} -> {preserved}");
    assert!(raised > preserved + 200.0, "{preserved} -> {raised}");
}

#[test]
fn unvoiced_output_is_exactly_delayed() {
    let input = noise(3 * SETTLE);
    for quality in Quality::ALL {
        let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
        let latency = engine.latency();
        let output = render(&mut engine, &input, &voices(&[330.0], 1.0), 64);
        assert!(output[0][..latency].iter().all(|&sample| sample == 0.0));
        assert_eq!(output[0][latency..], input[..input.len() - latency]);
    }
}

#[test]
fn unvoiced_passthrough_is_shared_between_voices() {
    let input = noise(3 * SETTLE);
    let mut engine = PsolaEngine::new(SAMPLE_RATE, Quality::Balanced);
    let latency = engine.latency();
    let output = render(
        &mut engine,
        &input,
        &voices(&[220.0, 277.18, 329.63], 1.0),
        64,
    );
    let settled = latency + SETTLE / 2;
    for (n, expected) in input[..input.len() - latency]
        .iter()
        .enumerate()
        .skip(settled)
    {
        let sum: f32 = output[..3].iter().map(|voice| voice[n + latency]).sum();
        assert!(
            (sum - expected).abs() < 1e-5,
            "sample {n}: {sum} vs {expected}"
        );
    }
}

#[test]
fn unity_pitch_output_aligns_at_reported_latency() {
    let input = vowel(220.0, &VOWEL_AH, 3 * SETTLE);
    for quality in Quality::ALL {
        let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
        let latency = engine.latency();
        let output = render(&mut engine, &input, &voices(&[220.0], 1.0), 64);
        let source = &input[SETTLE - latency..input.len() - latency];
        let difference: Vec<_> = output[0][SETTLE..]
            .iter()
            .zip(source)
            .map(|(y, x)| y - x)
            .collect();
        assert!(rms(&difference) < 0.01 * rms(source), "{quality:?}");
    }
}

#[test]
fn output_is_independent_of_block_size() {
    let mut input = noise(SETTLE);
    input.extend(vibrato_vowel(2 * SETTLE));
    input.extend(noise(SETTLE));
    let controls = voices(&[146.83, 261.63, 440.0], 1.2);
    for quality in Quality::ALL {
        let mut engine = PsolaEngine::new(SAMPLE_RATE, quality);
        let reference = render(&mut engine, &input, &controls, 512);
        for block in [1, 7, 64] {
            engine.reset();
            let output = render(&mut engine, &input, &controls, block);
            for (channel, expected) in output.iter().zip(&reference) {
                assert!(
                    channel
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| (a - b).abs() <= 1e-5),
                    "{quality:?}, block {block}"
                );
            }
        }
    }
}
