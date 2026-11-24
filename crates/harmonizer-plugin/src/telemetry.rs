use harmonizer_dsp::Harmonizer;
use nice_plug::prelude::AtomicF32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const SILENCE_THRESHOLD: f32 = 2.5e-4;
// Mic hiss and room noise sit well above silence but shouldn't count as unpitched singing.
const UNPITCHED_MIN_PEAK: f32 = 0.01;
const METER_RELEASE_SECONDS: f32 = 0.3;
const METER_FLOOR: f32 = 1e-3;
const MISSING_AFTER_SECONDS: f32 = 1.5;
const UNPITCHED_AFTER_SECONDS: f32 = 3.0;

/// Audio-thread state the editor displays. Written with relaxed atomics: a torn read across
/// fields only means one stale frame.
pub struct Telemetry {
    pub detected_pitch: AtomicF32,
    pub latency_samples: AtomicU32,
    pub sample_rate: AtomicF32,
    pub note_levels: [AtomicF32; 128],
    pub input_meter: AtomicF32,
    pub input_missing: AtomicBool,
    pub input_unpitched: AtomicBool,
    pub notes_seen: AtomicBool,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            detected_pitch: AtomicF32::new(f32::NAN),
            latency_samples: AtomicU32::new(0),
            sample_rate: AtomicF32::new(48_000.0),
            note_levels: std::array::from_fn(|_| AtomicF32::new(0.0)),
            input_meter: AtomicF32::new(0.0),
            input_missing: AtomicBool::new(false),
            input_unpitched: AtomicBool::new(false),
            notes_seen: AtomicBool::new(false),
        }
    }
}

impl Telemetry {
    /// Returns whether anything visible changed.
    pub fn publish(&self, harmonizer: &Harmonizer) -> bool {
        let mut levels = [0.0f32; 128];
        for (_, voice) in harmonizer.voices().active() {
            let level = &mut levels[voice.note as usize & 127];
            *level = level.max(voice.envelope);
        }
        let mut changed = false;
        for (stored, level) in self.note_levels.iter().zip(levels) {
            changed |= stored.swap(level, Ordering::Relaxed) != level;
        }

        let pitch = harmonizer.detected_pitch().unwrap_or(f32::NAN);
        let previous = self.detected_pitch.swap(pitch, Ordering::Relaxed);
        changed |= !(previous.is_nan() && pitch.is_nan()) && previous != pitch;

        let latency = harmonizer.latency() as u32;
        changed |= self.latency_samples.swap(latency, Ordering::Relaxed) != latency;
        self.sample_rate
            .store(harmonizer.sample_rate(), Ordering::Relaxed);
        changed
    }
}

/// Watches what reaches the plugin so the editor can explain silence: no audio routed in, no
/// notes arriving, or input absolute mode can't track.
#[derive(Default)]
pub struct InputWatch {
    meter: f32,
    silent_samples: u64,
    unpitched_samples: u64,
    notes_seen: bool,
}

impl InputWatch {
    pub fn note_received(&mut self) {
        self.notes_seen = true;
    }

    pub fn peak(input: &[&mut [f32]]) -> f32 {
        input
            .iter()
            .flat_map(|c| c.iter())
            .fold(0.0, |peak, s| peak.max(s.abs()))
    }

    /// `pitched` is whether absolute mode's tracker found a pitch, or `None` in relative mode.
    pub fn observe(&mut self, peak: f32, samples: usize, sample_rate: f32, pitched: Option<bool>) {
        let release = (-(samples as f32) / (METER_RELEASE_SECONDS * sample_rate)).exp();
        self.meter = peak.max(self.meter * release);
        if self.meter < METER_FLOOR {
            self.meter = 0.0;
        }

        let silent = peak < SILENCE_THRESHOLD;
        self.silent_samples = if silent {
            self.silent_samples + samples as u64
        } else {
            0
        };
        self.unpitched_samples = match pitched {
            Some(false) if peak >= UNPITCHED_MIN_PEAK => self.unpitched_samples + samples as u64,
            _ => 0,
        };
    }

    /// Returns whether anything visible changed.
    pub fn publish(&self, telemetry: &Telemetry, sample_rate: f32) -> bool {
        let seconds = |samples: u64| samples as f32 / sample_rate;
        let previous = telemetry.input_meter.load(Ordering::Relaxed);
        let mut changed = (self.meter - previous).abs() > 0.05 * previous.max(METER_FLOOR);
        if changed {
            telemetry.input_meter.store(self.meter, Ordering::Relaxed);
        }
        for (flag, value) in [
            (
                &telemetry.input_missing,
                seconds(self.silent_samples) >= MISSING_AFTER_SECONDS,
            ),
            (
                &telemetry.input_unpitched,
                seconds(self.unpitched_samples) >= UNPITCHED_AFTER_SECONDS,
            ),
            (&telemetry.notes_seen, self.notes_seen),
        ] {
            changed |= flag.swap(value, Ordering::Relaxed) != value;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;
    const BLOCK: usize = 480;

    fn feed(watch: &mut InputWatch, seconds: f32, peak: f32, pitched: Option<bool>) {
        for _ in 0..(seconds * RATE / BLOCK as f32).round() as usize {
            watch.observe(peak, BLOCK, RATE, pitched);
        }
    }

    fn flags(watch: &InputWatch) -> (bool, bool) {
        let telemetry = Telemetry::default();
        watch.publish(&telemetry, RATE);
        (
            telemetry.input_missing.load(Ordering::Relaxed),
            telemetry.input_unpitched.load(Ordering::Relaxed),
        )
    }

    #[test]
    fn meter_repaints_during_a_small_block_decay() {
        let mut watch = InputWatch::default();
        let telemetry = Telemetry::default();
        watch.observe(1.0, BLOCK, RATE, Some(true));
        assert!(watch.publish(&telemetry, RATE));
        let mut repaints = 0;
        for _ in 0..30 {
            watch.observe(0.0, BLOCK, RATE, Some(true));
            repaints += usize::from(watch.publish(&telemetry, RATE));
        }
        assert!(repaints > 0, "no repaint during a 300 ms decay");
        assert!(telemetry.input_meter.load(Ordering::Relaxed) < 0.4);
        feed(&mut watch, 30.0, 0.0, Some(false));
        watch.publish(&telemetry, RATE);
        assert_eq!(telemetry.input_meter.load(Ordering::Relaxed), 0.0);
    }

    #[test]
    fn flags_missing_input_only_after_sustained_silence() {
        let mut watch = InputWatch::default();
        feed(&mut watch, 1.4, 0.0, Some(false));
        assert_eq!(flags(&watch), (false, false));
        feed(&mut watch, 0.2, 0.0, Some(false));
        assert_eq!(flags(&watch), (true, false));
        feed(&mut watch, 0.01, 0.5, Some(true));
        assert_eq!(flags(&watch), (false, false));
    }

    #[test]
    fn flags_unpitched_input_only_when_loud_and_absolute() {
        let mut watch = InputWatch::default();
        feed(&mut watch, 5.0, 0.003, Some(false));
        assert_eq!(flags(&watch), (false, false), "room noise isn't singing");
        feed(&mut watch, 5.0, 0.5, None);
        assert_eq!(flags(&watch), (false, false), "relative mode never tracks");
        feed(&mut watch, 2.9, 0.5, Some(false));
        assert_eq!(flags(&watch), (false, false));
        feed(&mut watch, 0.2, 0.5, Some(false));
        assert_eq!(flags(&watch), (false, true));
        feed(&mut watch, 0.01, 0.5, Some(true));
        assert_eq!(flags(&watch), (false, false));
    }
}
