//! `GET /api/sessions/{id}/conversation`: an agent session as its conversation (what you said, what the agent
//! said, what it did in between), for the phone's focus view. The same [`Conversation`] the desktop app's focus
//! mode draws, built here from the transcript bro bound the session to ([`crate::AgentFiles::transcript`]) and
//! tailed per session: the first request reads the whole conversation ([`Conversation::history`]), each one after
//! only the bytes written since the last.
//!
//! `?since=<rev>` answers `{ rev, unchanged: true }` while nothing moved; `?limit=` caps the items sent (the
//! latest, default [`LIMIT`]).

use crate::rest::api_error;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bro_core::Harness;
use bro_core::sessions::conversation::{Conversation, Item, NoteKind, Step, StepKind, StepState};
use bro_core::sessions::live::Tail;
use bro_core::sessions::subagents::{Follower, Subagent, SubagentState};
use bro_core::sessions::tasks::{Task, TaskKind};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// Items sent when the client doesn't say.
pub(crate) const LIMIT: usize = 150;

/// Each session's tailed transcript, by session id.
pub(crate) struct Feeds {
    map: Mutex<HashMap<String, Slot>>,
    /// Bumped for every feed started, so a rebuilt conversation never repeats an old `rev`. It starts at the time
    /// this bridge did (ms), so the next bridge's revs don't repeat this one's either: phones keep the revs they read.
    generation: AtomicU64,
}

impl Default for Feeds {
    fn default() -> Self {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        Feeds { map: Mutex::default(), generation: AtomicU64::new(now) }
    }
}

/// A session's feed, locked on its own: a long transcript's first read doesn't hold up the other sessions'.
struct Slot {
    path: PathBuf,
    harness: Harness,
    feed: Arc<Mutex<Feed>>,
}

struct Feed {
    tail: Tail,
    conversation: Conversation,
    generation: u64,
    /// The media the conversation names, as [`bro_core::util::path_key`]s, and the rev they're from ([`Feeds::names`]).
    named: Option<(String, std::collections::HashSet<String>)>,
    work: Option<FollowedWork>,
}

struct FollowedWork {
    since_ms: Option<i64>,
    follower: Follower,
    revision: u64,
}

impl Feeds {
    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The conversation in `path` with what was written since the last call read in, and its `rev`.
    pub(crate) fn read(&self, session_id: &str, path: &FsPath, harness: Harness) -> (String, Conversation) {
        let feed = {
            let mut map = self.map.lock();
            match map.get(session_id) {
                Some(slot) if slot.path == path && slot.harness == harness => slot.feed.clone(),
                _ => {
                    let feed = Feed { tail: Tail::new(path), conversation: Conversation::new(harness), generation: self.next_generation(), named: None, work: None };
                    let feed = Arc::new(Mutex::new(feed));
                    map.insert(session_id.to_owned(), Slot { path: path.to_owned(), harness, feed: feed.clone() });
                    feed
                }
            }
        };
        let mut feed = feed.lock();
        if let Ok(read) = feed.tail.read() {
            if read.reset {
                feed.conversation = Conversation::new(harness);
                feed.generation = self.next_generation();
                feed.work = None;
            }
            // a long transcript's first read holds only its latest lines: the conversation is read from its start
            let whole = read.began.filter(|b| *b > 0).map(|_| Conversation::history(harness, path, feed.tail.lines_end()));
            match whole {
                Some(Ok(whole)) => feed.conversation = whole,
                _ => {
                    feed.conversation.ingest_lines(&read.lines);
                }
            }
        }
        (format!("{}.{}", feed.generation, feed.conversation.revision), feed.conversation.clone())
    }

    /// Follow the same child transcripts as desktop. Child-only changes must move the phone's revision even
    /// when the parent's turn has ended and its transcript hasn't changed.
    fn read_work(&self, session_id: &str, path: &FsPath, harness: Harness, since_ms: Option<i64>, running: bool) -> (String, Conversation) {
        let (rev, mut conversation) = self.read(session_id, path, harness);
        let Some(feed) = self.map.lock().get(session_id).map(|slot| slot.feed.clone()) else {
            return (rev, conversation);
        };
        let mut feed = feed.lock();
        if feed.work.as_ref().is_none_or(|w| w.since_ms != since_ms) {
            let since = since_ms.and_then(|ms| u64::try_from(ms).ok()).map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms));
            feed.work = Some(FollowedWork { since_ms, follower: Follower::new(harness, path, since), revision: self.next_generation() });
        }
        let work = feed.work.as_mut().unwrap();
        if work.follower.poll(&conversation.subagents, Instant::now()) {
            work.revision += 1;
        }
        conversation.subagents = work.follower.subagents().iter().map(|s| s.as_ref().clone()).collect();
        let now_ms = chrono::Utc::now().timestamp_millis();
        if !running {
            for sub in &mut conversation.subagents {
                if sub.state.running() {
                    sub.state = SubagentState::Stopped;
                    sub.ended_ms.get_or_insert(now_ms);
                }
            }
        }
        for task in &mut conversation.tasks {
            if task.state.running() && (!running || !task.live(now_ms, since_ms)) {
                let expired = running && task.expires_ms.is_some_and(|at| at <= now_ms);
                task.state = if expired { SubagentState::Done } else { SubagentState::Stopped };
                task.ended_ms = if expired { task.expires_ms } else { Some(now_ms) };
            }
        }
        let tasks = conversation.tasks.iter().filter(|t| t.state.running()).count();
        (format!("{rev}.{}.{}.{tasks}", work.revision, u8::from(running)), conversation)
    }

    /// Whether the conversation in `path` names the file `key` (a [`bro_core::util::path_key`]): a picture sent
    /// with a message, a file a reply names by its path ([`crate::media::named_in`]). Reads what's new first; the
    /// names are worked out again only when the conversation moved.
    pub(crate) fn names(&self, session_id: &str, path: &FsPath, harness: Harness, key: &str) -> bool {
        let (rev, conversation) = self.read(session_id, path, harness);
        let Some(feed) = self.map.lock().get(session_id).map(|slot| slot.feed.clone()) else {
            return false;
        };
        let mut feed = feed.lock();
        if feed.named.as_ref().is_none_or(|(seen, _)| *seen != rev) {
            let keys = crate::media::named_in(&conversation).iter().map(|(p, ..)| bro_core::util::path_key(p)).collect();
            feed.named = Some((rev, keys));
        }
        feed.named.as_ref().is_some_and(|(_, keys)| keys.contains(key))
    }

    /// Forget a session that's gone.
    pub(crate) fn forget(&self, session_id: &str) {
        self.map.lock().remove(session_id);
    }
}

#[derive(Deserialize)]
pub(crate) struct ConversationQuery {
    since: Option<String>,
    limit: Option<usize>,
}

pub(crate) async fn conversation(
    Path(id): Path<String>,
    Query(query): Query<ConversationQuery>,
    State(state): State<AppState>,
) -> Response {
    let Some((files, agent)) = state.conversation_source(&id) else {
        return api_error(StatusCode::NOT_FOUND, "Unknown terminal session.");
    };
    let Some(transcript) = files else {
        // an agent that hasn't written its transcript yet (or a shell): nothing to show yet
        return Json(json!({ "rev": "0", "ready": false, "total": 0, "items": [] })).into_response();
    };
    let harness = harness_of(agent.as_deref(), &transcript);
    let (since_ms, running) = {
        let inner = state.inner.lock();
        let session = inner.sessions.get(&id);
        (session.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s.summary.created_at).ok()).map(|t| t.timestamp_millis()),
            session.is_some_and(|s| s.summary.status == "running"))
    };
    let feeds = state.conversations.clone();
    let limit = query.limit.unwrap_or(LIMIT).clamp(1, bro_core::sessions::conversation::ITEMS_MAX);
    let since = query.since;
    let body = tokio::task::spawn_blocking(move || {
        let (rev, conversation) = feeds.read_work(&id, &transcript, harness, since_ms, running);
        if since.as_deref() == Some(rev.as_str()) {
            return json!({ "rev": rev, "unchanged": true });
        }
        to_json(&rev, &conversation, limit)
    })
    .await;
    match body {
        Ok(body) => Json(body).into_response(),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "The conversation could not be read."),
    }
}

/// The harness a transcript is written by: the session's agent when it's known, else the transcript's name
/// (Codex writes `rollout-*.jsonl`).
pub(crate) fn harness_of(agent: Option<&str>, transcript: &FsPath) -> Harness {
    match agent {
        Some("codex") => Harness::Codex,
        Some("claude") => Harness::Claude,
        _ if transcript.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-")) => {
            Harness::Codex
        }
        _ => Harness::Claude,
    }
}

fn harness_id(h: Harness) -> &'static str {
    match h {
        Harness::Claude => "claude",
        Harness::Codex => "codex",
        Harness::Pi => "pi",
        Harness::Omp => "omp",
        Harness::Antigravity => "antigravity",
    }
}

fn step_kind(k: StepKind) -> &'static str {
    match k {
        StepKind::Read => "read",
        StepKind::Edit => "edit",
        StepKind::Create => "create",
        StepKind::Delete => "delete",
        StepKind::Run => "run",
        StepKind::Search => "search",
        StepKind::Web => "web",
        StepKind::Agent => "agent",
        StepKind::Plan => "plan",
        StepKind::Ask => "ask",
        StepKind::Send => "send",
        // skills and MCP tools went as "other" before they had kinds of their own: the apps still get that
        StepKind::Skill | StepKind::Mcp | StepKind::Other => "other",
    }
}

fn step_state(s: StepState) -> &'static str {
    match s {
        StepState::Running => "running",
        StepState::Done => "done",
        StepState::Failed => "failed",
    }
}

fn note_kind(k: NoteKind) -> &'static str {
    match k {
        NoteKind::Interrupted => "interrupted",
        NoteKind::Compacted => "compacted",
        NoteKind::Error => "error",
        NoteKind::Worked => "worked",
        NoteKind::Subagent => "subagent",
    }
}

/// What the phone is sent: everything but the notes about subagents reporting back (the apps don't know them yet
/// and would show them as something else).
fn served(item: &Item) -> bool {
    !matches!(item, Item::Note { kind: NoteKind::Subagent, .. })
}

/// What sort of "other" step it is, for apps that know: a skill it used, an MCP server's tool (`detail` names the
/// server). Older apps go on reading `kind: "other"`.
fn step_family(k: StepKind) -> Option<&'static str> {
    match k {
        StepKind::Skill => Some("skill"),
        StepKind::Mcp => Some("mcp"),
        _ => None,
    }
}

fn step_json(s: &Step) -> Value {
    json!({
        "id": s.id,
        "kind": step_kind(s.kind),
        "family": step_family(s.kind),
        "label": s.label,
        "detail": s.detail,
        "state": step_state(s.state),
        "added": s.added,
        "removed": s.removed,
        "line": s.line,
        "at": s.at_ms,
        // an MCP app's page shows its result: the phone loads it from `/apps/{call}` (see crate::apps)
        "app": s.app.as_ref().map(|a| json!({ "server": a.server, "tool": a.tool, "resource": a.resource })),
    })
}

/// An item's key: the same item keeps it while the conversation grows (and its oldest items go).
fn key_of(item: &Item) -> String {
    match item {
        Item::You { at_ms, .. } => format!("you:{}", at_ms.unwrap_or_default()),
        Item::Said { at_ms, .. } => format!("said:{}", at_ms.unwrap_or_default()),
        Item::Did { steps } => format!("did:{}", steps.first().map_or("", |s| s.id.as_str())),
        Item::Note { kind, at_ms, .. } => format!("note:{}:{}", note_kind(*kind), at_ms.unwrap_or_default()),
    }
}

fn item_json(item: &Item, key: String) -> Value {
    match item {
        Item::You { text, images, pictures, command, at_ms } => json!({
            "kind": "you",
            "key": key,
            "text": bro_core::quips::collapse(text),
            "images": images,
            "pictures": pictures,
            "command": command,
            "at": at_ms,
        }),
        Item::Said { text, at_ms } => json!({ "kind": "said", "key": key, "text": text, "at": at_ms }),
        Item::Did { steps } => json!({
            "kind": "did",
            "key": key,
            "steps": steps.iter().map(step_json).collect::<Vec<_>>(),
        }),
        Item::Note { kind, text, at_ms } => json!({
            "kind": "note",
            "key": key,
            "note": note_kind(*kind),
            "text": text,
            "at": at_ms,
        }),
    }
}

fn subagent_activity(s: &Subagent) -> String {
    let first_line = |text: &str| text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string();
    if !s.state.running() {
        return s.result.as_deref().map(first_line).filter(|s| !s.is_empty()).unwrap_or_else(|| match s.state {
            SubagentState::Done => "Finished", SubagentState::Failed => "Stopped on an error", _ => "Stopped before it finished",
        }.to_string());
    }
    let c = s.conversation.as_deref();
    if let Some(step) = c.and_then(Conversation::running) { return step.label.clone(); }
    if let Some(thought) = c.and_then(|c| c.thought.as_ref()) { return thought.text.clone(); }
    c.and_then(|c| c.items.iter().rev().find_map(|item| match item.as_ref() {
        Item::Did { steps } => steps.last().map(|s| s.label.clone()),
        Item::Said { text, .. } => Some(first_line(text)).filter(|s| !s.is_empty()),
        _ => None,
    })).unwrap_or_else(|| "Getting started".to_string())
}

fn subagent_json(s: &Subagent) -> Value {
    json!({ "call": s.call, "id": s.id, "parent": s.parent, "depth": s.depth,
        "title": s.title, "name": s.name, "kind": s.kind, "model": s.model, "effort": s.effort,
        "state": s.state.word(), "startedAt": s.started_ms, "endedAt": s.ended_ms,
        "activity": subagent_activity(s) })
}

fn task_json(t: &Task) -> Value {
    let kind = match t.kind { TaskKind::Command => "command", TaskKind::Watch => "watch", TaskKind::Workflow => "workflow" };
    json!({ "id": t.id, "call": t.call, "kind": kind, "title": t.title, "detail": t.detail,
        "state": t.state.word(), "startedAt": t.started_ms, "endedAt": t.ended_ms,
        "expiresAt": t.expires_ms, "news": t.news })
}

fn to_json(rev: &str, c: &Conversation, limit: usize) -> Value {
    let all: Vec<&Item> = c.items.iter().map(|i| i.as_ref()).filter(|i| served(i)).collect();
    let total = all.len();
    let skip = total.saturating_sub(limit);
    // keys repeat only for items without a time: number the repeats
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut items = Vec::with_capacity(all.len() - skip);
    for (i, item) in all.into_iter().enumerate() {
        let base = key_of(item);
        let n = seen.entry(base.clone()).or_default();
        *n += 1;
        if i < skip {
            continue;
        }
        let key = if *n > 1 { format!("{base}#{n}") } else { base };
        items.push(item_json(item, key));
    }
    json!({
        "rev": rev,
        "ready": true,
        "harness": harness_id(c.harness),
        "model": c.model,
        "effort": c.effort,
        "cwd": c.cwd,
        "branch": c.branch,
        "version": c.version,
        "thought": c.thought.as_ref().map(|t| json!({ "text": t.text, "at": t.at_ms })),
        "total": total,
        "items": items,
        "subagents": c.subagents.iter().map(subagent_json).collect::<Vec<_>>(),
        "tasks": c.tasks.iter().map(task_json).collect::<Vec<_>>(),
    })
}

impl AppState {
    /// Where a session's conversation is read from: its transcript (None until bro bound one) and its agent.
    /// None for an unknown session.
    pub(crate) fn conversation_source(&self, session_id: &str) -> Option<(Option<PathBuf>, Option<String>)> {
        let inner = self.inner.lock();
        let session = inner.sessions.get(session_id)?;
        let agent = session
            .summary
            .agent
            .clone()
            .or_else(|| session.agent_hint.map(|a| a.id().to_owned()));
        Some((session.agent_files.transcript.clone(), agent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &FsPath, lines: &[Value]) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    fn you(text: &str, ts: &str) -> Value {
        json!({ "type": "user", "timestamp": ts, "message": { "role": "user", "content": text } })
    }

    fn said(id: &str, text: &str, ts: &str) -> Value {
        json!({ "type": "assistant", "timestamp": ts, "message": { "id": id, "role": "assistant", "content": [{ "type": "text", "text": text }] } })
    }

    #[test]
    fn a_transcript_reads_as_its_conversation_and_only_new_lines_move_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        write(&path, &[you("Fix the login bug", "2026-10-28T10:00:00.000Z"), said("m1", "On it.", "2026-10-28T10:00:02.000Z")]);
        let feeds = Feeds::default();
        let (rev, c) = feeds.read("s1", &path, Harness::Claude);
        let body = to_json(&rev, &c, LIMIT);
        assert_eq!(body["total"], 2);
        assert_eq!(body["items"][0]["kind"], "you");
        assert_eq!(body["items"][0]["text"], "Fix the login bug");
        assert_eq!(body["items"][1]["kind"], "said");
        assert_eq!(body["items"][1]["text"], "On it.");
        // nothing new: the same rev
        let (again, _) = feeds.read("s1", &path, Harness::Claude);
        assert_eq!(again, rev);
        // a new line moves it, and the old items keep their keys
        write(&path, &[you("And the signup page", "2026-10-28T10:01:00.000Z")]);
        let (next, c) = feeds.read("s1", &path, Harness::Claude);
        assert_ne!(next, rev);
        let later = to_json(&next, &c, LIMIT);
        assert_eq!(later["items"][0]["key"], body["items"][0]["key"]);
        assert_eq!(later["items"].as_array().unwrap().len(), 3);
        // the latest items only, when asked for fewer
        let tail = to_json(&next, &c, 1);
        assert_eq!(tail["total"], 3);
        assert_eq!(tail["items"].as_array().unwrap().len(), 1);
        assert_eq!(tail["items"][0]["text"], "And the signup page");
    }

    #[test]
    fn a_long_transcript_shows_its_whole_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        // more tool output between the prompt and the answer than a tail's first read takes (a few screenshots)
        let output = "x".repeat(bro_core::sessions::live::FIRST_READ_MAX as usize);
        let result = json!({ "type": "user", "timestamp": "2026-10-28T10:00:01.000Z", "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": output }] } });
        write(&path, &[you("Fix the login bug", "2026-10-28T10:00:00.000Z"), result, said("m1", "Fixed.", "2026-10-28T10:00:02.000Z")]);
        let feeds = Feeds::default();
        let (rev, c) = feeds.read("s1", &path, Harness::Claude);
        let body = to_json(&rev, &c, LIMIT);
        let kinds: Vec<&str> = body["items"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["you", "said"]);
        assert_eq!(body["items"][0]["text"], "Fix the login bug");
        // and goes on from there
        write(&path, &[you("And the signup page", "2026-10-28T10:01:00.000Z")]);
        let (next, c) = feeds.read("s1", &path, Harness::Claude);
        assert_ne!(next, rev);
        assert_eq!(c.items.len(), 3);
        // a bridge started later never repeats a rev a phone kept
        std::thread::sleep(std::time::Duration::from_millis(5));
        let (again, _) = Feeds::default().read("s1", &path, Harness::Claude);
        assert_ne!(again, next);
    }

    #[test]
    fn phone_conversations_keep_quip_shortcuts_instead_of_the_expanded_instructions() {
        let library = bro_core::quips::Library::default();
        let short = "Explain this :plain-english";
        let expanded = library.expand(short).unwrap();
        let item = Item::You { text: expanded, images: 0, pictures: vec![], command: false, at_ms: None };
        let value = item_json(&item, "you:1".into());
        assert_eq!(value["text"], short);
    }

    #[test]
    fn skills_and_mcp_tools_say_what_they_are_and_stay_other_for_old_apps() {
        let step = |kind| Step { id: "t1".into(), kind, label: "Navigate · figma.com".into(), detail: Some("JustBrowse".into()), state: StepState::Done, added: 0, removed: 0, line: None, at_ms: None, app: None };
        let mcp = step_json(&step(StepKind::Mcp));
        assert_eq!((mcp["kind"].as_str(), mcp["family"].as_str(), mcp["detail"].as_str()), (Some("other"), Some("mcp"), Some("JustBrowse")));
        assert_eq!(step_json(&step(StepKind::Skill))["family"], "skill");
        assert!(step_json(&step(StepKind::Run))["family"].is_null());
    }

    #[test]
    fn codex_transcripts_are_known_by_their_name() {
        assert_eq!(harness_of(None, FsPath::new("/x/rollout-2026-10-28.jsonl")), Harness::Codex);
        assert_eq!(harness_of(None, FsPath::new("/x/abc.jsonl")), Harness::Claude);
        assert_eq!(harness_of(Some("codex"), FsPath::new("/x/abc.jsonl")), Harness::Codex);
    }

    #[test]
    fn phones_follow_child_only_activity_and_completion_without_new_parent_output() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("session.jsonl");
        write(&parent, &[you("Check the tests", "2026-11-08T10:00:00Z")]);
        let folder = bro_core::sessions::subagents::claude_dir(&parent);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("agent-a1.meta.json"), json!({
            "description": "Check the tests", "name": "test-worker", "toolUseId": "spawn-1", "spawnDepth": 1,
        }).to_string()).unwrap();
        let child = folder.join("agent-a1.jsonl");
        write(&child, &[
            json!({"type":"user","isSidechain":true,"message":{"content":"Check the tests"}}),
            json!({"type":"assistant","isSidechain":true,"timestamp":"2026-11-08T10:00:01Z","message":{"id":"c1","stop_reason":"tool_use","content":[{"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"F:\\app\\tests.ts"}}]}}),
        ]);
        let feeds = Feeds::default();
        let (rev, c) = feeds.read_work("s1", &parent, Harness::Claude, None, true);
        let body = to_json(&rev, &c, 1);
        assert_eq!(body["subagents"][0]["state"], "running");
        assert_eq!(body["subagents"][0]["activity"], "Read tests.ts");
        assert_eq!(body["subagents"][0]["name"], "test-worker");
        assert_eq!(feeds.read_work("s1", &parent, Harness::Claude, None, true).0, rev);
        write(&child, &[json!({"type":"assistant","isSidechain":true,"timestamp":"2026-11-08T10:01:00Z","message":{"id":"c2","stop_reason":"end_turn","content":[{"type":"text","text":"All tests pass."}]}})]);
        let (next, c) = feeds.read_work("s1", &parent, Harness::Claude, None, true);
        assert_ne!(next, rev, "the parent hasn't written anything, but the phone must receive its child's completion");
        let body = to_json(&next, &c, 1);
        assert_eq!(body["subagents"][0]["state"], "done");
        assert_eq!(body["subagents"][0]["activity"], "All tests pass.");
        assert_eq!(body["total"], 1);
    }

    #[test]
    fn background_work_is_in_the_payload_and_cannot_outlive_the_process_or_its_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        write(&path, &[you("Build", "2026-11-08T10:00:00Z")]);
        let feeds = Feeds::default();
        feeds.read("s1", &path, Harness::Claude);
        let feed = feeds.map.lock().get("s1").unwrap().feed.clone();
        let now = chrono::Utc::now().timestamp_millis();
        feed.lock().conversation.tasks = vec![
            Task { id: "build".into(), call: "c1".into(), title: "Build the release".into(), started_ms: Some(now), ..Task::default() },
            Task { id: "expired".into(), call: "c2".into(), kind: TaskKind::Watch, title: "Watch logs".into(), expires_ms: Some(now - 1), ..Task::default() },
            Task { id: "previous-process".into(), call: "c3".into(), title: "Old command".into(), started_ms: Some(now - 60_000), ..Task::default() },
        ];
        let (rev, c) = feeds.read_work("s1", &path, Harness::Claude, Some(now - 1000), true);
        let body = to_json(&rev, &c, 1);
        assert_eq!(body["tasks"][0]["state"], "running");
        assert_eq!(body["tasks"][1]["state"], "done");
        assert_eq!(body["tasks"][2]["state"], "stopped");
        let (stopped, c) = feeds.read_work("s1", &path, Harness::Claude, Some(now - 1000), false);
        assert_ne!(stopped, rev);
        assert!(c.tasks.iter().all(|t| !t.state.running()));
        assert!(to_json(&stopped, &c, 1)["subagents"].is_array());
    }
}
