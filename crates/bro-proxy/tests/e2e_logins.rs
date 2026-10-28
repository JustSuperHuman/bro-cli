//! End-to-end: proxy mode's shared logins (Claude Code and Codex keep their own requests; only the
//! credentials change; limits move a request to the next login before any byte is sent).

mod common;
use bro_proxy::{LoginFamily, PoolLogin};
use common::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct LMock {
    seen: Mutex<Vec<Seen>>,
    /// login b is at its limit too
    b_limited: AtomicBool,
}

impl LMock {
    fn all(&self, part: &str) -> Vec<Seen> {
        self.seen.lock().iter().filter(|s| s.path.contains(part)).cloned().collect()
    }
}

fn hdr(h: &HeaderMap, k: &str) -> String {
    h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

async fn lmock(State(m): State<Arc<LMock>>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let path = uri.path().to_string();
    let full = uri.path_and_query().map(|p| p.to_string()).unwrap_or_default();
    m.seen.lock().push(Seen { method, path: full, headers: headers.clone(), body: serde_json::from_slice(&body).unwrap_or(Value::Null) });
    let auth = hdr(&headers, "authorization");
    let limited = |who: &str| who == "a" || (who == "b" && m.b_limited.load(Ordering::SeqCst));
    match path.as_str() {
        "/anthropic/v1/messages" => {
            let who = auth.strip_prefix("Bearer tok-").unwrap_or("");
            if who.is_empty() {
                return json_resp(401, json!({"type": "error", "error": {"type": "authentication_error", "message": "bad token"}}));
            }
            if limited(who) {
                let reset = if who == "a" { "4102444800" } else { "4102448400" };
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("anthropic-ratelimit-unified-status", "rejected"), ("anthropic-ratelimit-unified-reset", reset), ("request-id", "req_limited")],
                    axum::Json(json!({"type": "error", "error": {"type": "rate_limit_error", "message": format!("limit reached for {who}")}})),
                )
                    .into_response();
            }
            (
                [
                    (http::header::CONTENT_TYPE, "text/event-stream"),
                    (http::HeaderName::from_static("anthropic-ratelimit-unified-status"), "allowed"),
                    (http::HeaderName::from_static("anthropic-ratelimit-unified-5h-utilization"), "0.25"),
                    (http::HeaderName::from_static("anthropic-ratelimit-unified-7d-utilization"), "0.4"),
                ],
                ANTHROPIC_SSE,
            )
                .into_response()
        }
        "/anthropic/v1/models" => json_resp(200, json!({"data": [{"id": "claude-opus-4-8", "type": "model"}], "has_more": false})),
        "/codex/responses" => {
            let who = auth.strip_prefix("Bearer at-").unwrap_or("");
            if limited(who) {
                return json_resp(429, json!({"error": {"type": "usage_limit_reached", "message": "The usage limit has been reached", "plan_type": "pro", "resets_in_seconds": 3600}}));
            }
            (
                [
                    (http::header::CONTENT_TYPE, "text/event-stream"),
                    (http::HeaderName::from_static("x-codex-primary-used-percent"), "12.5"),
                    (http::HeaderName::from_static("x-codex-primary-window-minutes"), "300"),
                    (http::HeaderName::from_static("x-codex-secondary-used-percent"), "30"),
                    (http::HeaderName::from_static("x-codex-secondary-window-minutes"), "10080"),
                ],
                RESPONSES_SSE,
            )
                .into_response()
        }
        "/codex/models" => ([(http::header::ETAG, "\"models-v7\"")], axum::Json(json!({"models": [{"slug": "gpt-5.5"}, {"slug": "gpt-5.6-sol"}]}))).into_response(),
        _ => json_resp(404, json!({"error": {"message": format!("mock: no route {path}")}})),
    }
}

struct LEnv {
    mock: Arc<LMock>,
    proxy: ProxyHandle,
    client: reqwest::Client,
    _tmp: tempfile::TempDir,
    claude_file: PathBuf,
    codex_file: PathBuf,
}

const TOKEN: &str = "proxy-secret";

async fn lenv() -> LEnv {
    let mock = Arc::new(LMock::default());
    let app = Router::new().fallback(lmock).with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let tmp = tempfile::tempdir().unwrap();
    let claude_file = tmp.path().join("usage.json");
    let codex_file = tmp.path().join("codex-usage.json");
    // "b" has history, so "a" is the first pick of a fresh conversation
    std::fs::write(&claude_file, serde_json::to_vec(&json!({"usage": {"b": {"windowStart": chrono_now(), "windowRequests": 5}}})).unwrap()).unwrap();
    std::fs::write(&codex_file, serde_json::to_vec(&json!({"usage": {"codex:b": {"windowStart": chrono_now(), "windowRequests": 5}}})).unwrap()).unwrap();
    let opts = ProxyOptions {
        anthropic_base_url: format!("{base}/anthropic"),
        chatgpt_base_url: format!("{base}/codex"),
        pool_usage_file: Some(claude_file.clone()),
        codex_pool_usage_file: Some(codex_file.clone()),
        logins: Some(Arc::new(|family: LoginFamily| {
            let (prefix, root) = match family {
                LoginFamily::Claude => ("claude", "C:/pool/accounts"),
                LoginFamily::Codex => ("codex", "C:/codex-profiles"),
            };
            ["a", "b"]
                .iter()
                .map(|n| PoolLogin { id: format!("{prefix}:{n}"), dir: PathBuf::from(format!("{root}/{n}")), account: Some(format!("uuid-{n}")) })
                .collect()
        })),
        claude_token: Some(Arc::new(|dir: &Path, _| Ok(format!("tok-{}", dir.file_name().unwrap().to_string_lossy())))),
        codex_auth: Some(Arc::new(|home: &Path, _| {
            let n = home.file_name().unwrap().to_string_lossy().to_string();
            Ok(bro_core::creds::CodexAuth { access_token: format!("at-{n}"), account_id: Some(format!("acct-{n}")), api_key: None })
        })),
        ..ProxyOptions::default()
    };
    let cfg = ProxyConfig { bind: "127.0.0.1".into(), port: 0, token: Some(TOKEN.into()) };
    let proxy = tokio::task::spawn_blocking(move || ProxyHandle::start_with(cfg, opts)).await.unwrap().unwrap();
    proxy.upsert_route(route("kclaude", Upstream::ClaudeLogins, None, None));
    proxy.upsert_route(route("kcodex", Upstream::CodexLogins, None, None));
    LEnv { mock, proxy, client: reqwest::Client::new(), _tmp: tmp, claude_file, codex_file }
}

impl LEnv {
    fn url(&self, route: &str, path: &str) -> String {
        format!("{}/r/{route}{path}", self.proxy.base_url())
    }
    /// Claude Code as it calls its backend: its own OAuth token, identity and betas, no proxy key.
    async fn claude_code(&self, body: Value) -> reqwest::Response {
        self.client
            .post(self.url("kclaude", "/v1/messages"))
            .header("authorization", "Bearer home-oauth-token")
            .header("user-agent", "claude-cli/2.1.311 (external, cli)")
            .header("x-app", "cli")
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14")
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    async fn events(&self, n: usize) -> Vec<ProxyEvent> {
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

fn claude_code_body(session: &str) -> Value {
    let user_id = json!({"device_id": "dev-1", "account_uuid": "uuid-home", "session_id": session}).to_string();
    json!({
        "model": "claude-opus-4-8",
        "max_tokens": 32000,
        "stream": true,
        "system": [{"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude.", "cache_control": {"type": "ephemeral"}}],
        "metadata": {"user_id": user_id},
        "messages": [{"role": "user", "content": "hi"}]
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_code_keeps_its_request_and_moves_off_a_limited_login() {
    let e = lenv().await;
    let resp = e.claude_code(claude_code_body("s1")).await;
    assert_eq!(resp.status(), 200, "a proxy-mode route needs no proxy key from loopback");
    assert_eq!(resp.headers()["x-pool-account"], "claude:b");
    assert_eq!(resp.headers()["anthropic-ratelimit-unified-5h-utilization"], "0.25", "the harness reads its limits from the serving login");
    assert_eq!(resp.text().await.unwrap(), ANTHROPIC_SSE, "stream bytes pass through untouched");

    let seen = e.mock.all("/anthropic/v1/messages");
    assert_eq!(seen.len(), 2, "a was limited, b served");
    assert_eq!(hdr(&seen[0].headers, "authorization"), "Bearer tok-a");
    let b = &seen[1];
    assert_eq!(hdr(&b.headers, "authorization"), "Bearer tok-b", "only the credentials change");
    assert_eq!(hdr(&b.headers, "user-agent"), "claude-cli/2.1.311 (external, cli)");
    assert_eq!(hdr(&b.headers, "x-app"), "cli");
    let betas = hdr(&b.headers, "anthropic-beta");
    assert!(betas.contains("interleaved-thinking-2025-05-14") && betas.contains("oauth-2025-04-20"), "{betas}");
    assert_eq!(b.body["model"], "claude-opus-4-8", "the model Claude Code picked goes as it is");
    assert_eq!(b.body["system"].as_array().unwrap().len(), 1, "Claude Code's own system prompt, nothing added");
    let uid: Value = serde_json::from_str(b.body["metadata"]["user_id"].as_str().unwrap()).unwrap();
    assert_eq!((uid["account_uuid"].as_str(), uid["session_id"].as_str()), (Some("uuid-b"), Some("s1")), "names the serving account");

    let ev = e.events(1).await;
    assert_eq!(ev.last().unwrap().account.as_deref(), Some("claude:b"));
    let usage: Value = serde_json::from_slice(&std::fs::read(&e.claude_file).unwrap()).unwrap();
    assert_eq!(usage["usage"]["a"]["rateLimitedUntil"], 4_102_444_800_000i64, "cooled until its reset");
    assert_eq!(usage["usage"]["b"]["windowRequests"], 6, "counted under v1's account name");
    let status = e.proxy.logins_status(LoginFamily::Claude);
    let (a, b) = (&status[0], &status[1]);
    assert!(!a.available && b.available);
    assert_eq!((b.five_hour_pct, b.weekly_pct, b.conversations), (Some(25.0), Some(40.0), 1));

    // the same conversation (and its subagents) stays on b without asking a again
    let mut sub = claude_code_body("child");
    let sub_id = json!({"device_id": "dev-1", "account_uuid": "uuid-home", "session_id": "child", "parent_session_id": "s1"}).to_string();
    sub["metadata"]["user_id"] = json!(sub_id);
    assert_eq!(e.claude_code(sub).await.headers()["x-pool-account"], "claude:b");
    assert_eq!(e.mock.all("/anthropic/v1/messages").len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn when_every_claude_login_is_out_claude_code_gets_the_soonest_reset_as_sent() {
    let e = lenv().await;
    e.mock.b_limited.store(true, Ordering::SeqCst);
    let resp = e.claude_code(claude_code_body("s2")).await;
    assert_eq!(resp.status(), 429);
    assert_eq!(resp.headers()["anthropic-ratelimit-unified-status"], "rejected", "its own limit headers, for its own message");
    assert_eq!(resp.headers()["anthropic-ratelimit-unified-reset"], "4102444800", "a resets first");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["message"], "limit reached for a", "the upstream's body, as it came");

    // both cooling: the next request asks only the one that resets first
    let resp = e.claude_code(claude_code_body("s3")).await;
    assert_eq!(resp.status(), 429);
    let seen = e.mock.all("/anthropic/v1/messages");
    assert_eq!(seen.len(), 3);
    assert_eq!(hdr(&seen[2].headers, "authorization"), "Bearer tok-a");
}

#[tokio::test(flavor = "multi_thread")]
async fn other_clients_are_presented_as_claude_code() {
    let e = lenv().await;
    // Pi through the proxy: Anthropic Messages with the proxy's own key
    let resp = e
        .client
        .post(e.url("kclaude", "/v1/messages"))
        .header("x-api-key", TOKEN)
        .json(&json!({"model": "claude-sonnet-5", "max_tokens": 100, "stream": true, "system": "Be brief.", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let b = e.mock.all("/anthropic/v1/messages").pop().unwrap();
    assert_eq!(hdr(&b.headers, "authorization"), "Bearer tok-b");
    assert!(b.headers.get("x-api-key").is_none(), "the proxy key never leaves");
    assert!(hdr(&b.headers, "user-agent").starts_with("claude-cli/"));
    assert!(hdr(&b.headers, "anthropic-beta").contains("claude-code-20250219"));
    assert_eq!(b.body["system"][0]["text"], "You are Claude Code, Anthropic's official CLI for Claude.");
    assert_eq!(b.body["system"][1]["text"], "Be brief.");

    // the model list comes from the backend; the proxy key isn't shown to it
    let v: Value = e.client.get(e.url("kclaude", "/v1/models")).header("x-api-key", TOKEN).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["data"][0]["id"], "claude-opus-4-8");
    let m = e.mock.all("/anthropic/v1/models").pop().unwrap();
    assert!(m.headers.get("x-api-key").is_none() && m.headers.get("authorization").is_none());

    // routes that aren't proxy mode's still want the key
    e.proxy.upsert_route(route("plain", Upstream::Anthropic { base_url: "http://127.0.0.1:9".into(), api_key: None, bearer: false }, None, None));
    let resp = e.client.post(e.url("plain", "/v1/messages")).json(&json!({"model": "x", "messages": []})).send().await.unwrap();
    assert_eq!(resp.status(), 401);
    // and /health, which answers anyone, doesn't hand out a shared route's id (it is that route's key)
    let health: Value = e.client.get(format!("{}/health", e.proxy.base_url())).send().await.unwrap().json().await.unwrap();
    let ids: Vec<&str> = health["routes"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str()).collect();
    assert!(ids.contains(&"plain") && !ids.contains(&"kclaude") && !ids.contains(&"kcodex"), "{health}");
}

fn codex_body() -> Value {
    json!({
        "model": "gpt-5.5",
        "instructions": "You are Codex.",
        "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
        "tools": [],
        "reasoning": {"effort": "high", "summary": "auto"},
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
        "prompt_cache_key": "thread-1"
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_keeps_its_request_and_moves_off_a_spent_login() {
    let e = lenv().await;
    let resp = e
        .client
        .post(e.url("kcodex", "/backend-api/codex/responses"))
        .header("authorization", "Bearer home-access-token")
        .header("chatgpt-account-id", "home-acct")
        .header("originator", "codex_cli_rs")
        .header("version", "0.164.0")
        .header("session_id", "thread-1")
        .header("openai-beta", "responses=experimental")
        .json(&codex_body())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["x-codex-primary-used-percent"], "12.5", "Codex reads its limits from the serving login");
    assert_eq!(resp.text().await.unwrap(), RESPONSES_SSE);

    let seen = e.mock.all("/codex/responses");
    assert_eq!(seen.len(), 2, "a was spent, b served");
    assert_eq!((hdr(&seen[0].headers, "authorization").as_str(), hdr(&seen[0].headers, "chatgpt-account-id").as_str()), ("Bearer at-a", "acct-a"));
    let b = &seen[1];
    assert_eq!((hdr(&b.headers, "authorization").as_str(), hdr(&b.headers, "chatgpt-account-id").as_str()), ("Bearer at-b", "acct-b"));
    for (k, v) in [("originator", "codex_cli_rs"), ("version", "0.164.0"), ("session_id", "thread-1"), ("openai-beta", "responses=experimental")] {
        assert_eq!(hdr(&b.headers, k), v, "Codex's own {k} goes with it");
    }
    assert_eq!(b.body, codex_body(), "the body goes as Codex sent it");

    let usage: Value = serde_json::from_slice(&std::fs::read(&e.codex_file).unwrap()).unwrap();
    let until = usage["usage"]["codex:a"]["rateLimitedUntil"].as_i64().unwrap();
    assert!((until - chrono_now() - 3_600_000).abs() < 60_000, "cooled for the reset the backend gave");
    let status = e.proxy.logins_status(LoginFamily::Codex);
    assert_eq!((status[1].five_hour_pct, status[1].weekly_pct), (Some(12.5), Some(30.0)));

    // its WebSocket attempt gets 426, so it uses HTTP at once
    let ws = e.client.get(e.url("kcodex", "/backend-api/codex/responses")).header("upgrade", "websocket").header("connection", "Upgrade").send().await.unwrap();
    assert_eq!(ws.status(), 426);

    // /model lists exactly what the harness's own login gets
    let resp = e.client.get(e.url("kcodex", "/backend-api/codex/models?client_version=0.164.0")).header("authorization", "Bearer home-access-token").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()[http::header::ETAG], "\"models-v7\"");
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["models"][1]["slug"], "gpt-5.6-sol");
    let m = e.mock.all("/codex/models").pop().unwrap();
    assert_eq!(m.path, "/codex/models?client_version=0.164.0");
    assert_eq!(hdr(&m.headers, "authorization"), "Bearer home-access-token", "its own credentials");
}

#[tokio::test(flavor = "multi_thread")]
async fn when_every_codex_login_is_spent_codex_gets_the_backends_own_answer() {
    let e = lenv().await;
    e.mock.b_limited.store(true, Ordering::SeqCst);
    let resp = e
        .client
        .post(e.url("kcodex", "/backend-api/codex/responses"))
        .header("authorization", "Bearer home-access-token")
        .header("originator", "codex_cli_rs")
        .json(&codex_body())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["type"], "usage_limit_reached", "Codex shows its own usage-limit message");
    assert_eq!(v["error"]["resets_in_seconds"], 3600);
}
