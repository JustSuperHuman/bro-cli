//! Photo and video previews for bro's desktop sidebar.
//!
//! [`kind_of`] sorts a path into a media kind, [`load_image`] decodes stills, animations and SVG,
//! [`thumbnail`] makes a small preview of an image or a video, and [`Player`] plays video and
//! audio on background threads.
//!
//! Video and audio decode through Media Foundation on Windows (video above 720p on the GPU) and
//! through the ffmpeg CLI elsewhere, and on Windows too when Media Foundation can't read a file.
//! Everything here either blocks the calling thread or runs on its own threads; there is no async
//! runtime.

// All `unsafe` stays in `win`.
#![deny(unsafe_code)]

mod audio;
mod decode;
mod ffmpeg;
mod kind;
mod player;
mod still;
#[cfg(windows)]
#[allow(unsafe_code)]
mod win;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::Duration;

pub use ffmpeg::{ffmpeg_available, set_search_dirs};
pub use kind::{MediaKind, is_executable, kind_of};
pub use player::Player;
pub use still::{Image, is_animated, load_image};

/// Straight-alpha RGBA8, sRGB.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Rgba {
    /// Bytes in a `w` x `h` RGBA8 image.
    pub fn byte_len(w: u32, h: u32) -> usize {
        w as usize * h as usize * 4
    }

    /// The pixel at (x, y) as `[r, g, b, a]`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.w as usize + x as usize) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
}

/// Pixel data is never printed: a frame is megabytes.
impl std::fmt::Debug for Rgba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rgba({}x{})", self.w, self.h)
    }
}

/// What a video or audio file holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub duration: Option<Duration>,
    /// Display size of the video (rotation and pixel aspect applied), before any downscale.
    pub size: Option<(u32, u32)>,
    pub has_video: bool,
    pub has_audio: bool,
    /// The video codec ("H.264", "HEVC", "VP9", "AV1"...), or the audio codec of an audio-only file.
    pub codec: Option<String>,
    /// The audio codec ("AAC", "Opus", "MP3"...).
    pub audio_codec: Option<String>,
    /// Nominal frames per second.
    pub fps: Option<f32>,
    /// What decodes it: "Media Foundation", "Media Foundation (GPU)" or "ffmpeg".
    pub backend: &'static str,
}

/// A small preview: images (first frame) and videos (a frame ~10% in, or the first) fitted inside
/// `max_side` x `max_side`. Blocks for the decode (tens of ms for a photo or a video seek, more
/// when ffmpeg has to start), so call it off the UI thread.
pub fn thumbnail(path: &Path, max_side: u32) -> Result<Rgba, String> {
    if !path.is_file() {
        return Err(format!("{} doesn't exist", path.display()));
    }
    match kind_of(path) {
        MediaKind::Image => first_frame(path, max_side),
        MediaKind::Video => decode::video_thumbnail(path, max_side),
        // the cover art, when there is one (ffmpeg finds it)
        MediaKind::Audio => decode::video_thumbnail(path, max_side).map_err(|_| "no picture in this audio file".to_string()),
        MediaKind::Pdf => Err("no preview for PDFs".into()),
        // An unknown extension that still decodes: try it as an image, then as a video.
        MediaKind::Other => first_frame(path, max_side).or_else(|_| decode::video_thumbnail(path, max_side)).map_err(|_| "no preview for this file".to_string()),
    }
}

fn first_frame(path: &Path, max_side: u32) -> Result<Rgba, String> {
    let image = load_image(path, max_side)?;
    image.frames.into_iter().next().map(|(frame, _)| frame).ok_or_else(|| "the image has no frames".to_string())
}
