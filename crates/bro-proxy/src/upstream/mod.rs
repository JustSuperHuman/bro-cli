//! Talking to real upstreams: URL building, auth, retries, pool failover.
//!
//! [`send`] returns `Ok` only for 2xx responses; every non-2xx becomes a
//! [`ProxyError`] classified from the upstream body, so callers just render it
//! in the inbound dialect.

pub mod chatgpt;
pub mod oauth;
pub mod pool;

use crate::errors::{ErrorKind, ProxyError};
use crate::state::AppState;
use crate::{Route, Upstream};
use bro_core::creds::CodexAuth;
use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use std::pin::Pin;

pub(crate) type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Messages,
    CountTokens,
    Chat,
    Responses,
    Models,
}

pub(crate) struct UpstreamCall {
    pub endpoint: Endpoint,
    /// JSON body (None for GET)
    pub body: Option<Bytes>,
    /// Expect an SSE response
    pub stream: bool,
    /// Client headers worth forwarding (Anthropic passthrough: anthropic-beta, …)
    pub client_headers: HeaderMap,
    /// Conversation key (pool stickiness)
    pub session_key: Option<String>,
    /// Request was translated from an OpenAI client (Claude OAuth needs Claude Code headers)
    pub translated: bool,
    /// Pre-fetched ChatGPT credentials (decided before translation)
    pub codex_auth: Option<CodexAuth>,
}

impl UpstreamCall {
    pub fn get_models() -> Self {
        UpstreamCall {
            endpoint: Endpoint::Models,
            body: None,
            stream: false,
            client_headers: HeaderMap::new(),
            session_key: None,
            translated: false,
            codex_auth: None,
        }
    }
}

pub(crate) struct UpstreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: ByteStream,
    /// Pool account that served the request
    pub account: Option<String>,
}

impl UpstreamResponse {
    pub fn is_sse(&self) -> bool {
        self.headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("text/event-stream"))
    }
}

pub(crate) async fn read_all(mut body: ByteStream) -> Result<Bytes, ProxyError> {
    let mut buf = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk =
            chunk.map_err(|e| ProxyError::api(format!("upstream body: {e}")).with_status(502))?;
        buf.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(buf))
}

// ------------------------------------------------------------------ URLs

pub(crate) fn anthropic_url(base: &str, endpoint: Endpoint) -> String {
    let clean = base.trim_end_matches('/');
    let root = clean
        .strip_suffix("/v1/messages")
        .or_else(|| clean.strip_suffix("/v1"))
        .unwrap_or(clean);
    match endpoint {
        Endpoint::CountTokens => format!("{root}/v1/messages/count_tokens"),
        Endpoint::Models => format!("{root}/v1/models?limit=1000"),
        _ => format!("{root}/v1/messages"),
    }
}

pub(crate) fn openai_url(base: &str, endpoint: Endpoint) -> String {
    let clean = base.trim_end_matches('/');
    let root = clean
        .strip_suffix("/chat/completions")
        .or_else(|| clean.strip_suffix("/responses"))
        .unwrap_or(clean);
    match endpoint {
        Endpoint::Responses => format!("{root}/responses"),
        Endpoint::Models => format!("{root}/models"),
        _ => format!("{root}/chat/completions"),
    }
}

// ------------------------------------------------------------------ headers

const DROP_REQUEST_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "keep-alive",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "proxy-authorization",
    "proxy-authenticate",
    "authorization",
    "x-api-key",
    "accept-encoding",
    "cookie",
];

/// Anthropic-relevant client headers to forward (anthropic-*, x-stainless-*, user-agent, x-app…).
pub(crate) fn forwardable_client_headers(h: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in h {
        let name = k.as_str();
        if DROP_REQUEST_HEADERS.contains(&name) || name == "content-type" || name == "accept" {
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    out
}

pub(crate) fn set(h: &mut HeaderMap, k: &'static str, v: &str) {
    if let Ok(v) = HeaderValue::from_str(v) {
        h.insert(HeaderName::from_static(k), v);
    }
}

fn base_headers(stream: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    set(&mut h, "content-type", "application/json");
    set(
        &mut h,
        "accept",
        if stream {
            "text/event-stream"
        } else {
            "application/json"
        },
    );
    h
}

fn anthropic_headers(call: &UpstreamCall) -> HeaderMap {
    let mut h = base_headers(call.stream);
    for (k, v) in &call.client_headers {
        h.insert(k.clone(), v.clone());
    }
    if !h.contains_key("anthropic-version") {
        set(&mut h, "anthropic-version", "2023-06-01");
    }
    h
}

// ------------------------------------------------------------------ sending

pub(crate) async fn execute(
    state: &AppState,
    method: Method,
    url: &str,
    headers: HeaderMap,
    body: Option<Bytes>,
) -> Result<reqwest::Response, ProxyError> {
    let mut req = state.http.request(method, url).headers(headers);
    if let Some(b) = body {
        req = req.body(b);
    }
    req.send().await.map_err(|e| {
        let kind = if e.is_timeout() {
            ErrorKind::Timeout
        } else {
            ErrorKind::Api
        };
        let status = if e.is_timeout() { 504 } else { 502 };
        ProxyError::new(kind, format!("upstream unreachable ({url}): {e}")).with_status(status)
    })
}

/// 2xx → UpstreamResponse; otherwise read the body and classify.
pub(crate) async fn finish(
    resp: reqwest::Response,
    account: Option<String>,
) -> Result<UpstreamResponse, ProxyError> {
    let status = resp.status();
    let headers = resp.headers().clone();
    if !status.is_success() {
        let body = resp.bytes().await.unwrap_or_default();
        return Err(ProxyError::from_upstream(status.as_u16(), &headers, &body));
    }
    let body: ByteStream = Box::pin(resp.bytes_stream().map_err(std::io::Error::other));
    Ok(UpstreamResponse {
        status,
        headers,
        body,
        account,
    })
}

pub(crate) fn method_for(endpoint: Endpoint) -> Method {
    if endpoint == Endpoint::Models {
        Method::GET
    } else {
        Method::POST
    }
}

pub(crate) async fn send(
    state: &AppState,
    route: &Route,
    call: &UpstreamCall,
) -> Result<UpstreamResponse, ProxyError> {
    let method = method_for(call.endpoint);
    match &route.upstream {
        Upstream::OpenAiChat { base_url, api_key }
        | Upstream::OpenAiResponses { base_url, api_key } => {
            let mut h = base_headers(call.stream);
            if let Some(k) = api_key.as_deref().filter(|k| !k.is_empty()) {
                set(&mut h, "authorization", &format!("Bearer {k}"));
            }
            let resp = execute(
                state,
                method,
                &openai_url(base_url, call.endpoint),
                h,
                call.body.clone(),
            )
            .await?;
            finish(resp, None).await
        }
        Upstream::Anthropic {
            base_url,
            api_key,
            bearer,
        } => {
            let mut h = anthropic_headers(call);
            if let Some(k) = api_key.as_deref().filter(|k| !k.is_empty()) {
                if *bearer {
                    set(&mut h, "authorization", &format!("Bearer {k}"));
                } else {
                    set(&mut h, "x-api-key", k);
                }
            }
            let resp = execute(
                state,
                method,
                &anthropic_url(base_url, call.endpoint),
                h,
                call.body.clone(),
            )
            .await?;
            finish(resp, None).await
        }
        Upstream::ClaudeOAuth { config_dir } => oauth::send(state, config_dir, call, None).await,
        Upstream::ClaudePool { config_dirs } => pool::send(state, config_dirs, call).await,
        Upstream::ChatGptCodex { codex_home } => chatgpt::send(state, codex_home, call).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            anthropic_url("https://api.anthropic.com", Endpoint::Messages),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            anthropic_url("https://x/api/v1/", Endpoint::CountTokens),
            "https://x/api/v1/messages/count_tokens"
        );
        assert_eq!(
            anthropic_url("https://x/v1/messages", Endpoint::Messages),
            "https://x/v1/messages"
        );
        assert_eq!(
            openai_url("https://api.openai.com/v1", Endpoint::Chat),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            openai_url("http://localhost:11434/v1/", Endpoint::Models),
            "http://localhost:11434/v1/models"
        );
        assert_eq!(
            openai_url("https://a/v1/chat/completions", Endpoint::Chat),
            "https://a/v1/chat/completions"
        );
        assert_eq!(
            openai_url("https://a/v1", Endpoint::Responses),
            "https://a/v1/responses"
        );
    }

    #[test]
    fn forwards_only_safe_headers() {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("x-api-key", "local"),
            ("authorization", "Bearer local"),
            ("anthropic-beta", "a,b"),
            ("host", "127.0.0.1"),
            ("accept-encoding", "gzip"),
            ("x-stainless-os", "Windows"),
        ] {
            h.insert(HeaderName::from_static(k), HeaderValue::from_static(v));
        }
        let f = forwardable_client_headers(&h);
        assert_eq!(f.len(), 2);
        assert!(f.contains_key("anthropic-beta") && f.contains_key("x-stainless-os"));
    }
}
