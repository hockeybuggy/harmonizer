pub mod midi;
pub mod notes;
pub mod render;
pub mod wav;

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{ArgGroup, Parser, ValueEnum};
use harmonizer_dsp::{Mode, Params, Quality};

use crate::render::{NoteSource, RenderOptions, RenderSummary, render_audio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ModeArg {
    Absolute,
    Relative,
}

impl From<ModeArg> for Mode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Absolute => Mode::Absolute,
            ModeArg::Relative => Mode::Relative,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum QualityArg {
    Low,
    Balanced,
    High,
}

impl From<QualityArg> for Quality {
    fn from(value: QualityArg) -> Self {
        match value {
            QualityArg::Low => Quality::LowLatency,
            QualityArg::Balanced => Quality::Balanced,
            QualityArg::High => Quality::HighQuality,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(
    name = "harmonizer",
    about = "Offline MIDI-driven polyphonic harmonizer",
    group(ArgGroup::new("note_source").required(true).args(["midi", "notes"]))
)]
pub struct Cli {
    /// Input WAV file (16/24/32-bit int or 32-bit float, mono or stereo)
    #[arg(long)]
    pub input: PathBuf,

    /// Output WAV file (32-bit float, matching input sample rate and channel count)
    #[arg(long)]
    pub output: PathBuf,

    /// Standard MIDI file (format 0 or 1) driving the harmonizer voices
    #[arg(long, group = "note_source")]
    pub midi: Option<PathBuf>,

    /// Comma-separated MIDI notes held for the whole file (e.g. C3,E3,G3 or 60,64,67).
    /// Note names use Ableton's convention where C3 = 60 and C-2 = 0 (sharps '#', flats 'b').
    #[arg(long, group = "note_source")]
    pub notes: Option<String>,

    /// Pitch-shifting mode
    #[arg(long, value_enum, default_value_t = ModeArg::Absolute)]
    pub mode: ModeArg,

    /// Engine latency/quality trade-off
    #[arg(long, value_enum, default_value_t = QualityArg::Balanced)]
    pub quality: QualityArg,

    /// Wet (harmonized voices) level in dB (use -inf for silence)
    #[arg(long, default_value = "0", allow_hyphen_values = true, value_parser = parse_db)]
    pub wet_db: f32,

    /// Dry (unprocessed input) level in dB (use -inf for silence)
    #[arg(long, default_value = "-inf", allow_hyphen_values = true, value_parser = parse_db)]
    pub dry_db: f32,

    /// Formant shift in semitones (0 preserves input formants)
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub formant: f32,

    /// Reference MIDI note for relative mode (number or Ableton note name where C3 = 60)
    #[arg(long, default_value = "60", value_parser = parse_reference_note)]
    pub reference_note: f32,

    /// Pitch movement retained in absolute mode (percent, 0–100)
    #[arg(long, default_value_t = 100.0)]
    pub expression: f32,

    /// Voice envelope attack time in milliseconds
    #[arg(long, default_value_t = 10.0)]
    pub attack_ms: f32,

    /// Voice envelope release time in milliseconds
    #[arg(long, default_value_t = 120.0)]
    pub release_ms: f32,

    /// Velocity sensitivity (0.0 ignores velocity, 1.0 scales fully)
    #[arg(long, default_value_t = 0.5)]
    pub velocity_sensitivity: f32,

    /// Stereo spread across voices (0.0 centred, 1.0 full spread)
    #[arg(long, default_value_t = 0.5)]
    pub spread: f32,

    /// Pitch bend range in semitones
    #[arg(long, default_value_t = 2.0)]
    pub bend_range: f32,

    /// Velocity for notes passed via --notes (0.0..=1.0)
    #[arg(long, default_value_t = 1.0)]
    pub velocity: f32,

    /// Host processing block size in samples
    #[arg(long, default_value_t = 256)]
    pub block_size: usize,

    /// Keep the raw engine output without dropping the leading latency samples
    #[arg(long)]
    pub no_latency_compensation: bool,
}

pub fn parse_db(raw: &str) -> Result<f32, String> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("-inf") || trimmed.eq_ignore_ascii_case("-infinity") {
        return Ok(f32::NEG_INFINITY);
    }
    let db: f32 = trimmed
        .parse()
        .map_err(|_| format!("invalid dB value {raw:?} (expected a number or -inf)"))?;
    if db.is_nan() || db == f32::INFINITY {
        return Err(format!(
            "invalid dB value {raw:?} (expected a finite number or -inf)"
        ));
    }
    Ok(db)
}

pub fn db_to_gain(db: f32) -> f32 {
    if db == f32::NEG_INFINITY {
        0.0
    } else {
        10.0_f32.powf(db / 20.0)
    }
}

pub fn parse_reference_note(raw: &str) -> Result<f32, String> {
    let trimmed = raw.trim();
    if let Ok(value) = trimmed.parse::<f32>() {
        if !value.is_finite() || !(0.0..=127.0).contains(&value) {
            return Err(format!(
                "reference note {value} out of range (expected 0..=127)"
            ));
        }
        return Ok(value);
    }
    notes::parse_note(trimmed)
        .map(f32::from)
        .map_err(|err| err.to_string())
}

impl Cli {
    pub fn to_params(&self) -> Result<Params> {
        if !self.formant.is_finite() {
            bail!("--formant must be finite");
        }
        if !(0.0..=100.0).contains(&self.expression) {
            bail!("--expression must be in 0..=100");
        }
        if !self.attack_ms.is_finite() || self.attack_ms < 0.0 {
            bail!("--attack-ms must be non-negative");
        }
        if !self.release_ms.is_finite() || self.release_ms < 0.0 {
            bail!("--release-ms must be non-negative");
        }
        if !self.velocity_sensitivity.is_finite()
            || !(0.0..=1.0).contains(&self.velocity_sensitivity)
        {
            bail!("--velocity-sensitivity must be in 0.0..=1.0");
        }
        if !self.spread.is_finite() || !(0.0..=1.0).contains(&self.spread) {
            bail!("--spread must be in 0.0..=1.0");
        }
        if !self.bend_range.is_finite() || self.bend_range < 0.0 {
            bail!("--bend-range must be non-negative");
        }

        Ok(Params {
            mode: self.mode.into(),
            quality: self.quality.into(),
            wet_gain: db_to_gain(self.wet_db),
            dry_gain: db_to_gain(self.dry_db),
            formant_semitones: self.formant,
            reference_note: self.reference_note,
            expression: self.expression / 100.0,
            pitch_bend_range: self.bend_range,
            attack_ms: self.attack_ms,
            release_ms: self.release_ms,
            velocity_sensitivity: self.velocity_sensitivity,
            stereo_spread: self.spread,
        })
    }
}

pub fn run(cli: &Cli) -> Result<RenderSummary> {
    if cli.block_size == 0 {
        bail!("--block-size must be at least 1");
    }
    if !cli.velocity.is_finite() || !(0.0..=1.0).contains(&cli.velocity) {
        bail!("--velocity must be in 0.0..=1.0");
    }
    let params = cli.to_params()?;
    let input = wav::read_wav(&cli.input)?;

    let source = match (&cli.midi, &cli.notes) {
        (Some(midi_path), None) => {
            NoteSource::Events(midi::load_midi_file(midi_path, input.sample_rate)?)
        }
        (None, Some(notes_str)) => NoteSource::Held {
            notes: notes::parse_notes_list(notes_str)?,
            velocity: cli.velocity,
        },
        _ => bail!("specify exactly one of --midi or --notes"),
    };

    let options = RenderOptions {
        params,
        block_size: cli.block_size,
        compensate_latency: !cli.no_latency_compensation,
    };

    let (rendered, summary) = render_audio(&input, &source, &options);
    wav::write_wav(&cli.output, &rendered)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_defaults_match_dsp_params_default() {
        let cli = Cli::try_parse_from([
            "harmonizer",
            "--input",
            "in.wav",
            "--output",
            "out.wav",
            "--notes",
            "C3",
        ])
        .unwrap();
        assert_eq!(cli.to_params().unwrap(), Params::default());
        assert_eq!(cli.velocity, 1.0);
        assert_eq!(cli.block_size, 256);
        assert!(!cli.no_latency_compensation);
    }

    #[test]
    fn cli_requires_either_midi_or_notes_but_not_both() {
        assert!(
            Cli::try_parse_from(["harmonizer", "--input", "in.wav", "--output", "out.wav"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "harmonizer",
                "--input",
                "in.wav",
                "--output",
                "out.wav",
                "--notes",
                "C3",
                "--midi",
                "a.mid",
            ])
            .is_err()
        );
    }

    #[test]
    fn cli_maps_expression_percentage_to_dsp_amount() {
        let parse = |amount| {
            Cli::try_parse_from([
                "harmonizer",
                "--input",
                "in.wav",
                "--output",
                "out.wav",
                "--notes",
                "C3",
                "--expression",
                amount,
            ])
            .unwrap()
            .to_params()
        };
        assert_eq!(parse("25").unwrap().expression, 0.25);
        assert!(parse("101").is_err());
    }

    #[test]
    fn cli_accepts_negative_db_and_formant_values() {
        let cli = Cli::try_parse_from([
            "harmonizer",
            "--input",
            "in.wav",
            "--output",
            "out.wav",
            "--notes",
            "C3",
            "--wet-db",
            "-inf",
            "--dry-db",
            "-6",
            "--formant",
            "-2.5",
        ])
        .unwrap();
        let params = cli.to_params().unwrap();
        assert_eq!(params.wet_gain, 0.0);
        assert!((params.dry_gain - 10_f32.powf(-6.0 / 20.0)).abs() < 1e-6);
        assert_eq!(params.formant_semitones, -2.5);
    }
}
