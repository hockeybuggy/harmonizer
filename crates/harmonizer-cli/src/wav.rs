use std::path::Path;

use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn channel_count(&self) -> u16 {
        self.channels.len() as u16
    }
}

pub fn read_wav(path: &Path) -> Result<AudioBuffer> {
    let mut reader = WavReader::open(path)
        .with_context(|| format!("failed to open WAV file {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.channels > 2 {
        bail!(
            "unsupported channel count {} in {}: expected mono (1) or stereo (2)",
            spec.channels,
            path.display()
        );
    }
    if spec.sample_rate == 0 {
        bail!("WAV file {} has zero sample rate", path.display());
    }

    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("failed to decode 32-bit float WAV {}", path.display()))?,
        (SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| f32::from(v) / 32_768.0))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("failed to decode 16-bit int WAV {}", path.display()))?,
        (SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 8_388_608.0))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("failed to decode 24-bit int WAV {}", path.display()))?,
        (SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| (f64::from(v) / 2_147_483_648.0) as f32))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("failed to decode 32-bit int WAV {}", path.display()))?,
        (format, bits) => {
            bail!(
                "unsupported WAV format {format:?} {bits}-bit in {}: expected 16/24/32-bit int or 32-bit float",
                path.display()
            );
        }
    };

    let ch_count = usize::from(spec.channels);
    let frames = interleaved.len() / ch_count;
    let mut channels: Vec<Vec<f32>> = (0..ch_count).map(|_| Vec::with_capacity(frames)).collect();
    for frame in interleaved.chunks_exact(ch_count) {
        for (ch, &sample) in frame.iter().enumerate() {
            channels[ch].push(sample);
        }
    }

    Ok(AudioBuffer {
        sample_rate: spec.sample_rate,
        channels,
    })
}

pub fn write_wav(path: &Path, buffer: &AudioBuffer) -> Result<()> {
    let ch_count = buffer.channels.len();
    if ch_count == 0 || ch_count > 2 {
        bail!("cannot write WAV with {ch_count} channels");
    }
    let frames = buffer.frames();
    if buffer.channels.iter().any(|ch| ch.len() != frames) {
        bail!("channel buffers have mismatched lengths");
    }

    let spec = WavSpec {
        channels: ch_count as u16,
        sample_rate: buffer.sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec)
        .with_context(|| format!("failed to create WAV file {}", path.display()))?;
    for i in 0..frames {
        for channel in &buffer.channels {
            writer
                .write_sample(channel[i])
                .with_context(|| format!("failed to write WAV sample to {}", path.display()))?;
        }
    }
    writer
        .finalize()
        .with_context(|| format!("failed to finalise WAV file {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use hound::{SampleFormat, WavSpec, WavWriter};

    use super::*;

    fn temp_wav_path(name: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "harmonizer-cli-wav-{}-{}-{name}.wav",
            std::process::id(),
            id
        ))
    }

    #[test]
    fn reads_16_24_32_int_and_32_float_mono_and_stereo() {
        let p16 = temp_wav_path("16bit");
        {
            let mut w = WavWriter::create(
                &p16,
                WavSpec {
                    channels: 2,
                    sample_rate: 48_000,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                },
            )
            .unwrap();
            w.write_sample(16384_i16).unwrap();
            w.write_sample(-16384_i16).unwrap();
            w.finalize().unwrap();
        }
        let buf = read_wav(&p16).unwrap();
        std::fs::remove_file(&p16).ok();
        assert_eq!(buf.sample_rate, 48_000);
        assert_eq!(buf.channels.len(), 2);
        assert!((buf.channels[0][0] - 0.5).abs() < 1e-4);
        assert!((buf.channels[1][0] + 0.5).abs() < 1e-4);

        let p24 = temp_wav_path("24bit");
        {
            let mut w = WavWriter::create(
                &p24,
                WavSpec {
                    channels: 1,
                    sample_rate: 44_100,
                    bits_per_sample: 24,
                    sample_format: SampleFormat::Int,
                },
            )
            .unwrap();
            w.write_sample(4_194_304_i32).unwrap();
            w.write_sample(-4_194_304_i32).unwrap();
            w.finalize().unwrap();
        }
        let buf = read_wav(&p24).unwrap();
        std::fs::remove_file(&p24).ok();
        assert!((buf.channels[0][0] - 0.5).abs() < 1e-6);
        assert!((buf.channels[0][1] + 0.5).abs() < 1e-6);

        let p32i = temp_wav_path("32int");
        {
            let mut w = WavWriter::create(
                &p32i,
                WavSpec {
                    channels: 1,
                    sample_rate: 48_000,
                    bits_per_sample: 32,
                    sample_format: SampleFormat::Int,
                },
            )
            .unwrap();
            w.write_sample(1_073_741_824_i32).unwrap();
            w.finalize().unwrap();
        }
        let buf = read_wav(&p32i).unwrap();
        std::fs::remove_file(&p32i).ok();
        assert!((buf.channels[0][0] - 0.5).abs() < 1e-6);

        let p32f = temp_wav_path("32float");
        let original = AudioBuffer {
            sample_rate: 48_000,
            channels: vec![vec![0.25, -0.75], vec![-0.125, 0.5]],
        };
        write_wav(&p32f, &original).unwrap();
        let roundtrip = read_wav(&p32f).unwrap();
        std::fs::remove_file(&p32f).ok();
        assert_eq!(roundtrip, original);
    }

    #[test]
    fn rejects_more_than_two_channels() {
        let p3 = temp_wav_path("3ch");
        {
            let mut w = WavWriter::create(
                &p3,
                WavSpec {
                    channels: 3,
                    sample_rate: 48_000,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                },
            )
            .unwrap();
            w.write_sample(0_i16).unwrap();
            w.write_sample(0_i16).unwrap();
            w.write_sample(0_i16).unwrap();
            w.finalize().unwrap();
        }
        let err = read_wav(&p3).unwrap_err();
        std::fs::remove_file(&p3).ok();
        assert!(err.to_string().contains('3'));
    }
}
