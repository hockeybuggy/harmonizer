use crate::engine::{Engine, MAX_VOICES, VoiceBuffers, VoiceControl};
use crate::params::Quality;
use crate::util::DelayLine;

/// Placeholder: passes the input through at the engine's latency until PSOLA lands.
pub struct PsolaEngine {
    latency: usize,
    delay: DelayLine,
}

impl PsolaEngine {
    pub fn new(sample_rate: f32, quality: Quality) -> Self {
        let ms = match quality {
            Quality::LowLatency => 12.0,
            Quality::Balanced => 25.0,
            Quality::HighQuality => 45.0,
        };
        let latency = (sample_rate * ms / 1000.0) as usize;
        Self {
            latency,
            delay: DelayLine::new(latency),
        }
    }
}

impl Engine for PsolaEngine {
    fn latency(&self) -> usize {
        self.latency
    }

    fn reset(&mut self) {
        self.delay.reset();
    }

    fn reset_voice(&mut self, _slot: usize) {}

    fn process(
        &mut self,
        input: &[f32],
        voices: &[VoiceControl; MAX_VOICES],
        out: &mut VoiceBuffers,
    ) {
        for (i, &sample) in input.iter().enumerate() {
            let delayed = self.delay.process(sample, self.latency);
            for (slot, voice) in voices.iter().enumerate() {
                if voice.active {
                    out.voice_mut(slot)[i] += delayed;
                }
            }
        }
    }
}
