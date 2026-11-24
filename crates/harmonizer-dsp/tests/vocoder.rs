mod support;

use harmonizer_dsp::{Engine, Quality, VoiceBuffers, vocoder::VocoderEngine};
use std::time::Instant;
use support::{
    SAMPLE_RATE, VOWEL_AH, cents, dominant_frequency, fundamental, render, rms, sawtooth, sine,
    spectral_centroid, voices, vowel,
};

fn fifth() -> f32 {
    2.0_f32.powf(7.0 / 12.0)
}

#[test]
fn transposes_a_sine_by_its_pitch_ratio() {
    let input = sine(220.0, 32_768);
    let ratios = [1.5, 0.125, 8.0];
    for quality in Quality::ALL {
        let mut engine = VocoderEngine::new(SAMPLE_RATE, quality);
        let start = 2 * engine.latency();
        let output = render(&mut engine, &input, &voices(&ratios, 1.0), 512);
        for (slot, expected) in [330.0, 55.0, 880.0].into_iter().enumerate() {
            let actual = dominant_frequency(&output[slot][start..]);
            assert!(
                cents(actual, expected).abs() < 10.0,
                "{quality:?}: {actual} vs {expected} Hz"
            );
        }
    }
}

#[test]
fn transposes_the_fundamental_of_a_sawtooth() {
    let input = sawtooth(110.0, 24_576);
    let ratios = [fifth(), 0.5, 2.0];
    for quality in Quality::ALL {
        let mut engine = VocoderEngine::new(SAMPLE_RATE, quality);
        let start = 2 * engine.latency();
        let output = render(&mut engine, &input, &voices(&ratios, 1.0), 512);
        for (slot, ratio) in ratios.into_iter().enumerate() {
            let expected = 110.0 * ratio;
            let actual = fundamental(&output[slot][start..], expected * 0.85, expected * 1.15)
                .unwrap_or_else(|| {
                    panic!("{quality:?} at {ratio}: no fundamental near {expected} Hz")
                });
            assert!(
                cents(actual, expected).abs() < 10.0,
                "{quality:?} at {ratio}: {actual} Hz"
            );
        }
    }
}

#[test]
fn renders_independent_voice_pitches() {
    let input = sine(220.0, 32_768);
    let ratios = [1.0, 1.25, 1.5];
    for quality in Quality::ALL {
        let mut engine = VocoderEngine::new(SAMPLE_RATE, quality);
        let start = 2 * engine.latency();
        let output = render(&mut engine, &input, &voices(&ratios, 1.0), 64);
        for (slot, ratio) in ratios.into_iter().enumerate() {
            let actual = dominant_frequency(&output[slot][start..]);
            assert!(
                cents(actual, 220.0 * ratio).abs() < 10.0,
                "{quality:?} slot {slot}: {actual} Hz"
            );
        }
    }
}

#[test]
fn moves_formants_independently_of_pitch() {
    let input = vowel(150.0, &VOWEL_AH, 32_768);
    let mut controls = voices(&[fifth(), fifth()], 1.0);
    controls[1].formant_ratio = fifth();
    let centroid = |signal: &[f32]| spectral_centroid(signal, 300.0, 1600.0);
    for quality in Quality::ALL {
        let mut engine = VocoderEngine::new(SAMPLE_RATE, quality);
        let start = 2 * engine.latency();
        let original = centroid(&input[start..]);
        let output = render(&mut engine, &input, &controls, 512);
        let preserved = centroid(&output[0][start..]);
        let shifted = centroid(&output[1][start..]);
        eprintln!(
            "{quality:?}: original {original:.1}, preserved {preserved:.1}, shifted {shifted:.1} Hz"
        );
        assert!(
            (preserved - original).abs() < 70.0,
            "{quality:?}: {preserved} vs {original} Hz"
        );
        assert!(
            shifted - preserved > 200.0,
            "{quality:?}: {shifted} vs {preserved} Hz"
        );
    }
}

#[test]
fn unison_matches_the_reported_latency() {
    let mut seed = 1u32;
    let input: Vec<_> = (0..24_576)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        })
        .collect();
    for (quality, size) in Quality::ALL.into_iter().zip([1024, 2048, 4096]) {
        for (rate, latency) in [
            (SAMPLE_RATE, size),
            (96_000.0, size * 2),
            (32_000.0, size / 2),
        ] {
            let mut engine = VocoderEngine::new(rate, quality);
            assert_eq!(engine.latency(), latency, "{quality:?} at {rate} Hz");
            let output = render(&mut engine, &input, &voices(&[1.0], 1.0), 7);
            let out = &output[0][2 * latency..];
            let best_lag = (latency - 2..=latency + 2)
                .map(|lag| {
                    let corr = out
                        .iter()
                        .zip(&input[2 * latency - lag..])
                        .map(|(a, b)| a * b)
                        .sum::<f32>();
                    (lag, corr)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap()
                .0;
            assert_eq!(best_lag, latency, "{quality:?} at {rate} Hz");
            let gain_db = 20.0 * (rms(out) / rms(&input[latency..input.len() - latency])).log10();
            assert!(
                gain_db.abs() < 1.0,
                "{quality:?} at {rate} Hz: {gain_db} dB"
            );
        }
    }
}

#[test]
fn returning_to_unison_realigns_the_waveform() {
    let mut engine = VocoderEngine::new(SAMPLE_RATE, Quality::Balanced);
    let input = sine(220.0, 20_137);
    let change = 9_123;
    render(&mut engine, &input[..change], &voices(&[1.5], 1.0), 512);
    let output = render(&mut engine, &input[change..], &voices(&[1.0], 1.0), 512);
    let latency = engine.latency();
    let error = output[0][2 * latency..]
        .iter()
        .zip(&input[change + latency..])
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0, f32::max);
    assert!(error < 1e-5, "peak error {error}");
}

#[test]
fn discards_a_transposed_partial_above_nyquist() {
    let input = sine(13_000.0, 24_576);
    let mut engine = VocoderEngine::new(SAMPLE_RATE, Quality::LowLatency);
    let start = 2 * engine.latency();
    let output = render(&mut engine, &input, &voices(&[1.85], 1.85), 512);
    let alias = rms(&output[0][start..]);
    assert!(alias < 0.005, "alias RMS {alias}");
}

#[test]
fn preserves_isolated_attack_timing_when_transposing() {
    let onset = 6_377;
    let mut input = vec![0.0; 16_384];
    let mut seed = 43u32;
    for (i, sample) in input[onset..onset + 1_440].iter_mut().enumerate() {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *sample = ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * (-(i as f32) / 400.0).exp();
    }
    for quality in Quality::ALL {
        let mut engine = VocoderEngine::new(SAMPLE_RATE, quality);
        let at = onset + engine.latency();
        let attack = rms(&input[onset..onset + 144]);
        for ratios in [&[1.26][..], &[1.26, 0.75], &[1.0, 1.26, 0.75]] {
            engine.reset();
            let output = render(&mut engine, &input, &voices(ratios, 1.0), 7);
            let sum: Vec<f32> = (at - 960..at + 144)
                .map(|i| output[..ratios.len()].iter().map(|slot| slot[i]).sum())
                .collect();
            let pre_echo = rms(&sum[..960]) / attack;
            let onset_level = rms(&sum[960..]) / attack / (ratios.len() as f32).sqrt();
            eprintln!("{quality:?} {ratios:?}: pre-echo {pre_echo:.3}, onset {onset_level:.3}");
            assert!(
                pre_echo < 0.025,
                "{quality:?} {ratios:?}: pre-echo {pre_echo:.3}"
            );
            assert!(
                (0.9..1.1).contains(&onset_level),
                "{quality:?} {ratios:?}: onset {onset_level:.3}"
            );
        }
    }
}

#[test]
fn new_voice_does_not_attenuate_attack_before_its_first_frame() {
    let mut engine = VocoderEngine::new(SAMPLE_RATE, Quality::Balanced);
    let onset = 6_272;
    let mut input = vec![0.0; onset];
    input.extend(sine(1_000.0, 4_096));
    let delayed_onset = onset + engine.latency();
    let controls = voices(&[1.25], 1.0);
    let expected = render(&mut engine, &input, &controls, 512);
    for pitch in [1.5, 1.0] {
        engine.reset();
        render(&mut engine, &input[..delayed_onset], &controls, 512);
        let active = voices(&[1.25, pitch], 1.0);
        let tail = &input[delayed_onset..delayed_onset + 1_024];
        let output = render(&mut engine, tail, &active, 1_024);
        let error = output[0][..64]
            .iter()
            .zip(&expected[0][delayed_onset..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(error < 1e-5, "pitch {pitch}: peak error {error}");
        engine.reset();
        render(&mut engine, &input[..delayed_onset], &controls, 512);
        let split = render(&mut engine, tail, &active, 1);
        assert_eq!(output[0], split[0], "pitch {pitch}");
    }
}

#[test]
fn rendering_is_invariant_to_block_size() {
    let input = vowel(150.0, &VOWEL_AH, 12_337);
    let controls = voices(&[1.0, 1.25, 1.5], 0.85);
    for quality in Quality::ALL {
        let mut engine = VocoderEngine::new(SAMPLE_RATE, quality);
        let expected = render(&mut engine, &input, &controls, 1);
        for block in [7, 64, 512] {
            engine.reset();
            let actual = render(&mut engine, &input, &controls, block);
            for (slot, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                assert!(
                    actual
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| (a - b).abs() <= 1e-5),
                    "{quality:?} block {block} slot {slot}"
                );
            }
        }
    }
}

#[test]
fn a_new_voice_can_use_input_analysed_without_voices() {
    let mut engine = VocoderEngine::new(SAMPLE_RATE, Quality::Balanced);
    let input = sine(220.0, 8192);
    render(&mut engine, &input, &voices(&[], 1.0), 512);
    let output = render(&mut engine, &[0.0; 2048], &voices(&[1.5], 1.0), 512);
    assert!(rms(&output[0][1024..]) > 0.01);
}

#[test]
fn resetting_one_slot_discards_its_pending_audio() {
    let mut engine = VocoderEngine::new(SAMPLE_RATE, Quality::Balanced);
    let latency = engine.latency();
    let mut input = vec![0.0; 12_288 - latency];
    input.extend(sine(220.0, latency));
    let controls = voices(&[1.25, 1.25], 1.0);
    render(&mut engine, &input, &controls, 512);
    engine.reset_voice(0);
    let output = render(&mut engine, &[0.0; 256], &controls, 256);
    assert!(rms(&output[0]) < 1e-6);
    assert!(rms(&output[1]) > 0.1);
}

#[test]
#[ignore = "generate synthetic fixtures for eval_relative.py --vowels"]
fn write_phase_vowels() -> std::io::Result<()> {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/eval/phase/vowels");
    std::fs::create_dir_all(&directory)?;
    for f0 in [110, 150, 220] {
        for shift in [-5, 0, 4] {
            let frequency = f0 as f32 * 2.0_f32.powf(shift as f32 / 12.0);
            let samples = vowel(frequency, &VOWEL_AH, 2 * SAMPLE_RATE as usize);
            let bytes: Vec<_> = samples.into_iter().flat_map(f32::to_le_bytes).collect();
            std::fs::write(directory.join(format!("{f0}-{shift:+}.f32le")), bytes)?;
        }
    }
    Ok(())
}

#[test]
#[ignore = "release CPU benchmark"]
fn eight_voice_cpu() {
    let input = vowel(150.0, &VOWEL_AH, 480_000);
    let controls = voices(&[0.5, 0.75, 0.9, 1.1, 1.25, 1.5, 2.0, 2.5], 1.0);
    let mut buffers = VoiceBuffers::new(512);
    for quality in Quality::ALL {
        let mut engine = VocoderEngine::new(SAMPLE_RATE, quality);
        let start = Instant::now();
        for block in input.chunks(512) {
            buffers.begin_block(block.len());
            engine.process(block, &controls, &mut buffers);
            std::hint::black_box(&buffers);
        }
        let seconds = start.elapsed().as_secs_f64();
        eprintln!(
            "{quality:?}: {seconds:.3} s for 10 s ({:.1}x realtime)",
            10.0 / seconds
        );
    }
}
