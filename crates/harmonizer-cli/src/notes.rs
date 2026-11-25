use anyhow::{Result, bail};

pub fn parse_note(input: &str) -> Result<u8> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("note cannot be empty");
    }

    if trimmed
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b'-' || b == b'+')
    {
        let value: i32 = trimmed
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid MIDI note number {trimmed:?}"))?;
        if !(0..=127).contains(&value) {
            bail!("MIDI note {value} out of range (expected 0..=127)");
        }
        return Ok(value as u8);
    }

    let mut chars = trimmed.chars();
    let letter = chars.next().expect("non-empty");
    let pitch_class: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => bail!("invalid note {trimmed:?}: expected A-G note name or MIDI number 0..=127"),
    };

    let rest = chars.as_str();
    let (accidental, octave_str) = if let Some(s) = rest.strip_prefix('#') {
        (1, s)
    } else if let Some(s) = rest.strip_prefix('b') {
        (-1, s)
    } else {
        (0, rest)
    };

    let digits = octave_str.strip_prefix('-').unwrap_or(octave_str);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        bail!("invalid octave in note {trimmed:?} (expected e.g. C3 = 60, C-2 = 0)");
    }
    let octave: i32 = octave_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid octave in note {trimmed:?}"))?;

    // Ableton convention: C3 = 60, C-2 = 0.
    let midi = (octave + 2) * 12 + pitch_class + accidental;
    if !(0..=127).contains(&midi) {
        bail!("note {trimmed:?} resolves to MIDI {midi}, outside 0..=127");
    }
    Ok(midi as u8)
}

pub fn parse_notes_list(input: &str) -> Result<Vec<u8>> {
    if input.trim().is_empty() {
        bail!("--notes list cannot be empty");
    }
    input.split(',').map(parse_note).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ableton_note_names() {
        assert_eq!(parse_note("C-2").unwrap(), 0);
        assert_eq!(parse_note("C3").unwrap(), 60);
        assert_eq!(parse_note("E3").unwrap(), 64);
        assert_eq!(parse_note("G3").unwrap(), 67);
        assert_eq!(parse_note("A3").unwrap(), 69);
        assert_eq!(parse_note("F#2").unwrap(), 54);
        assert_eq!(parse_note("Bb3").unwrap(), 70);
        assert_eq!(parse_note("G8").unwrap(), 127);
    }

    #[test]
    fn parses_case_insensitive_and_enharmonic_names() {
        assert_eq!(parse_note("c3").unwrap(), 60);
        assert_eq!(parse_note("f#2").unwrap(), 54);
        assert_eq!(parse_note("bb3").unwrap(), 70);
        assert_eq!(parse_note("Cb3").unwrap(), 59);
        assert_eq!(parse_note("B#2").unwrap(), 60);
        assert_eq!(parse_note("c-2").unwrap(), 0);
    }

    #[test]
    fn parses_numeric_midi_notes() {
        assert_eq!(parse_note("0").unwrap(), 0);
        assert_eq!(parse_note("60").unwrap(), 60);
        assert_eq!(parse_note("127").unwrap(), 127);
    }

    #[test]
    fn rejects_invalid_or_out_of_range_notes() {
        for bad in ["", "128", "-1", "H3", "C", "G#8", "Cb-2", "C##3", "C3x"] {
            assert!(parse_note(bad).is_err(), "expected {bad:?} to fail");
        }
    }

    #[test]
    fn parses_comma_separated_note_lists() {
        assert_eq!(parse_notes_list("C3,E3,G3").unwrap(), vec![60, 64, 67]);
        assert_eq!(parse_notes_list("60, 64, 67").unwrap(), vec![60, 64, 67]);
        assert_eq!(parse_notes_list("C3,64,G3").unwrap(), vec![60, 64, 67]);
        assert!(parse_notes_list("").is_err());
        assert!(parse_notes_list("C3,").is_err());
    }
}
