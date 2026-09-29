//! Pulling a title / cwd / branch out of the head of a session file, per harness
//! (v1 `describeTranscript` and `describeRollout`, plus Pi/omp session headers).
use serde_json::Value;

/// Long prompts are truncated everywhere they're shown (v1 `MAX_TITLE`).
pub const MAX_TITLE: usize = 200;

/// What a transcript head says about its session. `title` is empty when the session
/// holds no real prompt (an abandoned start) — callers drop those.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Described {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub branch: String,
    /// Codex only: false for sub-agent threads and `codex exec` runs.
    pub interactive: bool,
}

/// Wrapper text Claude Code injects around slash commands, hook output and images.
const CLAUDE_NOISE: [&str; 10] = [
    "<local-command-caveat>",
    "<command-name>",
    "<command-message>",
    "<local-command-stdout>",
    "<user-prompt-submit-hook>",
    "<system-reminder>",
    "<bash-input>",
    "<bash-stdout>",
    "Caveat: The messages below",
    "Base directory for this skill:",
];

/// Text Codex injects around the conversation.
const CODEX_NOISE: [&str; 11] = [
    "# AGENTS.md instructions",
    "<environment_context",
    "<user_instructions",
    "<codex_internal_context",
    "<recommended_plugins",
    "<plugin",
    "<skill>",
    "<command-name>",
    "<command-message>",
    "<system-reminder",
    "<INSTRUCTIONS>",
];

fn is_noise(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

fn truncate(s: &str) -> String {
    s.chars().take(MAX_TITLE).collect()
}

/// Strip `[Image: …]` preambles and collapse whitespace (v1 `clean`).
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("[Image:") {
        out.push_str(&rest[..i]);
        out.push(' ');
        match rest[i..].find(']') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                // No closing bracket: v1's regex wouldn't match, keep the text.
                out.pop();
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "/name args" for a session opened by a slash command (v1 `commandTitle`).
pub fn command_title(s: &str) -> String {
    let Some(i) = s.find("<command-name>") else { return String::new() };
    let after = s[i + "<command-name>".len()..].trim_start();
    let name: String = after.chars().take_while(|c| *c != '<' && !c.is_whitespace()).collect();
    if name.is_empty() {
        return String::new();
    }
    let args = s
        .find("<command-args>")
        .map(|j| s[j + "<command-args>".len()..].trim_start())
        .map(|a| a.split('<').next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    let slash = if name.starts_with('/') { "" } else { "/" };
    if args.is_empty() { format!("{slash}{name}") } else { format!("{slash}{name} {args}") }
}

/// Text of a message's content: a plain string, or the text blocks of a block array
/// whose `type` is one of `kinds`.
fn text_of(content: Option<&Value>, kinds: &[&str]) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str).is_some_and(|t| kinds.contains(&t)))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Recover a snippet from a line the head read cut mid-object: the first
/// `marker` string literal, 2..=300 escaped chars (v1's regex fallback).
fn truncated_snippet(line: &str, marker: &str) -> Option<String> {
    let start = line.find(marker)? + marker.len();
    let mut captured = String::new();
    let mut count = 0;
    let mut chars = line[start..].chars();
    while count < 300 {
        let Some(c) = chars.next() else { break };
        match c {
            '"' => break,
            '\\' => {
                let Some(next) = chars.next() else { break };
                captured.push('\\');
                captured.push(next);
            }
            other => captured.push(other),
        }
        count += 1;
    }
    if count < 2 {
        return None;
    }
    serde_json::from_str::<String>(&format!("\"{captured}\"")).ok()
}

/// v1 `describeTranscript`: a Claude Code transcript head → title/cwd/branch.
pub fn describe_transcript(text: &str) -> Described {
    let (mut title, mut command, mut cwd, mut branch) = (String::new(), String::new(), String::new(), String::new());
    for line in text.split('\n') {
        if !line.starts_with('{') {
            continue;
        }
        let entry: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                if title.is_empty() && line.contains("\"type\":\"user\"") {
                    if let Some(snippet) = truncated_snippet(line, "\"type\":\"text\",\"text\":\"").map(|s| clean(&s))
                        && !snippet.is_empty()
                        && !is_noise(&snippet, &CLAUDE_NOISE)
                    {
                        title = truncate(&snippet);
                    }
                    if title.is_empty() && line.contains("\"type\":\"image\"") && command.is_empty() {
                        command = "(image)".into();
                    }
                }
                continue;
            }
        };
        if cwd.is_empty()
            && let Some(c) = entry.get("cwd").and_then(Value::as_str)
        {
            cwd = c.to_string();
        }
        if branch.is_empty()
            && let Some(b) = entry.get("gitBranch").and_then(Value::as_str)
        {
            branch = b.to_string();
        }
        let kind = entry.get("type").and_then(Value::as_str);
        if kind == Some("summary")
            && let Some(summary) = entry.get("summary").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
        {
            title = truncate(&clean(summary));
            if !cwd.is_empty() {
                break;
            }
            continue;
        }
        let truthy = |k: &str| entry.get(k).is_some_and(|v| v.as_bool().unwrap_or(!v.is_null()));
        if !title.is_empty() || kind != Some("user") || truthy("isMeta") || truthy("isSidechain") {
            continue;
        }
        let content = entry.pointer("/message/content");
        let raw = text_of(content, &["text"]);
        let body = clean(&raw);
        if body.is_empty() {
            let has_image = content
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("image")));
            if has_image && command.is_empty() {
                command = "(image)".into();
            }
            continue;
        }
        if is_noise(&body, &CLAUDE_NOISE) {
            if command.is_empty() {
                command = command_title(&raw);
            }
        } else {
            title = truncate(&body);
        }
    }
    Described { title: if title.is_empty() { command } else { title }, cwd, branch, ..Default::default() }
}

/// Byte ranges of `<image …>` / `</image …>` tags (case-insensitive, word boundary).
fn image_tags(s: &str) -> Vec<(usize, usize)> {
    let lower = s.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = lower[i..].find('<') {
        let start = i + off;
        let mut j = start + 1;
        if bytes.get(j) == Some(&b'/') {
            j += 1;
        }
        let is_tag = lower[j..].starts_with("image")
            && !bytes.get(j + 5).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_');
        if is_tag && let Some(end) = lower[j..].find('>') {
            out.push((start, j + end + 1));
            i = j + end + 1;
            continue;
        }
        i = start + 1;
    }
    out
}

fn strip_images(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (a, b) in image_tags(s) {
        out.push_str(&s[last..a]);
        out.push(' ');
        last = b;
    }
    out.push_str(&s[last..]);
    out
}

/// v1 `isInteractive`: sub-agent threads and `codex exec` runs aren't resumable work.
fn is_interactive(meta: Option<&Value>) -> bool {
    let Some(meta) = meta else { return false };
    if meta.get("thread_source").and_then(Value::as_str) == Some("subagent") {
        return false;
    }
    if meta.get("source").is_some_and(Value::is_object) {
        return false;
    }
    meta.get("originator").and_then(Value::as_str) != Some("codex_exec") && meta.get("source").and_then(Value::as_str) != Some("exec")
}

/// v1 `describeRollout`: a Codex rollout head → id/title/cwd/branch/interactive.
pub fn describe_rollout(text: &str) -> Described {
    let (mut title, mut command) = (String::new(), String::new());
    let mut meta: Option<Value> = None;
    for line in text.split('\n') {
        if !line.starts_with('{') {
            continue;
        }
        let entry: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                if title.is_empty() && line.contains("\"role\":\"user\"") {
                    if let Some(snippet) =
                        truncated_snippet(line, "\"type\":\"input_text\",\"text\":\"").map(|s| clean(&strip_images(&s)))
                        && !snippet.is_empty()
                        && !is_noise(&snippet, &CODEX_NOISE)
                    {
                        title = truncate(&snippet);
                    }
                    if title.is_empty() && line.contains("\"type\":\"input_image\"") && command.is_empty() {
                        command = "(image)".into();
                    }
                }
                continue;
            }
        };
        let payload = entry.get("payload").cloned().unwrap_or(Value::Null);
        let kind = entry.get("type").and_then(Value::as_str);
        if meta.is_none() && kind == Some("session_meta") {
            meta = Some(payload.clone());
        }
        if !title.is_empty() && meta.is_some() {
            break;
        }
        if !title.is_empty() {
            continue;
        }
        let ptype = payload.get("type").and_then(Value::as_str);
        let raw = if ptype == Some("message") && payload.get("role").and_then(Value::as_str) == Some("user") {
            text_of(payload.get("content"), &["input_text", "text"])
        } else if kind == Some("event_msg") && ptype == Some("user_message") {
            payload.get("message").and_then(Value::as_str).unwrap_or("").to_string()
        } else {
            String::new()
        };
        if raw.is_empty() {
            continue;
        }
        let body = clean(&strip_images(&raw));
        if body.is_empty() {
            if !image_tags(&raw).is_empty() && command.is_empty() {
                command = "(image)".into();
            }
            continue;
        }
        if is_noise(&body, &CODEX_NOISE) {
            if command.is_empty() {
                command = command_title(&raw);
            }
        } else {
            title = truncate(&body);
        }
    }
    let m = meta.as_ref();
    let s = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
    Described {
        id: s(m.and_then(|m| m.get("session_id")).or_else(|| m.and_then(|m| m.get("id")))),
        title: if title.is_empty() { command } else { title },
        cwd: s(m.and_then(|m| m.get("cwd"))),
        branch: s(m.and_then(|m| m.pointer("/git/branch"))),
        interactive: is_interactive(m),
    }
}

/// A Pi / omp session head: optional `{"type":"title"}` line, a `{"type":"session",
/// "id","cwd","title"?}` header, then `{"type":"message","message":{role,content}}`
/// entries (and `compaction` entries carrying a `shortSummary`). Title preference:
/// explicit title, first user prompt, compaction summary.
pub fn describe_pi(text: &str) -> Described {
    let mut out = Described { interactive: true, ..Default::default() };
    let (mut explicit, mut first, mut summary) = (String::new(), String::new(), String::new());
    for line in text.split('\n') {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        match entry.get("type").and_then(Value::as_str) {
            Some("title") if explicit.is_empty() => {
                explicit = entry.get("title").and_then(Value::as_str).unwrap_or("").to_string();
            }
            Some("session") if out.id.is_empty() => {
                out.id = entry.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                out.cwd = entry.get("cwd").and_then(Value::as_str).unwrap_or("").to_string();
                if explicit.is_empty() {
                    explicit = entry.get("title").and_then(Value::as_str).unwrap_or("").to_string();
                }
            }
            Some("compaction") if summary.is_empty() => {
                summary = entry.get("shortSummary").and_then(Value::as_str).unwrap_or("").to_string();
            }
            Some("message") if first.is_empty() && entry.pointer("/message/role").and_then(Value::as_str) == Some("user") => {
                let body = clean(&text_of(entry.pointer("/message/content"), &["text"]));
                if !body.is_empty() && !is_noise(&body, &CLAUDE_NOISE) {
                    first = body;
                }
            }
            _ => {}
        }
    }
    let pick = [explicit, first, summary].into_iter().map(|s| clean(&s)).find(|s| !s.is_empty()).unwrap_or_default();
    out.title = truncate(&pick);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jsonl(entries: &[Value]) -> String {
        entries.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn transcript_skips_noise_and_meta() {
        let text = jsonl(&[
            json!({"type": "user", "isMeta": true, "cwd": "F:\\proj", "gitBranch": "main", "message": {"content": "meta stuff"}}),
            json!({"type": "user", "message": {"content": "<system-reminder>ignore me</system-reminder>"}}),
            json!({"type": "user", "message": {"content": "<command-name>/review</command-name>\n<command-args> 42 </command-args>"}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "hi"}]}}),
            json!({"type": "user", "isSidechain": true, "message": {"content": "sidechain"}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "x"}, {"type": "text", "text": "[Image: 100x200]  Fix   the\nbug"}]}}),
        ]);
        let d = describe_transcript(&text);
        assert_eq!(d.title, "Fix the bug");
        assert_eq!(d.cwd, "F:\\proj");
        assert_eq!(d.branch, "main");
    }

    #[test]
    fn transcript_falls_back_to_command_or_image() {
        let text = jsonl(&[
            json!({"type": "user", "message": {"content": "<command-message>review</command-message>\n<command-name>review</command-name>\n<command-args>PR 7</command-args>"}}),
            json!({"type": "user", "message": {"content": "Base directory for this skill: /x"}}),
        ]);
        assert_eq!(describe_transcript(&text).title, "/review PR 7");
        let img = jsonl(&[json!({"type": "user", "message": {"content": [{"type": "image", "source": {}}]}})]);
        assert_eq!(describe_transcript(&img).title, "(image)");
        assert_eq!(describe_transcript("").title, "");
    }

    #[test]
    fn transcript_summary_and_truncated_line() {
        let text = jsonl(&[json!({"type": "summary", "summary": "Refactor the parser"}), json!({"type": "user", "cwd": "/p", "message": {"content": "later"}})]);
        let d = describe_transcript(&text);
        assert_eq!(d.title, "Refactor the parser");
        assert_eq!(d.cwd, "/p");
        let cut = r#"{"type":"user","message":{"content":[{"type":"text","text":"Look at this \"pic\" please"},{"type":"image","source":{"data":"AAAA"#;
        assert_eq!(describe_transcript(cut).title, "Look at this \"pic\" please");
        let long = format!("{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}", "x".repeat(500));
        assert_eq!(describe_transcript(&long).title.chars().count(), MAX_TITLE);
    }

    #[test]
    fn rollout_description() {
        let text = jsonl(&[
            json!({"type": "session_meta", "payload": {"id": "01a0ed53-d984-7ad0-badc-5b7024210c1a", "cwd": "F:\\z4", "originator": "codex-tui", "source": "vscode", "git": {"branch": "dev"}}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "sys"}]}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>\n<cwd>x</cwd>"}]}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "# AGENTS.md instructions for x"}]}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<image name=[Image #1] path=a.png></image> Explain this"}]}}),
        ]);
        let d = describe_rollout(&text);
        assert_eq!(d.id, "01a0ed53-d984-7ad0-badc-5b7024210c1a");
        assert_eq!(d.title, "Explain this");
        assert_eq!(d.cwd, "F:\\z4");
        assert_eq!(d.branch, "dev");
        assert!(d.interactive);
    }

    #[test]
    fn rollout_non_interactive_and_event_msg() {
        let exec = jsonl(&[
            json!({"type": "session_meta", "payload": {"id": "a", "originator": "codex_exec"}}),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": "do it"}}),
        ]);
        let d = describe_rollout(&exec);
        assert_eq!(d.title, "do it");
        assert!(!d.interactive);
        let sub = jsonl(&[json!({"type": "session_meta", "payload": {"id": "a", "source": {"subagent": {}}}})]);
        assert!(!describe_rollout(&sub).interactive);
        assert!(!describe_rollout("").interactive);
    }

    #[test]
    fn pi_description() {
        let text = jsonl(&[
            json!({"type": "session", "id": "abc", "cwd": "/w", "timestamp": "2026-01-01T00:00:00Z"}),
            json!({"type": "message", "message": {"role": "assistant", "content": [{"type": "text", "text": "hello"}]}}),
            json!({"type": "message", "message": {"role": "user", "content": [{"type": "text", "text": "Write  tests"}]}}),
        ]);
        let d = describe_pi(&text);
        assert_eq!((d.id.as_str(), d.cwd.as_str(), d.title.as_str()), ("abc", "/w", "Write tests"));
        let titled = jsonl(&[json!({"type": "title", "title": "Named"}), json!({"type": "session", "id": "x"})]);
        assert_eq!(describe_pi(&titled).title, "Named");
    }

    #[test]
    fn helpers() {
        assert_eq!(clean("  a [Image: 1x1] b\n\n c "), "a b c");
        assert_eq!(command_title("<command-name>/x</command-name>"), "/x");
        assert_eq!(command_title("nothing"), "");
        assert_eq!(strip_images("<IMAGE a>t</image>").trim(), "t");
        assert_eq!(strip_images("<images>").trim(), "<images>");
    }
}
