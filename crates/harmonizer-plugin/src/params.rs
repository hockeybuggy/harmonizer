use nice_plug::prelude::*;

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ModeParam {
    #[name = "Absolute (retune)"]
    Absolute,
    #[name = "Relative (transpose)"]
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
    #[id = "refnote"]
    pub reference_note: IntParam,
    #[id = "bendrange"]
    pub pitch_bend_range: IntParam,
    #[id = "attack"]
    pub attack: FloatParam,
    #[id = "release"]
    pub release: FloatParam,
    #[id = "velsens"]
    pub velocity_sensitivity: FloatParam,
    #[id = "spread"]
    pub stereo_spread: FloatParam,
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
            .with_value_to_string(formatters::v2s_i32_key_formatter())
            .with_string_to_value(formatters::s2v_i32_key_formatter()),
            pitch_bend_range: IntParam::new("Bend Range", 2, IntRange::Linear { min: 0, max: 24 })
                .with_unit(" st"),
            attack: time_param("Attack", 10.0, 2000.0),
            release: time_param("Release", 120.0, 5000.0),
            velocity_sensitivity: percent_param("Velocity", 0.5),
            stereo_spread: percent_param("Spread", 0.5),
        }
    }
}
