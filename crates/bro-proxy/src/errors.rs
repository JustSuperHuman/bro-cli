//! Error model and per-dialect error bodies.

use axum::response::{IntoResponse, Response};
use http::{HeaderMap, StatusCode};
use serde_json::{Value, json};

use crate::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidRequest,
    Authentication,
    Permission,
    NotFound,
    RequestTooLarge,
    RateLimit,
    Overloaded,
    Api,
    Timeout,
}

impl ErrorKind {
    pub fn from_status(status: u16) -> ErrorKind {
        match status {
            400 | 422 => ErrorKind::InvalidRequest,
            401 => ErrorKind::Authentication,
            403 => ErrorKind::Permission,
            404 => ErrorKind::NotFound,
            408 | 504 => ErrorKind::Timeout,
            413 => ErrorKind::RequestTooLarge,
            429 => ErrorKind::RateLimit,
            503 | 529 => ErrorKind::Overloaded,
            s if s >= 500 => ErrorKind::Api,
            _ => ErrorKind::InvalidRequest,
        }
    }

    pub fn anthropic_type(self) -> &'static str {
        match self {
            ErrorKind::InvalidRequest => "invalid_request_error",
            ErrorKind::Authentication => "authentication_error",
            ErrorKind::Permission => "permission_error",
            ErrorKind::NotFound => "not_found_error",
            ErrorKind::RequestTooLarge => "request_too_large",
            ErrorKind::RateLimit => "rate_limit_error",
            ErrorKind::Overloaded => "overloaded_error",
            ErrorKind::Api => "api_error",
            ErrorKind::Timeout => "timeout_error",
        }
    }

    pub fn from_anthropic_type(t: &str) -> Option<ErrorKind> {
        Some(match t {
            "invalid_request_error" => ErrorKind::InvalidRequest,
            "authentication_error" => ErrorKind::Authentication,
            "permission_error" => ErrorKind::Permission,
            "not_found_error" => ErrorKind::NotFound,
            "request_too_large" => ErrorKind::RequestTooLarge,
            "rate_limit_error" => ErrorKind::RateLimit,
            "overloaded_error" => ErrorKind::Overloaded,
            "api_error" => ErrorKind::Api,
            "timeout_error" => ErrorKind::Timeout,
            _ => return None,
        })
    }

    /// (type, code) in OpenAI's error vocabulary
    pub fn openai_type_code(self) -> (&'static str, &'static str) {
        match self {
            ErrorKind::InvalidRequest => ("invalid_request_error", "invalid_request"),
            ErrorKind::Authentication => ("authentication_error", "invalid_api_key"),
            ErrorKind::Permission => ("permission_error", "permission_denied"),
            ErrorKind::NotFound => ("invalid_request_error", "model_not_found"),
            ErrorKind::RequestTooLarge => ("invalid_request_error", "context_length_exceeded"),
            ErrorKind::RateLimit => ("rate_limit_error", "rate_limit_exceeded"),
            ErrorKind::Overloaded => ("server_error", "server_is_overloaded"),
            ErrorKind::Api => ("server_error", "server_error"),
            ErrorKind::Timeout => ("server_error", "timeout"),
        }
    }

    pub fn default_status(self) -> u16 {
        match self {
            ErrorKind::InvalidRequest => 400,
            ErrorKind::Authentication => 401,
            ErrorKind::Permission => 403,
            ErrorKind::NotFound => 404,
            ErrorKind::RequestTooLarge => 413,
            ErrorKind::RateLimit => 429,
            ErrorKind::Overloaded => 529,
            ErrorKind::Api => 500,
            ErrorKind::Timeout => 504,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProxyError {
    pub status: u16,
    pub kind: ErrorKind,
    pub message: String,
    /// Upstream `retry-after`, forwarded to the client
    pub retry_after: Option<String>,
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}: {}",
            self.status,
            self.kind.anthropic_type(),
            self.message
        )
    }
}

impl std::error::Error for ProxyError {}

impl ProxyError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        ProxyError {
            status: kind.default_status(),
            kind,
            message: message.into(),
            retry_after: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidRequest, message)
    }
    pub fn api(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Api, message)
    }
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// Classify an upstream error response (either dialect's body shape).
    pub fn from_upstream(status: u16, headers: &HeaderMap, body: &[u8]) -> Self {
        let text = String::from_utf8_lossy(body);
        let json: Option<Value> = serde_json::from_slice(body).ok();
        let err = json.as_ref().and_then(|j| j.get("error"));
        let message = err
            .and_then(|e| {
                e.get("message")
                    .and_then(Value::as_str)
                    .or_else(|| e.as_str())
            })
            .or_else(|| {
                json.as_ref()
                    .and_then(|j| j.get("detail"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                json.as_ref()
                    .and_then(|j| j.get("message"))
                    .and_then(Value::as_str)
            })
            .map(str::to_string)
            .unwrap_or_else(|| {
                let t = crate::util::truncate(text.trim(), 500);
                if t.is_empty() {
                    format!("upstream returned HTTP {status}")
                } else {
                    t.to_string()
                }
            });
        let typed = err
            .and_then(|e| e.get("type").and_then(Value::as_str))
            .and_then(ErrorKind::from_anthropic_type);
        let code = err
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let mut kind = typed.unwrap_or_else(|| ErrorKind::from_status(status));
        if status == 429 || code == "rate_limit_exceeded" {
            kind = ErrorKind::RateLimit;
        }
        if matches!(status, 503 | 529) {
            kind = ErrorKind::Overloaded;
        }
        if code == "context_length_exceeded" {
            kind = ErrorKind::InvalidRequest;
        }
        let retry_after = headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        ProxyError {
            status: if status < 400 { 502 } else { status },
            kind,
            message,
            retry_after,
        }
    }

    /// Classify an in-stream error event body (`{type, message}` or `{code, message}`).
    pub fn from_stream_error(err: &Value) -> Self {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("upstream stream error")
            .to_string();
        let t = err.get("type").and_then(Value::as_str).unwrap_or("");
        let code = err.get("code").and_then(Value::as_str).unwrap_or("");
        let kind = ErrorKind::from_anthropic_type(t).unwrap_or_else(|| {
            let hay = format!("{t} {code} {message}").to_lowercase();
            if hay.contains("rate_limit")
                || hay.contains("rate limit")
                || hay.contains("usage limit")
            {
                ErrorKind::RateLimit
            } else if hay.contains("overloaded") {
                ErrorKind::Overloaded
            } else if hay.contains("context_length") || hay.contains("invalid") {
                ErrorKind::InvalidRequest
            } else {
                ErrorKind::Api
            }
        });
        ProxyError::new(kind, message)
    }

    pub fn is_rate_limit(&self) -> bool {
        if self.kind == ErrorKind::RateLimit {
            return true;
        }
        let m = self.message.to_lowercase();
        m.contains("rate limit")
            || m.contains("usage limit")
            || m.contains("limit reached")
            || m.contains("too many requests")
    }

    /// Status as the given dialect's clients expect it.
    pub fn status_for(&self, dialect: Dialect) -> u16 {
        match dialect {
            Dialect::Anthropic => self.status,
            // 529 is Anthropic-specific
            _ if self.status == 529 => 503,
            _ => self.status,
        }
    }

    pub fn body_for(&self, dialect: Dialect) -> Value {
        match dialect {
            Dialect::Anthropic => json!({
                "type": "error",
                "error": { "type": self.kind.anthropic_type(), "message": self.message }
            }),
            Dialect::Chat | Dialect::Responses => {
                let (t, code) = self.kind.openai_type_code();
                json!({ "error": { "message": self.message, "type": t, "param": null, "code": code } })
            }
        }
    }

    pub fn into_response(self, dialect: Dialect) -> Response {
        let status =
            StatusCode::from_u16(self.status_for(dialect)).unwrap_or(StatusCode::BAD_GATEWAY);
        let mut resp = (status, axum::Json(self.body_for(dialect))).into_response();
        if let Some(ra) = self
            .retry_after
            .as_deref()
            .and_then(|v| http::HeaderValue::from_str(v).ok())
        {
            resp.headers_mut().insert("retry-after", ra);
        }
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_upstream_bodies() {
        let h = HeaderMap::new();
        let e = ProxyError::from_upstream(
            429,
            &h,
            br#"{"error":{"message":"slow down","type":"requests","code":"rate_limit_exceeded"}}"#,
        );
        assert_eq!(e.kind, ErrorKind::RateLimit);
        assert_eq!(
            e.body_for(Dialect::Anthropic)["error"]["type"],
            "rate_limit_error"
        );
        let e = ProxyError::from_upstream(
            529,
            &h,
            br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        assert_eq!(e.kind, ErrorKind::Overloaded);
        assert_eq!(e.status_for(Dialect::Chat), 503);
        assert_eq!(e.body_for(Dialect::Chat)["error"]["message"], "Overloaded");
        let e = ProxyError::from_upstream(503, &h, b"upstream connect error");
        assert_eq!(
            e.body_for(Dialect::Anthropic)["error"]["type"],
            "overloaded_error"
        );
        assert_eq!(e.message, "upstream connect error");
    }
}
