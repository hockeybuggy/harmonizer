mod support;

use support::*;

#[test]
fn measurement_helpers_find_known_pitches() {
    assert!(cents(dominant_frequency(&sine(441.0, 8192)), 441.0).abs() < 2.0);
    assert!(
        cents(
            fundamental(&sawtooth(110.0, 8192), 60.0, 1000.0).unwrap(),
            110.0
        )
        .abs()
            < 5.0
    );
    assert!(
        cents(
            fundamental(&vowel(196.0, &VOWEL_AH, 8192), 60.0, 1000.0).unwrap(),
            196.0
        )
        .abs()
            < 5.0
    );
    let low = spectral_centroid(&vowel(150.0, &VOWEL_AH, 16384), 300.0, 1600.0);
    let high = spectral_centroid(
        &vowel(150.0, &[1200.0, 1700.0, 2900.0], 16384),
        300.0,
        1600.0,
    );
    assert!(high > low + 150.0, "centroid {low} vs {high}");
}
