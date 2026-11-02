//! Bounded still thumbnails for mobile. `/raw?thumbnail=256` uses the same access checks as the original.
//! A PDF's thumbnail is its first page.
use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use image::{DynamicImage, ImageDecoder, ImageReader};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::UNIX_EPOCH;
use tokio::sync::Semaphore;

const MAX_SOURCE_BYTES: u64 = 32 << 20;
const MAX_DECODE_BYTES: u64 = 64 << 20;
const CACHE_BYTES: usize = 16 << 20;
const CACHE_ITEMS: usize = 64;
static DECODER: OnceLock<Arc<Semaphore>> = OnceLock::new();
static CACHE: OnceLock<Mutex<VecDeque<(String, Bytes)>>> = OnceLock::new();

fn cache() -> &'static Mutex<VecDeque<(String, Bytes)>> {
    CACHE.get_or_init(Mutex::default)
}

fn cached(key: &str) -> Option<Bytes> {
    let mut entries = cache().lock();
    let index = entries.iter().position(|(k, _)| k == key)?;
    let entry = entries.remove(index)?;
    let bytes = entry.1.clone();
    entries.push_back(entry);
    Some(bytes)
}

fn remember(key: String, bytes: Bytes) {
    let mut entries = cache().lock();
    entries.push_back((key, bytes));
    let mut total: usize = entries.iter().map(|(_, bytes)| bytes.len()).sum();
    while entries.len() > CACHE_ITEMS || total > CACHE_BYTES {
        if let Some((_, bytes)) = entries.pop_front() {
            total -= bytes.len();
        }
    }
}

fn decode(path: &Path, side: u32) -> Result<Bytes, String> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return first_page(path, side);
    }
    let mut reader = ImageReader::open(path)
        .map_err(|_| "The picture could not be opened.")?
        .with_guessed_format()
        .map_err(|_| "The picture format could not be read.")?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "This picture has no thumbnail.")?;
    let (width, height) = decoder.dimensions();
    if width as u64 * height as u64 * 4 > MAX_DECODE_BYTES {
        return Err(
            "This picture is too large for a thumbnail. Open it to see the original.".into(),
        );
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|_| "This picture has no thumbnail.")?;
    image.apply_orientation(orientation);
    // Decode only the first animation frame, and flatten transparency for a small JPEG.
    let small = image.thumbnail(side, side).to_rgba8();
    let rgb = image::RgbImage::from_fn(small.width(), small.height(), |x, y| {
        let pixel = small.get_pixel(x, y);
        let alpha = pixel[3] as u32;
        image::Rgb([0, 1, 2].map(|c| ((pixel[c] as u32 * alpha + 24 * (255 - alpha)) / 255) as u8))
    });
    jpeg(&rgb)
}

fn jpeg(rgb: &image::RgbImage) -> Result<Bytes, String> {
    let mut bytes = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 72)
        .encode_image(rgb)
        .map_err(|_| "The thumbnail could not be made.")?;
    Ok(Bytes::from(bytes))
}

/// A PDF's first page, drawn on white so its longer side is `side` pixels.
fn first_page(path: &Path, side: u32) -> Result<Bytes, String> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::vello_cpu::peniko::ImageAlphaType;
    let data = std::fs::read(path).map_err(|_| "The PDF could not be opened.")?;
    let pdf = Pdf::new(data).map_err(|_| "This PDF has no thumbnail.")?;
    let page = pdf.pages().first().ok_or("This PDF has no pages.")?;
    let (w, h) = page.render_dimensions();
    if !(w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0) {
        return Err("This PDF has no thumbnail.".into());
    }
    let scale = side as f32 / w.max(h);
    let settings = hayro::PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
    let pixmap = hayro::render(page, &hayro::RenderCache::new(), &InterpreterSettings::default(), &hayro::RenderSettings::default(), &settings);
    let (pw, ph) = (pixmap.width() as u32, pixmap.height() as u32);
    let rgba = pixmap.take_rgba8(ImageAlphaType::Alpha);
    let rgb = image::RgbImage::from_fn(pw, ph, |x, y| {
        let i = (y as usize * pw as usize + x as usize) * 4;
        image::Rgb([rgba[i], rgba[i + 1], rgba[i + 2]])
    });
    if rgb.width() == 0 || rgb.height() == 0 {
        return Err("This PDF has no thumbnail.".into());
    }
    jpeg(&rgb)
}

pub(crate) async fn serve(
    method: Method,
    path: PathBuf,
    requested: u32,
    headers: HeaderMap,
) -> Response {
    // Quantise variants, so arbitrary requests cannot fill the cache with near-identical pictures.
    let side = match requested.clamp(1, 512) {
        1..=128 => 128,
        129..=256 => 256,
        _ => 512,
    };
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.len() <= MAX_SOURCE_BYTES => metadata,
        Ok(_) => {
            return crate::rest::api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "This picture is too large for a thumbnail. Open it to see the original.",
            );
        }
        Err(_) => {
            return crate::rest::api_error(StatusCode::NOT_FOUND, crate::file_preview::NOT_FOUND);
        }
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map(|at| at.as_nanos())
        .unwrap_or(0);
    let key = format!(
        "{:x}",
        Sha256::digest(format!(
            "{}|{}|{modified}|{side}",
            path.display(),
            metadata.len()
        ))
    );
    let etag = format!("\"thumb-{key}\"");
    let response = Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "private, max-age=0, must-revalidate")
        .header(header::ETAG, &etag)
        .header("x-content-type-options", "nosniff");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|tags| {
            tags.split(',')
                .any(|tag| tag.trim().trim_start_matches("W/") == etag || tag.trim() == "*")
        })
    {
        return response
            .status(StatusCode::NOT_MODIFIED)
            .body(Body::empty())
            .unwrap();
    }
    let bytes = if let Some(bytes) = cached(&key) {
        bytes
    } else {
        let permit = DECODER
            .get_or_init(|| Arc::new(Semaphore::new(1)))
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let key_for_decode = key.clone();
        match tokio::task::spawn_blocking(move || {
            // The permit lives on the blocking thread even if the HTTP request is cancelled.
            let _permit = permit;
            if let Some(bytes) = cached(&key_for_decode) {
                return Ok(bytes);
            }
            let bytes = decode(&path, side)?;
            remember(key_for_decode, bytes.clone());
            Ok::<_, String>(bytes)
        })
        .await
        {
            Ok(Ok(bytes)) => bytes,
            _ => {
                return crate::rest::api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "This picture has no thumbnail. Open it to see the original.",
                );
            }
        }
    };
    response
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(bytes)
        })
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_bounds_pixels_and_transfer_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("large.png");
        let image = image::RgbImage::from_fn(1600, 1200, |x, y| {
            let n =
                (x.wrapping_mul(747796405) ^ y.wrapping_mul(2891336453)).wrapping_mul(277803737);
            image::Rgb([(n >> 16) as u8, (n >> 8) as u8, n as u8])
        });
        image.save(&path).unwrap();
        let original = std::fs::metadata(&path).unwrap().len();
        let bytes = decode(&path, 256).unwrap();
        let small = image::load_from_memory(&bytes).unwrap();
        assert_eq!((small.width(), small.height()), (256, 192));
        assert!(bytes.len() < 64 * 1024);
        assert!((bytes.len() as u64) * 20 < original);
        println!(
            "thumbnail benchmark: {original} original bytes -> {} thumbnail bytes",
            bytes.len()
        );
    }

    #[test]
    fn a_pdfs_thumbnail_is_its_first_page() {
        let root = tempfile::tempdir().unwrap();
        let page = "0 0.5 0 rg 0 0 612 792 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << >> >>".into(),
            format!("<< /Length {} >>\nstream\n{page}\nendstream", page.len()),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![];
        for (i, o) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            pdf.extend(format!("{o:010} 00000 n \n").bytes());
        }
        pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        let path = root.path().join("Guide.PDF");
        std::fs::write(&path, pdf).unwrap();
        let small = image::load_from_memory(&decode(&path, 256).unwrap()).unwrap().to_rgb8();
        assert!((255..=256).contains(&small.height()) && small.width() < small.height());
        let [r, g, b] = small.get_pixel(small.width() / 2, small.height() / 2).0;
        assert!(r < 30 && g > 100 && b < 30, "the page's green: {r},{g},{b}");
        assert!(decode(&root.path().join("missing.pdf"), 128).is_err());
    }

    #[tokio::test]
    async fn variants_revalidate_and_original_changes_invalidate_cache() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("picture.png");
        image::RgbaImage::from_pixel(600, 300, image::Rgba([40, 140, 200, 128]))
            .save(&path)
            .unwrap();
        let first = serve(Method::GET, path.clone(), 256, HeaderMap::new()).await;
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first.headers()[header::ETAG].clone();
        let mut conditional = HeaderMap::new();
        conditional.insert(header::IF_NONE_MATCH, etag.clone());
        let cached = serve(Method::GET, path.clone(), 256, conditional.clone()).await;
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
        let variant = serve(Method::HEAD, path.clone(), 512, conditional.clone()).await;
        assert_eq!(variant.status(), StatusCode::OK);
        assert_ne!(variant.headers()[header::ETAG], etag);
        image::RgbaImage::from_pixel(800, 400, image::Rgba([200, 90, 40, 255]))
            .save(&path)
            .unwrap();
        let changed = serve(Method::GET, path, 256, conditional).await;
        assert_eq!(changed.status(), StatusCode::OK);
        assert_ne!(changed.headers()[header::ETAG], etag);
    }
}
