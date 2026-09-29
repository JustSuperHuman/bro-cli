//! Token authentication, identical to Just Terminal's:
//!
//! * loopback peers need no token;
//! * everyone else presents it as `x-terminal-web-token: <token>`,
//!   `Authorization: Bearer <token>` or `?token=<token>`;
//! * the token is 24 random bytes, base64url without padding, persisted in
//!   `<data_root>/.terminal-web-token`.
//!
//! Legacy import: phones paired with Just Terminal hold its token. When bro's
//! data root has no token yet, [`load_or_create_token`] (with `import_legacy`)
//! copies the one Just Terminal used: `%LOCALAPPDATA%\TerminalWeb` (unpackaged
//! builds, or `TERMINAL_WEB_DATA_ROOT`) or the MSIX-redirected
//! `%LOCALAPPDATA%\Packages\<*Terminal*>\LocalCache\Local\TerminalWeb`,
//! preferring the most recently active one. Already-paired phones keep working.

use crate::state::AppState;
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Request, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use rand::RngCore;
use serde_json::json;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) const TOKEN_FILE: &str = ".terminal-web-token";

/// Whether a request may proceed. `peer` is `None` for in-process calls.
pub fn is_authorized(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    uri: &Uri,
    token: &str,
) -> bool {
    if peer.is_none_or(|peer| peer.ip().is_loopback() || is_mapped_loopback(peer)) {
        return true;
    }
    let presented = headers
        .get("x-terminal-web-token")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .map(|value| value.trim().to_owned())
        })
        .or_else(|| query_token(uri));
    presented.is_some_and(|presented| constant_time_eq(presented.as_bytes(), token.as_bytes()))
}

fn is_mapped_loopback(peer: SocketAddr) -> bool {
    match peer.ip() {
        std::net::IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .is_some_and(|mapped| mapped.is_loopback()),
        _ => false,
    }
}

/// `token=` from the query string, percent-decoded (clients use
/// `encodeURIComponent`).
fn query_token(uri: &Uri) -> Option<String> {
    uri.query()?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
        .map(percent_decode)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Route layer guarding every API and websocket route.
pub(crate) async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect| connect.0);
    if is_authorized(peer, request.headers(), request.uri(), &state.token) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "message": "Terminal web access token is required." })),
        )
            .into_response()
    }
}

fn read_token(path: &Path) -> Option<String> {
    let value = std::fs::read_to_string(path).ok()?;
    let value = value.trim();
    (!value.is_empty() && value.len() <= 512).then(|| value.to_owned())
}

/// Just Terminal data roots that may hold a token, most recently active first.
pub fn legacy_token_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(configured) = std::env::var_os("TERMINAL_WEB_DATA_ROOT") {
        roots.push(PathBuf::from(configured));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        roots.push(local.join("TerminalWeb"));
        if let Ok(packages) = std::fs::read_dir(local.join("Packages")) {
            for entry in packages.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("terminal")
                {
                    roots.push(
                        entry
                            .path()
                            .join("LocalCache")
                            .join("Local")
                            .join("TerminalWeb"),
                    );
                }
            }
        }
    }
    let activity = |root: &PathBuf| -> SystemTime {
        [".terminal-web-server.json", TOKEN_FILE]
            .iter()
            .filter_map(|name| {
                std::fs::metadata(root.join(name))
                    .and_then(|m| m.modified())
                    .ok()
            })
            .max()
            .unwrap_or(SystemTime::UNIX_EPOCH)
    };
    let mut candidates: Vec<(SystemTime, PathBuf)> = roots
        .into_iter()
        .filter(|root| root.join(TOKEN_FILE).is_file())
        .map(|root| (activity(&root), root.join(TOKEN_FILE)))
        .collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    candidates.into_iter().map(|(_, path)| path).collect()
}

/// Reads `<data_root>/.terminal-web-token`, importing a legacy token from
/// `legacy` (first readable wins) or generating a new one when absent.
pub fn load_or_create_token(data_root: &Path, legacy: &[PathBuf]) -> String {
    let _ = std::fs::create_dir_all(data_root);
    let path = data_root.join(TOKEN_FILE);
    if let Some(token) = read_token(&path) {
        return token;
    }
    let token = legacy
        .iter()
        .find_map(|candidate| read_token(candidate))
        .unwrap_or_else(|| {
            let mut bytes = [0u8; 24];
            rand::rng().fill_bytes(&mut bytes);
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        });
    if let Err(error) = crate::project_store::write_atomic(&path, format!("{token}\n").as_bytes()) {
        tracing::warn!("bro-bridge: could not save the access token: {error}");
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn remote() -> Option<SocketAddr> {
        Some("192.168.1.50:5555".parse().unwrap())
    }

    #[test]
    fn loopback_needs_no_token() {
        let headers = HeaderMap::new();
        let uri: Uri = "/api/bootstrap".parse().unwrap();
        assert!(is_authorized(
            Some("127.0.0.1:1".parse().unwrap()),
            &headers,
            &uri,
            "t"
        ));
        assert!(is_authorized(
            Some("[::1]:1".parse().unwrap()),
            &headers,
            &uri,
            "t"
        ));
        assert!(is_authorized(
            Some("[::ffff:127.0.0.1]:1".parse().unwrap()),
            &headers,
            &uri,
            "t"
        ));
        assert!(is_authorized(None, &headers, &uri, "t"));
    }

    #[test]
    fn remote_peers_need_the_token_in_any_supported_place() {
        let token = "abc_DEF-123";
        let bare: Uri = "/api/bootstrap".parse().unwrap();
        assert!(!is_authorized(remote(), &HeaderMap::new(), &bare, token));

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-terminal-web-token",
            HeaderValue::from_static("abc_DEF-123"),
        );
        assert!(is_authorized(remote(), &headers, &bare, token));

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer abc_DEF-123"),
        );
        assert!(is_authorized(remote(), &headers, &bare, token));

        let query: Uri = "/ws?x=1&token=abc_DEF-123".parse().unwrap();
        assert!(is_authorized(remote(), &HeaderMap::new(), &query, token));
        let encoded: Uri = "/ws?token=abc%5FDEF-123".parse().unwrap();
        assert!(is_authorized(remote(), &HeaderMap::new(), &encoded, token));

        let wrong: Uri = "/ws?token=abc_DEF-124".parse().unwrap();
        assert!(!is_authorized(remote(), &HeaderMap::new(), &wrong, token));
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Basic abc_DEF-123"),
        );
        assert!(!is_authorized(remote(), &headers, &bare, token));
    }

    #[test]
    fn token_is_created_once_and_legacy_tokens_are_imported() {
        let root = tempfile::tempdir().unwrap();
        let created = load_or_create_token(&root.path().join("a"), &[]);
        assert_eq!(created.len(), 32, "24 bytes base64url without padding");
        assert!(
            created
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_eq!(load_or_create_token(&root.path().join("a"), &[]), created);

        let legacy = root.path().join("legacy-token");
        std::fs::write(&legacy, "phone-paired-token\n").unwrap();
        let missing = root.path().join("missing");
        let imported = load_or_create_token(&root.path().join("b"), &[missing, legacy]);
        assert_eq!(imported, "phone-paired-token");
        assert_eq!(
            std::fs::read_to_string(root.path().join("b").join(TOKEN_FILE))
                .unwrap()
                .trim(),
            "phone-paired-token"
        );
    }

    #[test]
    fn percent_decoding_is_lenient() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
