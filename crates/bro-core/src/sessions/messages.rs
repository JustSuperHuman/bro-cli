//! The conversation inside a transcript, for "what did that chat say" questions:
//! user and assistant text in order, with tool calls reduced to one-line markers and
//! tool output, thinking, meta and injected context left out.

use crate::Harness;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// "user" | "assistant" | "tool" (a call marker such as `[Bash] cargo test`)
    pub role: String,
    pub text: String,
    /// RFC 3339 when the transcript records it
    pub at: Option<String>,
}

/// Tool-call markers keep only this much of their input.
const MARKER_CHARS: usize = 120;

/// The last `limit` messages of a transcript (0 = all). Blocking; reads the whole file.
pub fn read_messages(harness: Harness, path: &Path, limit: usize) -> std::io::Result<Vec<ChatMessage>> {
    let text = std::fs::read_to_string(path)?;
    let mut messages = match harness {
        Harness::Codex => parse_rollout(&text),
        _ => parse_claude(&text),
    };
    if limit > 0 && messages.len() > limit {
        messages.drain(..messages.len() - limit);
    }
    Ok(messages)
}

fn one_line(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let kept: String = flat.chars().take(limit).collect();
    format!("{kept}…")
}

/// Harness-injected text that is not something a person typed.
fn injected(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty()
        || [
            "<command-",
            "<local-command",
            "<system-reminder",
            "<environment_context",
            "<user_instructions",
            "<bash-",
            "Caveat:",
        ]
        .iter()
        .any(|prefix| t.starts_with(prefix))
}

fn push(out: &mut Vec<ChatMessage>, role: &str, text: String, at: Option<String>) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    // Consecutive assistant text blocks of one reply read as one message.
    if role == "assistant"
        && let Some(last) = out.last_mut()
        && last.role == "assistant"
    {
        last.text.push_str("\n\n");
        last.text.push_str(&text);
        return;
    }
    out.push(ChatMessage { role: role.into(), text, at });
}

fn tool_marker(name: &str, input: &Value) -> String {
    let detail = ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(|value| one_line(value, MARKER_CHARS))
        .unwrap_or_default();
    format!("[{name}] {detail}").trim().to_string()
}

fn parse_claude(text: &str) -> Vec<ChatMessage> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "user" | "assistant")
            || entry.get("isMeta").and_then(Value::as_bool) == Some(true)
            || entry.get("isSidechain").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let at = entry.get("timestamp").and_then(Value::as_str).map(str::to_owned);
        let Some(content) = entry.pointer("/message/content") else { continue };
        match content {
            Value::String(body) if kind == "user" => {
                if !injected(body) {
                    push(&mut out, "user", body.clone(), at);
                }
            }
            Value::String(body) => push(&mut out, "assistant", body.clone(), at),
            Value::Array(blocks) => {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let body = block.get("text").and_then(Value::as_str).unwrap_or("");
                            if kind == "assistant" {
                                push(&mut out, "assistant", body.into(), at.clone());
                            } else if !injected(body) {
                                push(&mut out, "user", body.into(), at.clone());
                            }
                        }
                        Some("tool_use") if kind == "assistant" => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = block.get("input").cloned().unwrap_or(Value::Null);
                            push(&mut out, "tool", tool_marker(name, &input), at.clone());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn parse_rollout(text: &str) -> Vec<ChatMessage> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        if entry.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let at = entry.get("timestamp").and_then(Value::as_str).map(str::to_owned);
        let Some(payload) = entry.get("payload") else { continue };
        match payload.get("type").and_then(Value::as_str) {
            Some("message") => {
                let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
                if !matches!(role, "user" | "assistant") {
                    continue;
                }
                let body = payload
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|block| block.get("text").and_then(Value::as_str))
                            .filter(|text| role == "assistant" || !injected(text))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                push(&mut out, role, body, at);
            }
            Some("function_call" | "custom_tool_call") => {
                let name = payload.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input = payload
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|args| serde_json::from_str(args).ok())
                    .unwrap_or_else(|| {
                        let raw = payload.get("input").and_then(Value::as_str).unwrap_or("");
                        serde_json::json!({ "command": raw })
                    });
                push(&mut out, "tool", tool_marker(name, &input), at);
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_transcript_keeps_conversation_and_drops_noise() {
        let text = [
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","timestamp":"2026-09-30T10:00:00Z","message":{"role":"user","content":"fix the build"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Looking."},{"type":"tool_use","name":"Bash","input":{"command":"cargo   build"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"meta"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Fixed."}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"All green."}]}}"#,
        ]
        .join("\n");
        let messages = parse_claude(&text);
        let roles: Vec<_> = messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
        assert_eq!(messages[0].at.as_deref(), Some("2026-09-30T10:00:00Z"));
        assert_eq!(messages[2].text, "[Bash] cargo build");
        assert_eq!(messages[3].text, "Fixed.\n\nAll green.");
    }

    #[test]
    fn codex_rollout_reads_messages_and_calls() {
        let text = [
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"restart the viewer"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":\"npm start\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Restarted."}]}}"#,
        ]
        .join("\n");
        let messages = parse_rollout(&text);
        let texts: Vec<_> = messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["restart the viewer", "[shell] npm start", "Restarted."]);
    }

    #[test]
    fn limit_keeps_the_newest_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let lines: Vec<String> = (0..5)
            .map(|i| format!(r#"{{"type":"user","message":{{"role":"user","content":"m{i}"}}}}"#))
            .collect();
        std::fs::write(&path, lines.join("\n")).unwrap();
        let messages = read_messages(Harness::Claude, &path, 2).unwrap();
        assert_eq!(messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["m3", "m4"]);
    }
}
