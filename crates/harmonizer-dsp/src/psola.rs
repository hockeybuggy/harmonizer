use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};

use crate::engine::{Engine, MAX_VOICES, VoiceBuffers, VoiceControl};
use crate::params::Quality;
use crate::pitch::{MAX_PITCH_HZ, PitchTracker, peak_offset};

const WINDOW_STEPS: usize = 512;

fn clear_inaudible(value: f32) -> f32 {
    if value.abs() < 1e-20 { 0.0 } else { value }
}

pub struct PsolaEngine {
    sample_rate: f32,
    latency: usize,
    lead: usize,
    time: i64,
    history: History,
    tracker: PitchTracker,
    analysis: AnalysisMarks,
    voices: [Synthesis; MAX_VOICES],
    window: [f32; WINDOW_STEPS + 1],
    fade_step: f32,
    passthrough_gain: f32,
}

impl PsolaEngine {
    pub fn new(sample_rate: f32, quality: Quality) -> Self {
        let (latency_ms, min_hz, window_ms) = match quality {
            Quality::LowLatency => (16.0, 80.0, 26.0),
            Quality::Balanced => (25.0, 60.0, 36.0),
            Quality::HighQuality => (45.0, 50.0, 45.0),
        };
        let latency = (sample_rate * latency_ms / 1000.0).round() as usize;
        let tracker = PitchTracker::new(sample_rate, min_hz, window_ms);
        let max_lag = tracker.max_lag();
        // A full grain radius takes priority over waiting for a newer analysis mark.
        let lead = max_lag;
        let size = (latency + 3 * max_lag + 4).next_power_of_two();
        let grain_capacity =
            (2.0 * lead as f32 * (4.0 * MAX_PITCH_HZ) / sample_rate).ceil() as usize + 4;
        Self {
            sample_rate,
            latency,
            lead,
            time: 0,
            history: History::new(size),
            tracker,
            analysis: AnalysisMarks::new(sample_rate, size),
            voices: std::array::from_fn(|_| Synthesis::new(grain_capacity)),
            window: std::array::from_fn(|i| {
                0.5 + 0.5 * (PI * i as f32 / WINDOW_STEPS as f32).cos()
            }),
            fade_step: (1.0 / (sample_rate * 0.004)).min(1.0),
            passthrough_gain: 1.0,
        }
    }

    fn analyse_sample(&mut self, sample: f32) {
        let index = self.history.index(self.time);
        self.history.samples[index] = sample;
        let period = self.tracker.push(sample);
        self.history.periods[index] = period.unwrap_or(0.0);
        self.analysis
            .push(&mut self.history, self.time, sample, period);
    }
}

impl Engine for PsolaEngine {
    fn latency(&self) -> usize {
        self.latency
    }

    fn detected_pitch(&self) -> Option<f32> {
        self.tracker
            .period()
            .map(|period| self.sample_rate / period)
    }

    fn reset(&mut self) {
        self.time = 0;
        self.history.reset();
        self.tracker.reset();
        self.analysis.reset();
        self.passthrough_gain = 1.0;
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
        voices: &[VoiceControl; MAX_VOICES],
        out: &mut VoiceBuffers,
    ) {
        if input.is_empty() {
            return;
        }
        let mut slots = [0; MAX_VOICES];
        let mut count = 0;
        for (slot, control) in voices.iter().enumerate() {
            if control.active {
                slots[count] = slot;
                count += 1;
                self.voices[slot]
                    .low_cut
                    .configure(control.pitch, self.sample_rate);
            } else {
                self.voices[slot].reset();
            }
        }

        // Every voice passes unvoiced input through unshifted, and identical copies add
        // coherently, so share that passthrough between them to keep consonants at their level.
        let passthrough_target = 1.0 / count.max(1) as f32;
        for (i, &sample) in input.iter().enumerate() {
            self.analyse_sample(sample);
            self.passthrough_gain +=
                (passthrough_target - self.passthrough_gain).clamp(-self.fade_step, self.fade_step);
            let time = self.time - self.latency as i64;
            let index = self.history.index(time);
            let dry = self.history.samples[index];
            let period = self.history.periods[index];
            let horizon = time + self.lead as i64;
            for &slot in &slots[..count] {
                let control = voices[slot];
                let voice = &mut self.voices[slot];
                let first = if voice.started { horizon } else { time };
                let mut centre = voice.next_centre.unwrap_or(first as f64);
                voice.started = true;
                while centre <= horizon as f64 {
                    let centre_period =
                        self.history.periods[self.history.index(centre.round() as i64)];
                    if centre_period == 0.0 {
                        voice.next_centre = None;
                        break;
                    }
                    let period = f64::from(centre_period);
                    let target_period = (f64::from(self.sample_rate) / f64::from(control.pitch))
                        .clamp(0.25 * period, 4.0 * period)
                        .max(1.0);
                    if let Some(mut mark) = self.analysis.nearest(centre) {
                        // Anchor neutral grains to the dry timeline rather than letting tiny
                        // period-estimation errors displace their phase.
                        if control.formant_ratio == 1.0
                            && (target_period - period).abs() < 0.0012 * period
                        {
                            mark.centre = centre;
                        }
                        voice.queue(
                            mark,
                            centre,
                            target_period,
                            control.formant_ratio,
                            self.lead,
                            self.latency,
                        );
                    }
                    centre += target_period;
                    voice.next_centre = Some(centre);
                }

                voice.mix = if period > 0.0 {
                    (voice.mix + self.fade_step).min(1.0)
                } else {
                    (voice.mix - self.fade_step).max(0.0)
                };
                let shifted = voice.render(time, dry, &self.history, &self.window);
                let passthrough = dry * self.passthrough_gain;
                out.voice_mut(slot)[i] += passthrough + voice.mix * (shifted - passthrough);
            }
            self.time += 1;
        }
    }
}

struct History {
    samples: Vec<f32>,
    filtered: Vec<f32>,
    periods: Vec<f32>,
    mask: usize,
}

impl History {
    fn new(size: usize) -> Self {
        Self {
            samples: vec![0.0; size],
            filtered: vec![0.0; size],
            periods: vec![0.0; size],
            mask: size - 1,
        }
    }

    fn index(&self, time: i64) -> usize {
        time as usize & self.mask
    }

    fn sample_at(&self, time: f64) -> f32 {
        let base = time.floor();
        let index = self.index(base as i64);
        let a = self.samples[index];
        let b = self.samples[(index + 1) & self.mask];
        a + (b - a) * (time - base) as f32
    }

    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.filtered.fill(0.0);
        self.periods.fill(0.0);
    }
}

#[derive(Clone, Copy)]
struct Mark {
    centre: f64,
    period: f32,
}

struct AnalysisMarks {
    marks: VecDeque<Mark>,
    continuous: bool,
    filter: [f32; 3],
    filter_step: f32,
}

impl AnalysisMarks {
    fn new(sample_rate: f32, history_size: usize) -> Self {
        let min_spacing = (0.6 * sample_rate / MAX_PITCH_HZ).floor().max(1.0) as usize;
        Self {
            marks: VecDeque::with_capacity(history_size / min_spacing + 4),
            continuous: false,
            filter: [0.0; 3],
            filter_step: 1.0 - (-TAU * 900.0 / sample_rate).exp(),
        }
    }

    fn reset(&mut self) {
        self.marks.clear();
        self.continuous = false;
        self.filter = [0.0; 3];
    }

    fn push(&mut self, history: &mut History, time: i64, mut sample: f32, period: Option<f32>) {
        for stage in &mut self.filter {
            *stage = clear_inaudible(*stage + self.filter_step * (sample - *stage));
            sample = *stage;
        }
        let index = history.index(time);
        history.filtered[index] = sample;

        let Some(period) = period else {
            self.continuous = false;
            return;
        };
        let (start, end) = if let Some(last) = self.marks.back().filter(|_| self.continuous) {
            let expected = last.centre + f64::from(period);
            let search = f64::from(0.18 * period);
            let end = (expected + search).floor() as i64;
            if end >= time {
                return;
            }
            ((expected - search).ceil() as i64, end)
        } else {
            // Seed from a full cycle, but do not overwrite marks on the delayed output timeline.
            let start = self
                .marks
                .back()
                .map_or(time - period.ceil() as i64, |last| {
                    (last.centre.ceil() as i64 + 1).max(time - period.ceil() as i64)
                });
            if start >= time {
                return;
            }
            (start, time - 1)
        };
        let filtered_at = |n| history.filtered[history.index(n)];
        let mut peak = start;
        for n in start + 1..=end {
            if filtered_at(n) > filtered_at(peak) {
                peak = n;
            }
        }
        let offset = peak_offset(
            filtered_at(peak - 1),
            filtered_at(peak),
            filtered_at(peak + 1),
        );
        if self.marks.len() == self.marks.capacity() {
            self.marks.pop_front();
        }
        self.marks.push_back(Mark {
            centre: peak as f64 + f64::from(offset),
            period,
        });
        self.continuous = true;
    }

    fn nearest(&self, time: f64) -> Option<Mark> {
        let after = self.marks.partition_point(|mark| mark.centre < time);
        let earlier = after.checked_sub(1).and_then(|i| self.marks.get(i));
        let later = self.marks.get(after);
        let mark = match (earlier, later) {
            (Some(a), Some(b)) if time - a.centre <= b.centre - time => a,
            (_, Some(b)) => b,
            (Some(a), None) => a,
            (None, None) => return None,
        };
        ((mark.centre - time).abs() <= f64::from(1.4 * mark.period)).then_some(*mark)
    }
}

struct Grain {
    centre: f64,
    source: f64,
    inverse_radius: f32,
    weight_floor: f32,
    formant_ratio: f64,
    neutral: bool,
}

struct Synthesis {
    started: bool,
    next_centre: Option<f64>,
    grains: Vec<Grain>,
    mix: f32,
    low_cut: LowCut,
}

impl Synthesis {
    fn new(grain_capacity: usize) -> Self {
        Self {
            started: false,
            next_centre: None,
            grains: Vec::with_capacity(grain_capacity),
            mix: 0.0,
            low_cut: LowCut::default(),
        }
    }

    fn reset(&mut self) {
        self.started = false;
        self.next_centre = None;
        self.grains.clear();
        self.mix = 0.0;
        self.low_cut = LowCut::default();
    }

    fn queue(
        &mut self,
        mark: Mark,
        centre: f64,
        target_period: f64,
        formant_ratio: f32,
        lead: usize,
        latency: usize,
    ) {
        let formant_ratio = formant_ratio.clamp(0.5, 2.0);
        let mut radius = (mark.period / formant_ratio).min(lead as f32);
        if formant_ratio > 1.0 {
            let available = latency as f64 - (mark.centre - centre) - 1.0;
            radius = radius.min(available as f32 / (formant_ratio - 1.0));
        }
        if radius > 0.0 && self.grains.len() < self.grains.capacity() {
            self.grains.push(Grain {
                centre,
                source: mark.centre,
                inverse_radius: radius.recip(),
                // Keep the Hann taper when grains barely overlap or leave gaps.
                weight_floor: (target_period as f32 / radius - 1.0).clamp(0.05, 1.0),
                formant_ratio: f64::from(formant_ratio),
                neutral: formant_ratio == 1.0 && mark.centre == centre,
            });
        }
    }

    fn render(
        &mut self,
        time: i64,
        dry: f32,
        history: &History,
        window: &[f32; WINDOW_STEPS + 1],
    ) -> f32 {
        let mut shifted = 0.0;
        let mut neutral = 0.0;
        let mut weight = 0.0;
        let mut weighted_floor = 0.0;
        let mut i = 0;
        while i < self.grains.len() {
            let grain = &self.grains[i];
            let offset = time as f64 - grain.centre;
            let phase = offset.abs() as f32 * grain.inverse_radius;
            if phase < 1.0 {
                let index = phase * WINDOW_STEPS as f32;
                let base = index as usize;
                let a = window[base];
                let gain = a + (window[base + 1] - a) * (index - base as f32);
                if grain.neutral {
                    neutral += gain;
                } else {
                    shifted +=
                        gain * history.sample_at(grain.source + offset * grain.formant_ratio);
                }
                weight += gain;
                weighted_floor += gain * grain.weight_floor;
            } else if offset > 0.0 {
                self.grains.swap_remove(i);
                continue;
            }
            i += 1;
        }
        // Blend the floors with the grains so a bend cannot change gain at a window edge.
        let normalise = weight / (weight * weight).max(weighted_floor).max(f32::MIN_POSITIVE);
        self.low_cut.process(shifted * normalise) + dry * neutral * normalise
    }
}

#[derive(Default)]
struct LowCut {
    cutoff: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    band_state: f32,
    low_state: f32,
}

impl LowCut {
    fn configure(&mut self, pitch: f32, sample_rate: f32) {
        let cutoff = (0.4 * pitch).min(100.0);
        if self.cutoff != cutoff {
            self.cutoff = cutoff;
            let g = (PI * cutoff / sample_rate).tan();
            self.a1 = 1.0 / (1.0 + g * (g + std::f32::consts::SQRT_2));
            self.a2 = g * self.a1;
            self.a3 = g * self.a2;
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let difference = input - self.low_state;
        let band = self.a1 * self.band_state + self.a2 * difference;
        let low = self.low_state + self.a2 * self.band_state + self.a3 * difference;
        self.band_state = clear_inaudible(2.0 * band - self.band_state);
        self.low_state = clear_inaudible(2.0 * low - self.low_state);
        input - std::f32::consts::SQRT_2 * band - low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_mark_filter_has_no_denormal_tail_after_silence() {
        let mut marks = AnalysisMarks::new(192_000.0, 8);
        let mut history = History::new(8);
        marks.push(&mut history, 0, 1.0, None);
        for time in 1..192_000 {
            marks.push(&mut history, time, 0.0, None);
        }
        assert!(
            marks.filter.iter().all(|value| !value.is_subnormal()),
            "filter states: {:?}",
            marks.filter
        );
    }

    #[test]
    fn low_cut_has_no_denormal_tail_after_silence() {
        let mut filter = LowCut::default();
        filter.configure(110.0, 192_000.0);
        filter.process(1.0);
        let mut output = 0.0;
        for _ in 0..192_000 {
            output = filter.process(0.0);
        }
        assert!(
            [filter.band_state, filter.low_state, output]
                .iter()
                .all(|value| !value.is_subnormal()),
            "band state {}, low state {}, output {output}",
            filter.band_state,
            filter.low_state,
        );
    }
}
