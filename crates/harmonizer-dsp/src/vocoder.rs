use crate::engine::{Engine, MAX_VOICES, VoiceBuffers, VoiceControl};
use crate::params::Quality;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex, num_complex::Complex32};
use std::{f32::consts::TAU, mem, sync::Arc};

pub struct VocoderEngine {
    analysis: Analysis,
    inverse: Arc<dyn ComplexToReal<f32>>,
    synthesis: Vec<Complex32>,
    inverse_scratch: Vec<Complex32>,
    frame: Vec<f32>,
    window: Vec<f32>,
    synthesis_window: Vec<f32>,
    input: Vec<f32>,
    cursor: usize,
    hop: usize,
    transients: Transients,
    voices: [Voice; MAX_VOICES],
}

impl VocoderEngine {
    pub fn new(sample_rate: f32, quality: Quality) -> Self {
        let (base_size, overlap) = match quality {
            Quality::LowLatency => (1024, 4),
            Quality::Balanced => (2048, 4),
            Quality::HighQuality => (4096, 8),
        };
        let scaled = (base_size as f32 * sample_rate / 48_000.0).max(overlap as f32);
        let lower = 1 << (scaled as usize).ilog2();
        let size = if scaled > lower as f32 * 1.5 {
            lower * 2
        } else {
            lower
        };
        let hop = size / overlap;

        let mut planner = RealFftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let bins = forward.complex_len();
        let window: Vec<_> = (0..size)
            .map(|i| 0.5 - 0.5 * (TAU * i as f32 / size as f32).cos())
            .collect();
        // Squared Hann windows sum to 3/8 times the overlap count; realfft's inverse is unscaled.
        let gain = 1.0 / (size as f32 * overlap as f32 * 0.375);
        Self {
            analysis: Analysis::new(forward, hop, sample_rate),
            synthesis: inverse.make_input_vec(),
            inverse_scratch: inverse.make_scratch_vec(),
            frame: inverse.make_output_vec(),
            inverse,
            synthesis_window: window.iter().map(|w| w * gain).collect(),
            window,
            input: vec![0.0; size],
            cursor: 0,
            hop,
            transients: Transients::new(sample_rate, size, hop),
            voices: std::array::from_fn(|_| Voice::new(size, bins)),
        }
    }

    fn render_frame(&mut self, controls: &[VoiceControl; MAX_VOICES]) {
        for ((dst, sample), window) in self
            .frame
            .iter_mut()
            .zip(
                self.input[self.cursor..]
                    .iter()
                    .chain(&self.input[..self.cursor]),
            )
            .zip(&self.window)
        {
            *dst = sample * window;
        }
        self.analysis.process(&mut self.frame);

        for (voice, control) in self.voices.iter_mut().zip(controls) {
            if !control.active {
                continue;
            }
            voice.synthesise(&self.analysis, control, &mut self.synthesis);
            self.inverse
                .process_with_scratch(
                    &mut self.synthesis,
                    &mut self.frame,
                    &mut self.inverse_scratch,
                )
                .unwrap();
            let (head, tail) = voice.output.split_at_mut(self.cursor);
            for ((dst, sample), window) in tail
                .iter_mut()
                .chain(head)
                .zip(&self.frame)
                .zip(&self.synthesis_window)
            {
                *dst += sample * window;
            }
        }
    }

    fn attack_gain(&self, controls: &[VoiceControl; MAX_VOICES]) -> f32 {
        let mut shifted = 0;
        for (voice, control) in self.voices.iter().zip(controls) {
            if !control.active || !voice.initialised {
                continue;
            }
            if control.pitch != 1.0 {
                shifted += 1;
            } else if control.formant_ratio == 1.0 {
                return 0.0;
            }
        }
        // The delayed attack copies are coherent, even when the shifted voices are not.
        1.0 / shifted.max(1) as f32
    }
}

impl Engine for VocoderEngine {
    fn latency(&self) -> usize {
        self.input.len()
    }

    fn reset(&mut self) {
        self.input.fill(0.0);
        self.cursor = 0;
        self.analysis.previous_phase.fill(0.0);
        self.transients.reset();
        for voice in &mut self.voices {
            voice.reset();
        }
    }

    fn reset_voice(&mut self, slot: usize) {
        self.voices[slot].reset();
    }

    fn process(
        &mut self,
        input: &[f32],
        controls: &[VoiceControl; MAX_VOICES],
        out: &mut VoiceBuffers,
    ) {
        if input.is_empty() {
            return;
        }
        for (voice, control) in self.voices.iter_mut().zip(controls) {
            if !control.active {
                voice.suspended = true;
                continue;
            }
            if voice.suspended {
                voice.reset();
            }
        }
        let mut offset = 0;
        while offset < input.len() {
            let attack_gain = self.attack_gain(controls);
            let count = (self.hop - self.cursor % self.hop).min(input.len() - offset);
            let end = self.cursor + count;
            let delay = &mut self.input[self.cursor..end];
            let (mix, attack) = self.transients.advance(delay, attack_gain);
            for (slot, (voice, control)) in self.voices.iter_mut().zip(controls).enumerate() {
                if !control.active {
                    continue;
                }
                let output = &mut out.voice_mut(slot)[offset..offset + count];
                let pending = &mut voice.output[self.cursor..end];
                if let Some(attack) = attack.filter(|_| voice.initialised && control.pitch != 1.0) {
                    for (((dst, pending), &weight), &dry) in
                        output.iter_mut().zip(pending).zip(mix.iter()).zip(attack)
                    {
                        let wet = mem::take(pending);
                        *dst += wet * (1.0 - weight) + dry;
                    }
                } else {
                    for (dst, pending) in output.iter_mut().zip(pending) {
                        *dst += mem::take(pending);
                    }
                }
            }
            if attack.is_some() {
                mix.fill(0.0);
            }
            delay.copy_from_slice(&input[offset..offset + count]);
            self.cursor = end & (self.input.len() - 1);
            offset += count;
            // A frame ending here contributes only from the next sample on, so its input is N samples behind.
            if self.cursor.is_multiple_of(self.hop) {
                self.transients.detect(&self.input[end - self.hop..end]);
                self.render_frame(controls);
            }
        }
    }
}

struct Transients {
    mix: Vec<f32>,
    attack: Vec<f32>,
    attack_gain: f32,
    attack_step: f32,
    cursor: usize,
    latency: usize,
    step: usize,
    hold: usize,
    fade_out: usize,
    look_back: usize,
    protect_before: usize,
    recovery: usize,
    decay: f32,
    background: f32,
    since_onset: usize,
}

impl Transients {
    fn new(sample_rate: f32, latency: usize, hop: usize) -> Self {
        let samples = |ms: f32| (sample_rate * ms / 1000.0).round() as usize;
        let step = samples(1.0).next_power_of_two().min(hop);
        let hold = samples(3.5);
        let fade_out = samples(7.0).max(1);
        let recovery = samples(20.0);
        Self {
            mix: vec![0.0; (latency + hold + fade_out).next_power_of_two()],
            attack: vec![0.0; hop],
            attack_gain: 1.0,
            attack_step: 1.0 / samples(4.0).max(1) as f32,
            cursor: 0,
            latency,
            step,
            hold,
            fade_out,
            look_back: (latency * 3 / 4).min(samples(24.0)),
            protect_before: (latency / 2).min(samples(14.0)),
            recovery,
            decay: (-(step as f32) / (0.012 * sample_rate)).exp(),
            background: 0.0,
            since_onset: recovery,
        }
    }

    fn reset(&mut self) {
        self.mix.fill(0.0);
        self.attack_gain = 1.0;
        self.cursor = 0;
        self.background = 0.0;
        self.since_onset = self.recovery;
    }

    fn advance(&mut self, delayed: &[f32], target: f32) -> (&mut [f32], Option<&[f32]>) {
        let start = self.cursor;
        let end = start + delayed.len();
        self.cursor = end & (self.mix.len() - 1);
        let mix = &mut self.mix[start..end];
        if !mix.iter().any(|&weight| weight > 0.0) {
            self.attack_gain = target;
            return (mix, None);
        }
        let attack = &mut self.attack[..delayed.len()];
        for ((attack, &sample), &weight) in attack.iter_mut().zip(delayed).zip(mix.iter()) {
            if weight == 0.0 {
                self.attack_gain = target;
            } else {
                self.attack_gain +=
                    (target - self.attack_gain).clamp(-self.attack_step, self.attack_step);
            }
            *attack = sample * weight * self.attack_gain;
        }
        (mix, Some(attack))
    }

    fn detect(&mut self, newest: &[f32]) {
        for (i, chunk) in newest.chunks_exact(self.step).enumerate() {
            let power = chunk.iter().map(|s| s * s).sum::<f32>() / self.step as f32;
            if self.since_onset >= self.recovery && power > (8.0 * self.background).max(1e-6) {
                let onset = self.latency - newest.len() + i * self.step;
                self.protect(onset);
                self.since_onset = 0;
            }
            self.since_onset = (self.since_onset + self.step).min(self.recovery);
            self.background = self.background * self.decay + power * (1.0 - self.decay);
        }
    }

    fn protect(&mut self, onset: usize) {
        // Look back before the attack: a shifted FFT frame spreads it into earlier samples.
        let start = onset.saturating_sub(self.look_back);
        let full = onset.saturating_sub(self.protect_before);
        let release = onset + self.hold;
        let mask = self.mix.len() - 1;
        for offset in start..release + self.fade_out {
            let fade_in = (offset - start) as f32 / (full - start).max(1) as f32;
            let fade_out = 1.0 - offset.saturating_sub(release) as f32 / self.fade_out as f32;
            let weight = fade_in.min(fade_out);
            let bin = &mut self.mix[(self.cursor + offset) & mask];
            *bin = bin.max(weight);
        }
    }
}

struct Analysis {
    forward: Arc<dyn RealToComplex<f32>>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    magnitude: Vec<f32>,
    previous_phase: Vec<f32>,
    advance: Vec<f32>,
    advance_per_bin: f32,
    envelope: Vec<f32>,
    log_magnitude: Vec<f32>,
    envelope_radius: usize,
    peaks: Vec<Region>,
    owners: Vec<usize>,
    previous_owners: Vec<usize>,
}

struct Region {
    peak: usize,
    end: usize,
}

impl Analysis {
    fn new(forward: Arc<dyn RealToComplex<f32>>, hop: usize, sample_rate: f32) -> Self {
        let bins = forward.complex_len();
        let size = forward.len();
        Self {
            spectrum: forward.make_output_vec(),
            scratch: forward.make_scratch_vec(),
            forward,
            magnitude: vec![0.0; bins],
            previous_phase: vec![0.0; bins],
            advance: vec![0.0; bins],
            advance_per_bin: TAU * hop as f32 / size as f32,
            envelope: vec![0.0; bins],
            log_magnitude: vec![0.0; bins],
            envelope_radius: (85.0 * size as f32 / sample_rate).round() as usize,
            peaks: Vec::with_capacity(bins),
            owners: vec![0; bins],
            previous_owners: vec![0; bins],
        }
    }

    fn process(&mut self, frame: &mut [f32]) {
        self.forward
            .process_with_scratch(frame, &mut self.spectrum, &mut self.scratch)
            .unwrap();

        let mut loudest = 0.0_f32;
        for (k, bin) in self.spectrum.iter().enumerate() {
            let magnitude = bin.norm();
            let phase = bin.arg();
            let expected = k as f32 * self.advance_per_bin;
            self.advance[k] = expected + wrap_phase(phase - self.previous_phase[k] - expected);
            self.previous_phase[k] = phase;
            self.magnitude[k] = magnitude;
            loudest = loudest.max(magnitude);
        }
        self.find_regions(loudest * 1e-4);
        self.smooth_envelope((loudest * 1e-3).max(1e-20));
    }

    fn find_regions(&mut self, floor: f32) {
        mem::swap(&mut self.owners, &mut self.previous_owners);
        self.peaks.clear();
        let mut start = 0;
        let mut last_peak = None;
        for (k, &magnitude) in self.magnitude.iter().enumerate() {
            if magnitude <= floor
                || (k > 0 && magnitude <= self.magnitude[k - 1])
                || (k + 1 < self.magnitude.len() && magnitude < self.magnitude[k + 1])
            {
                continue;
            }
            if let Some(peak) = last_peak {
                let end = (peak + k) / 2 + 1;
                self.peaks.push(Region { peak, end });
                self.owners[start..end].fill(peak);
                start = end;
            }
            last_peak = Some(k);
        }
        if let Some(peak) = last_peak {
            let end = self.magnitude.len();
            self.peaks.push(Region { peak, end });
            self.owners[start..end].fill(peak);
        }
    }

    fn smooth_envelope(&mut self, floor: f32) {
        let radius = self.envelope_radius;
        let len = self.magnitude.len();
        // Fill the valleys between partials before smoothing so they don't become formant dips.
        for (k, magnitude) in self.log_magnitude.iter_mut().enumerate() {
            let neighbours = &self.magnitude[k.saturating_sub(radius)..(k + radius + 1).min(len)];
            *magnitude = neighbours.iter().copied().fold(floor, f32::max).ln();
        }
        let log = |k: usize| self.log_magnitude[k];
        let mut start = 0;
        let mut end = (radius + 1).min(len);
        let mut sum = (start..end).map(log).sum::<f32>();
        for (k, envelope) in self.envelope.iter_mut().enumerate() {
            *envelope = (sum / (end - start) as f32).exp();
            if k >= radius {
                sum -= log(start);
                start += 1;
            }
            if end < len {
                sum += log(end);
                end += 1;
            }
        }
    }
}

struct Voice {
    output: Vec<f32>,
    rotation: Vec<f32>,
    previous_rotation: Vec<f32>,
    initialised: bool,
    suspended: bool,
}

impl Voice {
    fn new(size: usize, bins: usize) -> Self {
        Self {
            output: vec![0.0; size],
            rotation: vec![0.0; bins],
            previous_rotation: vec![0.0; bins],
            initialised: false,
            suspended: false,
        }
    }

    fn reset(&mut self) {
        self.output.fill(0.0);
        self.initialised = false;
        self.suspended = false;
    }

    fn synthesise(
        &mut self,
        analysis: &Analysis,
        control: &VoiceControl,
        spectrum: &mut [Complex32],
    ) {
        let pitch = control.pitch.clamp(0.25, 4.0);
        let formant = control.formant_ratio.max(f32::MIN_POSITIVE);
        let formant_step = formant.recip();
        let last = spectrum.len() - 1;
        let nyquist_advance = last as f32 * analysis.advance_per_bin;
        spectrum.fill(Complex32::ZERO);
        let mut start = 0;
        for &Region { peak, end } in &analysis.peaks {
            let advance = analysis.advance[peak];
            let delta = (pitch - 1.0) * advance;
            // Keep phase relative to the analysis peak so one rotation locks its entire region.
            let phase = if self.initialised && pitch != 1.0 {
                wrap_phase(self.previous_rotation[analysis.previous_owners[peak]] + delta)
            } else {
                0.0
            };
            self.rotation[peak] = phase;
            if pitch > 1.0 && advance * pitch > nyquist_advance {
                start = end;
                continue;
            }

            // Moving the whole peak region keeps the window's lobes intact. The sign change
            // for an odd bin shift keeps the rotation referenced to the centre of the window.
            let shift = (delta / analysis.advance_per_bin).round() as isize;
            let (sin, cos) = phase.sin_cos();
            let rotation = if shift % 2 == 0 {
                Complex32::new(cos, sin)
            } else {
                Complex32::new(-cos, -sin)
            };
            let kept_start = start.max((-shift).max(0) as usize);
            let kept_end = end.min((spectrum.len() as isize - shift).max(0) as usize);
            for src in kept_start..kept_end {
                let dst = src.wrapping_add_signed(shift);
                let gain = if pitch == formant {
                    1.0
                } else {
                    let x = (dst as f32 * formant_step).min(last as f32);
                    let lo = (x as usize).min(last - 1);
                    let a = analysis.envelope[lo];
                    let b = analysis.envelope[lo + 1];
                    let envelope = a + (x - lo as f32) * (b - a);
                    (envelope / analysis.envelope[src]).clamp(0.125, 8.0)
                };
                spectrum[dst] += analysis.spectrum[src] * rotation * gain;
            }
            start = end;
        }
        spectrum[0].im = 0.0;
        spectrum[last].im = 0.0;
        mem::swap(&mut self.rotation, &mut self.previous_rotation);
        self.initialised = !analysis.peaks.is_empty();
    }
}

fn wrap_phase(phase: f32) -> f32 {
    phase - TAU * (phase / TAU).round()
}
