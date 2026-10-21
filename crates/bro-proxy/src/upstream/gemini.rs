//! Gemini on Cloud Code Assist (`{host}/v1internal:streamGenerateContent`) with a Google AI
//! subscription sign-in, presenting as the Antigravity CLI: Code Assist only serves Argon to
//! the CLI's User-Agent.
//!
//! The call body is `{"model": <wire id>, "request": <generateContent request>}` (see
//! `translate::a2gemini`); the Code Assist envelope with the account's project is built here.

use super::{Endpoint, UpstreamCall, UpstreamResponse, execute, finish, set};
use crate::errors::{ErrorKind, ProxyError};
use crate::state::AppState;
use bytes::Bytes;
use futures_util::stream;
use http::{HeaderMap, Method, StatusCode};
use serde_json::{Value, json};
use std::path::Path;

fn hosts(state: &AppState) -> Vec<String> {
    if state.opts.gemini_base_urls.is_empty() {
        bro_core::gemini::base_urls()
    } else {
        state.opts.gemini_base_urls.iter().map(|u| u.trim_end_matches('/').to_string()).collect()
    }
}

fn headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    set(&mut h, "user-agent", &bro_core::gemini::user_agent());
    set(&mut h, "authorization", &format!("Bearer {token}"));
    set(&mut h, "content-type", "application/json");
    h
}

/// The Code Assist envelope around a generateContent request (the CLI's key order).
pub(crate) fn envelope(call_body: &Value, project: &str) -> Value {
    json!({
        "project": project,
        "requestId": format!("agent-{}", uuid::Uuid::new_v4()),
        "request": call_body.get("request").cloned().unwrap_or_else(|| json!({})),
        "model": call_body.get("model").cloned().unwrap_or(Value::Null),
        "userAgent": "antigravity",
        "requestType": "agent",
    })
}

/// `12.3s` → whole seconds, rounded up.
fn retry_seconds(delay: &str) -> Option<u64> {
    let secs: f64 = delay.trim().strip_suffix('s')?.parse().ok()?;
    Some(secs.max(0.0).ceil() as u64)
}

/// Google's error body (`{"error": {code, status, message, details}}`) as a ProxyError, with
/// RetryInfo as retry-after and a hint where the raw message is opaque.
pub(crate) fn classify(status: u16, headers: &HeaderMap, body: &[u8]) -> ProxyError {
    let mut e = ProxyError::from_upstream(status, headers, body);
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let details = v.pointer("/error/details").and_then(Value::as_array).cloned().unwrap_or_default();
    if e.retry_after.is_none() {
        e.retry_after = details
            .iter()
            .find_map(|d| d.get("retryDelay").and_then(Value::as_str).and_then(retry_seconds))
            .map(|s| s.to_string());
    }
    let reason = details.iter().find_map(|d| d.get("reason").and_then(Value::as_str)).unwrap_or("");
    match status {
        404 => {
            e.message = format!(
                "Gemini says this model isn't available to your account ({}). Gemini 4 Argon needs Google AI Pro or Ultra; `bro gemini status` lists the models you have.",
                e.message
            )
        }
        403 if reason == "VALIDATION_REQUIRED" => {
            e.message = format!("Google wants you to verify this account in the browser first: {}", e.message)
        }
        429 if reason == "QUOTA_EXHAUSTED" => {
            e.message = format!("your Google AI subscription is out of Gemini quota for now: {}", e.message)
        }
        _ => {}
    }
    e
}

/// Send to the first Code Assist host that answers; a 401 refreshes the token once.
pub(crate) async fn send(state: &AppState, creds_file: &Path, call: &UpstreamCall) -> Result<UpstreamResponse, ProxyError> {
    let mut auth = state.gemini_auth(creds_file, false).await?;
    let call_body: Value = match &call.body {
        Some(b) => serde_json::from_slice(b).map_err(|e| ProxyError::api(format!("gemini request: {e}")))?,
        None => json!({}),
    };
    let mut forced = false;
    'auth: loop {
        let (method, body) = match call.endpoint {
            Endpoint::Models => ("fetchAvailableModels", json!({ "project": auth.project })),
            _ => ("streamGenerateContent?alt=sse", envelope(&call_body, &auth.project)),
        };
        let body = Bytes::from(serde_json::to_vec(&body).unwrap_or_default());
        let all = hosts(state);
        let mut last_err = None;
        for (i, host) in all.iter().enumerate() {
            let more = i + 1 < all.len();
            let url = format!("{host}/v1internal:{method}");
            let resp = match execute(state, Method::POST, &url, headers(&auth.access_token), Some(body.clone())).await {
                Ok(r) => r,
                Err(e) if more => {
                    last_err = Some(e);
                    continue;
                }
                Err(e) => return Err(e),
            };
            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED && !forced {
                tracing::info!("Gemini 401; refreshing the Google token and retrying");
                forced = true;
                auth = state.gemini_auth(creds_file, true).await?;
                continue 'auth;
            }
            if status.is_success() {
                if call.endpoint == Endpoint::Models {
                    return models_response(resp).await;
                }
                return finish(resp, None).await;
            }
            let resp_headers = resp.headers().clone();
            let bytes = resp.bytes().await.unwrap_or_default();
            let e = classify(status.as_u16(), &resp_headers, &bytes);
            // Another host may have it; a 4xx about the request or quota won't change.
            if more && (status == StatusCode::NOT_FOUND || status.is_server_error()) {
                tracing::debug!(%url, %status, "Code Assist host failed; trying the next");
                last_err = Some(e);
                continue;
            }
            return Err(e);
        }
        return Err(last_err.unwrap_or_else(|| ProxyError::new(ErrorKind::Api, "no Code Assist host configured")));
    }
}

/// `fetchAvailableModels` as an OpenAI-style `{data: [{id}]}` list.
async fn models_response(resp: reqwest::Response) -> Result<UpstreamResponse, ProxyError> {
    let status = resp.status();
    let headers = resp.headers().clone();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    let data: Vec<Value> = v
        .get("models")
        .and_then(Value::as_object)
        .map(|m| m.keys().map(|id| json!({ "id": id })).collect())
        .unwrap_or_default();
    let body = Bytes::from(serde_json::to_vec(&json!({ "data": data })).unwrap_or_default());
    Ok(UpstreamResponse { status, headers, body: Box::pin(stream::once(async move { Ok(body) })), account: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_wraps_request_with_project() {
        let e = envelope(&json!({ "model": "gemini-4-argon-medium", "request": { "contents": [] } }), "proj-1");
        assert_eq!(e["project"], "proj-1");
        assert_eq!(e["model"], "gemini-4-argon-medium");
        assert_eq!(e["userAgent"], "antigravity");
        assert_eq!(e["requestType"], "agent");
        assert!(e["requestId"].as_str().unwrap().starts_with("agent-"));
        assert_eq!(e["request"]["contents"], json!([]));
    }

    #[test]
    fn classifies_google_errors() {
        let body = json!({ "error": { "code": 429, "status": "RESOURCE_EXHAUSTED", "message": "Resource has been exhausted",
            "details": [ { "@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "RATE_LIMIT_EXCEEDED" },
                         { "@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "12.3s" } ] } });
        let e = classify(429, &HeaderMap::new(), body.to_string().as_bytes());
        assert_eq!(e.kind, ErrorKind::RateLimit);
        assert_eq!(e.retry_after.as_deref(), Some("13"));
        let e = classify(404, &HeaderMap::new(), br#"{"error":{"code":404,"message":"Requested entity was not found."}}"#);
        assert!(e.message.contains("Google AI Pro or Ultra"));
    }
}
