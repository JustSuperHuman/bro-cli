//! Bounded still thumbnails for mobile. `/raw?thumbnail=256` uses the same access checks as the original.
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
    let mut bytes = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 72)
        .encode_image(&rgb)
        .map_err(|_| "The thumbnail could not be made.")?;
    Ok(Bytes::from(bytes))
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
