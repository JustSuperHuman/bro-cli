//! What the orchestrator knows about the workspace before it answers: every
//! terminal tab, where it is, what is running in it and what it is doing,
//! rendered once per turn into the system prompt (and on demand through the
//! `list_sessions` tool).

use super::{OrchestratorConfig, TranscriptItem};
use crate::agents;
use crate::session::SessionView;
use crate::state::AppState;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Screen lines quoted per session in the snapshot.
const TAIL_LINES: usize = 12;
/// Fewer lines each once the workspace is crowded, so the prompt stays bounded.
const CROWDED_TAIL_LINES: usize = 4;
const CROWDED_AT: usize = 12;
const TAIL_WIDTH: usize = 160;
/// Conversation history sent back to the model (older items are trimmed).
const HISTORY_ITEMS: usize = 80;
const HISTORY_CHARS: usize = 90_000;
const TOOL_RESULT_CHARS: usize = 12_000;

pub fn idle_seconds(updated_at: &str) -> Option<i64> {
    let at = DateTime::parse_from_rfc3339(updated_at).ok()?;
    Some((Utc::now() - at.with_timezone(&Utc)).num_seconds().max(0))
}

fn describe_idle(seconds: Option<i64>) -> String {
    match seconds {
        None => "unknown".into(),
        Some(s) if s < 5 => "just now".into(),
        Some(s) if s < 60 => format!("{s}s ago"),
        Some(s) if s < 3600 => format!("{}m ago", s / 60),
        Some(s) => format!("{}h ago", s / 3600),
    }
}

fn read_first_line(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().next().map(|line| line.trim().to_string())
}

/// Branch name (or a short detached hash) for the repository containing
/// `cwd`, read straight from `.git/HEAD` so it costs one stat per tab.
pub fn git_branch(cwd: &str) -> Option<String> {
    let mut directory = PathBuf::from(cwd.trim());
    if !directory.is_dir() {
        return None;
    }
    for _ in 0..40 {
        let marker = directory.join(".git");
        let git_dir = if marker.is_dir() {
            Some(marker)
        } else if marker.is_file() {
            read_first_line(&marker)
                .and_then(|line| {
                    line.strip_prefix("gitdir:")
                        .map(|rest| rest.trim().to_string())
                })
                .map(|target| {
                    let target = PathBuf::from(target);
                    if target.is_absolute() {
                        target
                    } else {
                        directory.join(target)
                    }
                })
        } else {
            None
        };
        if let Some(git_dir) = git_dir {
            let head = read_first_line(&git_dir.join("HEAD"))?;
            return Some(match head.strip_prefix("ref:") {
                Some(reference) => reference
                    .trim()
                    .trim_start_matches("refs/heads/")
                    .to_string(),
                None => head.chars().take(8).collect(),
            });
        }
        if !directory.pop() {
            break;
        }
    }
    None
}

/// The last `lines` meaningful screen rows, right-trimmed and clipped.
pub fn screen_tail(text: &str, lines: usize, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = text
        .lines()
        .map(|line| line.trim_end().chars().take(width).collect::<String>())
        .collect();
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
    let start = rows.len().saturating_sub(lines);
    rows.drain(..start);
    rows
}

fn shell_name(shell: &str) -> String {
    let trimmed = shell.trim().trim_matches('"');
    let first = trimmed.split_whitespace().next().unwrap_or(trimmed);
    first
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(first)
        .trim_end_matches(".exe")
        .to_string()
}

fn project_name(app: &AppState, project_id: Option<&str>) -> Option<String> {
    let id = project_id?;
    app.projects()
        .into_iter()
        .find(|project| project.id == id)
        .map(|project| project.name)
}

fn agent_state(view: &SessionView) -> (Option<String>, Option<String>, String) {
    let agent = view
        .summary
        .agent
        .as_deref()
        .and_then(agents::Agent::parse)
        .map(|agent| agent.label().to_string());
    let activity = view.summary.agent_activity.clone();
    let note = match (agent.as_deref(), activity.as_deref()) {
        (None, _) => "shell".to_string(),
        (Some(label), Some("working")) => format!("{label}, working on a turn"),
        (Some(label), Some("awaiting")) => {
            let title = view
                .prompt
                .as_ref()
                .and_then(|prompt| prompt.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("a question");
            format!("{label}, WAITING FOR AN ANSWER: {title}")
        }
        (Some(label), _) => format!("{label}, idle at its prompt"),
    };
    (agent, activity, note)
}

/// One record per tab, in creation order, for the tool and the snapshot.
pub fn session_records(app: &AppState, tail_lines: usize) -> Vec<Value> {
    let mut views = app.session_views();
    views.sort_by(|left, right| left.summary.created_at.cmp(&right.summary.created_at));
    views
        .iter()
        .map(|view| {
            let (agent, activity, note) = agent_state(view);
            let idle = idle_seconds(&view.summary.updated_at);
            let tail = screen_tail(&view.screen, tail_lines, TAIL_WIDTH);
            json!({
                "id": view.summary.id,
                "title": view.summary.title,
                "cwd": view.summary.cwd,
                "gitBranch": git_branch(&view.summary.cwd),
                "project": project_name(app, view.summary.project_id.as_deref()),
                "shell": shell_name(&view.summary.shell),
                "status": view.summary.status,
                "agent": agent,
                "agentActivity": activity,
                "summary": note,
                "prompt": view.prompt,
                "lastOutput": describe_idle(idle),
                "idleSeconds": idle,
                "screenTail": tail
            })
        })
        .collect()
}

/// The workspace block of the system prompt.
pub fn workspace_snapshot(app: &AppState) -> String {
    let count = app.session_views().len();
    let tail_lines = if count > CROWDED_AT {
        CROWDED_TAIL_LINES
    } else {
        TAIL_LINES
    };
    let records = session_records(app, tail_lines);
    let mut out = String::new();
    out.push_str(&format!(
        "## Workspace right now ({} at {})\n",
        match records.len() {
            0 => "no terminals open".to_string(),
            1 => "1 terminal".to_string(),
            n => format!("{n} terminals"),
        },
        Utc::now().format("%Y-%m-%d %H:%M UTC")
    ));
    if records.is_empty() {
        out.push_str("There are no terminal sessions. Offer to open one with create_session.\n");
        return out;
    }
    for (index, record) in records.iter().enumerate() {
        let field = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let mut line = format!(
            "{}. \"{}\" — id {} — {}",
            index + 1,
            field("title"),
            field("id"),
            field("cwd")
        );
        if let Some(branch) = record.get("gitBranch").and_then(Value::as_str) {
            line.push_str(&format!(" (git {branch})"));
        }
        if let Some(project) = record.get("project").and_then(Value::as_str) {
            line.push_str(&format!(" — project \"{project}\""));
        }
        line.push_str(&format!(
            " — {} — {} — last output {}",
            field("shell"),
            field("summary"),
            field("lastOutput")
        ));
        if field("status") == "exited" {
            line.push_str(" — EXITED");
        }
        out.push_str(&line);
        out.push('\n');
        if let Some(tail) = record.get("screenTail").and_then(Value::as_array)
            && !tail.is_empty()
        {
            out.push_str("   screen:\n");
            for row in tail.iter().filter_map(Value::as_str) {
                out.push_str("   │ ");
                out.push_str(row);
                out.push('\n');
            }
        }
    }
    out
}

pub fn system_prompt(app: &AppState, config: &OrchestratorConfig) -> String {
    let mut prompt = String::new();
    prompt.push_str(concat!(
        "You are the Orchestrator built into bro, the user's agentic terminal workspace. You live in a side panel ",
        "next to the user's terminal sessions and you see all of them: shells and coding agents (Claude Code, Codex, ",
        "Pi, omp) that bro runs on this machine, mirrored to the web and mobile clients. Your job is to tell the user what every tab is doing, spot the ones that are stuck, failing or ",
        "waiting on them, and drive tabs on their behalf through your tools.\n\n",
        "## How to work\n",
        "- The workspace list below is a live snapshot taken as this message was sent. Answer status questions from it ",
        "directly; call read_session when you need more of a tab's screen or scrollback.\n",
        "- Refer to tabs by their title (and directory when titles collide). Tool calls take the session id shown in the list.\n",
        "- Tabs running Claude Code or Codex are AI coding agents. They accept a message like a person typing: use ",
        "send_input with submit=true, then wait_for_output before reading the result. Their screens show what they ",
        "are working on; summarize that in plain language (task, current step, whether they need the user).\n",
        "- A tab marked WAITING FOR AN ANSWER is blocked on a rendered question; tell the user what it asks and offer to ",
        "answer it with answer_prompt (only answer yourself when the user has said what to choose).\n",
        "- These are the user's real terminals. Read before you type. Never run destructive commands (deleting files, ",
        "resetting git state, killing processes, force pushes) unless the user explicitly asked for exactly that. ",
        "Do not type into a tab whose agent is mid-turn unless the user asked you to interrupt it.\n",
        "- After changing something (input sent, tab opened or closed), verify the effect with the tools rather than ",
        "assuming it worked, and report what you saw.\n",
        "- Ask before closing a tab that looks busy. Renaming and opening tabs needs no confirmation.\n",
        "- Be concise. Lead with the answer. For status reports use one short line per tab: title, what it is doing, ",
        "and anything that needs attention. Use markdown sparingly (bullets, `code` for commands and paths).\n\n"
    ));
    prompt.push_str(&format!(
        "You are running as model `{}` through {}.\n\n",
        config.model,
        if config.provider == "openrouter" {
            "OpenRouter"
        } else {
            "a custom OpenAI-compatible endpoint"
        }
    ));
    let projects = app.projects();
    if !projects.is_empty() {
        prompt.push_str("## Projects (directories the tabs are grouped under)\n");
        for project in projects.iter().take(40) {
            prompt.push_str(&format!("- {} — {}\n", project.name, project.cwd));
        }
        prompt.push('\n');
    }
    prompt.push_str(&workspace_snapshot(app));
    prompt
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}\n…[trimmed]")
}

/// Rebuilds the OpenAI message list from the stored transcript so the model
/// sees its own earlier tool calls and their results.
pub fn history_messages(items: &[TranscriptItem]) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    let mut chars = 0usize;
    let mut trimmed = false;
    let start = items.len().saturating_sub(HISTORY_ITEMS);
    if start > 0 {
        trimmed = true;
    }
    // Walk backwards so the newest context survives the character budget.
    let mut selected: Vec<&TranscriptItem> = Vec::new();
    for item in items[start..].iter().rev() {
        let size = item.text.len()
            + item
                .tool
                .as_ref()
                .map(|tool| tool.result.len())
                .unwrap_or(0)
            + item
                .tool_calls
                .iter()
                .map(|call| call.arguments.len())
                .sum::<usize>();
        if chars + size > HISTORY_CHARS && !selected.is_empty() {
            trimmed = true;
            break;
        }
        chars += size;
        selected.push(item);
    }
    selected.reverse();
    // A tool result without the assistant call that produced it is rejected
    // by every provider; drop leading orphans.
    while selected.first().is_some_and(|item| item.role == "tool") {
        selected.remove(0);
        trimmed = true;
    }
    if trimmed {
        messages.push(json!({
            "role": "user",
            "content": "[Earlier parts of this conversation were trimmed for length.]"
        }));
        messages.push(json!({ "role": "assistant", "content": "Understood." }));
    }
    for item in selected {
        match item.role.as_str() {
            "user" => messages.push(json!({ "role": "user", "content": item.text })),
            "assistant" => {
                if item.status == "cancelled" && item.text.is_empty() && item.tool_calls.is_empty()
                {
                    continue;
                }
                let mut message = json!({
                    "role": "assistant",
                    "content": if item.text.is_empty() { Value::Null } else { Value::String(item.text.clone()) }
                });
                if !item.tool_calls.is_empty() {
                    message["tool_calls"] = json!(
                        item.tool_calls
                            .iter()
                            .map(|call| json!({
                                "id": call.id,
                                "type": "function",
                                "function": { "name": call.name, "arguments": call.arguments }
                            }))
                            .collect::<Vec<_>>()
                    );
                }
                messages.push(message);
            }
            "tool" => {
                if let Some(tool) = &item.tool {
                    let content = if tool.ok {
                        clip(&tool.result, TOOL_RESULT_CHARS)
                    } else {
                        format!("Error: {}", clip(&tool.result, TOOL_RESULT_CHARS))
                    };
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": tool.call_id,
                        "content": content
                    }));
                }
            }
            _ => {}
        }
    }
    // A cancelled turn can leave an assistant tool call with no result; give
    // the model a placeholder so the transcript stays well-formed.
    let mut repaired: Vec<Value> = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        repaired.push(message.clone());
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
                let answered = messages[index + 1..]
                    .iter()
                    .take(calls.len())
                    .any(|candidate| {
                        candidate.get("tool_call_id").and_then(Value::as_str) == Some(id)
                    });
                if !answered {
                    repaired.push(json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": "[cancelled before this tool ran]"
                    }));
                }
            }
        }
    }
    repaired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::{ToolCall, ToolRecord};

    fn item(role: &str, text: &str) -> TranscriptItem {
        TranscriptItem {
            id: format!("{role}-{}", text.len()),
            rev: 1,
            seq: 1,
            turn_id: "t".into(),
            role: role.into(),
            text: text.into(),
            reasoning: None,
            tool_calls: Vec::new(),
            tool: None,
            status: "done".into(),
            at: String::new(),
            finished_at: None,
            model: None,
        }
    }

    #[test]
    fn history_round_trips_tool_calls_and_results() {
        let mut assistant = item("assistant", "");
        assistant.tool_calls.push(ToolCall {
            id: "call_1".into(),
            name: "read_session".into(),
            arguments: "{\"sessionId\":\"a\"}".into(),
        });
        let mut tool = item("tool", "");
        tool.tool = Some(ToolRecord {
            call_id: "call_1".into(),
            name: "read_session".into(),
            arguments: json!({ "sessionId": "a" }),
            summary: "Read a".into(),
            result: "PS C:\\>".into(),
            ok: true,
        });
        let messages = history_messages(&[
            item("user", "hi"),
            assistant,
            tool,
            item("assistant", "done"),
        ]);
        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["name"],
            "read_session"
        );
        assert_eq!(messages[2]["tool_call_id"], "call_1");
        assert_eq!(messages[2]["content"], "PS C:\\>");
    }

    #[test]
    fn unanswered_tool_calls_get_a_placeholder_result() {
        let mut assistant = item("assistant", "");
        assistant.status = "cancelled".into();
        assistant.tool_calls.push(ToolCall {
            id: "call_9".into(),
            name: "send_input".into(),
            arguments: "{}".into(),
        });
        let messages = history_messages(&[item("user", "go"), assistant]);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2]["tool_call_id"], "call_9");
    }

    #[test]
    fn screen_tail_drops_trailing_blank_rows_and_clips_width() {
        let tail = screen_tail("one\ntwo   \n\n\n", 5, 2);
        assert_eq!(tail, vec!["on".to_string(), "tw".to_string()]);
    }

    #[test]
    fn git_branch_reads_head_from_a_parent_directory() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        let nested = root.path().join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            git_branch(nested.to_str().unwrap()).as_deref(),
            Some("feature/x")
        );
        assert_eq!(git_branch("Z:\\definitely\\missing"), None);
    }
}
