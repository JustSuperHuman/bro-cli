//! OpenAI-compatible `POST {base_url}/audio/transcriptions`: API-key lookup, a
//! hand-built multipart body, and HTTP errors reduced to one readable line.

use serde_json::Value;
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Legacy model retried when the default model is rejected (older accounts /
/// OpenAI-compatible servers that don't know it).
pub const FALLBACK_MODEL: &str = "whisper-1";

/// What to send besides the audio.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub keywords: Vec<String>,
}

/// `gpt-transcribe` takes `languages[]` and `keywords[]`; the older models take
/// a singular `language` and only a free-text `prompt`.
fn is_gpt_transcribe(model: &str) -> bool {
    model.starts_with("gpt-transcribe")
}

fn clean(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// The form fields for a request, in order (`file` is added separately).
pub fn fields(req: &Request) -> Vec<(&'static str, String)> {
    let mut out = vec![("model", req.model.clone()), ("response_format", "json".to_string())];
    let modern = is_gpt_transcribe(&req.model);
    if let Some(lang) = clean(&req.language) {
        out.push((if modern { "languages[]" } else { "language" }, lang.to_string()));
    }
    let keywords: Vec<&str> = req.keywords.iter().map(|k| k.trim()).filter(|k| !k.is_empty()).collect();
    let mut prompt = clean(&req.prompt).map(str::to_string);
    if modern {
        out.extend(keywords.iter().map(|k| ("keywords[]", k.to_string())));
    } else if !keywords.is_empty() {
        let vocab = format!("Vocabulary: {}.", keywords.join(", "));
        prompt = Some(match prompt {
            Some(p) => format!("{vocab} {p}"),
            None => vocab,
        });
    }
    if let Some(p) = prompt {
        out.push(("prompt", p));
    }
    out
}

/// A `multipart/form-data` body: the text fields, then `file` as `audio.wav`.
pub fn multipart(boundary: &str, fields: &[(&str, String)], wav: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(wav.len() + 512);
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes());
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn boundary() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("----bro-voice-{nanos:x}-{:x}", N.fetch_add(1, Ordering::Relaxed))
}

fn one_line(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

/// The API's `error.message` (or the raw body) as a single short line.
fn api_message(body: &str) -> (String, String) {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let err = &json["error"];
    let message = err["message"].as_str().or_else(|| err.as_str()).map(str::to_string).unwrap_or_else(|| body.to_string());
    let code = err["code"].as_str().or_else(|| err["type"].as_str()).unwrap_or("").to_string();
    (one_line(&message, 160), code)
}

/// One readable line for a failed HTTP response.
pub fn describe_http_error(status: u16, body: &str, key_env: &str) -> String {
    let (msg, code) = api_message(body);
    let detail = if msg.is_empty() { String::new() } else { format!(": {msg}") };
    match status {
        401 => format!("Transcription API rejected the key in {key_env} (401 unauthorized)"),
        403 => format!("Transcription access denied (403){detail}"),
        404 => format!("Transcription model or endpoint not found (404){detail}"),
        413 => "Recording too large for the transcription API (413)".to_string(),
        429 if code == "insufficient_quota" || msg.contains("quota") => {
            "Transcription quota exhausted (429): check billing on the API account".to_string()
        }
        429 => "Transcription rate limited (429): try again in a moment".to_string(),
        400 | 422 => format!("Transcription request rejected ({status}){detail}"),
        500..=599 => format!("Transcription server error ({status}){detail}"),
        _ => format!("Transcription failed (HTTP {status}){detail}"),
    }
}

/// Transport failures (no HTTP status) as one line.
pub fn describe_transport_error(err: &ureq::Error, url: &str) -> String {
    let host = url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or(url);
    match err {
        ureq::Error::Timeout(_) => format!("Transcription timed out after {}s", TIMEOUT.as_secs()),
        ureq::Error::HostNotFound => format!("Could not resolve {host}: check the network"),
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) => format!("Could not reach {host}: {}", one_line(&err.to_string(), 120)),
        other => format!("Transcription request failed: {}", one_line(&other.to_string(), 160)),
    }
}

/// The request failed because the server doesn't accept this model.
fn model_rejected(status: u16, body: &str) -> bool {
    let (msg, code) = api_message(body);
    matches!(status, 400 | 404) && (code.contains("model") || msg.to_ascii_lowercase().contains("model"))
}

/// Transcribe a WAV; `Ok("")` means the API heard nothing. If `allow_fallback`
/// and the server rejects the model, retries once with [`FALLBACK_MODEL`].
pub fn transcribe(base_url: &str, key: &str, key_env: &str, req: &Request, wav: &[u8], allow_fallback: bool) -> Result<String, String> {
    let url = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).http_status_as_error(false).build().into();
    let send = |req: &Request| -> Result<(u16, String), String> {
        let boundary = boundary();
        let body = multipart(&boundary, &fields(req), wav);
        let resp = agent
            .post(&url)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Accept", "application/json")
            .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send(&body[..])
            .map_err(|e| describe_transport_error(&e, &url))?;
        let status = resp.status().as_u16();
        let text = resp.into_body().read_to_string().unwrap_or_default();
        Ok((status, text))
    };
    let (mut status, mut text) = send(req)?;
    if allow_fallback && req.model != FALLBACK_MODEL && model_rejected(status, &text) {
        let fallback = Request { model: FALLBACK_MODEL.to_string(), ..req.clone() };
        (status, text) = send(&fallback)?;
    }
    if !(200..300).contains(&status) {
        return Err(describe_http_error(status, &text, key_env));
    }
    parse_text(&text)
}

/// `{"text": "..."}` -> trimmed text.
pub fn parse_text(body: &str) -> Result<String, String> {
    let json: Value = serde_json::from_str(body).map_err(|_| format!("Unexpected transcription response: {}", one_line(body, 120)))?;
    json["text"]
        .as_str()
        .map(|t| t.trim().to_string())
        .ok_or_else(|| format!("Unexpected transcription response: {}", one_line(body, 120)))
}

/// The API key: the process environment first, then the user/system variables in
/// the registry so a key set after the terminal started is still found.
pub fn resolve_key(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        return None;
    }
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).or_else(|| registry_environment_value(name))
}

#[cfg(windows)]
fn registry_environment_value(name: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    for key in [r"HKCU\Environment", r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment"] {
        let output = std::process::Command::new("reg")
            .args(["query", key, "/v", name])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let mut parts = line.trim().splitn(3, "    ");
            if parts.next().is_some_and(|f| f.eq_ignore_ascii_case(name))
                && parts.next().is_some_and(|kind| kind.starts_with("REG_"))
                && let Some(value) = parts.next().map(str::trim).filter(|v| !v.is_empty())
            {
                return Some(value.to_owned());
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn registry_environment_value(_name: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(model: &str) -> Request {
        Request {
            model: model.into(),
            language: Some("en".into()),
            prompt: Some("bro sessions".into()),
            keywords: vec!["Mal".into(), " ".into(), "Pinky".into()],
        }
    }

    #[test]
    fn fields_for_gpt_transcribe() {
        let f = fields(&req("gpt-transcribe"));
        assert_eq!(f, vec![
            ("model", "gpt-transcribe".to_string()),
            ("response_format", "json".to_string()),
            ("languages[]", "en".to_string()),
            ("keywords[]", "Mal".to_string()),
            ("keywords[]", "Pinky".to_string()),
            ("prompt", "bro sessions".to_string()),
        ]);
    }

    #[test]
    fn fields_for_older_models_fold_keywords_into_prompt() {
        let f = fields(&req("whisper-1"));
        assert!(f.contains(&("language", "en".to_string())));
        assert!(f.contains(&("prompt", "Vocabulary: Mal, Pinky. bro sessions".to_string())));
        assert!(!f.iter().any(|(k, _)| k.starts_with("keywords")));
        let bare = fields(&Request { model: "gpt-4o-mini-transcribe".into(), prompt: Some("  ".into()), ..Default::default() });
        assert_eq!(bare.len(), 2);
    }

    #[test]
    fn multipart_body_layout() {
        let body = multipart("XYZ", &[("model", "m".into()), ("prompt", "p q".into())], b"RIFFdata");
        let expect = "--XYZ\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nm\r\n\
                      --XYZ\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\np q\r\n\
                      --XYZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n\
                      RIFFdata\r\n--XYZ--\r\n";
        assert_eq!(String::from_utf8(body).unwrap(), expect);
        assert_ne!(boundary(), boundary());
    }

    #[test]
    fn http_error_lines() {
        let body = r#"{"error":{"message":"Incorrect API key provided: sk-abc.","type":"invalid_request_error","code":"invalid_api_key"}}"#;
        assert_eq!(describe_http_error(401, body, "OPENAI_API_KEY"), "Transcription API rejected the key in OPENAI_API_KEY (401 unauthorized)");
        let quota = r#"{"error":{"message":"You exceeded your current quota.","code":"insufficient_quota"}}"#;
        assert!(describe_http_error(429, quota, "K").contains("quota exhausted"));
        assert!(describe_http_error(429, "{}", "K").contains("rate limited"));
        let bad = r#"{"error":{"message":"Invalid file format.\n Supported formats: wav"}}"#;
        assert_eq!(describe_http_error(400, bad, "K"), "Transcription request rejected (400): Invalid file format. Supported formats: wav");
        assert_eq!(describe_http_error(502, "<html>bad gateway</html>", "K"), "Transcription server error (502): <html>bad gateway</html>");
        assert!(describe_http_error(418, "", "K").starts_with("Transcription failed (HTTP 418)"));
        let long = "x".repeat(500);
        assert!(describe_http_error(500, &long, "K").chars().count() < 220);
    }

    #[test]
    fn model_rejection_detection() {
        assert!(model_rejected(404, r#"{"error":{"message":"The model `gpt-transcribe` does not exist","code":"model_not_found"}}"#));
        assert!(model_rejected(400, r#"{"error":{"message":"Invalid model"}}"#));
        assert!(!model_rejected(400, r#"{"error":{"message":"Invalid file format"}}"#));
        assert!(!model_rejected(401, r#"{"error":{"message":"model"}}"#));
    }

    #[test]
    fn parses_text() {
        assert_eq!(parse_text(r#"{"text":"  hello Pinky \n"}"#), Ok("hello Pinky".to_string()));
        assert_eq!(parse_text(r#"{"text":""}"#), Ok(String::new()));
        assert!(parse_text("nope").is_err());
    }

    #[test]
    fn key_lookup_prefers_process_env() {
        let name = "BRO_VOICE_TEST_KEY_7F3A";
        // SAFETY: test-only variable name nothing else reads.
        unsafe { std::env::set_var(name, "  sk-test  ") };
        assert_eq!(resolve_key(name).as_deref(), Some("sk-test"));
        unsafe { std::env::remove_var(name) };
        assert_eq!(resolve_key(name), None);
        assert_eq!(resolve_key("bad name;"), None);
        assert_eq!(resolve_key(""), None);
    }

    /// Real round trip: 1 s of silence must be accepted (auth + model name).
    #[test]
    #[ignore = "network: needs OPENAI_API_KEY"]
    fn live_silence_round_trip() {
        let key = resolve_key("OPENAI_API_KEY").expect("OPENAI_API_KEY");
        let wav = crate::audio::wav(&vec![0i16; 16_000], 16_000);
        let req = Request { model: crate::DEFAULT_MODEL.into(), ..Default::default() };
        let text = transcribe(crate::DEFAULT_BASE_URL, &key, "OPENAI_API_KEY", &req, &wav, false).expect("transcribe");
        println!("silence -> {text:?}");
    }
}
