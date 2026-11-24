use crate::engine::{Engine, MAX_VOICES, VoiceBuffers, VoiceControl};
use crate::params::Quality;
use crate::util::DelayLine;

/// Placeholder: passes the input through at the engine's latency until the phase vocoder lands.
pub struct VocoderEngine {
    latency: usize,
    delay: DelayLine,
}

impl VocoderEngine {
    pub fn new(_sample_rate: f32, quality: Quality) -> Self {
        let latency = match quality {
            Quality::LowLatency => 1024,
            Quality::Balanced => 2048,
            Quality::HighQuality => 4096,
        };
        Self {
            latency,
            delay: DelayLine::new(latency),
        }
    }
}

impl Engine for VocoderEngine {
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
