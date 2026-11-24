#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// Track the input's pitch and resynthesise it at exactly each held note. Monophonic input.
    #[default]
    Absolute,
    /// Transpose any input by the interval between each held note and `reference_note`.
    Relative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Quality {
    LowLatency,
    #[default]
    Balanced,
    HighQuality,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::LowLatency, Quality::Balanced, Quality::HighQuality];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Everything about the harmonizer a user can change while it runs. Gains are linear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub mode: Mode,
    pub quality: Quality,
    pub wet_gain: f32,
    pub dry_gain: f32,
    /// Formant shift in semitones; `0.0` preserves the input's formants.
    pub formant_semitones: f32,
    /// [`Mode::Relative`] only: the MIDI note that leaves the input untransposed.
    pub reference_note: f32,
    pub pitch_bend_range: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// `0.0` ignores velocity, `1.0` scales voice gain fully by velocity.
    pub velocity_sensitivity: f32,
    /// `0.0` keeps every voice centred, `1.0` pans voices by pitch across the stereo field.
    pub stereo_spread: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            quality: Quality::default(),
            wet_gain: 1.0,
            dry_gain: 0.0,
            formant_semitones: 0.0,
            reference_note: 60.0,
            pitch_bend_range: 2.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            velocity_sensitivity: 0.5,
            stereo_spread: 0.5,
        }
    }
}
