//! State-machine tests for the four streaming translators, driven by recorded
//! SSE sequences in `tests/fixtures`.

use super::reasoning_cache::ReasoningCache;
use super::stream_a2chat::{AnthropicToChat, ChatOut};
use super::stream_a2responses::AnthropicToResponses;
use super::stream_chat2a::ChatToAnthropic;
use super::stream_responses2a::ResponsesToAnthropic;
use crate::anthropic::{Accumulator, ContentBlock, Delta, StreamEvent};
use crate::sse::SseParser;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;

const CHAT_TOOLS: &str = include_str!("../../tests/fixtures/chat_stream_tools.sse");
const RESPONSES_CODEX: &str = include_str!("../../tests/fixtures/responses_stream_codex.sse");
const ANTHROPIC_TOOLS: &str = include_str!("../../tests/fixtures/anthropic_stream_tools.sse");
const ANTHROPIC_ERROR: &str = include_str!("../../tests/fixtures/anthropic_stream_error.sse");

/// Parse a fixture, feeding it in awkward 7-byte chunks to exercise buffering.
fn sse_data(fixture: &str) -> Vec<String> {
    let mut p = SseParser::new();
    let mut out = Vec::new();
    for chunk in fixture.as_bytes().chunks(7) {
        out.extend(p.push(chunk).into_iter().map(|e| e.data));
    }
    out.extend(p.finish().into_iter().map(|e| e.data));
    out
}

fn anthropic_events(fixture: &str) -> Vec<StreamEvent> {
    sse_data(fixture)
        .iter()
        .map(|d| serde_json::from_str(d).unwrap())
        .collect()
}

fn names(evs: &[StreamEvent]) -> Vec<&'static str> {
    evs.iter().map(|e| e.name()).collect()
}

/// Every Anthropic stream must obey the protocol clients rely on.
fn assert_well_formed(evs: &[StreamEvent]) {
    assert!(
        matches!(evs.first(), Some(StreamEvent::MessageStart { .. })),
        "first event is message_start"
    );
    let mut open: Option<usize> = None;
    let mut next = 0;
    for e in evs {
        match e {
            StreamEvent::ContentBlockStart { index, .. } => {
                assert!(open.is_none(), "block {index} opened while {open:?} open");
                assert_eq!(*index, next, "indices are sequential");
                open = Some(*index);
                next += 1;
            }
            StreamEvent::ContentBlockDelta { index, .. } => {
                assert_eq!(open, Some(*index), "delta for open block")
            }
            StreamEvent::ContentBlockStop { index } => {
                assert_eq!(open, Some(*index));
                open = None;
            }
            StreamEvent::MessageDelta { .. } | StreamEvent::MessageStop => assert!(open.is_none()),
            _ => {}
        }
    }
}

#[test]
fn chat_stream_to_anthropic() {
    let mut t = ChatToAnthropic::new("claude-sonnet-4-5");
    let mut evs = Vec::new();
    for d in sse_data(CHAT_TOOLS) {
        evs.extend(t.push(&d));
    }
    evs.extend(t.finish());
    assert_well_formed(&evs);
    assert_eq!(
        names(&evs),
        vec![
            "message_start",
            "ping",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    assert_eq!(
        evs[5],
        StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::SignatureDelta {
                signature: "bro.chat".into()
            }
        }
    );
    let mut acc = Accumulator::new();
    evs.iter().for_each(|e| acc.push(e));
    let m = serde_json::to_value(acc.finish().unwrap()).unwrap();
    assert_eq!(m["id"], "msg_abc");
    assert_eq!(m["model"], "claude-sonnet-4-5");
    assert_eq!(m["content"][0]["thinking"], "The user wants two files.");
    assert_eq!(m["content"][1]["text"], "Reading both.");
    assert_eq!(
        m["content"][2],
        json!({"type": "tool_use", "id": "call_1", "name": "Read", "input": {"path": "a.txt"}})
    );
    assert_eq!(m["content"][3]["input"], json!({"path": "b.txt"}));
    assert_eq!(m["stop_reason"], "tool_use");
    assert_eq!(m["usage"]["input_tokens"], 20);
    assert_eq!(m["usage"]["cache_read_input_tokens"], 100);
    assert_eq!(m["usage"]["output_tokens"], 40);
}

#[test]
fn chat_stream_edge_cases() {
    // Whole tool call in one chunk (Ollama), name arriving after args, no [DONE].
    let mut t = ChatToAnthropic::new("m");
    let mut evs = Vec::new();
    for d in [
        r#"{"id":"x","choices":[{"index":0,"delta":{"role":"assistant","reasoning":"r"}}]}"#,
        r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"arguments":"{\"a\":"}}]}}]}"#,
        r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"f","arguments":"1}"}}]}}]}"#,
        r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"id":"c2","function":{"name":"g","arguments":{"b":2}}}]},"finish_reason":"stop"}]}"#,
    ] {
        evs.extend(t.push(d));
    }
    evs.extend(t.finish());
    assert_well_formed(&evs);
    let mut acc = Accumulator::new();
    evs.iter().for_each(|e| acc.push(e));
    let m = acc.finish().unwrap();
    assert_eq!(m.content.len(), 3);
    assert!(
        matches!(&m.content[1], ContentBlock::ToolUse { name, input, .. } if name == "f" && input == &json!({"a": 1}))
    );
    assert!(
        matches!(&m.content[2], ContentBlock::ToolUse { id, input, .. } if id == "c2" && input == &json!({"b": 2}))
    );
    assert_eq!(m.stop_reason.as_deref(), Some("tool_use"));

    // In-stream error
    let mut t = ChatToAnthropic::new("m");
    let evs = t.push(r#"{"error":{"message":"Rate limit reached","type":"requests","code":"rate_limit_exceeded"}}"#);
    assert!(
        matches!(evs.last(), Some(StreamEvent::Error { error }) if error.kind == "rate_limit_error")
    );
    assert!(t.is_finished());
}

#[test]
fn responses_stream_to_anthropic() {
    let mut t = ResponsesToAnthropic::new("claude-sonnet-4-5");
    let mut evs = Vec::new();
    for d in sse_data(RESPONSES_CODEX) {
        evs.extend(t.push(&d));
    }
    evs.extend(t.finish());
    assert_well_formed(&evs);
    assert!(t.early_error.is_none());
    let mut acc = Accumulator::new();
    evs.iter().for_each(|e| acc.push(e));
    assert!(acc.stopped);
    let m = serde_json::to_value(acc.finish().unwrap()).unwrap();
    assert_eq!(m["id"], "msg_123");
    assert_eq!(
        m["content"][0],
        json!({"type": "thinking", "thinking": "**Planning** the fix\n\nthen run tests", "signature": "bro.rs:gAAAAENC"})
    );
    assert_eq!(m["content"][1]["text"], "Running tests.");
    assert_eq!(
        m["content"][2],
        json!({"type": "tool_use", "id": "call_XYZ", "name": "Bash", "input": {"command": "cargo test"}})
    );
    assert_eq!(m["stop_reason"], "tool_use");
    assert_eq!(
        m["usage"],
        json!({"input_tokens": 500, "output_tokens": 80, "cache_read_input_tokens": 1500})
    );
}

#[test]
fn responses_stream_done_only_and_failures() {
    // Items that only arrive as output_item.done (no deltas), plus a custom tool.
    let mut t = ResponsesToAnthropic::new("m");
    let mut evs = Vec::new();
    for d in [
        json!({"type": "response.created", "response": {"id": "resp_9"}}),
        json!({"type": "response.output_item.done", "item": {"type": "reasoning", "id": "rs", "summary": [], "encrypted_content": "E"}}),
        json!({"type": "response.output_item.done", "item": {"type": "message", "id": "m", "content": [{"type": "output_text", "text": "hi"}]}}),
        json!({"type": "response.output_item.added", "item": {"type": "custom_tool_call", "id": "ct", "call_id": "c9", "name": "apply_patch"}}),
        json!({"type": "response.custom_tool_call_input.delta", "item_id": "ct", "delta": "*** Begin \"Patch\"\n"}),
        json!({"type": "response.custom_tool_call_input.delta", "item_id": "ct", "delta": "end"}),
        json!({"type": "response.output_item.done", "item": {"type": "custom_tool_call", "id": "ct", "call_id": "c9", "name": "apply_patch", "input": "*** Begin \"Patch\"\nend"}}),
        json!({"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}}),
    ] {
        evs.extend(t.push(&d.to_string()));
    }
    assert_well_formed(&evs);
    let mut acc = Accumulator::new();
    evs.iter().for_each(|e| acc.push(e));
    let m = acc.finish().unwrap();
    assert_eq!(
        m.content[0],
        ContentBlock::Thinking {
            thinking: "".into(),
            signature: Some("bro.rs:E".into())
        }
    );
    assert_eq!(m.content[1], ContentBlock::text("hi"));
    assert!(
        matches!(&m.content[2], ContentBlock::ToolUse { input, .. } if input == &json!({"input": "*** Begin \"Patch\"\nend"}))
    );
    assert_eq!(m.stop_reason.as_deref(), Some("tool_use"));

    // Failure before any output → early_error for a proper HTTP status.
    let mut t = ResponsesToAnthropic::new("m");
    let evs = t.push(&json!({"type": "response.failed", "response": {"error": {"code": "rate_limit_exceeded", "message": "slow"}}}).to_string());
    assert!(
        matches!(evs.last(), Some(StreamEvent::Error { error }) if error.kind == "rate_limit_error")
    );
    assert_eq!(
        t.early_error.as_ref().unwrap().kind,
        crate::errors::ErrorKind::RateLimit
    );

    // Truncated stream
    let mut t = ResponsesToAnthropic::new("m");
    t.push(&json!({"type": "response.created", "response": {"id": "r"}}).to_string());
    t.push(&json!({"type": "response.output_text.delta", "delta": "par"}).to_string());
    let evs = t.finish();
    assert_eq!(names(&evs), vec!["content_block_stop", "error"]);
}

#[test]
fn anthropic_stream_to_chat() {
    let cache = Arc::new(ReasoningCache::new());
    let mut t = AnthropicToChat::new("claude-sonnet-4-5", true, Some(cache.clone()));
    let mut outs = Vec::new();
    for e in anthropic_events(ANTHROPIC_TOOLS) {
        outs.extend(t.push(&e));
    }
    outs.extend(t.finish());
    assert_eq!(outs.last(), Some(&ChatOut::Done));
    let chunks: Vec<Value> = outs
        .iter()
        .filter_map(|o| {
            if let ChatOut::Chunk(c) = o {
                Some(c.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        chunks[0]["choices"][0]["delta"],
        json!({"role": "assistant", "content": ""})
    );
    assert_eq!(chunks[0]["id"], "chatcmpl-01XYZ");
    assert!(
        chunks
            .iter()
            .all(|c| c["object"] == "chat.completion.chunk")
    );
    let delta = |i: usize| chunks[i]["choices"][0]["delta"].clone();
    assert_eq!(delta(1), json!({"reasoning_content": "I should check "}));
    assert_eq!(delta(3), json!({"content": "Let me look "}));
    assert_eq!(
        delta(5),
        json!({"tool_calls": [{"index": 0, "id": "toolu_01A", "type": "function", "function": {"name": "get_weather", "arguments": ""}}]})
    );
    let args: String = chunks
        .iter()
        .filter_map(|c| {
            c["choices"][0]["delta"]["tool_calls"][0]
                .clone()
                .as_object()
                .cloned()
        })
        .filter(|tc| tc["index"] == 0)
        .map(|tc| {
            tc["function"]["arguments"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_eq!(args, "{\"city\": \"Paris\"}");
    let n = chunks.len();
    assert_eq!(chunks[n - 2]["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(chunks[n - 1]["choices"], json!([]));
    assert_eq!(chunks[n - 1]["usage"]["prompt_tokens"], 3012);
    assert_eq!(chunks[n - 1]["usage"]["completion_tokens"], 89);
    assert_eq!(
        cache.get("toolu_01A").unwrap(),
        vec![ContentBlock::Thinking {
            thinking: "I should check the weather.".into(),
            signature: Some("EqQBCkgIARABGAIiQ".into())
        }]
    );
}

#[test]
fn anthropic_error_to_chat_and_responses() {
    let evs = anthropic_events(ANTHROPIC_ERROR);
    let mut c = AnthropicToChat::new("m", false, None);
    let outs: Vec<ChatOut> = evs.iter().flat_map(|e| c.push(e)).collect();
    assert!(
        matches!(&outs[outs.len() - 2], ChatOut::Chunk(v) if v["error"]["code"] == "server_is_overloaded")
    );
    assert_eq!(outs.last(), Some(&ChatOut::Done));

    let mut r = AnthropicToResponses::new("m", HashSet::new());
    let outs: Vec<(String, Value)> = evs.iter().flat_map(|e| r.push(e)).collect();
    let (name, body) = outs.last().unwrap();
    assert_eq!(name, "response.failed");
    assert_eq!(body["response"]["error"]["code"], "server_is_overloaded");
    assert_eq!(body["response"]["status"], "failed");
}

#[test]
fn anthropic_stream_to_responses() {
    let custom = HashSet::from(["apply_patch".to_string()]);
    let mut t = AnthropicToResponses::new("claude-opus", custom);
    let mut outs = Vec::new();
    for e in anthropic_events(ANTHROPIC_TOOLS) {
        outs.extend(t.push(&e));
    }
    outs.extend(t.finish());
    let kinds: Vec<&str> = outs.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.reasoning_summary_part.added",
            "response.reasoning_summary_text.delta",
            "response.reasoning_summary_text.delta",
            "response.reasoning_summary_text.done",
            "response.reasoning_summary_part.done",
            "response.output_item.done",
            "response.output_item.added",
            "response.content_part.added",
            "response.output_text.delta",
            "response.output_text.delta",
            "response.output_text.done",
            "response.content_part.done",
            "response.output_item.done",
            "response.output_item.added",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.delta",
            "response.function_call_arguments.done",
            "response.output_item.done",
            "response.output_item.added",
            "response.custom_tool_call_input.delta",
            "response.custom_tool_call_input.done",
            "response.output_item.done",
            "response.completed",
        ]
    );
    for (i, (_, body)) in outs.iter().enumerate() {
        assert_eq!(body["sequence_number"], i as u64);
    }
    assert_eq!(outs[0].1["response"]["id"], "resp_01XYZ");
    assert_eq!(
        outs[8].1["item"]["encrypted_content"],
        "bro.at:EqQBCkgIARABGAIiQ"
    );
    assert_eq!(
        outs[8].1["item"]["summary"][0]["text"],
        "I should check the weather."
    );
    assert_eq!(
        outs[15].1["item"]["content"][0]["text"],
        "Let me look that up."
    );
    assert_eq!(outs[21].1["item"]["arguments"], "{\"city\":\"Paris\"}");
    assert_eq!(outs[21].1["item"]["call_id"], "toolu_01A");
    assert_eq!(outs[25].1["item"]["type"], "custom_tool_call");
    assert_eq!(outs[25].1["item"]["input"], "*** Begin Patch\n");
    let done = &outs[26].1["response"];
    assert_eq!(done["status"], "completed");
    assert_eq!(done["output"].as_array().unwrap().len(), 4);
    assert_eq!(done["usage"]["input_tokens"], 3012);
    assert_eq!(done["usage"]["input_tokens_details"]["cached_tokens"], 3000);
    assert_eq!(done["usage"]["output_tokens"], 89);
}

#[test]
fn chained_responses_upstream_to_chat_client() {
    // Chat client on a Responses upstream goes through the Anthropic hub.
    let mut up = ResponsesToAnthropic::new("gpt-5.5");
    let mut down = AnthropicToChat::new("gpt-5.5", false, None);
    let mut outs = Vec::new();
    for d in sse_data(RESPONSES_CODEX) {
        for e in up.push(&d) {
            outs.extend(down.push(&e));
        }
    }
    let text: String = outs
        .iter()
        .filter_map(|o| {
            if let ChatOut::Chunk(c) = o {
                c["choices"][0]["delta"]["content"]
                    .as_str()
                    .map(String::from)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(text, "Running tests.");
    assert_eq!(outs.last(), Some(&ChatOut::Done));
}
