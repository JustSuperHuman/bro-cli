//! Audio playback for [`crate::Player`]: a decoder thread keeps a short queue of samples in the
//! output device's format, and the cpal callback plays them and moves the player's clock, so
//! the audio is the master clock while it plays.

use crate::decode::{self, AudioChunk, Backend};
use crate::player::{Seek, Shared, lock};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

/// Seconds of audio decoded ahead of the device.
const AHEAD_SECS: f64 = 0.75;
/// A hole in the stream shorter than this is filled with silence (the clock runs through it);
/// a longer one is jumped.
const MAX_GAP_SECS: f64 = 5.0;

/// Samples waiting for the device, already in its rate and channel layout.
#[derive(Default)]
pub(crate) struct AudioQueue {
    /// The seek (counted by the player) the queued samples belong to.
    pub seq: u64,
    pub chunks: VecDeque<Chunk>,
    /// Frames queued and not yet played.
    pub frames: usize,
    /// The decoder reached the end since that seek.
    pub eos: bool,
    /// Finished and played out.
    pub drained: bool,
}

pub(crate) struct Chunk {
    /// Timeline seconds of the first frame.
    pub t: f64,
    pub data: Vec<f32>,
    /// Frames already played.
    pub pos: usize,
}

/// The default output device, playing whatever the queue holds (silence when empty or paused).
struct Output {
    _stream: cpal::Stream,
    rate: u32,
    channels: usize,
}

fn open_output(shared: &Arc<Shared>) -> Result<Output, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no audio output device")?;
    let supported = device.default_output_config().map_err(|e| format!("no usable audio output: {e}"))?;
    let rate = supported.sample_rate();
    let channels = (supported.channels() as usize).max(1);
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, shared.clone(), rate, channels),
        SampleFormat::I16 => build::<i16>(&device, config, shared.clone(), rate, channels),
        SampleFormat::I32 => build::<i32>(&device, config, shared.clone(), rate, channels),
        SampleFormat::U16 => build::<u16>(&device, config, shared.clone(), rate, channels),
        SampleFormat::F64 => build::<f64>(&device, config, shared.clone(), rate, channels),
        other => return Err(format!("unsupported audio output format {other}")),
    }
    .map_err(|e| format!("can't open the audio output: {e}"))?;
    stream.play().map_err(|e| format!("can't start the audio output: {e}"))?;
    Ok(Output { _stream: stream, rate, channels })
}

fn build<T>(device: &cpal::Device, config: StreamConfig, shared: Arc<Shared>, rate: u32, channels: usize) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let ts = info.timestamp();
            let latency = ts.playback.duration_since(ts.callback).min(Duration::from_millis(500));
            fill(data, latency, &shared, rate as f64, channels);
        },
        // A lost device stops the callbacks; the clock notices and runs on the wall clock.
        |_err| {},
        None,
    )
}

/// The device callback: play queued samples at the player's volume and anchor the clock to them.
fn fill<T: SizedSample + FromSample<f32>>(data: &mut [T], latency: Duration, shared: &Shared, rate: f64, channels: usize) {
    let now = Instant::now();
    let silence = T::from_sample_(0.0f32);
    if !shared.playing.load(Relaxed) {
        data.fill(silence);
        lock(&shared.clock).alive = Some(now);
        return;
    }
    let gain = if shared.muted.load(Relaxed) { 0.0 } else { f32::from_bits(shared.volume.load(Relaxed)) };
    let wanted = data.len() / channels;
    let mut written = 0;
    let mut first = None;
    let (seq, drained) = {
        let mut q = lock(&shared.audio);
        while written < wanted {
            let Some(chunk) = q.chunks.front_mut() else { break };
            let frames = chunk.data.len() / channels;
            first.get_or_insert(chunk.t + chunk.pos as f64 / rate);
            let n = (frames - chunk.pos).min(wanted - written);
            let src = &chunk.data[chunk.pos * channels..(chunk.pos + n) * channels];
            for (d, s) in data[written * channels..(written + n) * channels].iter_mut().zip(src) {
                *d = T::from_sample_(s * gain);
            }
            chunk.pos += n;
            written += n;
            if chunk.pos >= frames {
                q.chunks.pop_front();
            }
            q.frames = q.frames.saturating_sub(n);
        }
        if q.chunks.is_empty() && q.eos {
            q.drained = true;
        }
        (q.seq, q.drained)
    };
    data[written * channels..].fill(silence);
    if written > 0 {
        shared.audio_cv.notify_all();
    }
    let mut clock = lock(&shared.clock);
    clock.alive = Some(now);
    if clock.seq != seq {
        return;
    }
    if let Some(t0) = first {
        // The first sample of this buffer is heard `latency` from now.
        clock.base = t0;
        clock.at = now + latency;
        clock.limit = Some(t0 + written as f64 / rate);
        clock.fed = Some(now);
    } else if drained {
        // The audio is over (the video may go on): the wall clock takes over from here.
        clock.limit = None;
    }
}

/// The "bro-media-audio" thread: owns the output stream and the decoder, keeps the queue topped up.
pub(crate) fn run(path: PathBuf, backend: Backend, shared: Arc<Shared>, rx: Receiver<Seek>) {
    // Media Foundation wants the multithreaded apartment, and cpal would otherwise make this
    // thread single-threaded first (it accepts an MTA).
    #[cfg(windows)]
    let _com = crate::win::Com::init();
    let output = match open_output(&shared) {
        Ok(output) => output,
        // No speakers: the video plays silently on the wall clock.
        Err(_) => {
            lock(&shared.clock).audio = false;
            return;
        }
    };
    let mut decoder = match decode::open_audio(&path, output.rate, backend) {
        Ok(decoder) => decoder,
        Err(e) => {
            shared.set_error(e);
            lock(&shared.clock).audio = false;
            return;
        }
    };
    if let Some(slot) = decoder.proc_slot() {
        lock(&shared.procs).push(slot);
    }
    let (rate, channels) = (output.rate as f64, output.channels);
    let ahead = (AHEAD_SECS * rate) as usize;
    let mut seq = 0;
    let mut pass_base = 0.0;
    let mut pass = 0;
    let mut eos = false;
    let mut resampler: Option<Resampler> = None;
    // Timeline seconds the next samples should start at; holes and overlaps are evened out against it.
    let mut next = Some(0.0);
    loop {
        if shared.stop.load(Relaxed) {
            return;
        }
        let first = if eos {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(seek) => Some(seek),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.try_recv() {
                Ok(seek) => Some(seek),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };
        if let Some(seek) = rx.try_iter().last().or(first) {
            seq = seek.seq;
            pass_base = 0.0;
            pass = 0;
            resampler = None;
            next = Some(seek.to);
            eos = false;
            if let Err(e) = decoder.seek(seek.to) {
                shared.set_error(e);
                eos = true;
                finish(&shared, seq);
            }
            continue;
        }
        if eos {
            continue;
        }
        {
            let q = lock(&shared.audio);
            if q.seq == seq && q.frames >= ahead {
                let _ = shared.audio_cv.wait_timeout(q, Duration::from_millis(50));
                continue;
            }
        }
        match decoder.next_chunk() {
            Ok(Some(chunk)) => {
                let t = pass_base + chunk.t;
                let data = convert(chunk, output.rate, channels, &mut resampler);
                push(&shared, seq, t, data, &mut next, rate, channels);
            }
            Ok(None) => {
                let len = shared.loop_len();
                if shared.looping.load(Relaxed) && len > 0.0 && decoder.seek(0.0).is_ok() {
                    pass += 1;
                    pass_base = pass as f64 * len;
                    shared.pass.fetch_max(pass, Relaxed);
                    resampler = None;
                    continue;
                }
                eos = true;
                finish(&shared, seq);
            }
            Err(e) => {
                shared.set_error(e);
                eos = true;
                finish(&shared, seq);
            }
        }
    }
}

/// Queue samples for seek `seq`: trim what overlaps the queue's end, fill a short hole
/// with silence, so the timeline the clock follows stays continuous.
fn push(shared: &Shared, seq: u64, mut t: f64, mut data: Vec<f32>, next: &mut Option<f64>, rate: f64, channels: usize) {
    let mut q = lock(&shared.audio);
    if q.seq != seq || data.len() < channels {
        return;
    }
    if let Some(expect) = *next {
        let gap = t - expect;
        if gap * rate < -0.5 {
            let skip = (-gap * rate).round() as usize;
            if skip * channels >= data.len() {
                return;
            }
            data.drain(..skip * channels);
            t = expect;
        } else if gap * rate > 2.0 && gap < MAX_GAP_SECS {
            let n = (gap * rate).round() as usize;
            q.chunks.push_back(Chunk { t: expect, data: vec![0.0; n * channels], pos: 0 });
            q.frames += n;
        }
    }
    let frames = data.len() / channels;
    *next = Some(t + frames as f64 / rate);
    q.frames += frames;
    q.drained = false;
    q.chunks.push_back(Chunk { t, data, pos: 0 });
}

fn finish(shared: &Shared, seq: u64) {
    let mut q = lock(&shared.audio);
    if q.seq == seq {
        q.eos = true;
        if q.chunks.is_empty() {
            q.drained = true;
        }
    }
    drop(q);
    shared.wake();
}

/// A decoded chunk in the device's layout and rate.
fn convert(chunk: AudioChunk, rate: u32, channels: usize, resampler: &mut Option<Resampler>) -> Vec<f32> {
    let data = remix(&chunk.data, chunk.channels.max(1), channels);
    if chunk.rate == rate || chunk.rate == 0 {
        return data;
    }
    if resampler.as_ref().is_none_or(|r| r.from != chunk.rate) {
        *resampler = Some(Resampler::new(chunk.rate, rate, channels));
    }
    let Some(r) = resampler.as_mut() else { return data };
    let mut out = Vec::with_capacity(data.len() * rate as usize / chunk.rate as usize + channels * 2);
    r.process(&data, &mut out);
    out
}

/// Interleaved `from`-channel samples as `to` channels: through stereo, with a standard downmix
/// for 5.1 and 7.1, and the extra outputs of a surround device left silent.
pub(crate) fn remix(data: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return data.to_vec();
    }
    const C: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let mut out = Vec::with_capacity(data.len() / from * to);
    for f in data.chunks_exact(from) {
        let (l, r) = match from {
            1 => (f[0], f[0]),
            // FL FR FC LFE BL BR
            6 => ((f[0] + C * f[2] + C * f[4]) / (1.0 + 2.0 * C), (f[1] + C * f[2] + C * f[5]) / (1.0 + 2.0 * C)),
            // FL FR FC LFE BL BR SL SR
            8 => ((f[0] + C * (f[2] + f[4] + f[6])) / (1.0 + 3.0 * C), (f[1] + C * (f[2] + f[5] + f[7])) / (1.0 + 3.0 * C)),
            _ => (f[0], f[1]),
        };
        match to {
            1 => out.push((l + r) * 0.5),
            _ => {
                out.push(l);
                out.push(r);
                out.extend(std::iter::repeat_n(0.0, to - 2));
            }
        }
    }
    out
}

/// Linear-interpolation resampler for interleaved frames, carrying its phase across chunks.
pub(crate) struct Resampler {
    from: u32,
    step: f64,
    pos: f64,
    prev: Vec<f32>,
    channels: usize,
    primed: bool,
}

impl Resampler {
    pub(crate) fn new(from: u32, to: u32, channels: usize) -> Resampler {
        Resampler { from, step: from as f64 / to.max(1) as f64, pos: 0.0, prev: vec![0.0; channels], channels, primed: false }
    }

    /// Output frames at positions `pos, pos + step, ...` over the sequence [prev, input...].
    pub(crate) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        let n = input.len() / ch;
        if n == 0 {
            return;
        }
        if !self.primed {
            self.prev.copy_from_slice(&input[..ch]);
            self.primed = true;
        }
        while self.pos < n as f64 {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            for c in 0..ch {
                let a = if i == 0 { self.prev[c] } else { input[(i - 1) * ch + c] };
                let b = input[i * ch + c];
                out.push(a + (b - a) * frac);
            }
            self.pos += self.step;
        }
        self.pos -= n as f64;
        self.prev.copy_from_slice(&input[(n - 1) * ch..n * ch]);
    }
}
