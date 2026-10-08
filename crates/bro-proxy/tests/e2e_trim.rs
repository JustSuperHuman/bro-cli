//! End-to-end: optional request compression (llmtrim) on a Claude OAuth passthrough route.

mod common;
use bro_proxy::TrimLevel;
use common::*;

const ANSI_LOG: &str = "\u{1b}[32m   Compiling\u{1b}[0m bro v2\n\u{1b}[1m\u{1b}[31merror\u{1b}[0m: mismatched types\n  0% [    ]\r 50% [==  ]\r100% [====] done\n";

/// Claude Code's shape: cached system, and the newest message carries the breakpoint.
fn turn_request(outputs: &[&str]) -> Value {
    let mut messages = vec![json!({"role": "user", "content": [{"type": "text", "text": "build it"}]})];
    for (i, out) in outputs.iter().enumerate() {
        let id = format!("toolu_{i}");
        messages.push(json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "cargo build"}}]}));
        messages.push(json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": out}]}));
    }
    messages.last_mut().unwrap()["content"][0]["cache_control"] = json!({"type": "ephemeral"});
    json!({
        "model": "claude-opus-5-5", "max_tokens": 32000, "stream": false,
        "system": [{"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}}],
        "metadata": {"user_id": "user_x_session_trim"},
        "tools": [{"name": "Bash", "description": "Run", "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}}}],
        "messages": messages,
    })
}

fn without_markers(v: &Value) -> String {
    let mut v = v.clone();
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("cache_control");
                m.values_mut().for_each(walk);
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    serde_json::to_string(&v).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn trim_is_off_by_default_and_toggles_live_with_a_stable_prefix() {
    let e = env().await;
    e.proxy.upsert_route(route("me", Upstream::ClaudeOAuth { config_dir: PathBuf::from("/x/work") }, None, None));
    assert_eq!(e.proxy.trim(), (false, TrimLevel::Safe));

    // Off: forwarded as received.
    let resp = e.post("me", "/v1/messages", turn_request(&[ANSI_LOG])).await;
    assert_eq!(resp.status(), 200);
    let seen = e.mock.last("/v1/messages");
    assert_eq!(seen.headers["authorization"], "Bearer tok-work", "same OAuth path as a pool/other-login route");
    assert_eq!(seen.body["messages"][2]["content"][0]["content"], ANSI_LOG);
    let ev = e.events(1).await;
    assert_eq!((ev[0].trim_tokens_before, ev[0].trim_saved()), (None, 0));

    // On: the newest tool output loses terminal noise; the event reports the estimate.
    e.proxy.set_trim(true, TrimLevel::Safe);
    assert_eq!(e.proxy.trim(), (true, TrimLevel::Safe));
    let resp = e.post("me", "/v1/messages", turn_request(&[ANSI_LOG])).await;
    assert_eq!(resp.status(), 200);
    let first = e.mock.last("/v1/messages").body;
    let out = first["messages"][2]["content"][0]["content"].as_str().unwrap();
    assert!(!out.contains('\u{1b}') && !out.contains('\r') && out.contains("error: mismatched types"));
    assert_eq!(first["messages"][2]["content"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(first["system"][0]["text"], "You are Claude Code.");
    let ev = e.events(2).await;
    let last = ev.last().unwrap();
    assert!(last.trim_saved() > 0, "{:?} -> {:?}", last.trim_tokens_before, last.trim_tokens_after);

    // Next turn: the earlier result is frozen in the cached prefix and replayed byte for byte.
    tokio::time::sleep(Duration::from_millis(100)).await; // memo record runs on a blocking thread
    let resp = e.post("me", "/v1/messages", turn_request(&[ANSI_LOG, "ok\n"])).await;
    assert_eq!(resp.status(), 200);
    let second = e.mock.last("/v1/messages").body;
    for i in 0..3 {
        assert_eq!(without_markers(&second["messages"][i]), without_markers(&first["messages"][i]), "message {i}");
    }

    // count_tokens sees the same compression.
    let resp = e.post("me", "/v1/messages/count_tokens", turn_request(&[ANSI_LOG])).await;
    assert_eq!(resp.status(), 200);
    let counted = e.mock.last("/messages/count_tokens").body;
    assert!(!counted["messages"][2]["content"][0]["content"].as_str().unwrap().contains('\u{1b}'));

    // Off again: untouched.
    e.proxy.set_trim(false, TrimLevel::Safe);
    e.post("me", "/v1/messages", turn_request(&[ANSI_LOG])).await;
    assert_eq!(e.mock.last("/v1/messages").body["messages"][2]["content"][0]["content"], ANSI_LOG);
}
