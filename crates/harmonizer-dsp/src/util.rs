pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

pub fn semitones_to_ratio(semitones: f32) -> f32 {
    (semitones / 12.0).exp2()
}

/// Fixed-capacity delay line whose delay can change at runtime without allocating.
pub struct DelayLine {
    buffer: Vec<f32>,
    write: usize,
}

impl DelayLine {
    pub fn new(max_delay: usize) -> Self {
        Self {
            buffer: vec![0.0; max_delay + 1],
            write: 0,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
    }

    pub fn process(&mut self, input: f32, delay: usize) -> f32 {
        let len = self.buffer.len();
        debug_assert!(delay < len);
        self.buffer[self.write] = input;
        let read = (self.write + len - delay) % len;
        self.write = (self.write + 1) % len;
        self.buffer[read]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_line_delays_by_requested_samples() {
        let mut delay = DelayLine::new(8);
        let out: Vec<f32> = (1..=6).map(|n| delay.process(n as f32, 3)).collect();
        assert_eq!(out, [0.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn zero_delay_passes_through() {
        let mut delay = DelayLine::new(4);
        assert_eq!(delay.process(5.0, 0), 5.0);
    }

    #[test]
    fn a4_is_440() {
        assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-4);
        assert!((midi_to_hz(81.0) - 880.0).abs() < 1e-3);
    }
}
