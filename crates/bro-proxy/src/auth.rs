//! Local client authentication.
//!
//! - No token configured: only loopback peers are served (any key accepted).
//! - Token configured: `x-api-key` or `Authorization: Bearer` must match; any peer.

use crate::Dialect;
use crate::errors::{ErrorKind, ProxyError};
use crate::state::AppState;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use http::HeaderMap;
use std::net::SocketAddr;
use std::sync::Arc;

pub(crate) fn dialect_for_path(path: &str) -> Dialect {
    if path.contains("/chat/completions") {
        Dialect::Chat
    } else if path.contains("/responses") || path.ends_with("/models") {
        Dialect::Responses
    } else {
        Dialect::Anthropic
    }
}

pub(crate) fn presented_key(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.strip_prefix("Bearer ")
                        .or_else(|| v.strip_prefix("bearer "))
                })
        })
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) fn check(
    token: Option<&str>,
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    path: &str,
) -> Result<(), ProxyError> {
    match token {
        None => {
            let loopback = peer.is_none_or(|p| p.ip().is_loopback());
            if loopback {
                Ok(())
            } else {
                Err(ProxyError::new(
                    ErrorKind::Permission,
                    "proxy only accepts loopback clients (no token configured)",
                ))
            }
        }
        Some(_) if path == "/health" => Ok(()),
        Some(t) => match presented_key(headers) {
            Some(k) if constant_time_eq(k.trim().as_bytes(), t.as_bytes()) => Ok(()),
            _ => Err(ProxyError::new(
                ErrorKind::Authentication,
                "invalid or missing proxy api key",
            )),
        },
    }
}

pub(crate) async fn middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let path = req.uri().path().to_string();
    match check(state.token.as_deref(), peer, req.headers(), &path) {
        Ok(()) => next.run(req).await,
        Err(e) => {
            tracing::warn!(?peer, path, "proxy rejected client: {}", e.message);
            e.into_response(dialect_for_path(&path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_rules() {
        let lo: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.9:5000".parse().unwrap();
        let mut h = HeaderMap::new();
        assert!(check(None, Some(lo), &h, "/r/x/v1/messages").is_ok());
        assert_eq!(
            check(None, Some(lan), &h, "/r/x/v1/messages")
                .unwrap_err()
                .status,
            403
        );
        assert_eq!(
            check(Some("tok"), Some(lan), &h, "/r/x/v1/messages")
                .unwrap_err()
                .status,
            401
        );
        assert!(check(Some("tok"), Some(lan), &h, "/health").is_ok());
        h.insert("authorization", "Bearer tok".parse().unwrap());
        assert!(check(Some("tok"), Some(lan), &h, "/r/x/v1/messages").is_ok());
        let mut h = HeaderMap::new();
        h.insert("x-api-key", "nope".parse().unwrap());
        assert!(check(Some("tok"), Some(lo), &h, "/r/x/v1/messages").is_err());
        assert_eq!(dialect_for_path("/r/x/v1/chat/completions"), Dialect::Chat);
        assert_eq!(dialect_for_path("/r/x/v1/messages"), Dialect::Anthropic);
    }
}
