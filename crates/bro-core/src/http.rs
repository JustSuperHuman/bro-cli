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
    /// `retry-after` in seconds, when the server sent one as a number
    pub retry_after: Option<u64>,
}

impl JsonResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn finish(resp: ureq::http::Response<ureq::Body>) -> JsonResponse {
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let text = resp.into_body().read_to_string().unwrap_or_default();
    JsonResponse { status, body: serde_json::from_str(&text).unwrap_or(Value::Null), retry_after }
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

/// `GET url`: the status and the body as text.
pub fn get_text(url: &str, timeout: Duration) -> anyhow::Result<(u16, String)> {
    let resp = agent(timeout).get(url).call().with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    let text = resp.into_body().read_to_string().with_context(|| format!("reading {url}"))?;
    Ok((status, text))
}

/// `GET url` into the file at `to` (any size; ureq's text readers stop at 10 MB). Errors on a
/// non-2xx status.
pub fn download(url: &str, to: &std::path::Path, timeout: Duration) -> anyhow::Result<u64> {
    let resp = agent(timeout).get(url).header("user-agent", "bro").call().with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        anyhow::bail!("GET {url}: HTTP {status}");
    }
    let mut file = std::fs::File::create(to).with_context(|| format!("creating {}", to.display()))?;
    std::io::copy(&mut resp.into_body().into_reader(), &mut file).with_context(|| format!("downloading {url}"))
}

/// `POST url` with a JSON body.
pub fn post_json(url: &str, body: &Value, timeout: Duration) -> anyhow::Result<JsonResponse> {
    post_json_with(url, &[], body, timeout)
}

/// `POST url` with a JSON body and extra headers.
pub fn post_json_with(url: &str, headers: &[(&str, &str)], body: &Value, timeout: Duration) -> anyhow::Result<JsonResponse> {
    let mut req = agent(timeout).post(url).header("accept", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send_json(body).with_context(|| format!("POST {url}"))?;
    Ok(finish(resp))
}

/// `POST url` with a form-encoded body (OAuth token endpoints).
pub fn post_form(url: &str, form: &[(&str, &str)], timeout: Duration) -> anyhow::Result<JsonResponse> {
    let resp = agent(timeout)
        .post(url)
        .header("accept", "application/json")
        .send_form(form.iter().copied())
        .with_context(|| format!("POST {url}"))?;
    Ok(finish(resp))
}
