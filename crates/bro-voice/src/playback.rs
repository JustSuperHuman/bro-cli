//! Speech output: GPT-Live's 24 kHz PCM16 played on the default output
//! device. The server streams in real time, so a small queue in front of the
//! cpal callback is enough; [`Player::clear`] drops what is queued (the user
//! pressed the key to talk over it).

use anyhow::{Context, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Rate GPT-Live speaks at.
pub const SOURCE_RATE: u32 = 24_000;
/// Queued audio beyond this (seconds) is dropped from the front: a stalled
/// device must not make speech lag further and further behind.
const MAX_QUEUE_SECS: usize = 20;

type Queue = Arc<Mutex<VecDeque<f32>>>;

/// Converts 24 kHz mono PCM16 to the device rate (linear interpolation).
struct Upsampler {
    step: f64,
    pos: f64,
    prev: Option<f32>,
}

impl Upsampler {
    fn new(to_rate: u32) -> Upsampler {
        Upsampler { step: SOURCE_RATE as f64 / to_rate.max(1) as f64, pos: 0.0, prev: None }
    }

    fn push(&mut self, input: impl IntoIterator<Item = f32>, out: &mut VecDeque<f32>) {
        for x in input {
            let Some(prev) = self.prev else {
                self.prev = Some(x);
                continue;
            };
            while self.pos < 1.0 {
                out.push_back(prev + (x - prev) * self.pos as f32);
                self.pos += self.step;
            }
            self.pos -= 1.0;
            self.prev = Some(x);
        }
    }
}

/// An open output stream. Dropping it stops playback.
pub struct Player {
    _stream: cpal::Stream,
    queue: Queue,
    upsampler: Mutex<Upsampler>,
    rate: u32,
}

/// 24 kHz little-endian PCM16 bytes -> f32 samples.
pub fn pcm16_to_f32(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32)
}

impl Player {
    pub fn open(host: &cpal::Host) -> anyhow::Result<Player> {
        let device = host.default_output_device().context("no speakers found (no default output device)")?;
        let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "speakers".into());
        let supported = device.default_output_config().with_context(|| format!("{name}: no usable output format"))?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let config = supported.config();
        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, channels, queue.clone()),
            SampleFormat::I16 => build::<i16>(&device, config, channels, queue.clone()),
            SampleFormat::I32 => build::<i32>(&device, config, channels, queue.clone()),
            SampleFormat::U16 => build::<u16>(&device, config, channels, queue.clone()),
            SampleFormat::F64 => build::<f64>(&device, config, channels, queue.clone()),
            other => return Err(anyhow!("{name}: unsupported output format {other}")),
        }
        .with_context(|| format!("{name}: could not open the output stream"))?;
        stream.play().with_context(|| format!("{name}: could not start playback"))?;
        Ok(Player { _stream: stream, queue, upsampler: Mutex::new(Upsampler::new(rate)), rate })
    }

    /// Queue 24 kHz PCM16 bytes.
    pub fn push_pcm16(&self, bytes: &[u8]) {
        let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        self.upsampler.lock().unwrap_or_else(|p| p.into_inner()).push(pcm16_to_f32(bytes), &mut queue);
        let cap = MAX_QUEUE_SECS * self.rate as usize;
        if queue.len() > cap {
            let excess = queue.len() - cap;
            queue.drain(..excess);
        }
    }

    /// Drop everything not yet played.
    pub fn clear(&self) {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// Seconds of speech still queued.
    pub fn pending_secs(&self) -> f32 {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).len() as f32 / self.rate.max(1) as f32
    }
}

fn build<T>(device: &cpal::Device, config: StreamConfig, channels: usize, queue: Queue) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = channels.max(1);
    device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], _| {
            let mut queue = queue.lock().unwrap_or_else(|p| p.into_inner());
            for frame in data.chunks_mut(channels) {
                let sample = T::from_sample_(queue.pop_front().unwrap_or(0.0));
                frame.fill(sample);
            }
        },
        |_err| {},
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_decodes_little_endian() {
        let bytes = [0xFF, 0x7F, 0x01, 0x80, 0, 0];
        let v: Vec<f32> = pcm16_to_f32(&bytes).collect();
        assert!((v[0] - 1.0).abs() < 1e-6);
        assert!((v[1] + 1.0).abs() < 1e-4);
        assert_eq!(v[2], 0.0);
    }

    #[test]
    fn upsampler_hits_the_device_rate() {
        for rate in [48_000u32, 44_100, 24_000, 16_000] {
            let mut up = Upsampler::new(rate);
            let mut out = VecDeque::new();
            let second: Vec<f32> = (0..SOURCE_RATE).map(|i| i as f32 / SOURCE_RATE as f32).collect();
            for chunk in second.chunks(480) {
                up.push(chunk.iter().copied(), &mut out);
            }
            let n = out.len() as i64;
            assert!((n - rate as i64).abs() <= 3, "{rate}: {n}");
        }
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;

    /// The default output device drains queued audio in real time (pushes
    /// silence, so nothing is heard).
    #[test]
    #[ignore = "needs an audio output device"]
    fn default_device_plays_the_queue() {
        let player = Player::open(&cpal::default_host()).expect("output device");
        player.push_pcm16(&vec![0u8; SOURCE_RATE as usize * 2]);
        let queued = player.pending_secs();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let left = player.pending_secs();
        eprintln!("device {} Hz: queued {queued:.2}s, {left:.2}s left after 0.6s", player.rate);
        assert!(queued > 0.9 && left < queued - 0.3, "queued {queued}, left {left}");
        player.clear();
        assert_eq!(player.pending_secs(), 0.0);
    }
}
