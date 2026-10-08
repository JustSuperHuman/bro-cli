//! `GET|HEAD /api/sessions/{id}/raw?path=[&download=1]`: a workspace file's
//! bytes streamed from disk with HTTP Range support, for `<video>`/`<audio>`
//! (iOS AVPlayer insists on exact 206s), images and downloads. Same resolver
//! and root checks as `/file`; no size limit; never buffers the whole file.

use crate::file_preview::{self, Kind};
use crate::rest::api_error;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path as FsPath;
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const CHUNK: usize = 64 * 1024;

/// `Content-Type` for served bytes. Markup is served as plain text, and the
/// CSP sandbox keeps anything else (SVG) from acting as a page.
pub(crate) fn content_type(path: &FsPath) -> &'static str {
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if let Some(mime) =
        file_preview::media_mime(&extension).or_else(|| file_preview::image_mime(&extension))
    {
        return mime;
    }
    match extension.as_str() {
        "pdf" => "application/pdf",
        "txt" | "log" | "csv" | "tsv" => "text/plain; charset=utf-8",
        _ if file_preview::language_for(path) != "plaintext" || extension.is_empty() => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}

/// What a `Range` header asks for, given the file length.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RangeRequest {
    /// No usable single range: send everything (absent, malformed, multi-range).
    Full,
    /// Inclusive byte range.
    Partial(u64, u64),
    Unsatisfiable,
}

pub(crate) fn parse_range(header: Option<&str>, total: u64) -> RangeRequest {
    let Some(spec) = header
        .map(str::trim)
        .and_then(|value| value.strip_prefix("bytes="))
    else {
        return RangeRequest::Full;
    };
    if spec.contains(',') {
        return RangeRequest::Full;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeRequest::Full;
    };
    let (first, last) = (first.trim(), last.trim());
    let number = |text: &str| text.parse::<u64>().ok();
    if first.is_empty() {
        // `-n`: the final n bytes.
        return match number(last) {
            Some(0) => RangeRequest::Unsatisfiable,
            Some(_) if total == 0 => RangeRequest::Unsatisfiable,
            Some(n) => RangeRequest::Partial(total - n.min(total), total - 1),
            None => RangeRequest::Full,
        };
    }
    let Some(start) = number(first) else {
        return RangeRequest::Full;
    };
    let end = if last.is_empty() {
        u64::MAX
    } else {
        match number(last) {
            Some(end) if end >= start => end,
            _ => return RangeRequest::Full, // syntactically invalid: ignore it
        }
    };
    if start >= total {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Partial(start, end.min(total - 1))
}

fn http_date(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time)
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

/// RFC 5987 `filename*` value (`UTF-8''…`).
fn encoded_filename(name: &str) -> String {
    let mut out = String::from("UTF-8''");
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Streams `len` bytes of an already positioned file in 64 KiB chunks.
fn file_body(file: tokio::fs::File, len: u64) -> Body {
    let stream = futures_util::stream::unfold(Some(file.take(len)), |reader| async move {
        let mut reader = reader?;
        let mut buffer = vec![0; CHUNK];
        match reader.read(&mut buffer).await {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                Some((Ok::<_, std::io::Error>(Bytes::from(buffer)), Some(reader)))
            }
            Err(error) => Some((Err(error), None)),
        }
    });
    Body::from_stream(stream)
}

pub(crate) async fn raw(
    method: Method,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let Some(roots) = state.file_roots(&id) else {
        return api_error(StatusCode::NOT_FOUND, "Unknown terminal session.");
    };
    let target = query.get("path").cloned().unwrap_or_default();
    let resolved = match tokio::task::spawn_blocking(move || {
        file_preview::resolve_path(&roots, &target)
    })
    .await
    {
        Ok(Ok(resolved)) if resolved.kind == Kind::File => resolved,
        Ok(Ok(_)) => return api_error(StatusCode::BAD_REQUEST, "This path is not a file."),
        Ok(Err(message)) => {
            let status = match message.as_str() {
                file_preview::NOT_FOUND => StatusCode::NOT_FOUND,
                file_preview::OUTSIDE => StatusCode::FORBIDDEN,
                _ => StatusCode::BAD_REQUEST,
            };
            return api_error(status, message);
        }
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "File could not be opened.",
            );
        }
    };
    let Ok(mut file) = tokio::fs::File::open(&resolved.path).await else {
        return api_error(StatusCode::NOT_FOUND, file_preview::NOT_FOUND);
    };
    let Ok(metadata) = file.metadata().await else {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "File could not be inspected.",
        );
    };
    let total = metadata.len();
    let modified = metadata.modified().ok();
    let modified_ms = modified
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis());
    let etag = format!("\"{total:x}-{modified_ms:x}\"");
    let last_modified = modified.map(http_date);

    let header_text =
        |name: header::HeaderName| headers.get(name).and_then(|value| value.to_str().ok());
    let mut range = parse_range(header_text(header::RANGE), total);
    if let Some(condition) = header_text(header::IF_RANGE)
        && condition.trim() != etag
        && Some(condition.trim()) != last_modified.as_deref()
    {
        range = RangeRequest::Full; // the file changed since the client cached part of it
    }
    let not_modified = range == RangeRequest::Full
        && header_text(header::IF_NONE_MATCH).is_some_and(|tags| {
            tags.split(',')
                .any(|tag| tag.trim() == etag || tag.trim() == "*")
        });

    let name = resolved
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let download = query
        .get("download")
        .is_some_and(|value| value == "1" || value == "true");
    let disposition = format!(
        "{}; filename*={}",
        if download { "attachment" } else { "inline" },
        encoded_filename(&name)
    );
    let mut response = Response::builder()
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, content_type(&resolved.path))
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, "private, max-age=0, must-revalidate")
        .header("x-content-type-options", "nosniff")
        .header(header::CONTENT_SECURITY_POLICY, "sandbox");
    if let Some(last_modified) = &last_modified {
        response = response.header(header::LAST_MODIFIED, last_modified);
    }
    let (status, start, length) = match range {
        _ if not_modified => (StatusCode::NOT_MODIFIED, 0, 0),
        RangeRequest::Full => (StatusCode::OK, 0, total),
        RangeRequest::Partial(start, end) => {
            response = response.header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            );
            (StatusCode::PARTIAL_CONTENT, start, end - start + 1)
        }
        RangeRequest::Unsatisfiable => {
            response = response.header(header::CONTENT_RANGE, format!("bytes */{total}"));
            (StatusCode::RANGE_NOT_SATISFIABLE, 0, 0)
        }
    };
    let streams = matches!(status, StatusCode::OK | StatusCode::PARTIAL_CONTENT);
    if streams {
        response = response.header(header::CONTENT_LENGTH, length);
    }
    let body = if streams && method != Method::HEAD {
        if start > 0 && file.seek(SeekFrom::Start(start)).await.is_err() {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "File could not be read.");
        }
        file_body(file, length)
    } else {
        Body::empty()
    };
    response
        .status(status)
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_follow_rfc_9110() {
        use RangeRequest::*;
        assert_eq!(parse_range(None, 100), Full);
        assert_eq!(parse_range(Some("bytes=0-0"), 100), Partial(0, 0));
        assert_eq!(parse_range(Some("bytes=10-"), 100), Partial(10, 99));
        assert_eq!(parse_range(Some("bytes=-5"), 100), Partial(95, 99));
        assert_eq!(parse_range(Some("bytes=-500"), 100), Partial(0, 99));
        assert_eq!(parse_range(Some("bytes=90-500"), 100), Partial(90, 99));
        assert_eq!(parse_range(Some("bytes=100-"), 100), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), 100), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), Full);
        assert_eq!(parse_range(Some("bytes=5-1"), 100), Full);
        assert_eq!(parse_range(Some("items=0-1"), 100), Full);
        assert_eq!(parse_range(Some("bytes=x-1"), 100), Full);
    }

    #[test]
    fn filenames_and_types() {
        assert_eq!(
            encoded_filename("my clip ü.mp4"),
            "UTF-8''my%20clip%20%C3%BC.mp4"
        );
        assert_eq!(content_type(FsPath::new("a.MOV")), "video/quicktime");
        assert_eq!(content_type(FsPath::new("a.flac")), "audio/flac");
        assert_eq!(
            content_type(FsPath::new("a.html")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            content_type(FsPath::new("LICENSE")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(content_type(FsPath::new("a.svg")), "image/svg+xml");
        assert_eq!(content_type(FsPath::new("a.pdf")), "application/pdf");
        assert_eq!(
            content_type(FsPath::new("a.bin")),
            "application/octet-stream"
        );
    }
}
