//! Shared harness for the end-to-end tests: a mock upstream axum server (port 0)
//! plus a real ProxyHandle (port 0) wired to it. No real network.
#![allow(dead_code)]

pub use axum::Router;
pub use axum::body::Bytes;
pub use axum::extract::State;
pub use axum::response::{IntoResponse, Response};
pub use bro_proxy::anthropic::{Accumulator, StreamEvent};
pub use bro_proxy::sse::SseParser;
pub use bro_proxy::{ProxyConfig, ProxyEvent, ProxyHandle, ProxyOptions, Route, Upstream};
pub use http::{HeaderMap, Method, StatusCode, Uri};
pub use parking_lot::Mutex;
pub use serde_json::{Value, json};
pub use std::path::{Path, PathBuf};
pub use std::sync::Arc;
pub use std::time::Duration;

pub const CHAT_SSE: &str = include_str!("../fixtures/chat_stream_tools.sse");
pub const RESPONSES_SSE: &str = include_str!("../fixtures/responses_stream_codex.sse");
pub const ANTHROPIC_SSE: &str = include_str!("../fixtures/anthropic_stream_tools.sse");

// ------------------------------------------------------------------ mock upstream

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: Method,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Value,
}

#[derive(Default)]
pub struct Mock {
    pub seen: Mutex<Vec<Seen>>,
}

impl Mock {
    pub fn last(&self, path_part: &str) -> Seen {
        self.seen
            .lock()
            .iter()
            .rev()
            .find(|s| s.path.contains(path_part))
            .cloned()
            .expect("request seen")
    }
    pub fn count(&self, path_part: &str) -> usize {
        self.seen
            .lock()
            .iter()
            .filter(|s| s.path.contains(path_part))
            .count()
    }
}

pub fn sse(body: &'static str) -> Response {
    ([(http::header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

pub fn json_resp(status: u16, v: Value) -> Response {
    (StatusCode::from_u16(status).unwrap(), axum::Json(v)).into_response()
}

pub async fn mock_handler(
    State(mock): State<Arc<Mock>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    mock.seen.lock().push(Seen {
        method: method.clone(),
        path: path.clone(),
        headers: headers.clone(),
        body: v.clone(),
    });
    let stream = v.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();

    if path.starts_with("/err429/") {
        return json_resp(
            429,
            json!({"error": {"message": "Rate limit reached for requests", "type": "requests", "code": "rate_limit_exceeded"}}),
        );
    }
    if path.starts_with("/err529/") {
        return json_resp(
            529,
            json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
        );
    }
    if path.starts_with("/earlyfail/") {
        return sse(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_x\"}}\n\nevent: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"You've hit your usage limit\"}}}\n\n",
        );
    }
    if path.starts_with("/pool/") && auth == "Bearer tok-a" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("anthropic-ratelimit-unified-reset", "4102444800")],
            axum::Json(json!({"type": "error", "error": {"type": "rate_limit_error", "message": "usage limit reached"}})),
        )
            .into_response();
    }
    if path.starts_with("/poolstream/") && auth == "Bearer tok-a" {
        // rate limit surfaced as the first SSE event, before any content
        return sse(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"rate limit\"}}\n\n",
        );
    }

    if path.ends_with("/chat/completions") {
        if stream {
            return sse(CHAT_SSE);
        }
        return json_resp(
            200,
            json!({
                "id": "chatcmpl-9", "object": "chat.completion", "created": 1, "model": v["model"],
                "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                    "role": "assistant", "content": "Checking.", "reasoning_content": "plan",
                    "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "Read", "arguments": "{\"path\":\"a\"}"}}]
                }}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 7, "total_tokens": 57}
            }),
        );
    }
    if path.ends_with("/responses") {
        if stream {
            return sse(RESPONSES_SSE);
        }
        return json_resp(
            200,
            json!({
                "id": "resp_j", "object": "response", "status": "completed",
                "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "hello"}]}],
                "usage": {"input_tokens": 9, "output_tokens": 2, "total_tokens": 11}
            }),
        );
    }
    if path.ends_with("/messages/count_tokens") {
        return json_resp(200, json!({"input_tokens": 42}));
    }
    if path.ends_with("/v1/messages") {
        if stream {
            return sse(ANTHROPIC_SSE);
        }
        return json_resp(
            200,
            json!({
                "id": "msg_j", "type": "message", "role": "assistant", "model": v["model"],
                "content": [
                    {"type": "thinking", "thinking": "hmm", "signature": "SIG"},
                    {"type": "text", "text": "Hi there"},
                    {"type": "tool_use", "id": "toolu_9", "name": "get_weather", "input": {"city": "Oslo"}}
                ],
                "stop_reason": "tool_use", "stop_sequence": null,
                "usage": {"input_tokens": 11, "output_tokens": 6, "cache_read_input_tokens": 100}
            }),
        );
    }
    if path.ends_with("/models") {
        if path.contains("/codex/") {
            return json_resp(
                200,
                json!({"models": [
                    {"slug": "gpt-5.5", "display_name": "GPT-5.5", "supported_reasoning_levels": [{"effort": "low"}, {"effort": "medium"}, {"effort": "high"}]},
                    {"slug": "gpt-5.4-mini", "display_name": "GPT-5.4-Mini", "supported_reasoning_levels": [{"effort": "medium"}]}
                ]}),
            );
        }
        return json_resp(
            200,
            json!({"object": "list", "data": [{"id": "mock-model"}]}),
        );
    }
    json_resp(
        404,
        json!({"error": {"message": format!("mock: no route {path}")}}),
    )
}

pub struct Env {
    pub mock: Arc<Mock>,
    pub mock_base: String,
    pub proxy: ProxyHandle,
    pub client: reqwest::Client,
    pub _tmp: tempfile::TempDir,
    pub pool_file: PathBuf,
}

pub async fn env_with(token: Option<&str>) -> Env {
    let mock = Arc::new(Mock::default());
    let app = Router::new()
        .fallback(mock_handler)
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let tmp = tempfile::tempdir().unwrap();
    let pool_file = tmp.path().join("usage.json");
    let opts = ProxyOptions {
        anthropic_base_url: format!("{mock_base}/pool"),
        chatgpt_base_url: format!("{mock_base}/codex"),
        openai_base_url: format!("{mock_base}/v1"),
        pool_usage_file: Some(pool_file.clone()),
        claude_token: Some(Arc::new(|dir: &Path, _force: bool| {
            Ok(format!(
                "tok-{}",
                dir.file_name().unwrap().to_string_lossy()
            ))
        })),
        codex_auth: Some(Arc::new(|_home: &Path, _force: bool| {
            Ok(bro_core::creds::CodexAuth {
                access_token: "chatgpt-at".into(),
                account_id: Some("acct-1".into()),
                api_key: None,
            })
        })),
        keepalive: Duration::from_secs(15),
    };
    let cfg = ProxyConfig {
        bind: "127.0.0.1".into(),
        port: 0,
        token: token.map(str::to_string),
    };
    let proxy = tokio::task::spawn_blocking(move || ProxyHandle::start_with(cfg, opts))
        .await
        .unwrap()
        .unwrap();
    Env {
        mock,
        mock_base,
        proxy,
        client: reqwest::Client::new(),
        _tmp: tmp,
        pool_file,
    }
}

pub async fn env() -> Env {
    env_with(None).await
}

pub fn route(
    id: &str,
    upstream: Upstream,
    default_model: Option<&str>,
    small: Option<&str>,
) -> Route {
    Route {
        id: id.into(),
        upstream,
        default_model: default_model.map(str::to_string),
        small_model: small.map(str::to_string),
        model_map: vec![],
        label: id.into(),
    }
}

impl Env {
    pub fn url(&self, route: &str, path: &str) -> String {
        format!("{}/r/{route}{path}", self.proxy.base_url())
    }
    pub async fn post(&self, route: &str, path: &str, body: Value) -> reqwest::Response {
        self.client
            .post(self.url(route, path))
            .header("x-api-key", "local-key")
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "interleaved-thinking-2025-05-14")
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    /// Wait until the recorder emitted `n` events (streams finish asynchronously).
    pub async fn events(&self, n: usize) -> Vec<ProxyEvent> {
        for _ in 0..200 {
            let ev = self.proxy.recent(500);
            if ev.len() >= n {
                return ev;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.proxy.recent(500)
    }
}

pub fn sse_events(body: &str) -> Vec<(Option<String>, String)> {
    let mut p = SseParser::new();
    let mut v: Vec<_> = p
        .push(body.as_bytes())
        .into_iter()
        .map(|e| (e.event, e.data))
        .collect();
    v.extend(p.finish().into_iter().map(|e| (e.event, e.data)));
    v
}

pub fn anthropic_request(stream: bool) -> Value {
    json!({
        "model": "claude-sonnet-4-5-20250929",
        "max_tokens": 4096,
        "stream": stream,
        "system": [{"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}}],
        "metadata": {"user_id": "user_x_session_1"},
        "tools": [{"name": "Read", "description": "read", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}}],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "read a and b"}, {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBOR"}}]}
        ]
    })
}

pub fn accumulate_anthropic(body: &str) -> (Vec<String>, Value) {
    let evs = sse_events(body);
    let names: Vec<String> = evs
        .iter()
        .map(|(e, _)| e.clone().unwrap_or_default())
        .collect();
    let mut acc = Accumulator::new();
    for (name, data) in &evs {
        let ev: StreamEvent =
            serde_json::from_str(data).unwrap_or_else(|e| panic!("bad event {name:?} {data}: {e}"));
        assert_eq!(
            Some(ev.name()),
            name.as_deref(),
            "event: header matches type"
        );
        acc.push(&ev);
    }
    (names, serde_json::to_value(acc.finish().unwrap()).unwrap())
}

pub fn chat_request(stream: bool) -> Value {
    json!({
        "model": "claude-sonnet-4-5",
        "stream": stream,
        "stream_options": {"include_usage": true},
        "max_tokens": 1000,
        "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}],
        "messages": [
            {"role": "system", "content": "Be brief."},
            {"role": "user", "content": [{"type": "text", "text": "weather?"}, {"type": "image_url", "image_url": {"url": "https://x/y.png"}}]}
        ]
    })
}

pub fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
