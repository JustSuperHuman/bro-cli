//! Picks a decoder for a video or audio file: Media Foundation on Windows, the ffmpeg CLI
//! elsewhere and wherever Media Foundation gives up. Decoders are created on the thread that uses
//! them (Media Foundation objects stay on their COM thread).

use crate::{Info, Rgba};
use std::path::Path;
use std::process::Child;
use std::sync::{Arc, Mutex};

/// A running ffmpeg, shared so [`crate::Player`]'s drop can kill it while a decoder thread is
/// blocked reading its pipe.
pub(crate) type ProcSlot = Arc<Mutex<Option<Child>>>;

pub(crate) trait VideoDecoder {
    /// The next frame in presentation order with its timestamp in seconds; None at the end.
    fn next_frame(&mut self) -> Result<Option<(f64, Rgba)>, String>;
    /// Continue from `to` seconds. Decoding may restart at the key frame before it, so callers
    /// drop frames until `to`.
    fn seek(&mut self, to: f64) -> Result<(), String>;
    /// Nominal seconds per frame.
    fn frame_interval(&self) -> f64;
    fn proc_slot(&self) -> Option<ProcSlot> {
        None
    }
}

/// Interleaved f32 samples starting at `t` seconds.
pub(crate) struct AudioChunk {
    pub t: f64,
    pub rate: u32,
    pub channels: usize,
    pub data: Vec<f32>,
}

pub(crate) trait AudioDecoder {
    /// The next block of samples; None at the end.
    fn next_chunk(&mut self) -> Result<Option<AudioChunk>, String>;
    fn seek(&mut self, to: f64) -> Result<(), String>;
    fn proc_slot(&self) -> Option<ProcSlot> {
        None
    }
}

/// Which decoders to try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// Media Foundation first on Windows, then ffmpeg.
    Auto,
    /// ffmpeg only (what Media Foundation already failed on, and tests).
    Ffmpeg,
}

/// Why a backend couldn't open a file (only Media Foundation reports these).
#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum OpenError {
    /// The container opened but no decoder is installed for this stream's codec.
    NoCodec { codec: String, video: bool },
    /// The backend can't read this kind of file at all.
    Unsupported,
    Failed(String),
}

pub(crate) struct Opened {
    pub info: Info,
    /// None for a file without (decodable-as-video) pictures, i.e. audio only.
    pub video: Option<Box<dyn VideoDecoder>>,
}

/// Probe `path` and open its video stream with frames fitted inside `max_side`.
pub(crate) fn open_video(path: &Path, max_side: u32, backend: Backend) -> Result<Opened, String> {
    if !path.is_file() {
        return Err(format!("{} doesn't exist", path.display()));
    }
    #[cfg(windows)]
    let mf = match backend {
        Backend::Auto => match crate::win::open_video(path, max_side) {
            Ok(opened) => return Ok(opened),
            Err(e) => Some(e),
        },
        Backend::Ffmpeg => None,
    };
    #[cfg(not(windows))]
    let mf: Option<OpenError> = {
        let _ = backend;
        None
    };
    let ff = crate::ffmpeg::tools().map(|tools| crate::ffmpeg::open_video(&tools, path, max_side));
    match ff {
        Some(Ok(opened)) => Ok(opened),
        Some(Err(e)) => Err(e),
        None => Err(no_ffmpeg(mf, "play")),
    }
}

/// Open the first audio stream, delivering samples at `rate` where the decoder can resample.
pub(crate) fn open_audio(path: &Path, rate: u32, backend: Backend) -> Result<Box<dyn AudioDecoder>, String> {
    #[cfg(windows)]
    let mf = match backend {
        Backend::Auto => match crate::win::MfAudio::open(path, rate) {
            Ok(audio) => return Ok(Box::new(audio)),
            Err(e) => Some(e),
        },
        Backend::Ffmpeg => None,
    };
    #[cfg(not(windows))]
    let mf: Option<OpenError> = {
        let _ = backend;
        None
    };
    match crate::ffmpeg::tools() {
        Some(tools) => crate::ffmpeg::FfAudio::open(&tools, path, rate).map(|a| Box::new(a) as Box<dyn AudioDecoder>),
        None => Err(no_ffmpeg(mf, "play the sound of")),
    }
}

/// A frame ~10% into the video (or the first), fitted inside `max_side`.
pub(crate) fn video_thumbnail(path: &Path, max_side: u32) -> Result<Rgba, String> {
    if !path.is_file() {
        return Err(format!("{} doesn't exist", path.display()));
    }
    #[cfg(windows)]
    let mf = match crate::win::open_video(path, max_side) {
        Ok(Opened { info, video: Some(mut video) }) => match grab(video.as_mut(), info.duration.map(|d| d.as_secs_f64())) {
            Ok(frame) => return Ok(frame),
            Err(e) => Some(OpenError::Failed(e)),
        },
        // Audio only to Media Foundation; ffmpeg may still find cover art.
        Ok(_) => Some(OpenError::Failed("no video in this file".into())),
        Err(e) => Some(e),
    };
    #[cfg(not(windows))]
    let mf: Option<OpenError> = None;
    match crate::ffmpeg::tools() {
        Some(tools) => crate::ffmpeg::thumbnail(&tools, path, max_side),
        None => Err(no_ffmpeg(mf, "preview")),
    }
}

/// The first frame at or after 10% of `duration`; the last frame when that's past the end, the
/// very first when the seek finds nothing.
#[cfg(windows)]
pub(crate) fn grab(video: &mut dyn VideoDecoder, duration: Option<f64>) -> Result<Rgba, String> {
    let at = duration.map_or(0.0, |d| d * 0.1);
    if at > 0.0 {
        video.seek(at)?;
    }
    let half = video.frame_interval() / 2.0;
    let mut last = None;
    // Bounded: a seek lands on the key frame before `at`, at most a few hundred frames back.
    for _ in 0..3000 {
        match video.next_frame()? {
            Some((t, frame)) if t + half >= at => return Ok(frame),
            Some((_, frame)) => last = Some(frame),
            None => break,
        }
    }
    if let Some(frame) = last {
        return Ok(frame);
    }
    if at > 0.0 {
        video.seek(0.0)?;
        if let Some((_, frame)) = video.next_frame()? {
            return Ok(frame);
        }
    }
    Err("no video frames in this file".into())
}

/// The error when Media Foundation (Windows) failed or doesn't exist (elsewhere) and ffmpeg isn't installed.
pub(crate) fn no_ffmpeg(mf: Option<OpenError>, verb: &str) -> String {
    const INSTALL: &str = "install ffmpeg (bro install ffmpeg)";
    match mf {
        Some(OpenError::NoCodec { codec, video: true }) => {
            format!("this video's codec ({codec}) isn't installed on Windows; {INSTALL} to {verb} it here, or open it in another app")
        }
        Some(OpenError::NoCodec { codec, video: false }) => {
            format!("this file's audio codec ({codec}) isn't installed on Windows; {INSTALL} to {verb} it here, or open it in another app")
        }
        Some(OpenError::Unsupported) => format!("Windows can't read this file; {INSTALL} to {verb} it here, or open it in another app"),
        Some(OpenError::Failed(e)) => format!("{e}; {INSTALL} to try another decoder"),
        None => format!("bro needs ffmpeg to {verb} this file; {INSTALL}"),
    }
}
