//! End-to-end: passthrough, pool failover, errors, auth, events and lifecycle.

mod common;
use common::*;

// ------------------------------------------------------------------ passthrough

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_passthrough_streams_bytes_unchanged() {
    let e = env().await;
    let mut r = route(
        "pt",
        Upstream::Anthropic {
            base_url: format!("{}/anthropic", e.mock_base),
            api_key: Some("ak-up".into()),
            bearer: true,
        },
        None,
        None,
    );
    r.model_map = vec![(
        "claude-sonnet-4-5-20250929".into(),
        "claude-sonnet-4-5".into(),
    )];
    e.proxy.upsert_route(r);
    let resp = e.post("pt", "/v1/messages", anthropic_request(true)).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        ANTHROPIC_SSE,
        "bytes pass through untouched"
    );
    let seen = e.mock.last("/anthropic/v1/messages");
    assert_eq!(seen.headers["authorization"], "Bearer ak-up");
    assert!(
        seen.headers.get("x-api-key").is_none(),
        "local key never forwarded"
    );
    assert_eq!(
        seen.headers["anthropic-beta"],
        "interleaved-thinking-2025-05-14"
    );
    assert_eq!(seen.body["model"], "claude-sonnet-4-5");
    assert_eq!(
        seen.body["system"][0]["cache_control"]["type"], "ephemeral",
        "body otherwise untouched"
    );
    let ev = e.events(1).await;
    assert_eq!(ev.last().unwrap().output_tokens, Some(89));
    assert_eq!(ev.last().unwrap().input_tokens, Some(3012));
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_passthrough_json_and_count_tokens_and_models() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "oa",
        Upstream::OpenAiChat {
            base_url: format!("{}/v1", e.mock_base),
            api_key: Some("sk".into()),
        },
        Some("gpt-4.1"),
        None,
    ));
    let mut req = chat_request(false);
    req["model"] = json!("gpt-4.1-mini:low");
    let v: Value = e
        .post("oa", "/v1/chat/completions", req)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["id"], "chatcmpl-9");
    let seen = e.mock.last("/chat/completions");
    assert_eq!(seen.body["model"], "gpt-4.1-mini");
    assert_eq!(seen.body["reasoning_effort"], "low");
    assert_eq!(seen.body["messages"][0]["content"], "Be brief.");

    // count_tokens: estimated for non-Anthropic upstreams
    let v: Value = e
        .post(
            "oa",
            "/v1/messages/count_tokens",
            json!({"model": "x", "messages": [{"role": "user", "content": "12345678"}]}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(v["input_tokens"].as_u64().unwrap() > 1);
    // models: route models + upstream list, dual-shape
    let v: Value = e
        .client
        .get(e.url("oa", "/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["object"], "list");
    assert_eq!(v["data"][0]["id"], "gpt-4.1");
    assert_eq!(v["data"][1]["id"], "mock-model");
    assert_eq!(v["data"][0]["type"], "model");

    // count_tokens forwarded for Anthropic upstreams
    e.proxy.upsert_route(route(
        "an",
        Upstream::Anthropic {
            base_url: format!("{}/anthropic", e.mock_base),
            api_key: None,
            bearer: false,
        },
        None,
        None,
    ));
    let v: Value = e
        .post(
            "an",
            "/v1/messages/count_tokens",
            json!({"model": "claude-x", "messages": []}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["input_tokens"], 42);
    assert_eq!(e.mock.last("count_tokens").method, Method::POST);
}

// ------------------------------------------------------------------ pool

#[tokio::test(flavor = "multi_thread")]
async fn pool_fails_over_on_rate_limit_before_bytes() {
    let e = env().await;
    let dirs: Vec<PathBuf> = vec!["C:/pool/accounts/a".into(), "C:/pool/accounts/b".into()];
    e.proxy.upsert_route(route(
        "pool",
        Upstream::ClaudePool {
            config_dirs: dirs.clone(),
        },
        None,
        None,
    ));

    // Make "a" the least-loaded pick by giving "b" history.
    std::fs::write(
        &e.pool_file,
        serde_json::to_vec(
            &json!({"usage": {"b": {"windowStart": chrono_now(), "windowRequests": 5}}}),
        )
        .unwrap(),
    )
    .unwrap();
    let resp = e
        .post("pool", "/v1/messages", anthropic_request(false))
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["x-pool-account"], "b");
    let m: Value = resp.json().await.unwrap();
    assert_eq!(m["content"][1]["text"], "Hi there");
    assert_eq!(e.mock.count("/pool/v1/messages"), 2);

    let usage: Value = serde_json::from_slice(&std::fs::read(&e.pool_file).unwrap()).unwrap();
    assert!(usage["usage"]["a"]["rateLimitedUntil"].as_i64().unwrap() >= 4_102_444_800_000);
    assert_eq!(usage["usage"]["b"]["windowRequests"], 6);
    let status = e.proxy.pool_status(&dirs);
    assert!(!status[0].available && status[1].available);
    let ev = e.events(1).await;
    assert_eq!(ev.last().unwrap().account.as_deref(), Some("b"));

    // sticky + a stays cooled down: streaming request goes straight to b
    let resp = e
        .post("pool", "/v1/messages", anthropic_request(true))
        .await;
    assert_eq!(resp.text().await.unwrap(), ANTHROPIC_SSE);
    assert_eq!(e.mock.count("/pool/v1/messages"), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn pool_fails_over_on_in_stream_rate_limit() {
    let e = env().await;
    // base url for this test's pool path
    let dirs: Vec<PathBuf> = vec!["C:/pool/a".into(), "C:/pool/b".into()];
    e.proxy.upsert_route(route(
        "pool",
        Upstream::ClaudePool { config_dirs: dirs },
        None,
        None,
    ));
    std::fs::write(
        &e.pool_file,
        serde_json::to_vec(
            &json!({"usage": {"b": {"windowStart": chrono_now(), "windowRequests": 5}}}),
        )
        .unwrap(),
    )
    .unwrap();
    // Point OAuth base at /poolstream via a second proxy with different options.
    let tmp = tempfile::tempdir().unwrap();
    let opts = ProxyOptions {
        anthropic_base_url: format!("{}/poolstream", e.mock_base),
        pool_usage_file: Some(tmp.path().join("usage.json")),
        claude_token: Some(Arc::new(|dir: &Path, _| {
            Ok(format!(
                "tok-{}",
                dir.file_name().unwrap().to_string_lossy()
            ))
        })),
        ..ProxyOptions::default()
    };
    std::fs::copy(&e.pool_file, tmp.path().join("usage.json")).unwrap();
    let p2 = tokio::task::spawn_blocking(move || {
        ProxyHandle::start_with(
            ProxyConfig {
                bind: "127.0.0.1".into(),
                port: 0,
                token: None,
            },
            opts,
        )
    })
    .await
    .unwrap()
    .unwrap();
    p2.upsert_route(route(
        "pool",
        Upstream::ClaudePool {
            config_dirs: vec!["C:/pool/a".into(), "C:/pool/b".into()],
        },
        None,
        None,
    ));
    let resp = e
        .client
        .post(format!("{}/r/pool/v1/messages", p2.base_url()))
        .json(&anthropic_request(true))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["x-pool-account"], "b");
    assert_eq!(resp.text().await.unwrap(), ANTHROPIC_SSE);
    p2.shutdown();
}

// ------------------------------------------------------------------ errors & auth

#[tokio::test(flavor = "multi_thread")]
async fn errors_render_in_inbound_dialect() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "r429",
        Upstream::OpenAiChat {
            base_url: format!("{}/err429/v1", e.mock_base),
            api_key: None,
        },
        Some("m"),
        None,
    ));
    e.proxy.upsert_route(route(
        "r529",
        Upstream::Anthropic {
            base_url: format!("{}/err529", e.mock_base),
            api_key: None,
            bearer: false,
        },
        None,
        None,
    ));

    let resp = e
        .post("r429", "/v1/messages", anthropic_request(false))
        .await;
    assert_eq!(resp.status(), 429);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(
        v,
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Rate limit reached for requests"}})
    );

    let resp = e
        .post("r529", "/v1/chat/completions", chat_request(true))
        .await;
    assert_eq!(resp.status(), 503);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["code"], "server_is_overloaded");

    let resp = e
        .post("r529", "/v1/messages", anthropic_request(false))
        .await;
    assert_eq!(resp.status(), 529);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["error"]["type"],
        "overloaded_error"
    );

    let resp = e
        .post("nope", "/v1/messages", anthropic_request(false))
        .await;
    assert_eq!(resp.status(), 404);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["error"]["type"],
        "not_found_error"
    );
    let resp = e
        .post("nope", "/v1/chat/completions", chat_request(false))
        .await;
    assert_eq!(resp.status(), 404);
    assert!(resp.json::<Value>().await.unwrap()["error"]["message"].is_string());

    let resp = e
        .client
        .post(e.url("r429", "/v1/messages"))
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["error"]["type"],
        "invalid_request_error"
    );

    let ev = e.events(3).await;
    assert!(ev.iter().any(|x| x.status == 429 && x.error.is_some()));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_auth_and_health() {
    let e = env_with(Some("s3cret")).await;
    e.proxy.upsert_route(route(
        "oa",
        Upstream::OpenAiChat {
            base_url: format!("{}/v1", e.mock_base),
            api_key: None,
        },
        Some("m"),
        None,
    ));
    let resp = e
        .post("oa", "/v1/chat/completions", chat_request(false))
        .await; // sends x-api-key: local-key
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["error"]["type"],
        "authentication_error"
    );
    let resp = e
        .client
        .post(e.url("oa", "/v1/chat/completions"))
        .bearer_auth("s3cret")
        .json(&chat_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let h: Value = e
        .client
        .get(format!("{}/health", e.proxy.base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(h["ok"], true);
    assert_eq!(h["routes"][0]["id"], "oa");
    assert_eq!(e.proxy.routes().len(), 1);
    e.proxy.remove_route("oa");
    assert!(e.proxy.routes().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn events_callbacks_and_shutdown() {
    let e = env().await;
    let got = Arc::new(Mutex::new(Vec::<ProxyEvent>::new()));
    let g = got.clone();
    e.proxy.on_event(Box::new(move |ev| g.lock().push(ev)));
    e.proxy.upsert_route(route(
        "ds",
        Upstream::OpenAiChat {
            base_url: format!("{}/v1", e.mock_base),
            api_key: None,
        },
        Some("m"),
        None,
    ));
    e.post("ds", "/v1/messages", anthropic_request(false)).await;
    e.events(1).await;
    assert_eq!(got.lock().len(), 1);
    assert_eq!(got.lock()[0].route_id, "ds");
    assert!(e.proxy.base_url().starts_with("http://127.0.0.1:"));
    let port = e.proxy.port();
    e.proxy.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        e.client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .is_err()
    );
}

#[test]
fn fixed_port_scans_upward() {
    let first = ProxyHandle::start(ProxyConfig {
        bind: "127.0.0.1".into(),
        port: 0,
        token: None,
    })
    .unwrap();
    let second = ProxyHandle::start(ProxyConfig {
        bind: "127.0.0.1".into(),
        port: first.port(),
        token: None,
    })
    .unwrap();
    assert!(second.port() > first.port() && second.port() <= first.port() + 20);
}
