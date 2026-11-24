#![allow(dead_code)]

use harmonizer_dsp::{Engine, MAX_VOICES, VoiceBuffers, VoiceControl};
use realfft::RealFftPlanner;
use std::f32::consts::TAU;

pub const SAMPLE_RATE: f32 = 48_000.0;

pub fn sine(freq: f32, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| 0.5 * (n as f32 * freq * TAU / SAMPLE_RATE).sin())
        .collect()
}

/// Band-limited sawtooth, so every harmonic is present at a known level.
pub fn sawtooth(freq: f32, len: usize) -> Vec<f32> {
    let harmonics = (SAMPLE_RATE / 2.0 / freq) as usize;
    (0..len)
        .map(|n| {
            let phase = n as f32 * freq * TAU / SAMPLE_RATE;
            (1..=harmonics)
                .map(|k| (k as f32 * phase).sin() / k as f32)
                .sum::<f32>()
                * 0.3
        })
        .collect()
}

/// A crude sung vowel: a band-limited pulse train at `freq` whose harmonics are weighted by
/// resonances at `formants` (Hz). Moving the pitch keeps the resonances where they are, which is
/// what formant-preserving shifters must reproduce.
pub fn vowel(freq: f32, formants: &[f32], len: usize) -> Vec<f32> {
    let harmonics = (SAMPLE_RATE / 2.0 / freq) as usize;
    let weights: Vec<f32> = (1..=harmonics)
        .map(|k| formant_envelope(k as f32 * freq, formants))
        .collect();
    let norm = 0.5 / weights.iter().sum::<f32>().max(1e-6).sqrt();
    (0..len)
        .map(|n| {
            let phase = n as f32 * freq * TAU / SAMPLE_RATE;
            weights
                .iter()
                .enumerate()
                .map(|(k, w)| w * ((k + 1) as f32 * phase).sin())
                .sum::<f32>()
                * norm
        })
        .collect()
}

pub fn formant_envelope(freq: f32, formants: &[f32]) -> f32 {
    formants
        .iter()
        .map(|&f| {
            let bandwidth = 80.0 + f * 0.08;
            1.0 / (1.0 + ((freq - f) / bandwidth).powi(2))
        })
        .sum()
}

pub const VOWEL_AH: [f32; 3] = [800.0, 1150.0, 2900.0];

pub fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

/// Hann-windowed magnitude spectrum, `len / 2 + 1` bins.
pub fn magnitude_spectrum(samples: &[f32]) -> Vec<f32> {
    let len = samples.len();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(len);
    let mut input: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(n, s)| s * (0.5 - 0.5 * (TAU * n as f32 / len as f32).cos()))
        .collect();
    let mut output = fft.make_output_vec();
    fft.process(&mut input, &mut output).unwrap();
    output.iter().map(|c| c.norm()).collect()
}

/// Frequency of the strongest spectral peak, refined by parabolic interpolation.
pub fn dominant_frequency(samples: &[f32]) -> f32 {
    let spectrum = magnitude_spectrum(samples);
    let bin_hz = SAMPLE_RATE / samples.len() as f32;
    let peak = (1..spectrum.len() - 1)
        .max_by(|&a, &b| spectrum[a].total_cmp(&spectrum[b]))
        .unwrap();
    let (a, b, c) = (spectrum[peak - 1], spectrum[peak], spectrum[peak + 1]);
    let offset = 0.5 * (a - c) / (a - 2.0 * b + c);
    (peak as f32 + offset) * bin_hz
}

/// Frequencies of the `count` strongest local maxima of the spectrum, strongest first.
pub fn spectral_peaks(samples: &[f32], count: usize) -> Vec<f32> {
    let spectrum = magnitude_spectrum(samples);
    let bin_hz = SAMPLE_RATE / samples.len() as f32;
    let mut peaks: Vec<usize> = (1..spectrum.len() - 1)
        .filter(|&k| spectrum[k] > spectrum[k - 1] && spectrum[k] >= spectrum[k + 1])
        .collect();
    peaks.sort_by(|&a, &b| spectrum[b].total_cmp(&spectrum[a]));
    peaks.truncate(count);
    peaks.iter().map(|&k| k as f32 * bin_hz).collect()
}

/// Fundamental frequency by normalised autocorrelation, for harmonically rich signals whose
/// strongest partial may not be the fundamental.
pub fn fundamental(samples: &[f32], min_hz: f32, max_hz: f32) -> Option<f32> {
    let min_lag = (SAMPLE_RATE / max_hz) as usize;
    let max_lag = (SAMPLE_RATE / min_hz) as usize;
    let len = samples.len().checked_sub(max_lag + 1)?;
    let corr = |lag: usize| {
        let (mut xy, mut xx, mut yy) = (0.0f64, 0.0f64, 0.0f64);
        for n in 0..len {
            let (x, y) = (samples[n] as f64, samples[n + lag] as f64);
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        (xy / (xx * yy).sqrt().max(1e-12)) as f32
    };
    let values: Vec<f32> = (min_lag..=max_lag + 1).map(corr).collect();
    let best = values.iter().cloned().fold(f32::MIN, f32::max);
    if best < 0.5 {
        return None;
    }
    // The first lag that gets close to the best avoids octave-down errors on periodic signals.
    let i = (1..values.len() - 1).find(|&i| {
        values[i] >= 0.9 * best && values[i] >= values[i - 1] && values[i] >= values[i + 1]
    })?;
    let (a, b, c) = (values[i - 1], values[i], values[i + 1]);
    let offset = 0.5 * (a - c) / (a - 2.0 * b + c);
    Some(SAMPLE_RATE / ((min_lag + i) as f32 + offset))
}

/// Energy-weighted mean frequency of the spectrum between `lo` and `hi` Hz.
pub fn spectral_centroid(samples: &[f32], lo: f32, hi: f32) -> f32 {
    let spectrum = magnitude_spectrum(samples);
    let bin_hz = SAMPLE_RATE / samples.len() as f32;
    let (mut weighted, mut total) = (0.0, 0.0);
    for (k, m) in spectrum.iter().enumerate() {
        let f = k as f32 * bin_hz;
        if (lo..=hi).contains(&f) {
            weighted += f * m * m;
            total += m * m;
        }
    }
    weighted / total.max(1e-12)
}

/// Runs `engine` over `input` in blocks of `block_size`, returning each voice slot's output.
pub fn render(
    engine: &mut impl Engine,
    input: &[f32],
    controls: &[VoiceControl; MAX_VOICES],
    block_size: usize,
) -> Vec<Vec<f32>> {
    let mut buffers = VoiceBuffers::new(block_size);
    let mut outputs = vec![Vec::with_capacity(input.len()); MAX_VOICES];
    for block in input.chunks(block_size) {
        buffers.begin_block(block.len());
        engine.process(block, controls, &mut buffers);
        for (slot, output) in outputs.iter_mut().enumerate() {
            output.extend_from_slice(buffers.voice(slot));
        }
    }
    outputs
}

pub fn voices(pitches: &[f32], formant_ratio: f32) -> [VoiceControl; MAX_VOICES] {
    let mut controls = [VoiceControl::default(); MAX_VOICES];
    for (control, &pitch) in controls.iter_mut().zip(pitches) {
        *control = VoiceControl {
            active: true,
            pitch,
            formant_ratio,
        };
    }
    controls
}

pub fn cents(actual: f32, expected: f32) -> f32 {
    1200.0 * (actual / expected).log2()
}
