//! The installed Claude Code and Codex, run through proxy mode's routes against a mock Anthropic API and a mock
//! ChatGPT Codex backend: each stays signed in with its own (fake) login, and the request reaches the backend
//! signed as the pool's login instead. Everything is local: temp homes, fake tokens, no real account touched.
//!
//! `cargo test -p bro-proxy --test real_harnesses -- --ignored --nocapture`

mod common;
use base64::Engine;
use bro_proxy::{LoginFamily, PoolLogin};
use common::*;
use std::process::{Command, Stdio};

const ANSWER: &str = "PONG-FROM-THE-POOL";

fn anthropic_text() -> String {
    let ev = |name: &str, data: Value| format!("event: {name}\ndata: {data}\n\n");
    [
        ev("message_start", json!({"type": "message_start", "message": {"id": "msg_pool", "type": "message", "role": "assistant", "model": "claude-sonnet-4-5", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 12, "output_tokens": 1}}})),
        ev("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
        ev("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": ANSWER}})),
        ev("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
        ev("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 6}})),
        ev("message_stop", json!({"type": "message_stop"})),
    ]
    .concat()
}

fn responses_text() -> String {
    let ev = |name: &str, data: Value| format!("event: {name}\ndata: {data}\n\n");
    let item = json!({"id": "msg_pool", "type": "message", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": ANSWER, "annotations": []}]});
    [
        ev("response.created", json!({"type": "response.created", "response": {"id": "resp_pool", "object": "response", "status": "in_progress", "output": []}})),
        ev("response.output_item.added", json!({"type": "response.output_item.added", "output_index": 0, "item": {"id": "msg_pool", "type": "message", "role": "assistant", "status": "in_progress", "content": []}})),
        ev("response.content_part.added", json!({"type": "response.content_part.added", "item_id": "msg_pool", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}})),
        ev("response.output_text.delta", json!({"type": "response.output_text.delta", "item_id": "msg_pool", "output_index": 0, "content_index": 0, "delta": ANSWER})),
        ev("response.output_text.done", json!({"type": "response.output_text.done", "item_id": "msg_pool", "output_index": 0, "content_index": 0, "text": ANSWER})),
        ev("response.output_item.done", json!({"type": "response.output_item.done", "output_index": 0, "item": item})),
        ev("response.completed", json!({"type": "response.completed", "response": {"id": "resp_pool", "object": "response", "status": "completed", "output": [item], "usage": {"input_tokens": 20, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 6, "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 26}}})),
    ]
    .concat()
}

async fn backend(State(m): State<Arc<Mock>>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let path = uri.path().to_string();
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    m.seen.lock().push(Seen { method, path: path.clone(), headers, body: v.clone() });
    let stream = v.get("stream").and_then(Value::as_bool).unwrap_or(false);
    match path.as_str() {
        "/anthropic/v1/messages" if stream => ([(http::header::CONTENT_TYPE, "text/event-stream")], anthropic_text()).into_response(),
        "/anthropic/v1/messages" => json_resp(200, json!({"id": "msg_pool", "type": "message", "role": "assistant", "model": v["model"], "content": [{"type": "text", "text": ANSWER}], "stop_reason": "end_turn", "stop_sequence": null, "usage": {"input_tokens": 12, "output_tokens": 6}})),
        "/anthropic/v1/messages/count_tokens" => json_resp(200, json!({"input_tokens": 10})),
        "/codex/responses" => ([(http::header::CONTENT_TYPE, "text/event-stream")], responses_text()).into_response(),
        // Codex asks chatgpt.com which backend its workspace uses before it sends anything to a custom address
        "/backend-api/wham/accounts/check" => json_resp(200, json!({"accounts": [{"id": "home-acct", "workspace_backend_origin": "https://chatgpt.com", "account_routing_override": "NO_CONSTRAINT", "structure": "personal"}], "account_ordering": ["home-acct"], "default_account_id": "home-acct"})),
        _ => json_resp(404, json!({"error": {"message": format!("mock: no {path}")}})),
    }
}

struct Setup {
    mock: Arc<Mock>,
    /// the mock backend's own address
    base: String,
    proxy: ProxyHandle,
    tmp: tempfile::TempDir,
}

async fn setup() -> Setup {
    let mock = Arc::new(Mock::default());
    let app = Router::new().fallback(backend).with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let tmp = tempfile::tempdir().unwrap();
    let opts = ProxyOptions {
        anthropic_base_url: format!("{base}/anthropic"),
        chatgpt_base_url: format!("{base}/codex"),
        pool_usage_file: Some(tmp.path().join("usage.json")),
        codex_pool_usage_file: Some(tmp.path().join("codex-usage.json")),
        logins: Some(Arc::new(|family: LoginFamily| {
            let id = format!("{}:pool", family.as_str());
            vec![PoolLogin { id, dir: PathBuf::from("C:/fake/pool"), account: Some("pool-account-uuid".into()) }]
        })),
        claude_token: Some(Arc::new(|_: &Path, _| Ok("pool-oauth-token".into()))),
        codex_auth: Some(Arc::new(|_: &Path, _| Ok(bro_core::creds::CodexAuth { access_token: "pool-access-token".into(), account_id: Some("pool-acct".into()), api_key: None }))),
        ..ProxyOptions::default()
    };
    let cfg = ProxyConfig { bind: "127.0.0.1".into(), port: 0, token: Some("not-the-harness-token".into()) };
    let proxy = tokio::task::spawn_blocking(move || ProxyHandle::start_with(cfg, opts)).await.unwrap().unwrap();
    proxy.upsert_route(route("kclaude", Upstream::ClaudeLogins, None, None));
    proxy.upsert_route(route("kcodex", Upstream::CodexLogins, None, None));
    Setup { mock, base, proxy, tmp }
}

/// Run a harness (through bro's own shim handling) with only `env` added, no stdin, for at most two minutes.
fn run(program: &str, args: Vec<String>, env: &[(&str, String)], home: &Path) -> (bool, String, String) {
    let (exe, args) = bro_core::launch::program_and_args(program, args);
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).current_dir(home);
    for k in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "OPENAI_API_KEY", "OPENAI_BASE_URL", "BRO_SESSION_ID"] {
        cmd.env_remove(k);
    }
    cmd.env("HOME", home).env("USERPROFILE", home);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().expect("the harness starts");
    let out = std::thread::spawn(move || child.wait_with_output());
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !out.is_finished() {
        assert!(std::time::Instant::now() < deadline, "{program} took over two minutes");
        std::thread::sleep(Duration::from_millis(200));
    }
    let o = out.join().unwrap().unwrap();
    (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn hdr(h: &HeaderMap, k: &str) -> String {
    h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the installed Claude Code against a local mock"]
async fn real_claude_code_keeps_its_login_and_is_served_as_the_pools() {
    if bro_core::launch::which("claude").is_none() {
        eprintln!("claude isn't installed: skipped");
        return;
    }
    let s = setup().await;
    let home = s.tmp.path().join("home");
    let config = s.tmp.path().join("claude");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    // Claude Code checks a saved sign-in with Anthropic directly (profile, bootstrap), not through
    // ANTHROPIC_BASE_URL, and gives up on a fake one there. A long-lived subscription token
    // (`CLAUDE_CODE_OAUTH_TOKEN`, what `claude setup-token` makes) has nothing to refresh, so it goes on to the
    // request under test: the same OAuth bearer, OAuth beta and claude-cli identity as a saved login's.
    std::fs::write(config.join(".claude.json"), json!({"hasCompletedOnboarding": true, "bypassPermissionsModeAccepted": true, "oauthAccount": {"accountUuid": "home-account-uuid", "emailAddress": "home@example.com", "organizationUuid": "home-org-uuid"}}).to_string()).unwrap();
    let env = [
        ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-home-login".into()),
        ("CLAUDE_CONFIG_DIR", config.to_string_lossy().into_owned()),
        ("ANTHROPIC_BASE_URL", format!("{}/r/kclaude", s.proxy.base_url())),
        ("_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL", "1".into()),
        ("DISABLE_AUTOUPDATER", "1".into()),
        ("DISABLE_TELEMETRY", "1".into()),
        ("DISABLE_ERROR_REPORTING", "1".into()),
    ];
    let args = ["-p", "Reply with one word.", "--model", "claude-sonnet-4-5", "--output-format", "text"].map(String::from).to_vec();
    let s2 = s.mock.clone();
    let (ok, out, err) = tokio::task::spawn_blocking(move || run("claude", args, &env, &home)).await.unwrap();
    for r in s2.seen.lock().iter() {
        eprintln!("  backend: {} {} auth={}", r.method, r.path, hdr(&r.headers, "authorization"));
    }
    let seen: Vec<Seen> = s2.seen.lock().iter().filter(|r| r.path == "/anthropic/v1/messages").cloned().collect();
    eprintln!("claude ok={ok}\nstdout: {out}\nstderr: {err}\nbackend saw {} message request(s)", seen.len());
    for r in &seen {
        eprintln!("  auth={} ua={} beta={} model={}", hdr(&r.headers, "authorization"), hdr(&r.headers, "user-agent"), hdr(&r.headers, "anthropic-beta"), r.body["model"]);
    }
    assert!(!seen.is_empty(), "Claude Code's inference went through the proxy");
    let main = seen.iter().find(|r| r.body["model"] == "claude-sonnet-4-5").expect("the model it was told to use, as it sent it");
    assert_eq!(hdr(&main.headers, "authorization"), "Bearer pool-oauth-token", "signed as the pool's login, not its own");
    assert!(hdr(&main.headers, "user-agent").starts_with("claude-cli/"), "Claude Code's own identity");
    assert!(hdr(&main.headers, "anthropic-beta").contains("oauth-2025-04-20"), "its subscription sign-in, not an API key");
    let uid: Value = serde_json::from_str(main.body["metadata"]["user_id"].as_str().unwrap_or("{}")).unwrap_or(Value::Null);
    if uid.is_object() {
        assert_eq!(uid["account_uuid"], "pool-account-uuid", "names the serving account");
    }
    assert!(ok && out.contains(ANSWER), "Claude Code printed the pool's answer");
}

/// A JWT that Codex reads as a ChatGPT Pro login (never verified locally).
fn fake_jwt(claims: Value) -> String {
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!("{}.{}.sig", enc.encode(br#"{"alg":"none","typ":"JWT"}"#), enc.encode(claims.to_string()))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the installed Codex against a local mock"]
async fn real_codex_keeps_its_chatgpt_login_and_is_served_as_the_pools() {
    if bro_core::launch::which("codex").is_none() {
        eprintln!("codex isn't installed: skipped");
        return;
    }
    let s = setup().await;
    let home = s.tmp.path().join("home");
    let codex_home = s.tmp.path().join("codex");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&codex_home).unwrap();
    let auth_claims = json!({"chatgpt_plan_type": "pro", "chatgpt_account_id": "home-acct", "chatgpt_user_id": "user-home", "chatgpt_account_user_id": "user-home__home-acct"});
    let access = fake_jwt(json!({"exp": 4_102_444_800i64, "https://api.openai.com/auth": auth_claims, "https://api.openai.com/profile": {"email": "home@example.com"}}));
    let id_token = fake_jwt(json!({"exp": 4_102_444_800i64, "email": "home@example.com", "https://api.openai.com/auth": auth_claims}));
    let now = chrono::Utc::now().to_rfc3339();
    std::fs::write(codex_home.join("auth.json"), json!({"auth_mode": "chatgpt", "OPENAI_API_KEY": null, "tokens": {"id_token": id_token, "access_token": access, "refresh_token": "rt-home-login", "account_id": "home-acct"}, "last_refresh": now}).to_string()).unwrap();
    let base = format!("{}/r/kcodex/backend-api/codex", s.proxy.base_url());
    let args = vec![
        "exec".into(),
        "--skip-git-repo-check".into(),
        "-m".into(),
        "gpt-5.5".into(),
        "-c".into(),
        format!("openai_base_url={}", serde_json::to_string(&base).unwrap()),
        "-c".into(),
        "features.enable_request_compression=false".into(),
        // chatgpt.com's own endpoints (workspace discovery, apps) are the mock's here
        "-c".into(),
        format!("chatgpt_base_url={}", serde_json::to_string(&format!("{}/backend-api/", s.base)).unwrap()),
        "Reply with one word.".into(),
    ];
    let env = [("CODEX_HOME", codex_home.to_string_lossy().into_owned())];
    let s2 = s.mock.clone();
    let (ok, out, err) = tokio::task::spawn_blocking(move || run("codex", args, &env, &home)).await.unwrap();
    let seen: Vec<Seen> = s2.seen.lock().iter().filter(|r| r.path.starts_with("/codex/")).cloned().collect();
    eprintln!("codex ok={ok}\nstdout: {out}\nstderr: {err}\nbackend saw:");
    for r in &seen {
        eprintln!("  {} {} auth={} acct={} originator={} model={}", r.method, r.path, hdr(&r.headers, "authorization"), hdr(&r.headers, "chatgpt-account-id"), hdr(&r.headers, "originator"), r.body["model"]);
    }
    let main = seen.iter().find(|r| r.path == "/codex/responses").expect("Codex's request went through the proxy");
    assert_eq!(hdr(&main.headers, "authorization"), "Bearer pool-access-token", "signed as the pool's login");
    assert_eq!(hdr(&main.headers, "chatgpt-account-id"), "pool-acct");
    assert!(!hdr(&main.headers, "originator").is_empty(), "Codex's own headers go with it");
    assert_eq!(main.body["model"], "gpt-5.5", "the model it was told to use");
    assert!(ok && out.contains(ANSWER), "Codex printed the pool's answer");
}
