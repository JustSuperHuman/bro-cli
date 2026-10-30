//! In-crate tests: tables for `kind_of` / `is_executable`, image decoding from generated files,
//! and playback of short ffmpeg-generated clips (skipped with a note when ffmpeg is missing).

use crate::audio::{Resampler, remix};
use crate::decode::Backend;
use crate::{Image, MediaKind, Player, Rgba, is_animated, is_executable, kind_of, load_image, thumbnail};
use image::{Delay, Frame, RgbaImage};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write sample");
    path
}

// ---- kinds ----

#[test]
fn kind_by_extension() {
    let cases = [
        ("photo.JPG", MediaKind::Image),
        ("a/b/icon.svg", MediaKind::Image),
        ("shot.heic", MediaKind::Image),
        ("anim.webp", MediaKind::Image),
        ("clip.MP4", MediaKind::Video),
        ("clip.webm", MediaKind::Video),
        ("movie.mkv", MediaKind::Video),
        ("song.mp3", MediaKind::Audio),
        ("voice.opus", MediaKind::Audio),
        ("paper.PDF", MediaKind::Pdf),
        ("main.rs", MediaKind::Other),
        ("notes.md", MediaKind::Other),
    ];
    for (path, kind) in cases {
        assert_eq!(kind_of(Path::new(path)), kind, "{path}");
    }
}

#[test]
fn kind_by_magic_when_the_extension_says_nothing() {
    let dir = tempdir();
    let mut ts = vec![0u8; 400];
    ts[0] = 0x47;
    ts[188] = 0x47;
    let cases: Vec<(&str, Vec<u8>, MediaKind)> = vec![
        ("png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(), MediaKind::Image),
        ("jpeg", vec![0xff, 0xd8, 0xff, 0xe0, 0, 0x10], MediaKind::Image),
        ("gif", b"GIF89a\x01\0\x01\0".to_vec(), MediaKind::Image),
        ("webp", b"RIFF\0\0\0\0WEBPVP8 ".to_vec(), MediaKind::Image),
        ("avif", b"\0\0\0\x1cftypavif\0\0\0\0".to_vec(), MediaKind::Image),
        ("svg", b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<!-- logo -->\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(), MediaKind::Image),
        ("pdf", b"%PDF-1.7\n".to_vec(), MediaKind::Pdf),
        ("mp4", b"\0\0\0\x20ftypisom\0\0\x02\0".to_vec(), MediaKind::Video),
        ("m4a", b"\0\0\0\x20ftypM4A \0\0\x02\0".to_vec(), MediaKind::Audio),
        ("mkv", vec![0x1a, 0x45, 0xdf, 0xa3, 0x01], MediaKind::Video),
        ("avi", b"RIFF\0\0\0\0AVI LIST".to_vec(), MediaKind::Video),
        ("wav", b"RIFF\0\0\0\0WAVEfmt ".to_vec(), MediaKind::Audio),
        ("mp3", b"ID3\x04\0\0\0\0\0\0".to_vec(), MediaKind::Audio),
        ("flac", b"fLaC\0\0\0\x22".to_vec(), MediaKind::Audio),
        ("ogg", b"OggS\0\x02\0\0\0\0\0\0\0\0\x01\x02\x03\x04\0\0\0\0\0\0\0\0\x01\x1e\x01vorbis".to_vec(), MediaKind::Audio),
        ("text", b"just some notes\n".to_vec(), MediaKind::Other),
    ];
    for (name, bytes, kind) in cases {
        assert_eq!(kind_of(&write(dir.path(), name, &bytes)), kind, "{name}");
        // same answer behind an extension nobody knows
        assert_eq!(kind_of(&write(dir.path(), &format!("{name}.bin"), &bytes)), kind, "{name}.bin");
    }
    // `.ts` is TypeScript unless the bytes say transport stream
    assert_eq!(kind_of(&write(dir.path(), "video.ts", &ts)), MediaKind::Video);
    assert_eq!(kind_of(&write(dir.path(), "app.ts", b"export const x = 1;\n")), MediaKind::Other);
    assert_eq!(kind_of(&dir.path().join("missing")), MediaKind::Other);
    assert_eq!(kind_of(dir.path()), MediaKind::Other);
}

#[test]
fn executables_are_never_opened() {
    for path in [
        "setup.exe", "SETUP.EXE", "x.msi", "run.bat", "run.cmd", "profile.ps1", "screen.scr", "old.com", "link.lnk", "x.vbs", "x.js", "tool.jar",
        "Thing.app", "build.sh", "x.hta", "x.url", "x.py", "x.reg", "photo.jpg.exe", "setup.exe.", "setup.exe . ",
    ] {
        assert!(is_executable(Path::new(path)), "{path}");
    }
    for path in ["photo.jpg", "clip.mp4", "notes.txt", "page.html", "data.json", "Makefile.am", "archive.zip"] {
        assert!(!is_executable(Path::new(path)), "{path}");
    }
    let dir = tempdir();
    assert!(is_executable(&write(dir.path(), "tool", b"MZ\x90\0\x03")));
    assert!(is_executable(&write(dir.path(), "script", b"#!/bin/sh\necho hi\n")));
    assert!(is_executable(&write(dir.path(), "elf", b"\x7fELF\x02\x01")));
    assert!(!is_executable(&write(dir.path(), "README", b"hello\n")));
}

// ---- images ----

fn save_png(dir: &Path, name: &str, img: &RgbaImage) -> PathBuf {
    let path = dir.join(name);
    img.save(&path).expect("save png");
    path
}

#[test]
fn png_downscales_keeping_aspect_and_alpha() {
    let dir = tempdir();
    // left half transparent, right half opaque red
    let img = RgbaImage::from_fn(400, 200, |x, _| if x < 200 { image::Rgba([0, 0, 255, 0]) } else { image::Rgba([255, 0, 0, 255]) });
    let path = save_png(dir.path(), "half.png", &img);
    let loaded = load_image(&path, 100).expect("load");
    assert_eq!(loaded.format, "PNG");
    assert_eq!(loaded.original, (400, 200));
    assert_eq!(loaded.frames.len(), 1);
    let (frame, delay) = &loaded.frames[0];
    assert_eq!((frame.w, frame.h, *delay), (100, 50, Duration::ZERO));
    assert_eq!(frame.px.len(), Rgba::byte_len(100, 50));
    assert_eq!(frame.pixel(10, 25)[3], 0, "transparent side stays transparent");
    assert_eq!(frame.pixel(90, 25), [255, 0, 0, 255]);
    // the edge between them doesn't pick up the transparent pixels' blue
    let edge = frame.pixel(50, 25);
    assert!(edge[2] < 8, "colour bleed at the alpha edge: {edge:?}");
    // never upscaled
    let small = load_image(&path, 1000).expect("load");
    assert_eq!((small.frames[0].0.w, small.frames[0].0.h), (400, 200));
}

#[test]
fn large_photo_downscale_is_exact_and_opaque() {
    let dir = tempdir();
    let img = image::RgbImage::from_fn(1200, 900, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
    let path = dir.path().join("photo.jpg");
    img.save(&path).expect("save jpeg");
    let loaded = load_image(&path, 64).expect("load");
    assert_eq!(loaded.format, "JPEG");
    assert_eq!(loaded.original, (1200, 900));
    let frame = &loaded.frames[0].0;
    assert_eq!((frame.w, frame.h), (64, 48));
    assert!(frame.px.chunks(4).all(|p| p[3] == 255));
}

#[test]
fn animated_gif_keeps_every_frame_and_delay() {
    let dir = tempdir();
    let path = dir.path().join("anim.gif");
    let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
    let delays = [50, 120, 0];
    {
        let file = std::fs::File::create(&path).expect("create gif");
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        encoder.set_repeat(image::codecs::gif::Repeat::Infinite).expect("repeat");
        let frames = colors.iter().zip(delays).map(|(c, ms)| Frame::from_parts(RgbaImage::from_pixel(40, 20, image::Rgba(*c)), 0, 0, Delay::from_numer_denom_ms(ms, 1)));
        encoder.encode_frames(frames).expect("encode gif");
    }
    let loaded = load_image(&path, 20).expect("load");
    assert_eq!(loaded.format, "GIF");
    assert_eq!(loaded.original, (40, 20));
    assert!(loaded.is_animated());
    assert_eq!(loaded.frames.len(), 3);
    let got: Vec<u128> = loaded.frames.iter().map(|(_, d)| d.as_millis()).collect();
    // a zero delay plays at 100 ms, as in browsers
    assert_eq!(got, [50, 120, 100]);
    for ((frame, _), c) in loaded.frames.iter().zip(colors) {
        assert_eq!((frame.w, frame.h), (20, 10));
        let p = frame.pixel(10, 5);
        assert!(p.iter().zip(c).all(|(a, b)| a.abs_diff(b) <= 8), "{p:?} vs {c:?}");
    }
    assert!(is_animated(&path), "told without decoding every frame");
}

#[test]
fn stills_are_not_animations() {
    let dir = tempdir();
    let png = save_png(dir.path(), "still.png", &RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])));
    assert!(!is_animated(&png));
    // a GIF of one frame is a still too
    let gif = dir.path().join("one.gif");
    {
        let file = std::fs::File::create(&gif).expect("create gif");
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        encoder.encode_frames([Frame::new(RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255])))]).expect("encode gif");
    }
    assert!(!is_animated(&gif));
    assert!(!is_animated(&write(dir.path(), "notes.txt", b"not a picture")));
    assert!(!is_animated(&dir.path().join("missing.gif")));
}

#[test]
fn svg_renders_to_fit_max_side() {
    let dir = tempdir();
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100">
        <rect width="200" height="100" fill="#ff0000"/>
        <circle cx="150" cy="50" r="40" fill="#0000ff"/>
    </svg>"##;
    let path = write(dir.path(), "shape.svg", svg.as_bytes());
    for (max, size) in [(64, (64, 32)), (400, (400, 200))] {
        let loaded = load_image(&path, max).expect("load svg");
        assert_eq!(loaded.format, "SVG");
        assert_eq!(loaded.original, (200, 100));
        let frame = &loaded.frames[0].0;
        assert_eq!((frame.w, frame.h), size, "vector art renders at the size asked for");
        let s = frame.w as f32 / 200.0;
        assert_eq!(frame.pixel((10.0 * s) as u32, (10.0 * s) as u32), [255, 0, 0, 255]);
        assert_eq!(frame.pixel((150.0 * s) as u32, (50.0 * s) as u32), [0, 0, 255, 255]);
    }
    // sniffed without the extension, and with text (system fonts)
    let text = r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60"><text x="5" y="45" font-size="40" fill="#000">Hi</text></svg>"##;
    let loaded = load_image(&write(dir.path(), "label", text.as_bytes()), 120).expect("load text svg");
    let inked = loaded.frames[0].0.px.chunks(4).filter(|p| p[3] > 128).count();
    println!("svg text: {inked} inked pixels");
    assert!(inked > 0 || std::env::var_os("CI").is_some(), "the text drew nothing");
}

/// `jpeg` with an Exif APP1 segment (orientation tag only) right after SOI.
fn with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut tiff = b"II*\0\x08\0\0\0\x01\0".to_vec();
    tiff.extend([0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00]);
    tiff.extend(orientation.to_le_bytes());
    tiff.extend([0, 0, 0, 0, 0, 0]);
    let mut app1 = vec![0xff, 0xe1];
    app1.extend((2 + 6 + tiff.len() as u16).to_be_bytes());
    app1.extend(b"Exif\0\0");
    app1.extend(tiff);
    let mut out = jpeg[..2].to_vec();
    out.extend(app1);
    out.extend(&jpeg[2..]);
    out
}

#[test]
fn exif_orientation_is_applied() {
    let dir = tempdir();
    // 40x20: left half red, right half blue
    let img = image::RgbImage::from_fn(40, 20, |x, _| if x < 20 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode_image(&img).expect("encode jpeg");
    // 6: turn 90 degrees clockwise to view, so the left half ends up on top
    let path = write(dir.path(), "turned.jpg", &with_orientation(&jpeg, 6));
    let loaded = load_image(&path, 100).expect("load");
    assert_eq!(loaded.original, (20, 40));
    let frame = &loaded.frames[0].0;
    assert_eq!((frame.w, frame.h), (20, 40));
    let (top, bottom) = (frame.pixel(10, 5), frame.pixel(10, 35));
    assert!(top[0] > 200 && top[2] < 60, "top {top:?}");
    assert!(bottom[2] > 200 && bottom[0] < 60, "bottom {bottom:?}");
    // 1 leaves it alone
    let path = write(dir.path(), "upright.jpg", &with_orientation(&jpeg, 1));
    assert_eq!(load_image(&path, 100).expect("load").original, (40, 20));
}

#[test]
fn image_errors_are_readable() {
    let dir = tempdir();
    let missing = load_image(&dir.path().join("nope.png"), 64).unwrap_err();
    assert!(missing.contains("doesn't exist"), "{missing}");
    let broken = load_image(&write(dir.path(), "broken.png", b"\x89PNG\r\n\x1a\nthis is not a png"), 64).unwrap_err();
    assert!(!broken.is_empty());
    let text = load_image(&write(dir.path(), "notes.txt", b"hello"), 64).unwrap_err();
    assert!(text.contains("can't"), "{text}");
}

#[test]
fn thumbnail_of_an_image_is_its_first_frame() {
    let dir = tempdir();
    let img = RgbaImage::from_pixel(300, 100, image::Rgba([10, 200, 30, 255]));
    let thumb = thumbnail(&save_png(dir.path(), "wide.png", &img), 60).expect("thumbnail");
    assert_eq!((thumb.w, thumb.h), (60, 20));
    assert_eq!(thumb.pixel(30, 10), [10, 200, 30, 255]);
}

// ---- audio helpers ----

#[test]
fn resampler_hits_the_output_rate() {
    for (from, to) in [(44_100u32, 48_000u32), (48_000, 44_100), (22_050, 48_000), (96_000, 48_000)] {
        let mut r = Resampler::new(from, to, 2);
        let mut out = Vec::new();
        let second: Vec<f32> = (0..from * 2).map(|i| (i / 2) as f32 / from as f32).collect();
        for block in second.chunks(2 * 441) {
            r.process(block, &mut out);
        }
        let frames = (out.len() / 2) as i64;
        assert!((frames - to as i64).abs() <= 2, "{from} -> {to}: {frames}");
        // a ramp stays a ramp
        assert!(out.chunks(2).zip(out.chunks(2).skip(1)).all(|(a, b)| b[0] >= a[0] - 1e-6));
    }
}

#[test]
fn remix_maps_channel_layouts() {
    assert_eq!(remix(&[0.5, -0.5], 1, 2), [0.5, 0.5, -0.5, -0.5]);
    let mono = remix(&[0.2, 0.4], 2, 1);
    assert!(mono.len() == 1 && (mono[0] - 0.3).abs() < 1e-6, "{mono:?}");
    assert_eq!(remix(&[0.1, 0.2], 2, 4), [0.1, 0.2, 0.0, 0.0]);
    let surround = remix(&[1.0; 6], 6, 2);
    assert_eq!(surround.len(), 2);
    assert!(surround.iter().all(|s| (*s - 1.0).abs() < 1e-5), "full-scale 5.1 stays full scale without clipping: {surround:?}");
}

#[test]
fn ffprobe_json_is_read() {
    let json = serde_json::json!({
        "streams": [
            {"codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080, "sample_aspect_ratio": "1:1",
             "avg_frame_rate": "30000/1001", "side_data_list": [{"side_data_type": "Display Matrix", "rotation": -90}]},
            {"codec_type": "audio", "codec_name": "aac"},
            {"codec_type": "video", "codec_name": "mjpeg", "width": 600, "height": 600, "disposition": {"attached_pic": 1}}
        ],
        "format": {"duration": "12.500000"}
    });
    let probe = crate::ffmpeg::parse_probe(&json);
    assert_eq!(probe.info.duration, Some(Duration::from_millis(12_500)));
    assert_eq!(probe.info.size, Some((1080, 1920)), "rotated a quarter turn");
    assert_eq!(probe.info.codec.as_deref(), Some("H.264"));
    assert_eq!(probe.info.audio_codec.as_deref(), Some("AAC"));
    assert_eq!(probe.rate, (30000, 1001));
    assert!(probe.info.has_video && probe.info.has_audio);

    let cover = serde_json::json!({
        "streams": [
            {"codec_type": "audio", "codec_name": "mp3"},
            {"codec_type": "video", "codec_name": "png", "width": 64, "height": 48, "sample_aspect_ratio": "0:1", "disposition": {"attached_pic": 1}}
        ],
        "format": {"duration": "3.0"}
    });
    let probe = crate::ffmpeg::parse_probe(&cover);
    assert!(!probe.info.has_video, "cover art isn't video");
    assert_eq!(probe.info.size, None);
    assert_eq!(probe.picture, Some((64, 48)));
    assert_eq!(probe.info.codec.as_deref(), Some("MP3"));
}

#[test]
fn missing_decoders_say_what_to_install() {
    use crate::decode::{OpenError, no_ffmpeg};
    let hevc = no_ffmpeg(Some(OpenError::NoCodec { codec: "HEVC".into(), video: true }), "play");
    assert_eq!(hevc, "this video's codec (HEVC) isn't installed on Windows; install ffmpeg (bro install ffmpeg) to play it here, or open it in another app");
    let opus = no_ffmpeg(Some(OpenError::NoCodec { codec: "Opus".into(), video: false }), "play the sound of");
    assert!(opus.contains("audio codec (Opus)") && opus.contains("bro install ffmpeg"), "{opus}");
    assert!(no_ffmpeg(Some(OpenError::Unsupported), "play").starts_with("Windows can't read this file"));
    assert_eq!(no_ffmpeg(None, "preview"), "bro needs ffmpeg to preview this file; install ffmpeg (bro install ffmpeg)");
}

#[test]
fn public_types_cross_threads() {
    fn send<T: Send>() {}
    fn sync<T: Sync>() {}
    send::<Player>();
    sync::<Player>();
    send::<Image>();
    send::<Rgba>();
}

// ---- video and audio (ffmpeg makes the samples) ----

/// Make a sample with ffmpeg: `args` then the output path. None (with a note) without ffmpeg.
fn sample(dir: &Path, name: &str, args: &[&str]) -> Option<PathBuf> {
    let Some(tools) = crate::ffmpeg::tools() else {
        eprintln!("ffmpeg isn't installed: skipping the {name} test");
        return None;
    };
    let out = dir.join(name);
    let status = Command::new(&tools.ffmpeg).args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).arg(&out).status().expect("run ffmpeg");
    assert!(status.success(), "ffmpeg couldn't make {name}");
    Some(out)
}

/// A 160x90 [`bar_clip_sized`].
fn bar_clip(dir: &Path, name: &str, secs: u32, codecs: &[&str]) -> Option<PathBuf> {
    bar_clip_sized(dir, name, secs, (160, 90), codecs)
}

/// A white bar a twentieth of the width wide moving right an eightieth of the width a frame over
/// black (25 fps), with a 440 Hz tone: where the bar is tells which moment a frame shows.
fn bar_clip_sized(dir: &Path, name: &str, secs: u32, (w, h): (u32, u32), codecs: &[&str]) -> Option<PathBuf> {
    let (bar, step) = (w / 20, w / 80);
    let video = format!("color=c=black:s={w}x{h}:r=25:d={secs}[bg];color=c=white:s={bar}x{h}:r=25:d={secs}[bar];[bg][bar]overlay=x='(n-1)*{step}':y=0[out0]");
    let audio = format!("sine=frequency=440:duration={secs}");
    let mut args = vec!["-f", "lavfi", "-i", &video, "-f", "lavfi", "-i", &audio];
    args.extend(codecs);
    args.push("-shortest");
    sample(dir, name, &args)
}

const H264_AAC: &[&str] = &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"];

/// The moment a bar-clip frame shows, from the bar's centre on the middle row.
fn bar_time(frame: &Rgba) -> f64 {
    let (mut sum, mut n) = (0.0, 0.0);
    for x in 0..frame.w {
        let p = frame.pixel(x, frame.h / 2);
        if (p[0] as u32 + p[1] as u32 + p[2] as u32) > 3 * 128 {
            sum += x as f64 + 0.5;
            n += 1.0;
        }
    }
    // as a fraction of the width: the bar's centre starts at 1/40 and moves 1/80 a frame
    let centre = sum / f64::max(n, 1.0) / frame.w as f64;
    (centre - 1.0 / 40.0) * 80.0 / 25.0
}

fn wait_frame(player: &mut Player, limit: Duration) -> Option<Rgba> {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if let Some(frame) = player.take_frame() {
            return Some(frame);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

/// ffmpeg's own frame at `at`, scaled to w x h.
fn reference(path: &Path, at: f64, w: u32, h: u32) -> Rgba {
    let tools = crate::ffmpeg::tools().expect("ffmpeg");
    let out = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{at:.3}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}:flags=bicubic"), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .output()
        .expect("run ffmpeg");
    assert_eq!(out.stdout.len(), Rgba::byte_len(w, h), "reference frame size");
    Rgba { w, h, px: out.stdout }
}

/// Mean absolute difference per channel, 0..255.
fn mean_diff(a: &Rgba, b: &Rgba) -> f64 {
    assert_eq!((a.w, a.h), (b.w, b.h));
    a.px.iter().zip(&b.px).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>() / a.px.len() as f64
}

/// Open, poster, play, pause, seek while paused, play to the end, drop.
fn exercise(path: &Path, backend: Backend) {
    let opened = Instant::now();
    let mut player = Player::open_with(path, 128, backend).expect("open");
    let info = player.info().clone();
    println!("{backend:?}: opened in {:?}: {info:?}", opened.elapsed());
    assert!(info.has_video && info.has_audio);
    assert_eq!(info.size, Some((160, 90)));
    let duration = info.duration.expect("duration").as_secs_f64();
    assert!((duration - 2.0).abs() < 0.15, "duration {duration}");
    player.set_muted(true);

    let poster = wait_frame(&mut player, Duration::from_secs(5)).expect("poster frame before play");
    assert_eq!((poster.w, poster.h), (128, 72));
    assert!(bar_time(&poster).abs() < 0.08, "poster is the first frame: {}", bar_time(&poster));
    assert!(!player.is_playing());

    player.play();
    assert!(player.is_playing());
    let mut shown = Vec::new();
    let until = Instant::now() + Duration::from_millis(800);
    while Instant::now() < until {
        if let Some(frame) = player.take_frame() {
            shown.push((player.position().as_secs_f64(), bar_time(&frame)));
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    println!("{backend:?}: {} frames in 0.8 s, audio device: {}", shown.len(), player.audio_running());
    assert!(shown.len() >= 8, "only {} frames in 0.8 s", shown.len());
    for (position, t) in &shown {
        assert!((position - t).abs() < 0.15, "frame for {t:.3} shown at {position:.3}");
    }
    let position = player.position().as_secs_f64();
    assert!(position > 0.4 && position < 1.3, "position {position} after 0.8 s");

    player.pause();
    assert!(!player.is_playing());
    while player.take_frame().is_some() {}
    let paused_at = player.position();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(player.position(), paused_at, "the clock stops while paused");
    assert!(player.take_frame().is_none(), "no frames while paused");
    assert_eq!(player.next_wake(), None);

    player.seek(Duration::from_millis(1500));
    assert_eq!(player.position(), Duration::from_millis(1500));
    let target = wait_frame(&mut player, Duration::from_secs(3)).expect("the seek's frame shows while paused");
    assert!((bar_time(&target) - 1.5).abs() < 0.07, "seek to 1.5 showed {}", bar_time(&target));

    player.play();
    let until = Instant::now() + Duration::from_secs(4);
    while !player.ended() && Instant::now() < until {
        player.take_frame();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(player.ended(), "didn't reach the end");
    assert!(!player.is_playing());
    assert_eq!(player.position(), info.duration.expect("duration"));
    assert_eq!(player.error(), None);

    let dropping = Instant::now();
    drop(player);
    assert!(dropping.elapsed() < Duration::from_millis(400), "drop took {:?}", dropping.elapsed());
}

#[test]
fn player_plays_seeks_and_pauses() {
    let dir = tempdir();
    let Some(path) = bar_clip(dir.path(), "bar.mp4", 2, H264_AAC) else { return };
    exercise(&path, Backend::Auto);
}

#[test]
fn player_plays_seeks_and_pauses_through_ffmpeg() {
    let dir = tempdir();
    let Some(path) = bar_clip(dir.path(), "bar.mp4", 2, H264_AAC) else { return };
    exercise(&path, Backend::Ffmpeg);
}

#[test]
fn player_loops_seamlessly() {
    let dir = tempdir();
    let Some(path) = bar_clip(dir.path(), "loop.mp4", 1, H264_AAC) else { return };
    let mut player = Player::open(&path, 64).expect("open");
    player.set_muted(true);
    player.set_looping(true);
    player.play();
    let (mut last, mut wrapped, mut frames_after_wrap) = (0.0, false, 0);
    let mut previous_frame = Instant::now();
    let mut longest_gap = Duration::ZERO;
    let until = Instant::now() + Duration::from_millis(2600);
    while Instant::now() < until {
        if player.take_frame().is_some() {
            longest_gap = longest_gap.max(previous_frame.elapsed());
            previous_frame = Instant::now();
            if wrapped {
                frames_after_wrap += 1;
            }
        }
        let position = player.position().as_secs_f64();
        if position + 0.5 < last {
            wrapped = true;
        }
        last = position;
        assert!(!player.ended(), "a looping player never ends");
        std::thread::sleep(Duration::from_millis(4));
    }
    println!("loop: longest gap between frames {longest_gap:?}");
    assert!(wrapped, "position never wrapped");
    assert!(frames_after_wrap >= 10, "{frames_after_wrap} frames after the loop");
    assert!(longest_gap < Duration::from_millis(250), "the loop stalled for {longest_gap:?}");
}

#[test]
fn drop_while_playing_is_prompt() {
    let dir = tempdir();
    let Some(path) = bar_clip(dir.path(), "drop.mp4", 2, H264_AAC) else { return };
    for backend in [Backend::Auto, Backend::Ffmpeg] {
        let mut player = Player::open_with(&path, 128, backend).expect("open");
        player.set_muted(true);
        player.play();
        std::thread::sleep(Duration::from_millis(150));
        // a scrub burst right before the drop
        for ms in [300, 900, 1200, 600] {
            player.seek(Duration::from_millis(ms));
        }
        let dropping = Instant::now();
        drop(player);
        assert!(dropping.elapsed() < Duration::from_millis(400), "{backend:?}: drop took {:?}", dropping.elapsed());
    }
}

#[test]
fn video_thumbnail_is_ten_percent_in() {
    let dir = tempdir();
    let Some(path) = bar_clip(dir.path(), "thumb.mp4", 2, H264_AAC) else { return };
    let thumb = thumbnail(&path, 64).expect("thumbnail");
    assert_eq!((thumb.w, thumb.h), (64, 36));
    assert!((bar_time(&thumb) - 0.2).abs() < 0.1, "thumbnail shows {}", bar_time(&thumb));
    assert!(mean_diff(&thumb, &reference(&path, 0.2, 64, 36)) < 12.0);
}

#[test]
fn rotated_video_shows_upright() {
    let dir = tempdir();
    let Some(clip) = bar_clip(dir.path(), "flat.mp4", 2, H264_AAC) else { return };
    let clip = clip.to_string_lossy().into_owned();
    let Some(path) = sample(dir.path(), "rotated.mp4", &["-display_rotation", "90", "-i", &clip, "-c", "copy"]) else { return };
    let mut player = Player::open(&path, 128).expect("open");
    assert_eq!(player.info().size, Some((90, 160)));
    let poster = wait_frame(&mut player, Duration::from_secs(5)).expect("poster");
    assert_eq!((poster.w, poster.h), (72, 128));
    let diff = mean_diff(&poster, &reference(&path, 0.0, 72, 128));
    assert!(diff < 12.0, "rotated frame differs from ffmpeg's by {diff}");
}

#[test]
fn odd_aspect_fills_the_frame() {
    // 1000x562 doesn't scale to whole even pixels: no letterbox lines may appear at the edges.
    let dir = tempdir();
    let Some(path) = sample(dir.path(), "wide.mp4", &["-f", "lavfi", "-i", "testsrc=duration=1:size=1000x562:rate=25", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
    else {
        return;
    };
    let mut player = Player::open(&path, 481).expect("open");
    let frame = wait_frame(&mut player, Duration::from_secs(5)).expect("poster");
    println!("{}x{} via {}", frame.w, frame.h, player.info().backend);
    assert!(frame.w >= 480 && frame.h >= 269 && frame.w <= 481 && frame.h <= 271);
    let reference = reference(&path, 0.0, frame.w, frame.h);
    for (name, rows) in [("top", 0..2), ("bottom", frame.h - 2..frame.h)] {
        let at = |img: &Rgba| rows.clone().flat_map(|y| (0..img.w).map(move |x| (x, y))).map(|(x, y)| img.pixel(x, y)).collect::<Vec<_>>();
        let (ours, theirs) = (at(&frame), at(&reference));
        let diff = ours.iter().zip(&theirs).map(|(a, b)| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>()).sum::<f64>() / (ours.len() * 4) as f64;
        assert!(diff < 25.0, "{name} edge differs from ffmpeg's by {diff} (letterboxing?)");
    }
}

#[test]
fn audio_only_files_play_to_the_end() {
    let dir = tempdir();
    let Some(path) = sample(dir.path(), "tone.m4a", &["-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "aac"]) else { return };
    let mut player = Player::open(&path, 64).expect("open");
    let info = player.info().clone();
    assert!(!info.has_video && info.has_audio, "{info:?}");
    assert_eq!(info.size, None);
    player.set_muted(true);
    assert!(player.take_frame().is_none());
    assert_eq!(player.next_wake(), None);
    player.play();
    std::thread::sleep(Duration::from_millis(400));
    let position = player.position().as_secs_f64();
    assert!(position > 0.1 && position < 0.6, "position {position} after 0.4 s");
    let until = Instant::now() + Duration::from_secs(3);
    while !player.ended() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(player.ended());
    assert_eq!(player.error(), None);
    let thumb = thumbnail(&path, 64).unwrap_err();
    assert!(thumb.contains("no picture"), "{thumb}");
}

#[test]
fn webm_plays_whatever_decodes_it() {
    let dir = tempdir();
    let codecs = ["-c:v", "libvpx-vp9", "-b:v", "200k", "-c:a", "libopus"];
    let Some(path) = bar_clip(dir.path(), "bar.webm", 2, &codecs) else { return };
    let mut player = Player::open(&path, 128).expect("open");
    println!("webm via {}: {:?}", player.info().backend, player.info());
    assert_eq!(player.info().codec.as_deref(), Some("VP9"));
    let poster = wait_frame(&mut player, Duration::from_secs(5)).expect("poster");
    assert!(bar_time(&poster).abs() < 0.08);
    player.set_muted(true);
    player.play();
    // the clock waits for the audio device to start
    let started = Instant::now();
    while player.position() < Duration::from_millis(40) && started.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(4));
    }
    let mut frames = 0;
    let until = Instant::now() + Duration::from_millis(600);
    while Instant::now() < until {
        frames += player.take_frame().is_some() as u32;
        std::thread::sleep(Duration::from_millis(4));
    }
    assert!(frames >= 8, "{frames} frames in 0.6 s");
    assert_eq!(player.error(), None);
}

#[test]
fn animated_webp_and_cover_art() {
    let dir = tempdir();
    let Some(webp) = sample(dir.path(), "anim.webp", &["-f", "lavfi", "-i", "testsrc=duration=1:size=96x64:rate=10", "-c:v", "libwebp_anim", "-loop", "0"]) else {
        return;
    };
    let loaded = load_image(&webp, 48).expect("animated webp");
    assert_eq!(loaded.format, "WebP");
    assert_eq!(loaded.frames.len(), 10);
    assert_eq!((loaded.frames[0].0.w, loaded.frames[0].0.h), (48, 32));
    // an MP3 with a cover picture: the picture is its thumbnail
    let Some(cover) = sample(dir.path(), "cover.png", &["-f", "lavfi", "-i", "color=c=red:s=64x48", "-frames:v", "1"]) else { return };
    let cover = cover.to_string_lossy().into_owned();
    let args = ["-f", "lavfi", "-i", "sine=duration=1", "-i", &cover, "-map", "0:a", "-map", "1:v", "-c:a", "libmp3lame", "-c:v", "png", "-disposition:v", "attached_pic"];
    let Some(mp3) = sample(dir.path(), "song.mp3", &args) else { return };
    let thumb = thumbnail(&mp3, 32).expect("cover art thumbnail");
    assert_eq!((thumb.w, thumb.h), (32, 24));
    let p = thumb.pixel(16, 12);
    assert!(p[0] > 200 && p[1] < 60, "cover is red: {p:?}");
}

#[cfg(windows)]
#[test]
fn avif_decodes_through_windows() {
    let dir = tempdir();
    let Some(path) = sample(dir.path(), "still.avif", &["-f", "lavfi", "-i", "color=c=blue:s=96x64", "-frames:v", "1", "-c:v", "libaom-av1", "-still-picture", "1"]) else {
        return;
    };
    match load_image(&path, 48) {
        Ok(loaded) => {
            assert_eq!(loaded.format, "AVIF");
            assert_eq!(loaded.original, (96, 64));
            let frame = &loaded.frames[0].0;
            assert_eq!((frame.w, frame.h), (48, 32));
            let p = frame.pixel(24, 16);
            assert!(p[2] > 200 && p[0] < 60, "blue: {p:?}");
        }
        // Not a failure of ours: the Store's AV1 Video Extension isn't on this machine.
        Err(e) if e.contains("AV1 Video Extension") => eprintln!("skipping: {e}"),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn ffmpeg_reads_stills_the_image_crate_cant() {
    let dir = tempdir();
    let Some(path) = sample(dir.path(), "still.avif", &["-f", "lavfi", "-i", "color=c=blue:s=96x64", "-frames:v", "1", "-c:v", "libaom-av1", "-still-picture", "1"]) else {
        return;
    };
    let loaded = crate::still::ffmpeg_still(&path, "avif", 48).expect("ffmpeg decodes AVIF");
    assert_eq!(loaded.format, "AVIF");
    assert_eq!(loaded.original, (96, 64));
    let frame = &loaded.frames[0].0;
    assert_eq!((frame.w, frame.h), (48, 32));
    let p = frame.pixel(24, 16);
    assert!(p[2] > 200 && p[0] < 60, "blue: {p:?}");
}

#[test]
fn frames_arrive_in_order_with_their_times() {
    let dir = tempdir();
    let encoders: [(&str, &[&str]); 3] = [
        ("h264.mp4", &["-c:v", "libx264", "-bf", "3", "-pix_fmt", "yuv420p", "-c:a", "aac"]),
        ("hevc.mp4", &["-c:v", "libx265", "-pix_fmt", "yuv420p", "-x265-params", "log-level=error:bframes=4", "-tag:v", "hvc1", "-c:a", "aac"]),
        ("av1.mp4", &["-c:v", "libaom-av1", "-cpu-used", "8", "-b:v", "200k", "-c:a", "aac"]),
    ];
    for (name, codecs) in encoders {
        let Some(path) = bar_clip(dir.path(), name, 2, codecs) else { return };
        // full size: no scaling blur, the bar lands on whole pixels
        let mut opened = match crate::decode::open_video(&path, 160, Backend::Auto) {
            Ok(opened) => opened,
            Err(e) => {
                eprintln!("{name}: no decoder here ({e}), skipping");
                continue;
            }
        };
        let video = opened.video.as_mut().expect("video");
        let mut seen = Vec::new();
        while let Some((t, frame)) = video.next_frame().expect("decode") {
            seen.push((t, bar_time(&frame)));
        }
        println!("{name} via {}: {} frames", opened.info.backend, seen.len());
        assert!(seen.len() >= 48, "{name}: {} frames", seen.len());
        for pair in seen.windows(2) {
            assert!(pair[1].0 > pair[0].0, "{name}: timestamps go backwards: {pair:?}");
        }
        for (t, bar) in &seen {
            assert!((t - bar).abs() < 0.015, "{name}: frame stamped {t:.3} shows {bar:.3}");
        }
        // a seek lands on the target frame, not the key frame before it
        video.seek(1.2).expect("seek");
        let first = std::iter::from_fn(|| video.next_frame().expect("decode")).find(|(t, _)| *t + 0.02 >= 1.2).expect("frame after seek");
        assert!((bar_time(&first.1) - first.0).abs() < 0.015 && (first.0 - 1.2).abs() < 0.015, "{name}: seek gave {:.3}", first.0);
    }
}

#[test]
fn large_video_plays_and_seeks() {
    // Over 720p: on Windows this takes the GPU path when the machine has a GPU.
    let dir = tempdir();
    let codecs = ["-c:v", "libx264", "-preset", "veryfast", "-bf", "3", "-pix_fmt", "yuv420p", "-c:a", "aac"];
    let Some(path) = bar_clip_sized(dir.path(), "hd.mp4", 2, (1920, 1080), &codecs) else { return };
    let mut player = Player::open(&path, 480).expect("open");
    println!("1080p via {}: {:?}", player.info().backend, player.info());
    assert_eq!(player.info().size, Some((1920, 1080)));
    let poster = wait_frame(&mut player, Duration::from_secs(5)).expect("poster");
    assert_eq!((poster.w, poster.h), (480, 270));
    assert!(bar_time(&poster).abs() < 0.03, "poster shows {}", bar_time(&poster));
    player.seek(Duration::from_millis(1200));
    let frame = wait_frame(&mut player, Duration::from_secs(5)).expect("frame after seek");
    assert!((bar_time(&frame) - 1.2).abs() < 0.03, "seek to 1.2 showed {}", bar_time(&frame));
    let diff = mean_diff(&frame, &reference(&path, 1.2, 480, 270));
    assert!(diff < 8.0, "frame differs from ffmpeg's by {diff}");
    player.set_muted(true);
    player.play();
    let started = Instant::now();
    while player.position() < Duration::from_millis(1240) && started.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(4));
    }
    let mut frames = 0;
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        if let Some(frame) = player.take_frame() {
            frames += 1;
            let (t, at) = (bar_time(&frame), player.position().as_secs_f64());
            assert!((t - at).abs() < 0.12, "frame for {t:.3} shown at {at:.3}");
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    assert!(frames >= 6, "{frames} frames in 0.5 s");
    assert_eq!(player.error(), None);
}

#[test]
fn large_frames_arrive_in_order() {
    let dir = tempdir();
    let encoders: [(&str, &[&str]); 2] = [
        ("hd264.mp4", &["-c:v", "libx264", "-preset", "veryfast", "-bf", "3", "-pix_fmt", "yuv420p", "-an"]),
        ("hd265.mp4", &["-c:v", "libx265", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-x265-params", "log-level=error:bframes=3", "-tag:v", "hvc1", "-an"]),
    ];
    for (name, codecs) in encoders {
        let Some(path) = bar_clip_sized(dir.path(), name, 1, (1920, 1080), codecs) else { return };
        let mut opened = match crate::decode::open_video(&path, 960, Backend::Auto) {
            Ok(opened) => opened,
            Err(e) => {
                eprintln!("{name}: no decoder here ({e}), skipping");
                continue;
            }
        };
        let video = opened.video.as_mut().expect("video");
        let mut seen = Vec::new();
        while let Some((t, frame)) = video.next_frame().expect("decode") {
            assert_eq!((frame.w, frame.h), (960, 540));
            seen.push((t, bar_time(&frame)));
        }
        println!("{name} via {}: {} frames", opened.info.backend, seen.len());
        assert!(seen.len() >= 24, "{name}: {} frames", seen.len());
        for pair in seen.windows(2) {
            assert!(pair[1].0 > pair[0].0, "{name}: timestamps go backwards: {pair:?}");
        }
        for (t, bar) in &seen {
            assert!((t - bar).abs() < 0.015, "{name}: frame stamped {t:.3} shows {bar:.3}");
        }
    }
}
