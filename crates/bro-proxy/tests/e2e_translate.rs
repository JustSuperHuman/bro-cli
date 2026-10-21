//! End-to-end: the four cross-dialect paths through a real proxy and a mock upstream.

mod common;
use common::*;

// ------------------------------------------------------------------ Anthropic client → Chat upstream

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_client_on_chat_upstream_streaming() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "ds",
        Upstream::OpenAiChat {
            base_url: format!("{}/v1", e.mock_base),
            api_key: Some("sk-up".into()),
        },
        Some("deepseek-reasoner"),
        None,
    ));
    let resp = e.post("ds", "/v1/messages", anthropic_request(true)).await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let (names, m) = accumulate_anthropic(&resp.text().await.unwrap());
    assert_eq!(names.first().unwrap(), "message_start");
    assert_eq!(names.last().unwrap(), "message_stop");
    assert_eq!(m["model"], "claude-sonnet-4-5-20250929");
    assert_eq!(m["content"][0]["thinking"], "The user wants two files.");
    assert_eq!(m["content"][1]["text"], "Reading both.");
    assert_eq!(m["content"][2]["input"], json!({"path": "a.txt"}));
    assert_eq!(m["content"][3]["id"], "call_2");
    assert_eq!(m["stop_reason"], "tool_use");

    let seen = e.mock.last("/chat/completions");
    assert_eq!(seen.headers["authorization"], "Bearer sk-up");
    assert_eq!(seen.body["model"], "deepseek-reasoner");
    assert_eq!(seen.body["stream_options"]["include_usage"], true);
    assert_eq!(
        seen.body["messages"][0],
        json!({"role": "system", "content": "You are Claude Code."})
    );
    assert_eq!(
        seen.body["messages"][1]["content"][1]["image_url"]["url"],
        "data:image/png;base64,iVBOR"
    );
    assert_eq!(seen.body["tools"][0]["function"]["name"], "Read");

    let ev = e.events(1).await;
    let last = ev.last().unwrap();
    assert_eq!(
        (last.inbound.as_str(), last.upstream.as_str(), last.status),
        ("anthropic", "openai_chat", 200)
    );
    assert_eq!(last.model, "deepseek-reasoner");
    assert!(last.stream);
    assert_eq!(last.input_tokens, Some(120));
    assert_eq!(last.output_tokens, Some(40));
    assert_eq!(last.cache_read_tokens, Some(100));
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_client_on_chat_upstream_json() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "ds",
        Upstream::OpenAiChat {
            base_url: format!("{}/v1", e.mock_base),
            api_key: None,
        },
        Some("deepseek-chat"),
        None,
    ));
    let resp = e.post("ds", "/v1/messages", anthropic_request(false)).await;
    assert_eq!(resp.status(), 200);
    let m: Value = resp.json().await.unwrap();
    assert_eq!(m["type"], "message");
    assert_eq!(
        m["content"][0],
        json!({"type": "thinking", "thinking": "plan", "signature": "bro.chat"})
    );
    assert_eq!(m["content"][1]["text"], "Checking.");
    assert_eq!(m["content"][2]["name"], "Read");
    assert_eq!(m["stop_reason"], "tool_use");
    assert_eq!(m["usage"]["input_tokens"], 50);
    assert!(
        e.mock
            .last("/chat/completions")
            .headers
            .get("authorization")
            .is_none()
    );
}

// ------------------------------------------------------------------ Anthropic client → ChatGPT Responses

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_client_on_chatgpt_codex() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "cx",
        Upstream::ChatGptCodex {
            codex_home: "C:/nonexistent/codex".into(),
        },
        None,
        None,
    ));

    // streaming
    let mut req = anthropic_request(true);
    req["thinking"] = json!({"type": "enabled", "budget_tokens": 31999});
    let resp = e.post("cx", "/v1/messages", req).await;
    assert_eq!(resp.status(), 200);
    let (names, m) = accumulate_anthropic(&resp.text().await.unwrap());
    assert_eq!(names.last().unwrap(), "message_stop");
    assert_eq!(m["content"][0]["signature"], "bro.rs:gAAAAENC");
    assert_eq!(m["content"][1]["text"], "Running tests.");
    assert_eq!(m["content"][2]["input"]["command"], "cargo test");

    let seen = e.mock.last("/codex/responses");
    assert_eq!(seen.headers["authorization"], "Bearer chatgpt-at");
    assert_eq!(seen.headers["chatgpt-account-id"], "acct-1");
    assert_eq!(seen.headers["originator"], "codex_cli_rs");
    assert_eq!(seen.headers["openai-beta"], "responses=experimental");
    assert!(seen.headers.contains_key("session_id"));
    // claude-sonnet → first model of the subscription list; effort clamped to its levels
    assert_eq!(seen.body["model"], "gpt-5.5");
    assert_eq!(
        seen.body["reasoning"],
        json!({"effort": "high", "summary": "auto"})
    );
    assert_eq!(seen.body["store"], false);
    assert_eq!(seen.body["stream"], true);
    assert_eq!(seen.body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(seen.body["instructions"], "You are Claude Code.");
    assert!(seen.body.get("max_output_tokens").is_none());
    assert!(seen.body["prompt_cache_key"].as_str().is_some());

    // non-streaming client, streaming-only backend → accumulated
    let mut req = anthropic_request(false);
    req["model"] = json!("claude-haiku-4-5");
    let resp = e.post("cx", "/v1/messages", req).await;
    assert_eq!(resp.status(), 200);
    let m: Value = resp.json().await.unwrap();
    assert_eq!(m["content"][1]["text"], "Running tests.");
    assert_eq!(m["usage"]["cache_read_input_tokens"], 1500);
    assert_eq!(
        e.mock.last("/codex/responses").body["model"],
        "gpt-5.4-mini"
    );

    // model list comes from the subscription
    let models: Value = e
        .client
        .get(e.url("cx", "/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["gpt-5.5", "gpt-5.4-mini"]);
    assert!(e.mock.last("/codex/models").path.contains("/codex/models"));
}

#[tokio::test(flavor = "multi_thread")]
async fn early_stream_failure_becomes_http_error() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "rs",
        Upstream::OpenAiResponses {
            base_url: format!("{}/earlyfail/v1", e.mock_base),
            api_key: None,
        },
        Some("gpt-5.5"),
        None,
    ));
    let resp = e.post("rs", "/v1/messages", anthropic_request(true)).await;
    assert_eq!(resp.status(), 429);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["type"], "rate_limit_error");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("usage limit")
    );
}

// ------------------------------------------------------------------ Chat client → Anthropic

#[tokio::test(flavor = "multi_thread")]
async fn chat_client_on_anthropic_upstream() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "an",
        Upstream::Anthropic {
            base_url: format!("{}/anthropic", e.mock_base),
            api_key: Some("ak-up".into()),
            bearer: false,
        },
        None,
        None,
    ));
    let resp = e
        .post("an", "/v1/chat/completions", chat_request(true))
        .await;
    assert_eq!(resp.status(), 200);
    let evs = sse_events(&resp.text().await.unwrap());
    assert_eq!(evs.last().unwrap().1, "[DONE]");
    let chunks: Vec<Value> = evs[..evs.len() - 1]
        .iter()
        .map(|(_, d)| serde_json::from_str(d).unwrap())
        .collect();
    assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
    let text: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect();
    assert_eq!(text, "Let me look that up.");
    let reasoning: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["reasoning_content"].as_str())
        .collect();
    assert_eq!(reasoning, "I should check the weather.");
    let finish: Vec<&str> = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["finish_reason"].as_str())
        .collect();
    assert_eq!(finish, vec!["tool_calls"]);
    let usage = &chunks.last().unwrap()["usage"];
    assert_eq!(usage["prompt_tokens"], 3012);
    assert_eq!(usage["completion_tokens"], 89);

    let seen = e.mock.last("/anthropic/v1/messages");
    assert_eq!(seen.headers["x-api-key"], "ak-up");
    assert_eq!(seen.headers["anthropic-version"], "2023-06-01");
    assert!(seen.headers.get("authorization").is_none());
    assert_eq!(seen.body["model"], "claude-sonnet-4-5");
    assert_eq!(seen.body["max_tokens"], 1000);
    assert_eq!(seen.body["system"][0]["text"], "Be brief.");
    assert_eq!(seen.body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(
        seen.body["messages"][0]["content"][1]["source"],
        json!({"type": "url", "url": "https://x/y.png"})
    );
    assert_eq!(
        seen.body["tools"][0]["input_schema"]["properties"]["city"]["type"],
        "string"
    );

    // non-streaming; then the tool turn comes back with the signed thinking restored
    let resp = e
        .post("an", "/v1/chat/completions", chat_request(false))
        .await;
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(v["choices"][0]["message"]["tool_calls"][0]["id"], "toolu_9");
    assert_eq!(v["usage"]["prompt_tokens"], 111);

    let mut follow = chat_request(false);
    follow["reasoning_effort"] = json!("medium");
    follow["max_tokens"] = json!(20000);
    follow["messages"].as_array_mut().unwrap().extend([
        json!({"role": "assistant", "content": "Hi there", "tool_calls": [{"id": "toolu_9", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Oslo\"}"}}]}),
        json!({"role": "tool", "tool_call_id": "toolu_9", "content": "sunny"}),
    ]);
    e.post("an", "/v1/chat/completions", follow).await;
    let seen = e.mock.last("/anthropic/v1/messages");
    assert_eq!(
        seen.body["messages"][1]["content"][0],
        json!({"type": "thinking", "thinking": "hmm", "signature": "SIG"})
    );
    assert_eq!(seen.body["thinking"]["type"], "enabled");
    assert_eq!(
        seen.body["messages"][2]["content"][0]["type"],
        "tool_result"
    );
}

// ------------------------------------------------------------------ Responses client (Codex) → Anthropic

#[tokio::test(flavor = "multi_thread")]
async fn responses_client_on_claude_oauth() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "co",
        Upstream::ClaudeOAuth {
            config_dir: "C:/x/work".into(),
        },
        Some("claude-opus-4-5"),
        None,
    ));
    let req = json!({
        "model": "gpt-5.5",
        "instructions": "You are Codex.",
        "stream": true,
        "prompt_cache_key": "conv-9",
        "tools": [
            {"type": "function", "name": "get_weather", "parameters": {"type": "object", "properties": {}}},
            {"type": "custom", "name": "apply_patch", "description": "patch files"}
        ],
        "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "go"}]}]
    });
    let resp = e.post("co", "/v1/responses", req.clone()).await;
    assert_eq!(resp.status(), 200);
    let evs = sse_events(&resp.text().await.unwrap());
    let kinds: Vec<&str> = evs.iter().map(|(k, _)| k.as_deref().unwrap()).collect();
    assert_eq!(kinds[0], "response.created");
    assert_eq!(*kinds.last().unwrap(), "response.completed");
    let items: Vec<Value> = evs
        .iter()
        .filter(|(k, _)| k.as_deref() == Some("response.output_item.done"))
        .map(|(_, d)| serde_json::from_str::<Value>(d).unwrap()["item"].clone())
        .collect();
    assert_eq!(items[0]["type"], "reasoning");
    assert_eq!(items[0]["encrypted_content"], "bro.at:EqQBCkgIARABGAIiQ");
    assert_eq!(items[1]["content"][0]["text"], "Let me look that up.");
    assert_eq!(items[2]["type"], "function_call");
    assert_eq!(items[2]["arguments"], "{\"city\":\"Paris\"}");
    assert_eq!(items[3]["type"], "custom_tool_call");
    assert_eq!(items[3]["input"], "*** Begin Patch\n");
    let done: Value = serde_json::from_str(&evs.last().unwrap().1).unwrap();
    assert_eq!(done["response"]["usage"]["input_tokens"], 3012);

    let seen = e.mock.last("/pool/v1/messages");
    assert_eq!(seen.headers["authorization"], "Bearer tok-work");
    let beta = seen.headers["anthropic-beta"].to_str().unwrap();
    assert!(
        beta.contains("oauth-2025-04-20") && beta.contains("claude-code-20250219"),
        "{beta}"
    );
    assert!(seen.headers.get("x-api-key").is_none());
    assert_eq!(seen.body["model"], "claude-opus-4-5");
    assert_eq!(
        seen.body["system"][0]["text"],
        "You are Claude Code, Anthropic's official CLI for Claude."
    );
    assert_eq!(seen.body["system"][1]["text"], "You are Codex.");
    assert_eq!(seen.body["metadata"]["user_id"], "conv-9");
    assert_eq!(
        seen.body["tools"][1]["input_schema"]["required"],
        json!(["input"])
    );

    // non-streaming
    let mut req = req;
    req["stream"] = json!(false);
    let v: Value = e
        .post("co", "/v1/responses", req)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["object"], "response");
    assert_eq!(v["status"], "completed");
    assert_eq!(v["output"][1]["content"][0]["text"], "Hi there");
    assert_eq!(v["output"][2]["call_id"], "toolu_9");
    assert_eq!(v["usage"]["input_tokens"], 111);
}

// ------------------------------------------------------------------ Gemini (Code Assist) upstream

fn gemini_sig() -> String {
    "GSIG".repeat(20)
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_client_on_gemini_code_assist() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "gm",
        Upstream::GeminiCodeAssist { creds_file: "C:/x/stale/oauth.json".into() },
        Some("gemini-4-argon"),
        None,
    ));
    let mut req = anthropic_request(true);
    req["thinking"] = json!({"type": "enabled", "budget_tokens": 31999});
    let resp = e.post("gm", "/v1/messages", req).await;
    assert_eq!(resp.status(), 200);
    let (names, m) = accumulate_anthropic(&resp.text().await.unwrap());
    assert_eq!(names.last().unwrap(), "message_stop");
    assert_eq!(m["model"], "claude-sonnet-4-5-20250929");
    assert_eq!(m["content"][0]["thinking"], "Need both files.");
    assert_eq!(m["content"][1]["text"], "Reading both.");
    assert_eq!(m["content"][2]["signature"], format!("bro.gs:{}", gemini_sig()));
    assert_eq!(m["content"][3]["input"], json!({"path": "a.txt"}));
    assert_eq!(m["content"][4]["input"], json!({"path": "b.txt"}));
    assert_eq!(m["stop_reason"], "tool_use");
    assert_eq!(m["usage"]["cache_read_input_tokens"], 200);

    // 401 with the stale token, then the refreshed one; the dead host was skipped
    let seen = e.mock.last(":streamGenerateContent");
    assert!(seen.path.starts_with("/ca/"));
    assert_eq!(seen.headers["authorization"], "Bearer google-at");
    assert!(seen.headers["user-agent"].to_str().unwrap().starts_with("antigravity/cli/"));
    assert_eq!(e.mock.count("/gone/"), 2);
    let b = &seen.body;
    assert_eq!(b["project"], "proj-7");
    assert_eq!(b["model"], "gemini-4-argon-high");
    assert_eq!(b["userAgent"], "antigravity");
    assert_eq!(b["requestType"], "agent");
    assert_eq!(b["request"]["systemInstruction"]["parts"][0]["text"], "You are Claude Code.");
    assert_eq!(b["request"]["contents"][0]["parts"][1]["inlineData"]["data"], "iVBOR");
    assert_eq!(b["request"]["tools"][0]["functionDeclarations"][0]["name"], "Read");
    assert_eq!(b["request"]["generationConfig"]["thinkingConfig"], json!({"includeThoughts": true, "thinkingLevel": "HIGH"}));

    let ev = e.events(1).await;
    let last = ev.last().unwrap();
    assert_eq!((last.inbound.as_str(), last.upstream.as_str(), last.status), ("anthropic", "gemini_code_assist", 200));
    assert_eq!(last.model, "gemini-4-argon");
    assert_eq!(last.input_tokens, Some(300));
    assert_eq!(last.output_tokens, Some(40));
}

#[tokio::test(flavor = "multi_thread")]
async fn responses_client_on_gemini_round_trips_signatures() {
    let e = env().await;
    e.proxy.upsert_route(route(
        "gm",
        Upstream::GeminiCodeAssist { creds_file: "C:/x/oauth.json".into() },
        Some("gemini-4-argon"),
        None,
    ));
    // non-streaming Codex request: Code Assist streams, the proxy folds it
    let req = json!({
        "model": "gemini-4-argon",
        "instructions": "You are Codex.",
        "reasoning": {"effort": "low"},
        "tools": [{"type": "function", "name": "Read", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}}],
        "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "read a"}]}]
    });
    let resp = e.post("gm", "/v1/responses", req.clone()).await;
    assert_eq!(resp.status(), 200);
    let r: Value = resp.json().await.unwrap();
    let out = r["output"].as_array().unwrap();
    let sig = format!("bro.at:bro.gs:{}", gemini_sig());
    assert!(out.iter().any(|i| i["type"] == "reasoning" && i["encrypted_content"] == sig.as_str()));
    let call = out.iter().find(|i| i["type"] == "function_call").unwrap().clone();
    assert_eq!(e.mock.last(":streamGenerateContent").body["model"], "gemini-4-argon-low");

    // Codex sends the turn back: the signature returns on the function call
    let mut next = req.clone();
    let input = next["input"].as_array_mut().unwrap();
    input.extend(out.iter().cloned());
    for c in out.iter().filter(|i| i["type"] == "function_call") {
        input.push(json!({"type": "function_call_output", "call_id": c["call_id"], "output": "A"}));
    }
    let resp = e.post("gm", "/v1/responses", next).await;
    assert_eq!(resp.status(), 200);
    let contents = e.mock.last(":streamGenerateContent").body["request"]["contents"].clone();
    let model_turn = contents.as_array().unwrap().iter().find(|c| c["role"] == "model").unwrap();
    let fc = model_turn["parts"].as_array().unwrap().iter().find(|p| p.get("functionCall").is_some()).unwrap();
    assert_eq!(fc["thoughtSignature"], gemini_sig());
    assert_eq!(fc["functionCall"]["id"], call["call_id"]);
    let last = contents.as_array().unwrap().last().unwrap();
    assert_eq!(last["parts"][0]["functionResponse"]["response"]["result"], "A");
    assert_eq!(last["parts"][1]["functionResponse"]["response"]["result"], "A");
}
