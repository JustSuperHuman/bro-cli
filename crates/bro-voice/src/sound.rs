//! Short sounds (PeonPing's voice lines and their previews): a whole file decoded in pure Rust (WAV, MP3, OGG
//! Vorbis, FLAC, AAC / M4A), fitted to the default output device's rate and channels, and played once at a
//! volume. The same on every OS: no ffmpeg, afplay or PowerShell player behind it.

use anyhow::{Context, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymErr;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Longer than any voice line: a mislabelled song stops here.
const MAX_SECS: u32 = 30;

/// Decoded audio: interleaved f32 frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Clip {
    pub fn duration(&self) -> Duration {
        let frames = self.samples.len() / self.channels.max(1);
        Duration::from_secs_f64(frames as f64 / self.rate.max(1) as f64)
    }

    /// The same sound at `rate` with `channels` (linear interpolation; mono spreads to every channel, more
    /// channels fold down to mono by averaging), times `volume`.
    pub fn fit(&self, rate: u32, channels: usize, volume: f32) -> Vec<f32> {
        let (src_ch, dst_ch) = (self.channels.max(1), channels.max(1));
        let frames = self.samples.len() / src_ch;
        if frames == 0 {
            return vec![];
        }
        let frame = |i: usize, c: usize| -> f32 {
            let i = i.min(frames - 1);
            if dst_ch == 1 && src_ch > 1 {
                (0..src_ch).map(|k| self.samples[i * src_ch + k]).sum::<f32>() / src_ch as f32
            } else {
                self.samples[i * src_ch + c.min(src_ch - 1)]
            }
        };
        let step = self.rate.max(1) as f64 / rate.max(1) as f64;
        let out_frames = ((frames as f64) / step).floor() as usize;
        let mut out = Vec::with_capacity(out_frames * dst_ch);
        for n in 0..out_frames {
            let pos = n as f64 * step;
            let (i, t) = (pos.floor() as usize, (pos.fract()) as f32);
            for c in 0..dst_ch {
                let (a, b) = (frame(i, c), frame(i + 1, c));
                out.push(((a + (b - a) * t) * volume).clamp(-1.0, 1.0));
            }
        }
        out
    }
}

/// Decode the whole file (at most [`MAX_SECS`]).
pub fn decode(path: &Path) -> anyhow::Result<Clip> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .with_context(|| format!("{}: not a sound bro can play", path.display()))?;
    let mut format = probed.format;
    let track = format.default_track().ok_or_else(|| anyhow!("{}: no audio in it", path.display()))?;
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).with_context(|| format!("{}: unsupported codec", path.display()))?;
    let (mut rate, mut channels) = (track.codec_params.sample_rate.unwrap_or(0), track.codec_params.channels.map_or(0, |c| c.count()));
    let mut samples: Vec<f32> = vec![];
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymErr::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymErr::ResetRequired) => break,
            Err(e) => return Err(anyhow!("{}: {e}", path.display())),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // a damaged frame: skip it, as players do
            Err(SymErr::DecodeError(_)) => continue,
            Err(e) => return Err(anyhow!("{}: {e}", path.display())),
        };
        let spec = *decoded.spec();
        rate = spec.rate;
        channels = spec.channels.count();
        let sb = match &mut buf {
            Some(b) if b.capacity() >= decoded.capacity() * channels => b,
            _ => buf.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
        };
        sb.copy_interleaved_ref(decoded);
        samples.extend_from_slice(sb.samples());
        if rate > 0 && samples.len() / channels.max(1) > (rate * MAX_SECS) as usize {
            break;
        }
    }
    if rate == 0 || channels == 0 || samples.is_empty() {
        bail!("{}: no sound in it", path.display());
    }
    Ok(Clip { rate, channels, samples })
}

type Queue = Arc<Mutex<(Vec<f32>, usize)>>;

/// Play `clip` once on the default output device at `volume` (0..1); blocks until it's played (or `stop` is
/// set).
pub fn play_clip(clip: &Clip, volume: f32, stop: Option<&AtomicBool>) -> anyhow::Result<()> {
    let host = cpal::default_host();
    let device = host.default_output_device().context("no speakers found (no default output device)")?;
    let supported = device.default_output_config().context("no usable output format")?;
    let rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let data = clip.fit(rate, channels, volume.clamp(0.0, 1.0));
    let len = data.len();
    let queue: Queue = Arc::new(Mutex::new((data, 0)));
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, queue.clone()),
        SampleFormat::I16 => build::<i16>(&device, config, queue.clone()),
        SampleFormat::I32 => build::<i32>(&device, config, queue.clone()),
        SampleFormat::U16 => build::<u16>(&device, config, queue.clone()),
        SampleFormat::F64 => build::<f64>(&device, config, queue.clone()),
        other => bail!("unsupported output format {other}"),
    }
    .context("could not open the output stream")?;
    stream.play().context("could not start playback")?;
    // the device's own buffer still holds the end once the queue is drained
    let deadline = Instant::now() + clip.duration() + Duration::from_secs(3);
    loop {
        std::thread::sleep(Duration::from_millis(20));
        let done = queue.lock().map(|q| q.1 >= len).unwrap_or(true);
        if done || stop.is_some_and(|s| s.load(Ordering::Relaxed)) || Instant::now() > deadline {
            break;
        }
    }
    if !stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
        std::thread::sleep(Duration::from_millis(150));
    }
    drop(stream);
    Ok(())
}

/// Decode and play `path` once at `volume`; blocks until it's played.
pub fn play_file(path: &Path, volume: f32) -> anyhow::Result<()> {
    play_clip(&decode(path)?, volume, None)
}

fn build<T>(device: &cpal::Device, config: StreamConfig, queue: Queue) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    device.build_output_stream::<T, _, _>(
        config,
        move |out: &mut [T], _| {
            let mut q = queue.lock().unwrap_or_else(|p| p.into_inner());
            let (data, pos) = &mut *q;
            for s in out.iter_mut() {
                *s = T::from_sample_(data.get(*pos).copied().unwrap_or(0.0));
                *pos = (*pos + 1).min(data.len());
            }
        },
        |_err| {},
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit PCM WAV of `frames` frames.
    fn wav(rate: u32, channels: u16, frames: usize) -> Vec<u8> {
        let data_len = frames * channels as usize * 2;
        let mut b = vec![];
        b.extend(b"RIFF");
        b.extend(((36 + data_len) as u32).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(channels.to_le_bytes());
        b.extend(rate.to_le_bytes());
        b.extend((rate * channels as u32 * 2).to_le_bytes());
        b.extend((channels * 2).to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend((data_len as u32).to_le_bytes());
        for i in 0..frames * channels as usize {
            b.extend(((i % 100) as i16 * 100).to_le_bytes());
        }
        b
    }

    #[test]
    fn decodes_wav_and_fits_the_device() {
        let dir = std::env::temp_dir().join(format!("bro-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("line.wav");
        std::fs::write(&f, wav(22_050, 1, 22_050)).unwrap();
        let clip = decode(&f).unwrap();
        assert_eq!((clip.rate, clip.channels, clip.samples.len()), (22_050, 1, 22_050));
        assert!((clip.duration().as_secs_f32() - 1.0).abs() < 0.01);
        let fitted = clip.fit(48_000, 2, 0.5);
        let frames = fitted.len() / 2;
        assert!((frames as i64 - 48_000).abs() <= 3, "{frames}");
        assert_eq!(fitted[0], fitted[1], "mono on both speakers");
        let peak = |v: &[f32]| v.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((peak(&fitted) - peak(&clip.samples) * 0.5).abs() < 0.01, "the volume scales it");
        assert!(decode(&dir.join("missing.wav")).is_err());
        std::fs::write(dir.join("junk.mp3"), b"not audio at all").unwrap();
        assert!(decode(&dir.join("junk.mp3")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stereo_folds_to_mono() {
        let clip = Clip { rate: 10, channels: 2, samples: vec![1.0, 0.0, 0.5, 0.5] };
        assert_eq!(clip.fit(10, 1, 1.0), vec![0.5, 0.5]);
        assert!(Clip { rate: 10, channels: 1, samples: vec![] }.fit(48_000, 2, 1.0).is_empty());
    }

    #[test]
    #[ignore = "needs an audio output device (plays half a second of quiet noise)"]
    fn plays_on_the_default_device() {
        let clip = Clip { rate: 24_000, channels: 1, samples: (0..12_000).map(|i| ((i % 50) as f32 / 50.0 - 0.5) * 0.02).collect() };
        let t = Instant::now();
        play_clip(&clip, 0.2, None).unwrap();
        assert!(t.elapsed() >= Duration::from_millis(450), "{:?}", t.elapsed());
    }
}
