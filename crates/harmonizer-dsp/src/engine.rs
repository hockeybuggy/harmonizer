/// The most voices (simultaneously sounding notes, including ones in their release) supported.
pub const MAX_VOICES: usize = 16;

/// What the harmonizer asks of one voice slot for the duration of a single `process` call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceControl {
    /// Whether the slot is sounding. Engines may skip inactive slots entirely.
    pub active: bool,
    /// [`crate::Mode::Absolute`]: the target fundamental in Hz.
    /// [`crate::Mode::Relative`]: the pitch ratio, e.g. `2.0` for an octave up.
    pub pitch: f32,
    /// Spectral envelope scaling. `1.0` preserves the input's formants; `2.0` moves them up an
    /// octave independently of the pitch.
    pub formant_ratio: f32,
}

impl Default for VoiceControl {
    fn default() -> Self {
        Self {
            active: false,
            pitch: 1.0,
            formant_ratio: 1.0,
        }
    }
}

/// Per-voice output scratch space, one planar channel per voice slot.
pub struct VoiceBuffers {
    data: Vec<f32>,
    max_block: usize,
    len: usize,
}

impl VoiceBuffers {
    pub fn new(max_block: usize) -> Self {
        Self {
            data: vec![0.0; max_block * MAX_VOICES],
            max_block,
            len: 0,
        }
    }

    pub fn max_block(&self) -> usize {
        self.max_block
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Sets the length of the current block and zeroes every voice's output.
    pub fn begin_block(&mut self, len: usize) {
        assert!(len <= self.max_block, "block of {len} exceeds max_block");
        self.len = len;
        for slot in 0..MAX_VOICES {
            self.voice_mut(slot).fill(0.0);
        }
    }

    pub fn voice(&self, slot: usize) -> &[f32] {
        let start = slot * self.max_block;
        &self.data[start..start + self.len]
    }

    pub fn voice_mut(&mut self, slot: usize) -> &mut [f32] {
        let start = slot * self.max_block;
        &mut self.data[start..start + self.len]
    }
}

/// A re-pitching engine that renders up to [`MAX_VOICES`] differently pitched copies of a mono
/// input.
///
/// Implementations allocate everything in their constructor. `process`, `reset` and
/// `reset_voice` must not allocate, lock, or do unbounded work: they run on the audio thread.
pub trait Engine {
    /// Delay in samples between an input sample and the output it produces. The harmonizer
    /// delays the dry signal by the same amount and reports it to the host.
    fn latency(&self) -> usize;

    /// Clears all analysis and synthesis state, e.g. after the transport jumps.
    fn reset(&mut self);

    /// Clears the synthesis state of one slot so a newly allocated note starts cleanly.
    fn reset_voice(&mut self, slot: usize);

    /// Consumes `input` (mono, `input.len() == out.len()`). For every active slot in `voices`,
    /// adds `input.len()` samples to `out.voice_mut(slot)`. Output sample `n` corresponds to input
    /// sample `n - latency()`. Inactive slots must be left untouched.
    ///
    /// The engine must still analyse the input when no voice is active so a note that starts
    /// later has history to work from.
    fn process(
        &mut self,
        input: &[f32],
        voices: &[VoiceControl; MAX_VOICES],
        out: &mut VoiceBuffers,
    );

    /// The most recently detected input fundamental in Hz, for engines that track pitch.
    fn detected_pitch(&self) -> Option<f32> {
        None
    }
}
