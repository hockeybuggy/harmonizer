use harmonizer_dsp::Harmonizer;
use nice_plug::prelude::AtomicF32;
use std::sync::atomic::{AtomicU32, Ordering};

/// Audio-thread state the editor displays. Written with relaxed atomics: a torn read across
/// fields only means one stale frame.
pub struct Telemetry {
    pub detected_pitch: AtomicF32,
    pub latency_samples: AtomicU32,
    pub sample_rate: AtomicF32,
    pub note_levels: [AtomicF32; 128],
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            detected_pitch: AtomicF32::new(f32::NAN),
            latency_samples: AtomicU32::new(0),
            sample_rate: AtomicF32::new(48_000.0),
            note_levels: std::array::from_fn(|_| AtomicF32::new(0.0)),
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
