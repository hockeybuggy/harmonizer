use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use harmonizer_dsp::{Harmonizer, Mode, Params, Quality};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

#[test]
fn processing_never_allocates() {
    let sample_rate = 48_000.0;
    let mut harmonizer = Harmonizer::new(sample_rate, 512);
    let mut left: Vec<f32> = (0..4096)
        .map(|n| (n as f32 * 220.0 * std::f32::consts::TAU / sample_rate).sin())
        .collect();
    let mut right = left.clone();

    assert_no_alloc(|| {
        for mode in [Mode::Absolute, Mode::Relative] {
            for quality in Quality::ALL {
                harmonizer.set_params(Params {
                    mode,
                    quality,
                    dry_gain: 0.5,
                    ..Params::default()
                });
                harmonizer.set_sustain(true);
                harmonizer.note_on(60, 1.0);
                harmonizer.note_on(64, 0.8);
                harmonizer.note_on(67, 0.6);
                for chunk in [7, 64, 512, 1000] {
                    let (l, r) = (&mut left[..chunk], &mut right[..chunk]);
                    harmonizer.process(&mut [l, r]);
                }
                harmonizer.note_off(60);
                harmonizer.process(&mut [&mut left[..256], &mut right[..256]]);
                harmonizer.note_on(60, 0.9);
                harmonizer.set_sustain(false);
                harmonizer.release_all();
                harmonizer.process(&mut [&mut left[..], &mut right[..]]);
                harmonizer.reset();
            }
        }
    });
}
