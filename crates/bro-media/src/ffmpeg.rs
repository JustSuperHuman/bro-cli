//! The ffmpeg CLI as a decoder: ffprobe for stream info, ffmpeg piping raw RGBA frames or f32le
//! samples. The backend everywhere but Windows, and Windows' fallback for what Media Foundation
//! can't open.

use crate::decode::{AudioChunk, AudioDecoder, Opened, ProcSlot, VideoDecoder};
use crate::{Info, Rgba};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Highest frame rate decoded; faster sources (slow-motion phone clips) get frames dropped.
const MAX_FPS: f64 = 60.0;
/// Samples per audio read (per channel): ~20 ms at 48 kHz.
const AUDIO_BLOCK: usize = 1024;

#[derive(Clone, Debug)]
pub(crate) struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

static EXTRA_DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static FOUND: Mutex<Option<(Instant, Option<Tools>)>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Directories to look for ffmpeg and ffprobe in before PATH, e.g. bro-core's tool search dirs,
/// which include PATH entries added after the app started.
pub fn set_search_dirs(dirs: Vec<PathBuf>) {
    *lock(&EXTRA_DIRS) = dirs;
    *lock(&FOUND) = None;
}

/// ffmpeg and ffprobe are installed: video and audio play everywhere, not only through Windows'
/// own codecs.
pub fn ffmpeg_available() -> bool {
    tools().is_some()
}

pub(crate) fn tools() -> Option<Tools> {
    let mut found = lock(&FOUND);
    if let Some((at, tools)) = &*found {
        // A miss is looked up again now and then: `bro install ffmpeg` may run while the app is open.
        if tools.is_some() || at.elapsed() < Duration::from_secs(20) {
            return tools.clone();
        }
    }
    let dirs = search_dirs();
    let find = |name: &str| dirs.iter().map(|d| d.join(name)).find(|p| p.is_file());
    let suffix = std::env::consts::EXE_SUFFIX;
    let tools = find(&format!("ffmpeg{suffix}")).zip(find(&format!("ffprobe{suffix}"))).map(|(ffmpeg, ffprobe)| Tools { ffmpeg, ffprobe });
    *found = Some((Instant::now(), tools.clone()));
    tools
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = lock(&EXTRA_DIRS).clone();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(windows) {
        // winget's links land here; the app may have started before they were added to PATH
        if let Some(local) = env("LOCALAPPDATA") {
            dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
        }
        if let Some(home) = env("USERPROFILE") {
            dirs.push(home.join("scoop").join("shims"));
        }
        if let Some(data) = env("ProgramData") {
            dirs.push(data.join("chocolatey").join("bin"));
        }
        if let Some(pf) = env("ProgramFiles") {
            dirs.push(pf.join("ffmpeg").join("bin"));
        }
    } else {
        // GUI apps on macOS don't inherit the shell's PATH, so Homebrew's bin is missing there.
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin", "/snap/bin"].map(PathBuf::from));
        if let Some(home) = env("HOME") {
            dirs.push(home.join(".local").join("bin"));
        }
    }
    dirs.retain(|d| !d.as_os_str().is_empty());
    dirs
}

/// No console window flashes up for each ffmpeg on Windows.
fn command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// The input argument; a path starting with '-' would read as an option.
fn input(path: &Path) -> PathBuf {
    if path.as_os_str().to_string_lossy().starts_with('-') { Path::new(".").join(path) } else { path.to_path_buf() }
}

/// Run to completion within `timeout`: (stdout, stderr, exited successfully).
fn run(cmd: &mut Command, timeout: Duration) -> Result<(Vec<u8>, String, bool), String> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("can't run ffmpeg: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("ffmpeg has no output pipe")?;
    let mut stderr = child.stderr.take().ok_or("ffmpeg has no error pipe")?;
    // Read both pipes while waiting: a full pipe would stall ffmpeg forever.
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    match status {
        Some(status) => Ok((out, err, status.success())),
        None => Err("ffmpeg took too long on this file".into()),
    }
}

/// The last line ffmpeg complained with, for an error message.
fn last_line(stderr: &str) -> String {
    stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("ffmpeg failed").to_string()
}

// ---- probing ----

pub(crate) struct Probe {
    pub info: Info,
    /// Display size of the first picture stream (a video, or an audio file's cover art).
    pub picture: Option<(u32, u32)>,
    /// Frame rate to decode at, as an ffmpeg rational ("30000/1001").
    pub rate: (u64, u64),
}

pub(crate) fn probe(tools: &Tools, path: &Path) -> Result<Probe, String> {
    let mut cmd = command(&tools.ffprobe);
    cmd.args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"]).arg(input(path));
    let (out, err, ok) = run(&mut cmd, Duration::from_secs(20))?;
    let json: Value = serde_json::from_slice(&out).map_err(|_| if ok { "ffprobe printed nonsense".to_string() } else { format!("can't read this file: {}", last_line(&err)) })?;
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    if !ok && streams.is_empty() {
        return Err(format!("can't read this file: {}", last_line(&err)));
    }
    Ok(parse_probe(&json))
}

pub(crate) fn parse_probe(json: &Value) -> Probe {
    let streams = json["streams"].as_array().map(Vec::as_slice).unwrap_or_default();
    let of = |kind: &'static str| streams.iter().filter(move |s| s["codec_type"] == kind);
    let cover = |s: &&Value| s["disposition"]["attached_pic"].as_i64() == Some(1);
    let video = of("video").find(|s| !cover(s));
    let picture_stream = video.or_else(|| of("video").next());
    let audio = of("audio").next();
    let secs = |v: &Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).filter(|d| d.is_finite() && *d > 0.0);
    let duration = secs(&json["format"]["duration"]).or_else(|| video.and_then(|v| secs(&v["duration"]))).or_else(|| audio.and_then(|a| secs(&a["duration"])));
    let picture = picture_stream.and_then(display_size);
    let rate = video.and_then(|v| ratio(&v["avg_frame_rate"]).or_else(|| ratio(&v["r_frame_rate"]))).filter(|&(n, d)| n as f64 / d as f64 <= 240.0);
    let rate = match rate {
        Some((n, d)) if n as f64 / d as f64 <= MAX_FPS => (n, d),
        Some(_) => (MAX_FPS as u64, 1),
        None => (25, 1),
    };
    let codec = |s: Option<&Value>| s.and_then(|s| s["codec_name"].as_str()).map(codec_name);
    let audio_codec = codec(audio);
    Probe {
        info: Info {
            duration: duration.map(Duration::from_secs_f64),
            size: video.and(picture),
            has_video: video.is_some(),
            has_audio: audio.is_some(),
            codec: codec(video).or_else(|| audio_codec.clone()),
            audio_codec,
            fps: video.map(|_| rate.0 as f32 / rate.1 as f32),
            backend: "ffmpeg",
        },
        picture,
        rate,
    }
}

/// "30000/1001" -> (30000, 1001); "0/0" and junk -> None.
fn ratio(v: &Value) -> Option<(u64, u64)> {
    let (n, d) = v.as_str()?.split_once(['/', ':'])?;
    let (n, d) = (n.trim().parse::<u64>().ok()?, d.trim().parse::<u64>().ok()?);
    (n > 0 && d > 0).then_some((n, d))
}

/// Width x height as shown: sample aspect ratio applied, swapped for a 90/270 degree rotation.
fn display_size(s: &Value) -> Option<(u32, u32)> {
    let (w, h) = (s["width"].as_u64()?, s["height"].as_u64()?);
    if w == 0 || h == 0 {
        return None;
    }
    let w = match ratio(&s["sample_aspect_ratio"]) {
        Some((n, d)) if n != d => (w * n / d).max(1),
        _ => w,
    };
    let side = s["side_data_list"].as_array().and_then(|l| l.iter().find_map(|d| d["rotation"].as_f64()));
    let tag = s["tags"]["rotate"].as_str().and_then(|r| r.parse::<f64>().ok());
    let quarter = side.or(tag).map(|r| (r.round() as i64).rem_euclid(180) == 90).unwrap_or(false);
    let (w, h) = if quarter { (h, w) } else { (w, h) };
    Some((w.min(u32::MAX as u64) as u32, h.min(u32::MAX as u64) as u32))
}

fn codec_name(name: &str) -> String {
    let known = match name {
        "h264" => "H.264",
        "hevc" => "HEVC",
        "vp9" => "VP9",
        "vp8" => "VP8",
        "av1" => "AV1",
        "mpeg4" => "MPEG-4",
        "mpeg2video" => "MPEG-2",
        "mpeg1video" => "MPEG-1",
        "wmv1" | "wmv2" | "wmv3" => "WMV",
        "vc1" => "VC-1",
        "mjpeg" => "MJPEG",
        "prores" => "ProRes",
        "theora" => "Theora",
        "aac" => "AAC",
        "mp3" => "MP3",
        "mp2" => "MP2",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "flac" => "FLAC",
        "alac" => "ALAC",
        "ac3" => "AC-3",
        "eac3" => "E-AC-3",
        "dts" => "DTS",
        "wmav1" | "wmav2" | "wmapro" | "wmalossless" => "WMA",
        n if n.starts_with("pcm_") => "PCM",
        n => return n.to_ascii_uppercase(),
    };
    known.to_string()
}

// ---- decoding ----

/// One running ffmpeg writing to a pipe; stderr is kept (the tail) for error messages.
struct Pipe {
    slot: ProcSlot,
    out: Option<ChildStdout>,
    err: Arc<Mutex<String>>,
}

impl Pipe {
    fn new() -> Pipe {
        Pipe { slot: Arc::new(Mutex::new(None)), out: None, err: Arc::new(Mutex::new(String::new())) }
    }

    fn start(&mut self, mut cmd: Command) -> Result<(), String> {
        self.stop();
        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("can't run ffmpeg: {e}"))?;
        self.out = child.stdout.take();
        lock(&self.err).clear();
        if let Some(mut stderr) = child.stderr.take() {
            let err = self.err.clone();
            let _ = std::thread::Builder::new().name("bro-media-ffmpeg".into()).spawn(move || {
                let mut buf = [0u8; 1024];
                while let Ok(n @ 1..) = stderr.read(&mut buf) {
                    let mut e = lock(&err);
                    e.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if e.len() > 4096 {
                        let cut = e.len() - 2048;
                        let cut = (cut..e.len()).find(|&i| e.is_char_boundary(i)).unwrap_or(0);
                        e.drain(..cut);
                    }
                }
            });
        }
        *lock(&self.slot) = Some(child);
        Ok(())
    }

    /// Fill `buf` from the pipe: the bytes read, fewer than `buf.len()` only at the end of the stream.
    fn read_full(&mut self, buf: &mut [u8]) -> usize {
        let Some(out) = self.out.as_mut() else { return 0 };
        let mut n = 0;
        while n < buf.len() {
            match out.read(&mut buf[n..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => n += k,
            }
        }
        n
    }

    /// Reap a finished ffmpeg: Err with its complaint when it failed.
    fn finish(&mut self) -> Result<(), String> {
        self.out = None;
        let status = lock(&self.slot).take().and_then(|mut child| child.wait().ok());
        match status {
            Some(s) if !s.success() => {
                // let the stderr reader catch up
                std::thread::sleep(Duration::from_millis(20));
                Err(format!("ffmpeg can't decode this file: {}", last_line(&lock(&self.err))))
            }
            _ => Ok(()),
        }
    }

    fn stop(&mut self) {
        self.out = None;
        if let Some(mut child) = lock(&self.slot).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.stop();
    }
}

fn base_args(cmd: &mut Command, path: &Path, at: f64) {
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    if at > 0.0 {
        // Before -i: a fast seek to the key frame, then decoded (not shown) up to `at`.
        cmd.args(["-ss", &format!("{at:.3}")]);
    }
    cmd.arg("-i").arg(input(path));
}

/// Raw RGBA frames at a constant rate (the fps filter), so frame n is at start + n / fps.
pub(crate) struct FfVideo {
    exe: PathBuf,
    path: PathBuf,
    w: u32,
    h: u32,
    rate: (u64, u64),
    pipe: Pipe,
    start: f64,
    index: u64,
}

impl FfVideo {
    fn new(tools: &Tools, path: &Path, (w, h): (u32, u32), rate: (u64, u64)) -> FfVideo {
        FfVideo { exe: tools.ffmpeg.clone(), path: path.to_path_buf(), w, h, rate, pipe: Pipe::new(), start: 0.0, index: 0 }
    }

    fn spawn(&mut self, at: f64) -> Result<(), String> {
        let mut cmd = command(&self.exe);
        base_args(&mut cmd, &self.path, at);
        let filter = format!("fps={}/{},scale={}:{}:flags=bicubic,setsar=1", self.rate.0, self.rate.1, self.w, self.h);
        cmd.args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &filter, "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]);
        self.pipe.start(cmd)?;
        self.start = at;
        self.index = 0;
        Ok(())
    }
}

impl VideoDecoder for FfVideo {
    fn next_frame(&mut self) -> Result<Option<(f64, Rgba)>, String> {
        let mut px = vec![0u8; Rgba::byte_len(self.w, self.h)];
        if self.pipe.read_full(&mut px) < px.len() {
            let finished = self.pipe.finish();
            return if self.index == 0 { finished.map(|_| None) } else { Ok(None) };
        }
        let t = self.start + self.index as f64 * self.rate.1 as f64 / self.rate.0 as f64;
        self.index += 1;
        Ok(Some((t, Rgba { w: self.w, h: self.h, px })))
    }

    fn seek(&mut self, to: f64) -> Result<(), String> {
        self.spawn(to)
    }

    fn frame_interval(&self) -> f64 {
        self.rate.1 as f64 / self.rate.0 as f64
    }

    fn proc_slot(&self) -> Option<ProcSlot> {
        Some(self.pipe.slot.clone())
    }
}

/// Stereo f32 at the output device's rate (ffmpeg resamples and downmixes).
pub(crate) struct FfAudio {
    exe: PathBuf,
    path: PathBuf,
    rate: u32,
    pipe: Pipe,
    start: f64,
    frames: u64,
}

impl FfAudio {
    pub(crate) fn open(tools: &Tools, path: &Path, rate: u32) -> Result<FfAudio, String> {
        let mut audio = FfAudio { exe: tools.ffmpeg.clone(), path: path.to_path_buf(), rate, pipe: Pipe::new(), start: 0.0, frames: 0 };
        audio.spawn(0.0)?;
        Ok(audio)
    }

    fn spawn(&mut self, at: f64) -> Result<(), String> {
        let mut cmd = command(&self.exe);
        base_args(&mut cmd, &self.path, at);
        cmd.args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "2", "-ar", &self.rate.to_string(), "-f", "f32le", "pipe:1"]);
        self.pipe.start(cmd)?;
        self.start = at;
        self.frames = 0;
        Ok(())
    }
}

impl AudioDecoder for FfAudio {
    fn next_chunk(&mut self) -> Result<Option<AudioChunk>, String> {
        let mut bytes = vec![0u8; AUDIO_BLOCK * 2 * 4];
        let n = self.pipe.read_full(&mut bytes) / 8 * 8;
        if n == 0 {
            let finished = self.pipe.finish();
            return if self.frames == 0 { finished.map(|_| None) } else { Ok(None) };
        }
        let data: Vec<f32> = bytes[..n].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let t = self.start + self.frames as f64 / self.rate as f64;
        self.frames += (data.len() / 2) as u64;
        Ok(Some(AudioChunk { t, rate: self.rate, channels: 2, data }))
    }

    fn seek(&mut self, to: f64) -> Result<(), String> {
        self.spawn(to)
    }

    fn proc_slot(&self) -> Option<ProcSlot> {
        Some(self.pipe.slot.clone())
    }
}

/// Probe and start decoding the video stream, frames fitted inside `max_side`.
pub(crate) fn open_video(tools: &Tools, path: &Path, max_side: u32) -> Result<Opened, String> {
    let probe = probe(tools, path)?;
    if !probe.info.has_video && !probe.info.has_audio {
        return Err("no video or audio in this file".into());
    }
    let video = match (probe.info.has_video, probe.info.size) {
        (true, Some((w, h))) => {
            let mut video = FfVideo::new(tools, path, crate::still::fit(w, h, max_side), probe.rate);
            video.spawn(0.0)?;
            Some(Box::new(video) as Box<dyn VideoDecoder>)
        }
        _ => None,
    };
    Ok(Opened { info: probe.info, video })
}

/// A still image's first picture fitted inside `max_side`, with the picture's own size (AVIF, HEIC, JPEG XL:
/// stills the `image` crate can't read).
pub(crate) fn still(tools: &Tools, path: &Path, max_side: u32) -> Result<(Rgba, (u32, u32)), String> {
    let probe = probe(tools, path)?;
    let original = probe.picture.ok_or("no picture in this file")?;
    let (w, h) = crate::still::fit(original.0, original.1, max_side);
    let mut cmd = command(&tools.ffmpeg);
    base_args(&mut cmd, path, 0.0);
    let filter = format!("scale={w}:{h}:flags=lanczos,setsar=1");
    cmd.args(["-map", "0:v:0", "-frames:v", "1", "-an", "-sn", "-dn", "-vf", &filter, "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]);
    let (out, err, _) = run(&mut cmd, Duration::from_secs(20))?;
    let len = Rgba::byte_len(w, h);
    if out.len() < len {
        return Err(format!("ffmpeg can't decode this file: {}", last_line(&err)));
    }
    Ok((Rgba { w, h, px: out[..len].to_vec() }, original))
}

/// One frame ~10% in (or the first; for an audio file, its cover art) in a single ffmpeg run.
pub(crate) fn thumbnail(tools: &Tools, path: &Path, max_side: u32) -> Result<Rgba, String> {
    let probe = probe(tools, path)?;
    let (w, h) = probe.picture.map(|(w, h)| crate::still::fit(w, h, max_side)).ok_or("no video in this file")?;
    let at = if probe.info.has_video { probe.info.duration.map_or(0.0, |d| d.as_secs_f64() * 0.1) } else { 0.0 };
    let grab = |at: f64| -> Result<Rgba, String> {
        let mut cmd = command(&tools.ffmpeg);
        base_args(&mut cmd, path, at);
        let filter = format!("scale={w}:{h}:flags=bicubic,setsar=1");
        cmd.args(["-map", "0:v:0", "-frames:v", "1", "-an", "-sn", "-dn", "-vf", &filter, "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]);
        let (out, err, _) = run(&mut cmd, Duration::from_secs(20))?;
        let len = Rgba::byte_len(w, h);
        if out.len() < len {
            return Err(format!("ffmpeg can't decode this file: {}", last_line(&err)));
        }
        Ok(Rgba { w, h, px: out[..len].to_vec() })
    };
    grab(at).or_else(|e| if at > 0.0 { grab(0.0) } else { Err(e) })
}
