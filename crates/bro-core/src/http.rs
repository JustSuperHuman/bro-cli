//! Minimal blocking HTTP helpers over ureq: JSON in, (status, JSON) out, with a
//! per-call global timeout and no status-as-error (callers branch on status).
use anyhow::Context;
use serde_json::Value;
use std::time::Duration;

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into()
}

/// A response: HTTP status plus the body parsed as JSON (`Value::Null` when the body
/// isn't JSON).
#[derive(Debug, Clone)]
pub struct JsonResponse {
    pub status: u16,
    pub body: Value,
}

impl JsonResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn finish(resp: ureq::http::Response<ureq::Body>) -> JsonResponse {
    let status = resp.status().as_u16();
    let text = resp.into_body().read_to_string().unwrap_or_default();
    JsonResponse { status, body: serde_json::from_str(&text).unwrap_or(Value::Null) }
}

/// `GET url` with extra headers.
pub fn get_json(url: &str, headers: &[(&str, &str)], timeout: Duration) -> anyhow::Result<JsonResponse> {
    let mut req = agent(timeout).get(url).header("accept", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.call().with_context(|| format!("GET {url}"))?;
    Ok(finish(resp))
}

/// `POST url` with a JSON body.
pub fn post_json(url: &str, body: &Value, timeout: Duration) -> anyhow::Result<JsonResponse> {
    let resp = agent(timeout)
        .post(url)
        .header("accept", "application/json")
        .send_json(body)
        .with_context(|| format!("POST {url}"))?;
    Ok(finish(resp))
}
