//! Still and animated images for previews: the `image` crate for raster formats, resvg for SVG,
//! and on Windows the Windows Imaging Component for AVIF, HEIC and anything else `image` can't
//! read (when the matching Store codec is installed).

use crate::Rgba;
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::{self, FilterType};
use image::metadata::Orientation;
use image::{AnimationDecoder, DynamicImage, Frames, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use resvg::{tiny_skia, usvg};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Animations stop decoding after this many frames...
const MAX_FRAMES: usize = 600;
/// ...or once their downscaled frames add up to this many bytes...
const MAX_ANIMATION_BYTES: usize = 256 << 20;
/// ...or after this long (the frames decoded so far play).
const MAX_ANIMATION_TIME: Duration = Duration::from_secs(8);
/// The largest side ever returned, whatever the caller asks for.
const MAX_SIDE: u32 = 8192;
/// The most one decoded picture (or an animation's canvases) may take in memory: a file that claims a huge
/// size (a few bytes can claim 60000 x 60000) fails cleanly instead of allocating gigabytes or aborting.
/// On Windows the system decoder then gets a try, scaling while it decodes.
const MAX_DECODE_BYTES: u64 = 1 << 30;
const TOO_LARGE: &str = "the image is too large to preview";

/// The `image` crate's limits for every decoder here.
fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    limits
}

/// Refuse a `w` x `h` picture needing more than [`MAX_DECODE_BYTES`] at `bytes_per_px`.
fn check_size(w: u32, h: u32, bytes_per_px: u64) -> Result<(), String> {
    if w as u64 * h as u64 * bytes_per_px > MAX_DECODE_BYTES { Err(TOO_LARGE.into()) } else { Ok(()) }
}

/// A decoded image, downscaled for display.
#[derive(Clone)]
pub struct Image {
    /// One frame for stills; every frame with its delay for animated GIF / WebP / APNG.
    pub frames: Vec<(Rgba, Duration)>,
    /// Size of the original before any downscale (after EXIF orientation).
    pub original: (u32, u32),
    /// "PNG", "JPEG", "GIF", "WebP", "SVG", "BMP", "ICO", "TIFF", "AVIF"...
    pub format: String,
}

impl Image {
    /// More than one frame to cycle through.
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image").field("format", &self.format).field("original", &self.original).field("frames", &self.frames.len()).finish()
    }
}

/// Decode an image, downscaled (high-quality filter) so neither side exceeds `max_side`; honour EXIF orientation;
/// SVG rasterised with resvg at a size fitting `max_side`. Formats: png, jpg/jpeg, gif (animated), webp (animated),
/// bmp, ico, tiff, svg, and avif/heic via the Windows Imaging Component on Windows when the image crate can't.
pub fn load_image(path: &Path, max_side: u32) -> Result<Image, String> {
    let max_side = max_side.clamp(1, MAX_SIDE);
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut reader = ImageReader::open(path).map_err(|e| read_error(path, e))?.with_guessed_format().map_err(|e| read_error(path, e))?;
    reader.limits(limits());
    let format = reader.format();
    if format.is_none() && (ext == "svg" || ext == "svgz" || svg_file(path)) {
        return load_svg(path, max_side);
    }
    let decoded = match format {
        Some(ImageFormat::Gif) => gif(path, max_side),
        Some(ImageFormat::WebP) => webp(path, max_side),
        Some(ImageFormat::Png) => png(path, max_side),
        Some(f) if f.reading_enabled() => reader.into_decoder().map_err(decode_error).and_then(|d| still(d, format_name(f), max_side)),
        _ => Err(format!("can't preview {} images", if ext.is_empty() { "these".to_string() } else { format!(".{ext}") })),
    };
    #[cfg(windows)]
    let decoded = match decoded {
        // AVIF, HEIC, JPEG XR, camera RAW...: Windows decodes them when the Store codec is there.
        Err(e) => crate::win::wic_load(path, max_side)
            .map(|(rgba, original, container)| Image { frames: vec![(rgba, Duration::ZERO)], original, format: wic_format(&ext, &container) })
            .map_err(|wic| if format.is_some_and(|f| f.reading_enabled()) { e } else { wic }),
        ok => ok,
    };
    // AVIF, HEIC and JPEG XL elsewhere (or without the Store codec): ffmpeg reads them when it's installed.
    match decoded {
        Err(e) if FFMPEG_STILLS.contains(&ext.as_str()) => ffmpeg_still(path, &ext, max_side).map_err(|_| {
            // Windows' own message already says which Store codec to add
            if cfg!(not(windows)) && crate::ffmpeg::tools().is_none() {
                format!("bro needs ffmpeg to preview .{ext} images; install ffmpeg (bro install ffmpeg)")
            } else {
                e
            }
        }),
        decoded => decoded,
    }
}

/// Still formats the `image` crate can't read that ffmpeg can.
const FFMPEG_STILLS: &[&str] = &["avif", "heic", "heif", "hif", "jxl"];

/// A still decoded by ffmpeg, fitted inside `max_side`.
pub(crate) fn ffmpeg_still(path: &Path, ext: &str, max_side: u32) -> Result<Image, String> {
    let tools = crate::ffmpeg::tools().ok_or("ffmpeg isn't installed")?;
    let (frame, original) = crate::ffmpeg::still(&tools, path, max_side)?;
    let format = match ext {
        "heic" => "HEIC".to_string(),
        "heif" | "hif" => "HEIF".to_string(),
        "jxl" => "JPEG XL".to_string(),
        ext => ext.to_ascii_uppercase(),
    };
    Ok(Image { frames: vec![(frame, Duration::ZERO)], original, format })
}

fn read_error(path: &Path, e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound { format!("{} doesn't exist", path.display()) } else { format!("can't read {}: {e}", path.display()) }
}

fn decode_error(e: image::ImageError) -> String {
    match e {
        image::ImageError::Limits(_) => "the image is too large to preview".into(),
        e => format!("can't decode this image: {e}"),
    }
}

fn format_name(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Avif => "AVIF",
        _ => "image",
    }
}

#[cfg(windows)]
fn wic_format(ext: &str, container: &str) -> String {
    match ext {
        "avif" => "AVIF".into(),
        "heic" => "HEIC".into(),
        "heif" | "hif" => "HEIF".into(),
        "jxr" | "wdp" | "hdp" => "JPEG XR".into(),
        "jxl" => "JPEG XL".into(),
        "dng" | "cr2" | "cr3" | "nef" | "arw" | "orf" | "rw2" | "raf" | "srw" | "pef" => "RAW".into(),
        _ if !container.is_empty() => container.into(),
        _ => ext.to_ascii_uppercase(),
    }
}

fn svg_file(path: &Path) -> bool {
    use std::io::Read;
    let mut head = Vec::with_capacity(1024);
    File::open(path).and_then(|f| f.take(1024).read_to_end(&mut head)).is_ok() && crate::kind::is_svg(&head)
}

fn still(mut decoder: impl ImageDecoder, format: &str, max_side: u32) -> Result<Image, String> {
    // `from_decoder` allocates the whole picture up front, unchecked: check it here
    if decoder.total_bytes() > MAX_DECODE_BYTES {
        return Err(TOO_LARGE.into());
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(decode_error)?;
    img.apply_orientation(orientation);
    let original = (img.width(), img.height());
    Ok(Image { frames: vec![(downscale_image(img, max_side), Duration::ZERO)], original, format: format.into() })
}

fn open(path: &Path) -> Result<BufReader<File>, String> {
    File::open(path).map(BufReader::new).map_err(|e| read_error(path, e))
}

/// The file is an animation: a GIF with a second frame, an animated WebP, an APNG. Reads no further than it needs
/// to tell (a GIF's first two frames, the others' headers). A GIF too big to look into counts as one.
pub fn is_animated(path: &Path) -> bool {
    let format = ImageReader::open(path).and_then(|r| r.with_guessed_format()).ok().and_then(|r| r.format());
    let Ok(file) = open(path) else { return false };
    match format {
        Some(ImageFormat::Gif) => GifDecoder::new(file).is_ok_and(|mut d| {
            let (w, h) = d.dimensions();
            check_size(w, h, CANVAS_BYTES_PER_PX).is_err() || (d.set_limits(limits()).is_ok() && d.into_frames().take(2).count() > 1)
        }),
        Some(ImageFormat::WebP) => WebPDecoder::new(file).is_ok_and(|d| d.has_animation()),
        Some(ImageFormat::Png) => PngDecoder::new(file).is_ok_and(|d| d.is_apng().unwrap_or(false)),
        _ => false,
    }
}

/// An animation's canvas, the frame being composed and the one kept for disposal: about 8 bytes a pixel.
const CANVAS_BYTES_PER_PX: u64 = 8;

fn gif(path: &Path, max_side: u32) -> Result<Image, String> {
    let mut decoder = GifDecoder::new(open(path)?).map_err(decode_error)?;
    decoder.set_limits(limits()).map_err(decode_error)?;
    let original = decoder.dimensions();
    check_size(original.0, original.1, CANVAS_BYTES_PER_PX)?;
    Ok(Image { frames: collect_frames(decoder.into_frames(), max_side)?, original, format: "GIF".into() })
}

fn webp(path: &Path, max_side: u32) -> Result<Image, String> {
    let decoder = WebPDecoder::new(open(path)?).map_err(decode_error)?;
    if !decoder.has_animation() {
        return still(decoder, "WebP", max_side);
    }
    let original = decoder.dimensions();
    check_size(original.0, original.1, CANVAS_BYTES_PER_PX)?;
    Ok(Image { frames: collect_frames(decoder.into_frames(), max_side)?, original, format: "WebP".into() })
}

fn png(path: &Path, max_side: u32) -> Result<Image, String> {
    let decoder = PngDecoder::with_limits(open(path)?, limits()).map_err(decode_error)?;
    if !decoder.is_apng().unwrap_or(false) {
        return still(decoder, "PNG", max_side);
    }
    let original = decoder.dimensions();
    check_size(original.0, original.1, CANVAS_BYTES_PER_PX)?;
    let frames = collect_frames(decoder.apng().map_err(decode_error)?.into_frames(), max_side)?;
    Ok(Image { frames, original, format: "APNG".into() })
}

/// Every frame of an animation, downscaled, within [`MAX_FRAMES`] / [`MAX_ANIMATION_BYTES`] /
/// [`MAX_ANIMATION_TIME`]. A file that breaks part-way keeps the frames before the break.
fn collect_frames(frames: Frames<'_>, max_side: u32) -> Result<Vec<(Rgba, Duration)>, String> {
    let mut out = Vec::new();
    let mut bytes = 0;
    let started = std::time::Instant::now();
    for frame in frames {
        if !out.is_empty() && started.elapsed() > MAX_ANIMATION_TIME {
            break;
        }
        let frame = match frame {
            Ok(frame) => frame,
            Err(_) if !out.is_empty() => break,
            Err(e) => return Err(decode_error(e)),
        };
        let delay = frame_delay(frame.delay());
        let rgba = downscale_rgba(frame.into_buffer(), max_side, FilterType::CatmullRom);
        bytes += rgba.px.len();
        out.push((rgba, delay));
        if out.len() >= MAX_FRAMES || bytes >= MAX_ANIMATION_BYTES {
            break;
        }
    }
    if out.is_empty() {
        return Err("the animation has no frames".into());
    }
    Ok(out)
}

/// Browsers play delays of 10 ms and less at 100 ms, and animations are authored against that.
fn frame_delay(delay: image::Delay) -> Duration {
    let (num, den) = delay.numer_denom_ms();
    let ms = num as f64 / den.max(1) as f64;
    Duration::from_secs_f64(if ms <= 10.0 { 0.1 } else { ms / 1000.0 })
}

// ---- SVG ----

/// SVGs are read whole and parsed into a tree: bigger ones aren't drawings.
const MAX_SVG_BYTES: u64 = 64 << 20;
/// A gzipped SVG (svgz) is unpacked whole too, and deflate packs up to ~1000:1: this keeps the unpacked
/// text within about [`MAX_DECODE_BYTES`].
const MAX_SVGZ_BYTES: usize = 1 << 20;

fn load_svg(path: &Path, max_side: u32) -> Result<Image, String> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_SVG_BYTES) {
        return Err("the SVG is too large to preview".into());
    }
    let data = std::fs::read(path).map_err(|e| read_error(path, e))?;
    if data.starts_with(&[0x1f, 0x8b]) && data.len() > MAX_SVGZ_BYTES {
        return Err("the SVG is too large to preview".into());
    }
    // Self-contained only, as a browser shows an SVG under a sandbox: pictures embedded as data: URLs draw,
    // other files the SVG points at are never read (usvg runs no scripts and fetches nothing from the network).
    let mut opt = usvg::Options { resources_dir: None, ..Default::default() };
    opt.image_href_resolver = usvg::ImageHrefResolver { resolve_data: usvg::ImageHrefResolver::default_data_resolver(), resolve_string: Box::new(|_, _| None) };
    // Loading the system fonts takes a while; only SVGs with text need them.
    if data.starts_with(&[0x1f, 0x8b]) || data.windows(5).any(|w| w == b"<text") {
        opt.fontdb = system_fonts();
    }
    let tree = parse_svg(&data, &opt, MAX_SVG_NODES)?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    let scale = max_side as f32 / w.max(h);
    let pw = ((w * scale).round() as u32).clamp(1, MAX_SIDE);
    let ph = ((h * scale).round() as u32).clamp(1, MAX_SIDE);
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or("the SVG is too large to render")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(pw as f32 / w, ph as f32 / h), &mut pixmap.as_mut());
    let original = ((w.round() as u32).max(1), (h.round() as u32).max(1));
    Ok(Image { frames: vec![(Rgba { w: pw, h: ph, px: pixmap.take_demultiplied() }, Duration::ZERO)], original, format: "SVG".into() })
}

/// More XML nodes than any drawing needs. The file caps alone don't bound the tree: a few MB of `<g/>` (or a
/// 1 MB svgz of them) would parse into gigabytes of nodes and abort the app.
const MAX_SVG_NODES: u32 = 2_000_000;

/// Parse SVG `data` (gzipped or not) into a tree, refusing documents with more than `nodes_limit` XML nodes.
fn parse_svg(data: &[u8], opt: &usvg::Options, nodes_limit: u32) -> Result<usvg::Tree, String> {
    let unpacked;
    let data = if data.starts_with(&[0x1f, 0x8b]) {
        unpacked = usvg::decompress_svgz(data).map_err(|e| format!("can't read this SVG: {e}"))?;
        &unpacked[..]
    } else {
        data
    };
    let text = std::str::from_utf8(data).map_err(|_| "can't read this SVG: it isn't UTF-8 text".to_string())?;
    let xml = usvg::roxmltree::ParsingOptions { allow_dtd: true, nodes_limit, ..Default::default() };
    let doc = usvg::roxmltree::Document::parse_with_options(text, xml).map_err(|e| match e {
        usvg::roxmltree::Error::NodesLimitReached => "the SVG is too complex to preview".to_string(),
        e => format!("can't read this SVG: {e}"),
    })?;
    usvg::Tree::from_xmltree(&doc, opt).map_err(|e| format!("can't read this SVG: {e}"))
}

fn system_fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

// ---- resampling ----

/// `w` x `h` fitted inside `max_side` x `max_side`, keeping the aspect; never upscales.
pub(crate) fn fit(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max_side {
        return (w.max(1), h.max(1));
    }
    let s = max_side as f64 / long as f64;
    (((w as f64 * s).round() as u32).clamp(1, max_side), ((h as f64 * s).round() as u32).clamp(1, max_side))
}

/// Downscale in the decoded pixel type when it's opaque (no full-size RGBA copy of a 48 MP photo),
/// through premultiplied RGBA when it isn't.
fn downscale_image(img: DynamicImage, max_side: u32) -> Rgba {
    let (w, h) = (img.width(), img.height());
    let (nw, nh) = fit(w, h, max_side);
    if img.color().has_alpha() || (nw, nh) == (w, h) {
        return downscale_rgba(img.into_rgba8(), max_side, FilterType::Lanczos3);
    }
    let img = if w >= nw * 4 && h >= nh * 4 { img.thumbnail_exact(nw * 2, nh * 2) } else { img };
    from_image(img.resize_exact(nw, nh, FilterType::Lanczos3).into_rgba8())
}

/// High-quality downscale of straight-alpha RGBA. Large reductions box-filter to twice the target
/// first: a wide kernel over every source pixel is what makes naive resizing slow.
pub(crate) fn downscale_rgba(mut img: RgbaImage, max_side: u32, filter: FilterType) -> Rgba {
    let (w, h) = img.dimensions();
    let (nw, nh) = fit(w, h, max_side);
    if (nw, nh) == (w, h) {
        return from_image(img);
    }
    // The filters assume premultiplied alpha; without it transparent pixels bleed their colour.
    let translucent = img.pixels().any(|p| p[3] != 255);
    if translucent {
        premultiply(&mut img);
    }
    if w >= nw * 4 && h >= nh * 4 {
        img = imageops::thumbnail(&img, nw * 2, nh * 2);
    }
    let mut out = imageops::resize(&img, nw, nh, filter);
    if translucent {
        unpremultiply(&mut out);
    }
    from_image(out)
}

/// Resize an opaque frame (video) to exactly `nw` x `nh`.
#[cfg(windows)]
pub(crate) fn resize_opaque(frame: Rgba, nw: u32, nh: u32) -> Rgba {
    if (frame.w, frame.h) == (nw, nh) {
        return frame;
    }
    let Some(img) = RgbaImage::from_raw(frame.w, frame.h, frame.px) else {
        return Rgba { w: nw, h: nh, px: vec![0; Rgba::byte_len(nw, nh)] };
    };
    let img = if frame.w >= nw * 4 && frame.h >= nh * 4 { imageops::thumbnail(&img, nw * 2, nh * 2) } else { img };
    from_image(imageops::resize(&img, nw, nh, FilterType::CatmullRom))
}

fn premultiply(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let a = p[3] as u16;
        for c in &mut p.0[..3] {
            *c = ((*c as u16 * a + 127) / 255) as u8;
        }
    }
}

fn unpremultiply(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let a = p[3] as u16;
        if a == 0 {
            p.0[..3].fill(0);
        } else if a < 255 {
            for c in &mut p.0[..3] {
                *c = ((*c as u16 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

pub(crate) fn from_image(img: RgbaImage) -> Rgba {
    let (w, h) = img.dimensions();
    Rgba { w, h, px: img.into_raw() }
}

/// A frame turned clockwise by `degrees` (0, 90, 180 or 270).
#[cfg(windows)]
pub(crate) fn rotate(frame: Rgba, degrees: u32) -> Rgba {
    let degrees = degrees % 360;
    if !matches!(degrees, 90 | 180 | 270) {
        return frame;
    }
    let Some(img) = RgbaImage::from_raw(frame.w, frame.h, frame.px) else {
        return Rgba::default();
    };
    from_image(match degrees {
        90 => imageops::rotate90(&img),
        180 => imageops::rotate180(&img),
        _ => imageops::rotate270(&img),
    })
}

#[cfg(test)]
mod limits {
    use super::*;

    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend(kind);
        c.extend(data);
        let mut crc_in = kind.to_vec();
        crc_in.extend(data);
        c.extend(crc32(&crc_in).to_be_bytes());
        c
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for b in bytes {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn a_tiny_file_claiming_a_huge_picture_fails_fast_without_allocating_it() {
        let dir = tempfile::tempdir().unwrap();
        // 75 bytes claiming 60000 x 60000 RGBA (14 GB decoded)
        let mut ihdr = 60000u32.to_be_bytes().to_vec();
        ihdr.extend(60000u32.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        let mut png_bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png_bytes.extend(chunk(b"IHDR", &ihdr));
        // a zlib stream holding ten zero bytes (one stored block)
        png_bytes.extend(chunk(b"IDAT", &[0x78, 0x01, 0x01, 0x0a, 0x00, 0xf5, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x0a, 0x00, 0x01]));
        png_bytes.extend(chunk(b"IEND", &[]));
        let p = dir.path().join("bomb.png");
        std::fs::write(&p, &png_bytes).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(png(&p, 256).unwrap_err(), TOO_LARGE);
        // a GIF whose canvas is 65535 x 65535 around a 1 x 1 frame (17 GB of canvas)
        let mut gif_bytes = b"GIF89a".to_vec();
        gif_bytes.extend(65535u16.to_le_bytes());
        gif_bytes.extend(65535u16.to_le_bytes());
        gif_bytes.extend([0x80, 0, 0, 0, 0, 0, 255, 255, 255, 0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 0x01, 0, 0x3b]);
        let g = dir.path().join("bomb.gif");
        std::fs::write(&g, &gif_bytes).unwrap();
        assert_eq!(gif(&g, 256).unwrap_err(), TOO_LARGE);
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
        // the ordinary sizes still decode
        assert!(check_size(16384, 16384, 4).is_ok());
        assert!(check_size(16384, 16385, 4).is_err());
    }

    #[test]
    fn svg_never_reads_the_files_it_points_at() {
        let dir = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255])).save(dir.path().join("red.png")).unwrap();
        let abs = dir.path().join("red.png").to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
            <rect width="40" height="20" fill="#0000ff"/>
            <image href="red.png" x="0" y="0" width="20" height="20"/>
            <image href="{abs}" x="20" y="0" width="20" height="20"/>
            </svg>"##
        );
        let p = dir.path().join("refs.svg");
        std::fs::write(&p, svg).unwrap();
        let loaded = load_image(&p, 40).unwrap();
        let f = &loaded.frames[0].0;
        assert_eq!(f.pixel(10, 10), [0, 0, 255, 255], "a relative reference stayed out");
        assert_eq!(f.pixel(30, 10), [0, 0, 255, 255], "an absolute one too");
        // a picture embedded in the SVG itself still draws
        let mut png_bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 255, 0, 255])).write_to(&mut png_bytes, ImageFormat::Png).unwrap();
        let b64 = base64(&png_bytes.into_inner());
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image href="data:image/png;base64,{b64}" width="20" height="20"/></svg>"#);
        std::fs::write(&p, svg).unwrap();
        let f = &load_image(&p, 20).unwrap().frames[0].0;
        assert_eq!(f.pixel(10, 10), [0, 255, 0, 255]);
    }

    #[test]
    fn an_svg_with_too_many_nodes_is_refused_and_svgz_still_reads() {
        let opt = usvg::Options::default();
        let small = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;
        assert!(parse_svg(small.as_bytes(), &opt, 100).is_ok());
        let many = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}</svg>"#, "<g/>".repeat(500));
        assert_eq!(parse_svg(many.as_bytes(), &opt, 100).err().as_deref(), Some("the SVG is too complex to preview"));
        assert!(parse_svg(many.as_bytes(), &opt, 1000).is_ok());
        // gzipped (svgz): unpacked, then held to the same limit
        let gz = gzip_stored(many.as_bytes());
        assert!(parse_svg(&gz, &opt, 1000).is_ok());
        assert_eq!(parse_svg(&gz, &opt, 100).err().as_deref(), Some("the SVG is too complex to preview"));
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pic.svgz");
        std::fs::write(&p, gzip_stored(small.as_bytes())).unwrap();
        assert_eq!(load_image(&p, 10).unwrap().format, "SVG");
    }

    /// `data` as a gzip file holding one stored (uncompressed) deflate block.
    fn gzip_stored(data: &[u8]) -> Vec<u8> {
        assert!(data.len() <= 0xffff);
        let crc = !data.iter().fold(!0u32, |mut c, &b| {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
            c
        });
        let len = data.len() as u16;
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff, 1];
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out
    }

    fn base64(bytes: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in bytes.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}
