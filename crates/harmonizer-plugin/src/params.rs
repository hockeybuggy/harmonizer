use nice_plug::prelude::*;
use std::sync::Arc;

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ModeParam {
    #[name = "Absolute"]
    Absolute,
    #[name = "Relative"]
    Relative,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum QualityParam {
    #[name = "Low latency"]
    LowLatency,
    #[name = "Balanced"]
    Balanced,
    #[name = "High quality"]
    HighQuality,
}

/// Live fills its device panel from this order, four parameters per column: mode and mix,
/// then the voice, then how notes play.
#[derive(Params)]
pub struct HarmonizerParams {
    #[id = "mode"]
    pub mode: EnumParam<ModeParam>,
    #[id = "quality"]
    pub quality: EnumParam<QualityParam>,
    #[id = "wet"]
    pub wet: FloatParam,
    #[id = "dry"]
    pub dry: FloatParam,
    #[id = "formant"]
    pub formant: FloatParam,
    #[id = "expression"]
    pub expression: FloatParam,
    #[id = "refnote"]
    pub reference_note: IntParam,
    #[id = "spread"]
    pub stereo_spread: FloatParam,
    #[id = "attack"]
    pub attack: FloatParam,
    #[id = "release"]
    pub release: FloatParam,
    #[id = "velsens"]
    pub velocity_sensitivity: FloatParam,
    #[id = "bendrange"]
    pub pitch_bend_range: IntParam,
}

fn gain_param(name: &str, default_db: f32) -> FloatParam {
    FloatParam::new(
        name,
        util::db_to_gain(default_db),
        FloatRange::Skewed {
            min: util::db_to_gain(util::MINUS_INFINITY_DB),
            max: util::db_to_gain(6.0),
            factor: FloatRange::gain_skew_factor(util::MINUS_INFINITY_DB, 6.0),
        },
    )
    .with_unit(" dB")
    .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
    .with_string_to_value(formatters::s2v_f32_gain_to_db())
}

fn time_param(name: &str, default_ms: f32, max_ms: f32) -> FloatParam {
    FloatParam::new(
        name,
        default_ms,
        FloatRange::Skewed {
            min: 1.0,
            max: max_ms,
            factor: FloatRange::skew_factor(-2.0),
        },
    )
    .with_unit(" ms")
    .with_value_to_string(formatters::v2s_f32_rounded(0))
}

fn percent_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

impl Default for HarmonizerParams {
    fn default() -> Self {
        Self {
            mode: EnumParam::new("Mode", ModeParam::Absolute),
            quality: EnumParam::new("Quality", QualityParam::Balanced),
            wet: gain_param("Wet", 0.0),
            dry: gain_param("Dry", util::MINUS_INFINITY_DB),
            formant: FloatParam::new(
                "Formant",
                0.0,
                FloatRange::Linear {
                    min: -12.0,
                    max: 12.0,
                },
            )
            .with_unit(" st")
            .with_step_size(0.1),
            reference_note: IntParam::new(
                "Reference Note",
                60,
                IntRange::Linear { min: 0, max: 127 },
            )
            .with_value_to_string(Arc::new(note_name))
            .with_string_to_value(Arc::new(parse_note_name)),
            pitch_bend_range: IntParam::new("Bend Range", 2, IntRange::Linear { min: 0, max: 24 })
                .with_unit(" st"),
            attack: time_param("Attack", 10.0, 2000.0),
            release: time_param("Release", 120.0, 5000.0),
            velocity_sensitivity: percent_param("Velocity", 0.5),
            stereo_spread: percent_param("Spread", 0.5),
            expression: percent_param("Expression", 1.0),
        }
    }
}

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Live's convention: MIDI note 60 is C3.
pub fn note_name(note: i32) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[note.rem_euclid(12) as usize],
        note.div_euclid(12) - 2
    )
}

fn parse_note_name(text: &str) -> Option<i32> {
    let text = text.trim();
    if let Ok(number) = text.parse::<i32>() {
        return Some(number);
    }
    let mut chars = text.chars();
    let mut pitch = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let octave = if let Some(rest) = rest.strip_prefix('#') {
        pitch += 1;
        rest
    } else if let Some(rest) = rest.strip_prefix('b') {
        pitch -= 1;
        rest
    } else {
        rest
    };
    let note = (octave.parse::<i32>().ok()? + 2) * 12 + pitch;
    (0..=127).contains(&note).then_some(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_use_live_octaves_and_round_trip() {
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(0), "C-2");
        for note in 0..=127 {
            assert_eq!(parse_note_name(&note_name(note)), Some(note));
        }
        assert_eq!(parse_note_name("Bb2"), Some(58));
        assert_eq!(parse_note_name("64"), Some(64));
    }
}
