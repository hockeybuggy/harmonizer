use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub(crate) const MAX_PITCH_HZ: f32 = 1000.0;

pub(crate) struct PitchTracker {
    window: Vec<f32>,
    write: usize,
    until_analysis: usize,
    hop: usize,
    min_lag: usize,
    max_lag: usize,
    period: Option<f32>,
    pending_octave: i8,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    time: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    energy: Vec<f32>,
}

impl PitchTracker {
    pub(crate) fn new(sample_rate: f32, min_hz: f32, window_ms: f32) -> Self {
        let min_lag = (sample_rate / MAX_PITCH_HZ).floor().max(2.0) as usize;
        let max_lag = (sample_rate / min_hz).ceil().max(min_lag as f32) as usize;
        let window = ((sample_rate * window_ms / 1000.0).round() as usize).max(max_lag + 2);
        let hop = (sample_rate * 0.003).round().max(1.0) as usize;
        let fft_len = (window + max_lag + 1).next_power_of_two();
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(fft_len);
        let inverse = planner.plan_fft_inverse(fft_len);
        Self {
            window: vec![0.0; window],
            write: 0,
            until_analysis: window,
            hop,
            min_lag,
            max_lag,
            period: None,
            pending_octave: 0,
            time: forward.make_input_vec(),
            spectrum: forward.make_output_vec(),
            scratch: vec![
                Complex32::default();
                forward.get_scratch_len().max(inverse.get_scratch_len())
            ],
            energy: vec![0.0; window + 1],
            forward,
            inverse,
        }
    }

    pub(crate) fn max_lag(&self) -> usize {
        self.max_lag
    }

    pub(crate) fn period(&self) -> Option<f32> {
        self.period
    }

    pub(crate) fn reset(&mut self) {
        self.write = 0;
        self.until_analysis = self.window.len();
        self.period = None;
        self.pending_octave = 0;
    }

    pub(crate) fn push(&mut self, sample: f32) -> Option<f32> {
        self.window[self.write] = sample;
        self.write += 1;
        if self.write == self.window.len() {
            self.write = 0;
        }
        self.until_analysis -= 1;
        if self.until_analysis == 0 {
            self.until_analysis = self.hop;
            let candidate = self.analyse();
            self.period = self.confirm_octave(candidate);
        }
        self.period
    }

    fn analyse(&mut self) -> Option<f32> {
        let len = self.window.len();
        let mean = self.window.iter().sum::<f32>() / len as f32;
        let frame = self.window[self.write..]
            .iter()
            .chain(&self.window[..self.write]);
        for (i, (&sample, dst)) in frame.zip(&mut self.time).enumerate() {
            let centred = sample - mean;
            *dst = centred;
            self.energy[i + 1] = self.energy[i] + centred * centred;
        }
        let energy = self.energy[len];
        if energy < 1e-9 * len as f32 {
            return None;
        }
        self.time[len..].fill(0.0);
        self.forward
            .process_with_scratch(&mut self.time, &mut self.spectrum, &mut self.scratch)
            .unwrap();
        for bin in &mut self.spectrum {
            *bin = Complex32::new(bin.norm_sqr(), 0.0);
        }
        self.inverse
            .process_with_scratch(&mut self.spectrum, &mut self.time, &mut self.scratch)
            .unwrap();

        let scale = 2.0 / self.time.len() as f32;
        let nsdf = &mut self.time[..=self.max_lag + 1];
        for (lag, value) in nsdf.iter_mut().enumerate() {
            let overlap_energy = self.energy[len - lag] + (energy - self.energy[lag]);
            *value *= scale / overlap_energy.max(1e-12);
        }
        let first_crossing = nsdf.iter().position(|&value| value <= 0.0)?;
        let mut peaks = (self.min_lag.max(first_crossing + 1)..=self.max_lag)
            .filter(|&lag| nsdf[lag] > nsdf[lag - 1] && nsdf[lag] >= nsdf[lag + 1]);
        let best = peaks.clone().map(|lag| nsdf[lag]).fold(0.0, f32::max);
        let voicing_threshold = if self.period.is_some() { 0.76 } else { 0.86 };
        let threshold = (0.9 * best).max(voicing_threshold);
        let lag = peaks.find(|&lag| nsdf[lag] >= threshold)?;
        let offset = peak_offset(nsdf[lag - 1], nsdf[lag], nsdf[lag + 1]);
        Some(lag as f32 + offset)
    }

    fn confirm_octave(&mut self, candidate: Option<f32>) -> Option<f32> {
        if let (Some(previous), Some(current)) = (self.period, candidate) {
            let ratio = current / previous;
            let direction = if (0.46..=0.55).contains(&ratio) {
                -1
            } else if (1.82..=2.18).contains(&ratio) {
                1
            } else {
                0
            };
            if direction != 0 && self.pending_octave != direction {
                self.pending_octave = direction;
                return self.period;
            }
        }
        self.pending_octave = 0;
        candidate
    }
}

pub(crate) fn peak_offset(left: f32, peak: f32, right: f32) -> f32 {
    let curvature = left - 2.0 * peak + right;
    if curvature < 0.0 {
        (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
    } else {
        0.0
    }
}
