//! Video/audio playback on background threads.
//!
//! "bro-media-video" probes the file, then decodes frames ahead into a small queue;
//! "bro-media-audio" decodes samples ahead for the output device. Both stamp what they produce
//! with the number of the seek they follow, so a seek drops stale work without waiting for the threads. The clock
//! runs on a timeline that keeps counting across seamless loops: the audio device moves it while
//! audio plays, the wall clock otherwise.

use crate::audio::AudioQueue;
use crate::decode::{self, Backend, ProcSlot, VideoDecoder};
use crate::{Info, Rgba};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long `open` waits for the file to be probed.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);
/// Decoded frames kept ahead stay under this many bytes (and between 2 and 8 frames).
const QUEUE_BYTES: usize = 32 << 20;
/// How often a UI with nothing due should look again while the decoder catches up.
const POLL: Duration = Duration::from_millis(10);
/// A frame this close to due counts as due.
const DUE_SLACK: f64 = 0.002;
/// The audio callback stopped this long ago: the device is gone, run on the wall clock.
const AUDIO_STALE: Duration = Duration::from_millis(300);
/// Longest the clock waits for audio (start-up, a seek, an underrun) before running without it.
const AUDIO_WAIT: Duration = Duration::from_secs(2);
/// Drop waits this long for the threads, then leaves them to finish on their own.
const DROP_WAIT: Duration = Duration::from_millis(250);

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A seek request for the decoder threads.
pub(crate) struct Seek {
    pub to: f64,
    pub seq: u64,
}

pub(crate) struct Frame {
    /// Timeline seconds (media time plus whole loops).
    pub t: f64,
    pub rgba: Rgba,
}

pub(crate) struct FrameQueue {
    pub seq: u64,
    pub frames: VecDeque<Frame>,
    pub cap: usize,
    /// The decoder reached the end since seek `seq`.
    pub eos: bool,
    /// Hand out the next frame whatever the clock says: the poster after open, the target after a seek.
    pub preroll: bool,
}

/// The playback clock, in timeline seconds.
pub(crate) struct Clock {
    pub seq: u64,
    pub playing: bool,
    /// Timeline seconds at `at`.
    pub base: f64,
    pub at: Instant,
    /// Lower bound while playing: the position play or seek started from (audio latency would
    /// otherwise pull the first reading back).
    pub floor: f64,
    /// The end of what the audio device was given: the clock can't pass it while audio drives.
    pub limit: Option<f64>,
    /// An output stream is (or is about to be) playing this player's audio.
    pub audio: bool,
    /// Last audio callback, and the last one that played samples (or play/seek: the wait starts).
    pub alive: Option<Instant>,
    pub fed: Option<Instant>,
}

impl Clock {
    pub fn now(&self, t: Instant) -> f64 {
        if !self.playing {
            return self.base;
        }
        let dt = if t >= self.at { (t - self.at).as_secs_f64() } else { -(self.at - t).as_secs_f64() };
        let v = (self.base + dt).max(self.floor);
        match self.limit {
            Some(limit) if self.audio_drives(t) => v.min(limit.max(self.floor)),
            _ => v,
        }
    }

    fn audio_drives(&self, t: Instant) -> bool {
        self.audio
            && self.fed.is_some_and(|f| t.saturating_duration_since(f) < AUDIO_WAIT)
            && self.alive.is_none_or(|a| t.saturating_duration_since(a) < AUDIO_STALE)
    }
}

/// State shared by the player, its threads and the audio callback. Locks are never nested.
pub(crate) struct Shared {
    pub stop: AtomicBool,
    pub looping: AtomicBool,
    pub playing: AtomicBool,
    pub volume: AtomicU32,
    pub muted: AtomicBool,
    /// Loop length in seconds (f64 bits): the container's duration, 0 when unknown.
    loop_len: AtomicU64,
    /// The furthest loop pass a decoder has started since the last seek.
    pub pass: AtomicU32,
    pub clock: Mutex<Clock>,
    pub frames: Mutex<FrameQueue>,
    pub frames_cv: Condvar,
    pub audio: Mutex<AudioQueue>,
    pub audio_cv: Condvar,
    error: Mutex<Option<String>>,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    pub procs: Mutex<Vec<ProcSlot>>,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            stop: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            volume: AtomicU32::new(1f32.to_bits()),
            muted: AtomicBool::new(false),
            loop_len: AtomicU64::new(0),
            pass: AtomicU32::new(0),
            clock: Mutex::new(Clock { seq: 0, playing: false, base: 0.0, at: Instant::now(), floor: 0.0, limit: None, audio: false, alive: None, fed: None }),
            frames: Mutex::new(FrameQueue { seq: 0, frames: VecDeque::new(), cap: 8, eos: false, preroll: true }),
            frames_cv: Condvar::new(),
            audio: Mutex::new(AudioQueue::default()),
            audio_cv: Condvar::new(),
            error: Mutex::new(None),
            waker: Mutex::new(None),
            procs: Mutex::new(Vec::new()),
        }
    }

    pub fn loop_len(&self) -> f64 {
        f64::from_bits(self.loop_len.load(Relaxed))
    }

    pub fn set_error(&self, e: String) {
        lock(&self.error).get_or_insert(e);
        self.wake();
    }

    pub fn wake(&self) {
        let waker = lock(&self.waker).clone();
        if let Some(wake) = waker {
            wake();
        }
    }
}

/// Video/audio playback on background threads. Frames are produced ahead and handed out when due;
/// audio plays through the default output device (cpal) and is the master clock when present, else
/// the wall clock. Dropping it stops every thread promptly.
pub struct Player {
    info: Info,
    shared: Arc<Shared>,
    video_tx: Option<Sender<Seek>>,
    audio_tx: Option<Sender<Seek>>,
    threads: Vec<JoinHandle<()>>,
    seq: u64,
    /// A video thread is decoding frames.
    video: bool,
}

impl Player {
    /// Open `path` and decode its first frame (shown by [`Player::take_frame`] before play).
    /// `max_side` caps the decoded frame size (scaled in the decoder, aspect kept). Blocks while the
    /// file is probed and its first frames decode: ~50-100 ms through Media Foundation (up to ~200
    /// ms for 4K), ~100-200 ms when ffprobe has to run. Call it off the UI thread if that matters.
    pub fn open(path: &Path, max_side: u32) -> Result<Player, String> {
        Player::open_with(path, max_side, Backend::Auto)
    }

    pub(crate) fn open_with(path: &Path, max_side: u32, backend: Backend) -> Result<Player, String> {
        let max_side = max_side.clamp(16, 8192);
        let shared = Arc::new(Shared::new());
        let (info_tx, info_rx) = mpsc::channel();
        let (video_tx, video_rx) = mpsc::channel();
        let video = spawn("bro-media-video", {
            let (path, shared) = (path.to_path_buf(), shared.clone());
            move || video_thread(path, max_side, backend, shared, video_rx, info_tx)
        })?;
        let (info, decoding) = match info_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(opened)) => opened,
            Ok(Err(e)) => {
                let _ = video.join();
                return Err(e);
            }
            Err(_) => {
                // The thread is stuck in the decoder: leave it, it exits when the call returns.
                shared.stop.store(true, Relaxed);
                return Err("timed out opening this file".into());
            }
        };
        let mut player = Player { info, shared, video_tx: Some(video_tx), audio_tx: None, threads: vec![video], seq: 0, video: decoding };
        if player.info.has_audio {
            lock(&player.shared.clock).audio = true;
            let backend = if player.info.backend == "ffmpeg" { Backend::Ffmpeg } else { backend };
            let (audio_tx, audio_rx) = mpsc::channel();
            let shared = player.shared.clone();
            let path = path.to_path_buf();
            match spawn("bro-media-audio", move || crate::audio::run(path, backend, shared, audio_rx)) {
                Ok(thread) => {
                    player.threads.push(thread);
                    player.audio_tx = Some(audio_tx);
                }
                Err(_) => lock(&player.shared.clock).audio = false,
            }
        }
        Ok(player)
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    pub fn play(&mut self) {
        if self.ended() {
            self.seek(Duration::ZERO);
        }
        let now = Instant::now();
        let mut clock = lock(&self.shared.clock);
        if !clock.playing {
            clock.playing = true;
            clock.at = now;
            clock.floor = clock.base;
            clock.fed = Some(now);
            if clock.audio {
                clock.limit = Some(clock.base);
            }
        }
        drop(clock);
        self.shared.playing.store(true, Relaxed);
    }

    pub fn pause(&mut self) {
        let now = Instant::now();
        let mut clock = lock(&self.shared.clock);
        if clock.playing {
            clock.base = clock.now(now);
            clock.at = now;
            clock.playing = false;
        }
        drop(clock);
        self.shared.playing.store(false, Relaxed);
    }

    pub fn toggle(&mut self) {
        if self.is_playing() { self.pause() } else { self.play() }
    }

    /// Playing and not at the end.
    pub fn is_playing(&self) -> bool {
        let playing = lock(&self.shared.clock).playing;
        playing && !self.ended()
    }

    /// Jump to `to` (clamped to the file). Accurate to the frame: decoding restarts at the key frame
    /// before `to` and frames before it are dropped, then the frame at `to` is handed out at once,
    /// paused or not.
    pub fn seek(&mut self, to: Duration) {
        let mut to = to.as_secs_f64();
        if let Some(d) = self.info.duration {
            // the very end still shows the last frame
            to = to.min((d.as_secs_f64() - 0.05).max(0.0));
        }
        self.seq += 1;
        let seq = self.seq;
        {
            let mut q = lock(&self.shared.frames);
            q.seq = seq;
            q.frames.clear();
            q.eos = !self.video;
            q.preroll = true;
        }
        self.shared.frames_cv.notify_all();
        {
            let mut a = lock(&self.shared.audio);
            a.seq = seq;
            a.chunks.clear();
            a.frames = 0;
            a.eos = false;
            a.drained = false;
        }
        self.shared.audio_cv.notify_all();
        self.shared.pass.store(0, Relaxed);
        {
            let now = Instant::now();
            let mut clock = lock(&self.shared.clock);
            clock.seq = seq;
            clock.base = to;
            clock.at = now;
            clock.floor = to;
            clock.fed = Some(now);
            clock.limit = clock.audio.then_some(to);
        }
        for tx in [&self.video_tx, &self.audio_tx].into_iter().flatten() {
            let _ = tx.send(Seek { to, seq });
        }
    }

    /// Where playback is, within the file (loops wrap back to zero).
    pub fn position(&self) -> Duration {
        let t = self.clock_now();
        let len = self.shared.loop_len();
        if len <= 0.0 {
            return Duration::from_secs_f64(t.max(0.0));
        }
        if self.ended() {
            return Duration::from_secs_f64(len);
        }
        let passes = (t / len).floor().clamp(0.0, self.shared.pass.load(Relaxed) as f64);
        Duration::from_secs_f64((t - passes * len).clamp(0.0, len))
    }

    /// 0..=1, applied in the audio callback (takes effect within ~10 ms).
    pub fn set_volume(&mut self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
        self.shared.volume.store(v.to_bits(), Relaxed);
    }

    pub fn set_muted(&mut self, m: bool) {
        self.shared.muted.store(m, Relaxed);
    }

    /// Loop seamlessly when the duration is known (the decoders wrap on their own); otherwise the
    /// next [`Player::take_frame`] after the end seeks back to the start.
    pub fn set_looping(&mut self, l: bool) {
        self.shared.looping.store(l, Relaxed);
    }

    /// Every stream played to its end (never while looping).
    pub fn ended(&self) -> bool {
        let audio = lock(&self.shared.clock).audio;
        let video_done = !self.video || {
            let q = lock(&self.shared.frames);
            q.eos && q.frames.is_empty() && !q.preroll
        };
        let audio_done = !audio || lock(&self.shared.audio).drained;
        if self.video || audio {
            return video_done && audio_done;
        }
        // Nothing decoding (audio-only without speakers): the wall clock reaches the end.
        self.info.duration.is_none_or(|d| self.clock_now() >= d.as_secs_f64())
    }

    /// The newest frame that is due now, if it changed since the last call (non-blocking; call
    /// every UI frame). Also hands out the poster after open and the target frame after a seek.
    pub fn take_frame(&mut self) -> Option<Rgba> {
        let playing = lock(&self.shared.clock).playing;
        if playing && self.shared.looping.load(Relaxed) && self.ended() {
            self.seek(Duration::ZERO);
        }
        let now = self.clock_now();
        let mut q = lock(&self.shared.frames);
        let mut out = None;
        if q.preroll && !q.frames.is_empty() {
            q.preroll = false;
            out = q.frames.pop_front();
        }
        while q.frames.front().is_some_and(|f| f.t <= now + DUE_SLACK) {
            out = q.frames.pop_front();
        }
        drop(q);
        if out.is_some() {
            self.shared.frames_cv.notify_all();
        }
        out.map(|f| f.rgba)
    }

    /// When the UI should wake next to show a frame: None when paused or ended (except to pick up a
    /// seek's frame while paused). Pair with [`Player::set_waker`] to avoid polling while the
    /// decoder catches up.
    pub fn next_wake(&self) -> Option<Instant> {
        let now = Instant::now();
        if !self.video {
            return None;
        }
        let (preroll, front, eos) = {
            let q = lock(&self.shared.frames);
            (q.preroll, q.frames.front().map(|f| f.t), q.eos)
        };
        if preroll && !eos {
            return Some(if front.is_some() { now } else { now + POLL });
        }
        let (playing, t) = {
            let clock = lock(&self.shared.clock);
            (clock.playing, clock.now(now))
        };
        if !playing {
            return None;
        }
        if self.ended() {
            return self.shared.looping.load(Relaxed).then_some(now);
        }
        match front {
            Some(ft) if ft <= t + DUE_SLACK => Some(now),
            Some(ft) => Some(now + Duration::from_secs_f64((ft - t).min(1.0))),
            None => Some(now + POLL),
        }
    }

    /// An error the decoder hit after opening (shown in the UI).
    pub fn error(&self) -> Option<String> {
        lock(&self.shared.error).clone()
    }

    /// Called from a decoder thread when a frame is queued into an empty queue, a seek's frame
    /// arrives, playback ends or an error occurs: wake an event-driven UI (say, an
    /// `EventLoopProxy::send_event`) instead of polling.
    pub fn set_waker(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        *lock(&self.shared.waker) = Some(Arc::new(wake));
    }

    fn clock_now(&self) -> f64 {
        lock(&self.shared.clock).now(Instant::now())
    }

    /// An output device is playing this player's audio (false on a machine without speakers).
    #[cfg(test)]
    pub(crate) fn audio_running(&self) -> bool {
        let clock = lock(&self.shared.clock);
        clock.audio && clock.alive.is_some()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Relaxed);
        self.shared.playing.store(false, Relaxed);
        self.video_tx = None;
        self.audio_tx = None;
        self.shared.frames_cv.notify_all();
        self.shared.audio_cv.notify_all();
        // A decoder blocked on ffmpeg's pipe returns once ffmpeg is gone.
        for slot in lock(&self.shared.procs).iter() {
            if let Some(child) = lock(slot).as_mut() {
                let _ = child.kill();
            }
        }
        let deadline = Instant::now() + DROP_WAIT;
        for thread in self.threads.drain(..) {
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            // Still inside a decoder call: detach; it sees `stop` and exits when the call returns.
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<JoinHandle<()>, String> {
    std::thread::Builder::new().name(name.into()).spawn(f).map_err(|e| format!("can't start a decoder thread: {e}"))
}

/// Frames ahead for a frame of this size.
fn queue_cap(size: Option<(u32, u32)>, max_side: u32) -> usize {
    let (w, h) = size.map(|(w, h)| crate::still::fit(w, h, max_side)).unwrap_or((max_side, max_side));
    (QUEUE_BYTES / Rgba::byte_len(w, h).max(1)).clamp(2, 8)
}

type Opened = Result<(Info, bool), String>;

/// The "bro-media-video" thread: probe, report, then decode until stopped.
fn video_thread(path: PathBuf, max_side: u32, backend: Backend, shared: Arc<Shared>, rx: Receiver<Seek>, info_tx: Sender<Opened>) {
    let opened = match decode::open_video(&path, max_side, backend) {
        Ok(opened) => opened,
        Err(e) => {
            let _ = info_tx.send(Err(e));
            return;
        }
    };
    let len = opened.info.duration.map_or(0.0, |d| d.as_secs_f64());
    shared.loop_len.store(len.to_bits(), Relaxed);
    let Some(mut video) = opened.video else {
        lock(&shared.frames).eos = true;
        let _ = info_tx.send(Ok((opened.info, false)));
        return;
    };
    lock(&shared.frames).cap = queue_cap(opened.info.size, max_side);
    if let Some(slot) = video.proc_slot() {
        lock(&shared.procs).push(slot);
    }
    let _ = info_tx.send(Ok((opened.info, true)));
    decode_video(video.as_mut(), &shared, &rx);
}

fn decode_video(video: &mut dyn VideoDecoder, shared: &Shared, rx: &Receiver<Seek>) {
    let interval = video.frame_interval();
    let mut seq = 0;
    let mut pass = 0;
    let mut pass_base = 0.0;
    // After a seek: frames before `target` are decoded and dropped, the last of them held in case
    // the target is past the final frame.
    let mut target: Option<f64> = None;
    let mut held: Option<Frame> = None;
    let mut eos = false;
    let mut last_push = Instant::now();
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
        // The newest seek wins; a scrub bar sends many.
        if let Some(seek) = rx.try_iter().last().or(first) {
            seq = seek.seq;
            pass = 0;
            pass_base = 0.0;
            held = None;
            eos = false;
            match video.seek(seek.to) {
                Ok(()) => target = Some(seek.to),
                Err(e) => {
                    shared.set_error(e);
                    finish(shared, seq);
                    eos = true;
                }
            }
            continue;
        }
        if eos {
            continue;
        }
        let frame = match video.next_frame() {
            Ok(Some((pts, rgba))) => Frame { t: pass_base + pts, rgba },
            Ok(None) => {
                let len = shared.loop_len();
                if shared.looping.load(Relaxed) && len > 0.0 && target.is_none() && video.seek(0.0).is_ok() {
                    pass += 1;
                    pass_base = pass as f64 * len;
                    shared.pass.fetch_max(pass, Relaxed);
                    continue;
                }
                target = None;
                if let Some(frame) = held.take() {
                    push(shared, seq, frame);
                }
                finish(shared, seq);
                eos = true;
                continue;
            }
            Err(e) => {
                shared.set_error(e);
                finish(shared, seq);
                eos = true;
                continue;
            }
        };
        if let Some(to) = target {
            if frame.t + interval / 2.0 < to {
                held = Some(frame);
                continue;
            }
            target = None;
            held = None;
        }
        // Behind the clock (decoding slower than real time): skip frames, but keep the picture moving.
        if shared.playing.load(Relaxed) && last_push.elapsed() < Duration::from_millis(100) {
            let now = lock(&shared.clock).now(Instant::now());
            if frame.t + interval < now && !lock(&shared.frames).preroll {
                continue;
            }
        }
        if push(shared, seq, frame) {
            last_push = Instant::now();
        }
    }
}

/// Queue a frame for seek `seq`, waiting while the queue is full. False when the frame is
/// stale (a seek is on its way) or the player is stopping.
fn push(shared: &Shared, seq: u64, frame: Frame) -> bool {
    let mut q = lock(&shared.frames);
    loop {
        if q.seq != seq || shared.stop.load(Relaxed) {
            return false;
        }
        if q.frames.len() < q.cap {
            break;
        }
        q = shared.frames_cv.wait_timeout(q, Duration::from_millis(50)).unwrap_or_else(PoisonError::into_inner).0;
    }
    let wake = q.frames.is_empty();
    q.frames.push_back(frame);
    drop(q);
    if wake {
        shared.wake();
    }
    true
}

/// Mark the stream after seek `seq` finished.
fn finish(shared: &Shared, seq: u64) {
    {
        let mut q = lock(&shared.frames);
        if q.seq == seq {
            q.eos = true;
            // nothing to show for this seek: don't wait for one
            if q.frames.is_empty() {
                q.preroll = false;
            }
        }
    }
    shared.wake();
}
