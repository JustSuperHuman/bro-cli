//! Agent chat transcripts on disk (Claude Code and Codex), so the orchestrator
//! can answer "what did Toast decide about the schema?" from the full
//! conversation rather than the visible screen. Built on `bro_core::sessions`.

use crate::state::AppState;
use bro_core::Harness;
use bro_core::sessions::{self, ListOpts, SessionInfo};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, SystemTime};

const LIST_SCAN: usize = 300;
const DEFAULT_MESSAGES: usize = 40;
const MAX_MESSAGES: usize = 400;
/// Longest single message quoted back; the rest is elided in the middle.
const MESSAGE_CHARS: usize = 4_000;

fn harness_of(agent: Option<&str>) -> Option<Harness> {
    match agent? {
        "claude" => Some(Harness::Claude),
        "codex" => Some(Harness::Codex),
        _ => None,
    }
}

fn modified_iso(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn record(info: &SessionInfo) -> Value {
    json!({
        "chatId": info.id,
        "agent": info.harness.label(),
        "title": info.title,
        "cwd": info.cwd,
        "project": info.project.as_ref().map(|project| project.name.clone()),
        "branch": info.branch,
        "modified": modified_iso(info.modified),
        "path": info.path,
    })
}

fn matches_project(info: &SessionInfo, project: &str) -> bool {
    let wanted = project.to_lowercase();
    info.project
        .as_ref()
        .is_some_and(|key| key.name.to_lowercase().contains(&wanted))
        || info
            .cwd
            .as_ref()
            .is_some_and(|cwd| cwd.to_string_lossy().to_lowercase().contains(&wanted))
}

/// Past chats, newest first, optionally narrowed to a project and a title search.
pub fn list(project: Option<&str>, query: Option<&str>, limit: usize) -> Value {
    let all = sessions::list(&ListOpts {
        limit: LIST_SCAN,
        harnesses: vec![Harness::Claude, Harness::Codex],
    });
    let query = query.map(str::to_lowercase);
    let found: Vec<Value> = all
        .iter()
        .filter(|info| project.is_none_or(|project| matches_project(info, project)))
        .filter(|info| {
            query
                .as_deref()
                .is_none_or(|query| info.title.to_lowercase().contains(query))
        })
        .take(limit)
        .map(record)
        .collect();
    json!({ "chats": found, "scanned": all.len() })
}

/// The transcript a live session is writing: newest chat of its agent in its
/// directory, touched since the session started.
fn live_transcript(app: &AppState, session_id: &str) -> Result<SessionInfo, String> {
    let summary = app
        .summary(session_id)
        .ok_or("The session is no longer available.")?;
    let harness = harness_of(summary.agent.as_deref()).ok_or(
        "That tab is not running Claude Code or Codex, so it has no chat transcript; use read_session.",
    )?;
    let started = DateTime::parse_from_rfc3339(&summary.created_at)
        .map(|at| SystemTime::from(at.with_timezone(&Utc)))
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .checked_sub(Duration::from_secs(5))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let cwd = Path::new(&summary.cwd);
    sessions::list(&ListOpts {
        limit: 120,
        harnesses: vec![harness],
    })
    .into_iter()
    .find(|info| {
        info.modified >= started
            && info
                .cwd
                .as_deref()
                .is_some_and(|dir| bro_core::util::same_path(dir, cwd))
    })
    .ok_or_else(|| {
        format!(
            "No {} transcript for \"{}\" yet (the agent writes one after its first message).",
            harness.label(),
            summary.title
        )
    })
}

fn find_chat(chat_id: &str) -> Result<SessionInfo, String> {
    [Harness::Claude, Harness::Codex]
        .into_iter()
        .find_map(|harness| sessions::find_by_id(harness, chat_id))
        .or_else(|| {
            // An id prefix, as a model may quote only the first block.
            sessions::list(&ListOpts {
                limit: LIST_SCAN,
                harnesses: vec![Harness::Claude, Harness::Codex],
            })
            .into_iter()
            .find(|info| info.id.starts_with(chat_id))
        })
        .ok_or_else(|| format!("No chat with id \"{chat_id}\". Call list_chats for ids."))
}

fn clip_middle(text: &str) -> String {
    let count = text.chars().count();
    if count <= MESSAGE_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(MESSAGE_CHARS * 2 / 3).collect();
    let tail: String = text.chars().skip(count - MESSAGE_CHARS / 3).collect();
    format!("{head}\n…[{} characters omitted]…\n{tail}", count - MESSAGE_CHARS)
}

/// A chat as readable text: the live session's (`session_id`) or a past one's.
pub fn read(
    app: &AppState,
    session_id: Option<&str>,
    chat_id: Option<&str>,
    messages: Option<u64>,
) -> Result<String, String> {
    let info = match (session_id, chat_id) {
        (_, Some(chat_id)) => find_chat(chat_id)?,
        (Some(session_id), None) => live_transcript(app, session_id)?,
        (None, None) => return Err("Pass sessionId (a live tab) or chatId (from list_chats).".into()),
    };
    let limit = messages
        .map(|count| count.clamp(1, MAX_MESSAGES as u64) as usize)
        .unwrap_or(DEFAULT_MESSAGES);
    let conversation = sessions::read_messages(info.harness, &info.path, limit)
        .map_err(|error| format!("Could not read {}: {error}", info.path.display()))?;
    let mut out = format!(
        "[{} chat {} — \"{}\" — {} — last active {}]\n[transcript file: {}]\n",
        info.harness.label(),
        info.id,
        info.title,
        info.cwd.as_deref().map(|cwd| cwd.display().to_string()).unwrap_or_default(),
        modified_iso(info.modified),
        info.path.display()
    );
    if conversation.is_empty() {
        out.push_str("(no messages yet)\n");
    }
    for message in conversation {
        let label = match message.role.as_str() {
            "user" => "USER",
            "assistant" => "AGENT",
            _ => "TOOL",
        };
        out.push_str(&format!("\n{label}: {}\n", clip_middle(&message.text)));
    }
    Ok(out)
}
