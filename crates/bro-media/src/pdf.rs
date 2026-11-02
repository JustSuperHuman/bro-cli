//! PDF pages as pictures, drawn by hayro: pure Rust, so a page looks the same on Windows, macOS and Linux with
//! nothing to install. A page comes back like any still ([`crate::Image`]) with its place in the document
//! ([`crate::Image::page`]), so it zooms, pans and copies as a picture does.

use crate::{Image, Rgba};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use std::path::Path;
use std::time::Duration;

/// The largest PDF read for a preview (it's read whole).
const MAX_BYTES: u64 = 512 << 20;
/// The longest side a page is drawn at, whatever is asked.
const MAX_SIDE: u32 = 8192;
/// The most pixels one drawn page may have (a poster-sized page asked for big still fits in memory).
const MAX_PIXELS: f32 = 48_000_000.0;
/// Points per inch, and the screen's pixels per inch a page's actual size is measured in.
const POINTS: f32 = 72.0;
const SCREEN: f32 = 96.0;

/// Draw page `page` (0-based; past the end is the last) of the PDF at `path`, on white, fitted so its longer
/// side is `max_side` pixels. Blocks while it draws (tens of ms for a page of text, more for a heavy scan), so
/// call it off the UI thread.
pub fn load_pdf_page(path: &Path, page: usize, max_side: u32) -> Result<Image, String> {
    let meta = std::fs::metadata(path).map_err(|e| read_error(path, e))?;
    if meta.len() > MAX_BYTES {
        return Err("this PDF is too large to preview".into());
    }
    let data = std::fs::read(path).map_err(|e| read_error(path, e))?;
    let pdf = Pdf::new(data).map_err(|e| match e {
        LoadPdfError::Decryption(_) => "this PDF is password-protected".to_string(),
        LoadPdfError::Invalid => "this PDF couldn't be read".to_string(),
    })?;
    let pages = pdf.pages();
    let count = pages.len();
    let index = page.min(count.checked_sub(1).ok_or("this PDF has no pages")?);
    let p = &pages[index];
    let (w, h) = p.render_dimensions();
    if !(w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0) {
        return Err("this PDF's page has no size".into());
    }
    let side = max_side.clamp(16, MAX_SIDE) as f32;
    let scale = (side / w.max(h)).min((MAX_PIXELS / (w * h)).sqrt()).min(u16::MAX as f32 / w.max(h));
    let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
    let pixmap = hayro::render(p, &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    let (pw, ph) = (pixmap.width() as u32, pixmap.height() as u32);
    if pw == 0 || ph == 0 {
        return Err("this PDF's page drew empty".into());
    }
    let px = pixmap.take_rgba8(ImageAlphaType::Alpha);
    // its actual size: the page at 100%, as a PDF reader shows it
    let original = (((w * SCREEN / POINTS).round() as u32).max(1), ((h * SCREEN / POINTS).round() as u32).max(1));
    Ok(Image { frames: vec![(Rgba { w: pw, h: ph, px }, Duration::ZERO)], original, format: "PDF".into(), page: Some((index, count)) })
}

fn read_error(path: &Path, e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("{} doesn't exist", path.display()),
        _ => format!("couldn't read {}: {e}", path.display()),
    }
}
