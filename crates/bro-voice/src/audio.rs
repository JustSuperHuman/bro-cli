//! Audio: downmix, 16 kHz resampling, levels, in-memory WAV, and the cpal
//! capture stream. The cpal callback only downmixes and forwards chunks; the
//! worker resamples and meters them.

/// Sample rate sent to the transcription API.
pub const TARGET_RATE: u32 = 16_000;

/// Interleaved frames -> mono by averaging channels.
pub fn downmix<T: Copy>(data: &[T], channels: usize, to_f32: impl Fn(T) -> f32) -> Vec<f32> {
    let channels = channels.max(1);
    if channels == 1 {
        return data.iter().map(|&s| to_f32(s)).collect();
    }
    data.chunks_exact(channels)
        .map(|frame| frame.iter().map(|&s| to_f32(s)).sum::<f32>() / channels as f32)
        .collect()
}

/// Streaming linear-interpolation resampler, mono f32 -> i16 (`TARGET_RATE` unless told otherwise).
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Input samples advanced per output sample.
    step: f64,
    /// Position of the next output sample, relative to `prev` (0 = `prev`).
    pos: f64,
    prev: Option<f32>,
}

impl Resampler {
    pub fn new(from_rate: u32) -> Resampler {
        Resampler::with_rates(from_rate, TARGET_RATE)
    }

    /// Any rate to any rate (GPT-Live takes 24 kHz).
    pub fn with_rates(from_rate: u32, to_rate: u32) -> Resampler {
        Resampler { step: from_rate.max(1) as f64 / to_rate.max(1) as f64, pos: 0.0, prev: None }
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<i16>) {
        for &x in input {
            let Some(prev) = self.prev else {
                self.prev = Some(x);
                continue;
            };
            // Emit every output position that falls in [prev, x).
            while self.pos < 1.0 {
                let y = prev + (x - prev) * self.pos as f32;
                out.push(to_i16(y));
                self.pos += self.step;
            }
            self.pos -= 1.0;
            self.prev = Some(x);
        }
    }
}

pub fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

/// RMS of i16 samples, 0..1.
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64 / i16::MAX as f64).powi(2)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// RMS mapped onto a meter: -60 dBFS..0 dBFS -> 0..1.
pub fn meter(rms: f32) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    ((20.0 * rms.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// A 16-bit PCM mono WAV file.
pub fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub use capture::{Capture, Sink, open};

mod capture {
    use anyhow::{Context, anyhow};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig};

    /// Receives downmixed mono chunks (or a fatal error) from the audio thread.
    pub type Sink = std::sync::Arc<dyn Fn(Result<Vec<f32>, String>) + Send + Sync + 'static>;

    /// An open, playing input stream. Dropping it stops capture.
    pub struct Capture {
        _stream: cpal::Stream,
        pub rate: u32,
    }

    /// Open the default input device with its default config and start it.
    pub fn open(host: &cpal::Host, sink: Sink) -> anyhow::Result<Capture> {
        let device = host.default_input_device().context("no microphone found (no default input device)")?;
        let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "microphone".into());
        let supported = device.default_input_config().with_context(|| format!("{name}: no usable input format"))?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, channels, sink),
            SampleFormat::I16 => build::<i16>(&device, config, channels, sink),
            SampleFormat::I32 => build::<i32>(&device, config, channels, sink),
            SampleFormat::U16 => build::<u16>(&device, config, channels, sink),
            SampleFormat::U8 => build::<u8>(&device, config, channels, sink),
            SampleFormat::I8 => build::<i8>(&device, config, channels, sink),
            SampleFormat::F64 => build::<f64>(&device, config, channels, sink),
            other => return Err(anyhow!("{name}: unsupported sample format {other}")),
        }
        .with_context(|| format!("{name}: could not open the input stream"))?;
        stream.play().with_context(|| format!("{name}: could not start recording"))?;
        Ok(Capture { _stream: stream, rate })
    }

    fn build<T>(device: &cpal::Device, config: StreamConfig, channels: usize, sink: Sink) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample + Send + 'static,
        f32: FromSample<T>,
    {
        let on_err = sink.clone();
        device.build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| sink(Ok(super::downmix(data, channels, f32::from_sample_))),
            move |err: cpal::Error| {
                // Glitches and reroutes are survivable; anything else ends the take.
                if !matches!(err.kind(), ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied) {
                    on_err(Err(err.to_string()));
                }
            },
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        let stereo = [1.0f32, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(downmix(&stereo, 2, |s| s), vec![0.5, 0.5, 0.0]);
        assert_eq!(downmix(&[0.25f32, 0.75], 1, |s| s), vec![0.25, 0.75]);
        // A trailing partial frame is dropped.
        assert_eq!(downmix(&[1.0f32, 1.0, 1.0], 2, |s| s), vec![1.0]);
        let ints = [i16::MAX, i16::MAX];
        let m = downmix(&ints, 2, |s| s as f32 / i16::MAX as f32);
        assert!((m[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn resampler_rates_and_continuity() {
        for rate in [48_000u32, 44_100, 16_000, 8_000] {
            let mut r = Resampler::new(rate);
            let mut out = Vec::new();
            // Feed one second in odd-sized chunks: output is ~16000 samples.
            let input: Vec<f32> = (0..rate).map(|i| (i as f32 / rate as f32) * 0.5).collect();
            for chunk in input.chunks(441) {
                r.push(chunk, &mut out);
            }
            let n = out.len() as i64;
            assert!((n - 16_000).abs() <= 2, "{rate}: got {n}");
            // A ramp stays monotonic across chunk boundaries.
            assert!(out.windows(2).all(|w| w[1] >= w[0]), "{rate}: not monotonic");
        }
    }

    #[test]
    fn resampler_interpolates() {
        // 32k -> 16k takes every other sample.
        let mut r = Resampler::new(32_000);
        let mut out = Vec::new();
        r.push(&[0.0, 0.1, 0.2, 0.3, 0.4, 0.5], &mut out);
        let expect: Vec<i16> = [0.0f32, 0.2, 0.4].iter().map(|&x| to_i16(x)).collect();
        assert_eq!(out, expect);
        // 8k -> 16k inserts midpoints.
        let mut r = Resampler::new(8_000);
        let mut out = Vec::new();
        r.push(&[0.0, 0.5, 1.0], &mut out);
        let expect: Vec<i16> = [0.0f32, 0.25, 0.5, 0.75].iter().map(|&x| to_i16(x)).collect();
        assert_eq!(out, expect);
    }

    #[test]
    fn levels() {
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[i16::MAX, -i16::MAX]) - 1.0).abs() < 1e-6);
        assert_eq!(meter(0.0), 0.0);
        assert!((meter(1.0) - 1.0).abs() < 1e-6);
        assert!((meter(0.001) - 0.0).abs() < 1e-6); // -60 dBFS
        assert!((meter(0.0316) - 0.5).abs() < 0.01); // -30 dBFS
    }

    #[test]
    fn wav_header() {
        let w = wav(&[1, -2, 3], 16_000);
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(w[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes(w[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(w[34..36].try_into().unwrap()), 16);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(&w[44..], &[1, 0, 0xFE, 0xFF, 3, 0]);
    }
}
