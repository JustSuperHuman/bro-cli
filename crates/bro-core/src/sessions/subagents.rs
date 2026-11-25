//! The subagents a session starts (Claude Code's Agent tool, Codex's spawned agents), for the desktop app's focus
//! mode: what each was asked, how it's getting on and what it reported back, with its own conversation.
//!
//! What the session's own transcript says of them is read with the rest of its conversation
//! ([`Conversation::subagents`]): the call that started each, its brief, how it ended and its report. Each one also
//! writes a transcript of its own, which a [`Follower`] finds and reads into a conversation of its own
//! ([`Conversation::subagent`]):
//!
//! - Claude Code keeps them beside the session's: `<session>/subagents/agent-<agentId>.jsonl`, each with an
//!   `agent-<agentId>.meta.json` (its type, name and description, the call that started it, the subagent that did
//!   when it's nested, and `finished`). A background subagent reports back in a `<task-notification>`; a fork's
//!   transcript starts with its parent's call.
//! - Codex writes each as a thread of its own in the same home, `sessions/YYYY/MM/DD/rollout-*-<thread>.jsonl`, its
//!   first `session_meta` naming its nickname, role and place in the team. A forked one carries its parent's history
//!   up to `subagent_history_start_ordinal`. Codex sends the task encrypted, so only its name shows; the answer
//!   reaches the parent as a `FINAL_ANSWER` message.

use super::conversation::{Conversation, Item};
use super::live::Tail;
use crate::Harness;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// A missing transcript is looked for again (and Claude's folder of them listed) this often.
const LOOK_EVERY: Duration = Duration::from_secs(2);
/// At most this many subagents are followed per session (the newest).
pub const FOLLOW_MAX: usize = 64;
/// A conversation remembers at most this many of the subagents it started (the newest).
pub const REMEMBER_MAX: usize = 200;

/// How a subagent is getting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SubagentState {
    #[default]
    Running,
    Done,
    Failed,
    /// Stopped before it finished: you stopped it, or the session it ran in ended.
    Stopped,
}

impl SubagentState {
    pub fn running(self) -> bool {
        self == SubagentState::Running
    }

    /// As a conversation's note about it spells it.
    pub fn word(self) -> &'static str {
        match self {
            SubagentState::Running => "running",
            SubagentState::Done => "done",
            SubagentState::Failed => "failed",
            SubagentState::Stopped => "stopped",
        }
    }

    pub fn from_word(word: &str) -> Option<SubagentState> {
        Some(match word {
            "running" => SubagentState::Running,
            "done" => SubagentState::Done,
            "failed" => SubagentState::Failed,
            "stopped" => SubagentState::Stopped,
            _ => return None,
        })
    }
}

/// One subagent, as far as what started it (and, once a [`Follower`] has read it, its own transcript) says.
#[derive(Debug, Clone, Default)]
pub struct Subagent {
    /// The call that started it: its step in the conversation that did (Claude's `tool_use` id, Codex's spawn call)
    pub call: String,
    /// Its own id: Claude's agentId, Codex's thread id (None until the transcript names it)
    pub id: Option<String>,
    /// The call of the subagent that started it (None: the session itself)
    pub parent: Option<String>,
    /// 1 for the session's own, 2 for theirs
    pub depth: u8,
    /// What it was asked to do, in a few words: Claude's description, Codex's task name in words
    pub title: String,
    /// What it goes by: Claude's `name` ("media-swipe"), Codex's nickname ("Aristotle")
    pub name: Option<String>,
    /// What sort it is ("Explore", "Plan", "fork", "worker"); None for the general-purpose one
    pub kind: Option<String>,
    /// The brief it was given (Codex keeps it encrypted: None)
    pub brief: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Codex: its place in the team ("/root/strength_tracking"), which its messages go by
    pub path: Option<String>,
    /// It runs alongside the session instead of holding up its turn (Claude's background agents, all of Codex's)
    pub background: bool,
    pub state: SubagentState,
    /// What it reported back: its last words
    pub result: Option<String>,
    pub started_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    /// The tools it used and the tokens it spent, when the harness counts them
    pub tool_uses: Option<u64>,
    pub tokens: Option<u64>,
    /// Its own transcript once it's found, and the conversation in it (a [`Follower`]'s)
    pub transcript: Option<PathBuf>,
    pub conversation: Option<Arc<Conversation>>,
}

impl Subagent {
    pub fn started(call: &str, title: String, at_ms: Option<i64>) -> Subagent {
        Subagent { call: call.to_string(), title, depth: 1, started_ms: at_ms, ..Subagent::default() }
    }

    /// What it goes by in a list: its own name when it has one, else what it was asked.
    pub fn label(&self) -> &str {
        self.name.as_deref().filter(|n| !n.trim().is_empty()).unwrap_or(&self.title)
    }

    /// How long it ran, or has been running (`now_ms`), in ms.
    pub fn elapsed_ms(&self, now_ms: i64) -> Option<i64> {
        let start = self.started_ms?;
        let end = if self.state.running() { now_ms } else { self.ended_ms.unwrap_or(now_ms) };
        Some((end - start).max(0))
    }
}

/// What sort of subagent a harness says it is, when that tells it apart (the general ones say nothing).
pub(crate) fn kind_of(raw: Option<&str>) -> Option<String> {
    let k = raw?.trim();
    (!k.is_empty() && !matches!(k.to_ascii_lowercase().as_str(), "general-purpose" | "claude" | "default" | "general")).then(|| k.to_string())
}

/// A model a subagent was asked to run on, unless it's "the parent's".
pub(crate) fn model_of(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|m| !m.is_empty() && *m != "inherit").map(str::to_string)
}

// ------------------------------------------------------------------------------------------- what the parent hears

/// One `<task-notification>`: Claude Code telling a session that a background task of its stopped (or, a Monitor's
/// event, that it saw something).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Notice {
    /// the task's id: an agent's agentId, a command's or a watch's task id
    pub task: Option<String>,
    /// the call that started it
    pub call: Option<String>,
    pub state: SubagentState,
    pub result: Option<String>,
    /// it says how the task ended (a `<status>`): a watch's events don't
    pub ended: bool,
    pub summary: Option<String>,
    /// what a watch saw
    pub event: Option<String>,
}

/// The `<task-notification>`s in `text`, in order.
pub(crate) fn notices(text: &str) -> Vec<Notice> {
    const OPEN: &str = "<task-notification>";
    let mut out = vec![];
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        let body = &rest[i + OPEN.len()..];
        // one ends where the next starts (and at its own close)
        let end = body.find(OPEN).unwrap_or(body.len());
        let block = &body[..end];
        let block = block.rfind("</task-notification>").map_or(block, |j| &block[..j]);
        let status = inner(block, "status").map(str::to_ascii_lowercase);
        let state = match status.as_deref() {
            Some("failed" | "error" | "errored") => SubagentState::Failed,
            Some("killed" | "stopped" | "cancelled" | "canceled" | "interrupted" | "aborted") => SubagentState::Stopped,
            Some("running" | "started") => SubagentState::Running,
            _ => SubagentState::Done,
        };
        let text = |name: &str| inner(block, name).filter(|t| !t.is_empty()).map(str::to_string);
        let ended = status.is_some() && !state.running();
        out.push(Notice { task: text("task-id"), call: text("tool-use-id"), state, result: text("result"), ended, summary: text("summary"), event: text("event") });
        rest = &body[end..];
    }
    out
}

/// The text between `<name>` and `</name>` in `block`, trimmed. A result can hold tags of its own: it runs to its
/// last close.
fn inner<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = block.find(&open)? + open.len();
    let end = if name == "result" { block.rfind(&close)? } else { start + block[start..].find(&close)? };
    (end >= start).then(|| block[start..end].trim())
}

/// A Codex agent's final answer to the agent that started it (an `agent_message` whose header says
/// `FINAL_ANSWER`): who sent it (its place in the team) and the answer.
pub(crate) fn codex_answer(p: &Value) -> Option<(String, String)> {
    let author = p.get("author")?.as_str()?.trim();
    let text: String = p.get("content")?.as_array()?.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect();
    let (_, rest) = text.split_once("Message Type:")?;
    let (kind, rest) = rest.split_once('\n').unwrap_or((rest, ""));
    if kind.trim() != "FINAL_ANSWER" || author.is_empty() {
        return None;
    }
    let answer = rest.split_once("Payload:").map_or(rest, |(_, a)| a).trim();
    Some((author.to_string(), answer.to_string()))
}

/// What Codex encrypts (a Fernet token) can't be read: it isn't a brief.
pub(crate) fn readable(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && !(t.starts_with("gAAAAA") && !t.contains(char::is_whitespace))
}

/// A conversation's note that a subagent reported back, as [`Item::Note`]'s text holds it: the call that started
/// it and how it ended.
pub fn note_text(call: &str, state: SubagentState) -> String {
    format!("{call}\n{}", state.word())
}

/// What a subagent note says: the call that started it, and how it ended.
pub fn read_note(text: &str) -> Option<(&str, SubagentState)> {
    let (call, word) = text.split_once('\n')?;
    Some((call, SubagentState::from_word(word)?))
}

// --------------------------------------------------------------------------------------------- their transcripts

/// The folder Claude Code keeps a session's subagents in: beside its transcript, named after it.
pub fn claude_dir(session_transcript: &Path) -> PathBuf {
    session_transcript.with_extension("").join("subagents")
}

/// What Claude Code writes beside a subagent's transcript.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeMeta {
    agent_type: Option<String>,
    description: Option<String>,
    name: Option<String>,
    tool_use_id: Option<String>,
    parent_agent_id: Option<String>,
    spawn_depth: Option<u8>,
    request_shape: Option<String>,
    model: Option<String>,
    finished: Option<bool>,
}

/// When a version 7 UUID (Codex's thread ids) was made: its first 48 bits, in ms.
fn uuid7_ms(id: &str) -> Option<i64> {
    let hex: String = id.chars().filter(|c| *c != '-').take(12).collect();
    (hex.len() == 12).then(|| i64::from_str_radix(&hex, 16).ok()).flatten()
}

/// Codex's transcript of thread `thread`, in the sessions folder `session_transcript` is in: in the day's folder it
/// was made on (as its id says, else `hint_ms`), or the day either side of it (the folders go by local time).
pub fn codex_rollout(session_transcript: &Path, thread: &str, hint_ms: Option<i64>) -> Option<PathBuf> {
    let tail = format!("-{thread}.jsonl");
    let found = |dir: &Path| -> Option<PathBuf> {
        std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&tail)))
    };
    let sessions = session_transcript.ancestors().nth(4).filter(|d| d.file_name().is_some_and(|n| n == "sessions"));
    if let Some(sessions) = sessions
        && let Some(ms) = uuid7_ms(thread).or(hint_ms)
        && let Some(day) = chrono::DateTime::from_timestamp_millis(ms).map(|d| d.date_naive())
    {
        use chrono::Datelike;
        for d in [Some(day), day.pred_opt(), day.succ_opt()].into_iter().flatten() {
            let dir = sessions.join(format!("{:04}", d.year())).join(format!("{:02}", d.month())).join(format!("{:02}", d.day()));
            if let Some(p) = found(&dir) {
                return Some(p);
            }
        }
    }
    // a home laid out some other way: beside the session's own
    found(session_transcript.parent()?)
}

/// Finds the transcripts of a session's subagents and reads each into a conversation of its own, a few at a time as
/// they grow (blocking: the session's transcript worker calls [`Follower::poll`] once a second).
pub struct Follower {
    harness: Harness,
    /// the session's own transcript (Claude's subagents sit in a folder beside it, Codex's in the same home)
    parent: PathBuf,
    /// when the session's process started: a subagent still running in a transcript last written before then was
    /// left behind by an earlier run
    since: Option<SystemTime>,
    followed: Vec<Followed>,
    /// Claude: when the folder was last listed, and the meta files read (their last change)
    listed: Option<Instant>,
    metas: HashMap<PathBuf, SystemTime>,
    out: Arc<Vec<Arc<Subagent>>>,
}

struct Followed {
    /// what's known of it, merged from what started it, its meta and its own transcript
    sub: Subagent,
    /// what the agent that started it was told of how it ended, and when
    told: (SubagentState, Option<i64>),
    told_result: Option<String>,
    /// Claude: the subagent that started it, by its agentId
    parent_id: Option<String>,
    /// Claude: its meta says it finished
    finished: bool,
    tail: Option<Tail>,
    conv: Option<Conversation>,
    /// its transcript's last change
    written: Option<SystemTime>,
    looked: Option<Instant>,
    /// its brief went first in its conversation (or was there already)
    briefed: bool,
    changed: bool,
    shown: Option<Arc<Subagent>>,
}

/// Two texts start the same way, whitespace aside (a brief, and the prompt its transcript starts with).
fn same_start(a: &str, b: &str) -> bool {
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect::<String>();
    let (a, b) = (flat(a), flat(b));
    !a.is_empty() && !b.is_empty() && (a.starts_with(&b) || b.starts_with(&a))
}

/// Set `slot` to `v`: whether that changed it.
fn put<T: PartialEq>(slot: &mut T, v: T) -> bool {
    if *slot == v {
        return false;
    }
    *slot = v;
    true
}

/// Set an optional `slot` to `v` when `v` says something.
fn fill<T: PartialEq + Clone>(slot: &mut Option<T>, v: &Option<T>) -> bool {
    match v {
        Some(v) if slot.as_ref() != Some(v) => {
            *slot = Some(v.clone());
            true
        }
        _ => false,
    }
}

impl Followed {
    fn new(sub: Subagent) -> Followed {
        let told = (sub.state, sub.ended_ms);
        let told_result = sub.result.clone();
        Followed { sub, told, told_result, parent_id: None, finished: false, tail: None, conv: None, written: None, looked: None, briefed: false, changed: true, shown: None }
    }

    /// What started it says (again): merged in.
    fn learn(&mut self, s: &Subagent) -> bool {
        let d = &mut self.sub;
        let mut c = false;
        if !s.title.trim().is_empty() {
            c |= put(&mut d.title, s.title.clone());
        }
        c |= fill(&mut d.id, &s.id);
        c |= fill(&mut d.name, &s.name);
        c |= fill(&mut d.kind, &s.kind);
        c |= fill(&mut d.brief, &s.brief);
        c |= fill(&mut d.path, &s.path);
        c |= fill(&mut d.started_ms, &s.started_ms);
        c |= fill(&mut d.tool_uses, &s.tool_uses);
        c |= fill(&mut d.tokens, &s.tokens);
        if d.model.is_none() {
            c |= fill(&mut d.model, &s.model);
        }
        if d.effort.is_none() {
            c |= fill(&mut d.effort, &s.effort);
        }
        let background = d.background || s.background;
        c |= put(&mut d.background, background);
        c |= put(&mut self.told, (s.state, s.ended_ms));
        c |= put(&mut self.told_result, s.result.clone());
        self.changed |= c;
        c
    }

    /// What Claude Code wrote beside its transcript.
    fn meta(&mut self, id: &str, m: &ClaudeMeta) -> bool {
        let d = &mut self.sub;
        let mut c = put(&mut d.id, Some(id.to_string()));
        if d.title.trim().is_empty()
            && let Some(t) = m.description.as_deref().or(m.name.as_deref()).map(str::trim).filter(|t| !t.is_empty())
        {
            c |= put(&mut d.title, t.to_string());
        }
        c |= fill(&mut d.name, &m.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string));
        c |= fill(&mut d.kind, &kind_of(m.agent_type.as_deref()));
        if d.model.is_none() {
            c |= fill(&mut d.model, &model_of(m.model.as_deref()));
        }
        if let Some(depth) = m.spawn_depth.filter(|n| *n > 0) {
            c |= put(&mut d.depth, depth);
        }
        let background = d.background || m.request_shape.as_deref() == Some("background");
        c |= put(&mut d.background, background);
        c |= put(&mut self.parent_id, m.parent_agent_id.clone().filter(|p| !p.is_empty()));
        c |= put(&mut self.finished, m.finished == Some(true));
        self.changed |= c;
        c
    }

    /// Find its transcript (when it's due a look) and read what's new in it.
    fn read(&mut self, harness: Harness, parent: &Path, now: Instant) -> bool {
        let mut changed = false;
        if self.tail.is_none() {
            if self.looked.is_some_and(|t| now.saturating_duration_since(t) < LOOK_EVERY) {
                return false;
            }
            self.looked = Some(now);
            let Some(id) = self.sub.id.clone() else { return false };
            let path = match harness {
                Harness::Claude => Some(claude_dir(parent).join(format!("agent-{id}.jsonl"))).filter(|p| p.is_file()),
                Harness::Codex => codex_rollout(parent, &id, self.sub.started_ms),
                Harness::Pi | Harness::Omp | Harness::Antigravity => None,
            };
            let Some(path) = path else { return false };
            self.tail = Some(Tail::new(&path));
            self.conv = Some(Conversation::subagent(harness));
            self.sub.transcript = Some(path);
            changed = true;
        }
        let Some(t) = self.tail.as_mut() else { return changed };
        let Ok(read) = t.read() else { return changed };
        let c = self.conv.get_or_insert_with(|| Conversation::subagent(harness));
        if read.reset {
            *c = Conversation::subagent(harness);
            self.briefed = false;
            changed = true;
        }
        let before = c.revision;
        if read.began.is_some_and(|b| b > 0) {
            // a long transcript's first read holds only its end: the conversation is read from its start
            match Conversation::subagent_history(harness, t.path(), t.lines_end()) {
                Ok(whole) => *c = whole,
                Err(_) => {
                    c.ingest_lines(&read.lines);
                }
            }
        } else {
            c.ingest_lines(&read.lines);
        }
        if !read.lines.is_empty() || self.written.is_none() {
            let written = std::fs::metadata(t.path()).and_then(|m| m.modified()).ok();
            changed |= put(&mut self.written, written);
        }
        // a fork's brief isn't in its own transcript: it goes first, as what it was asked
        if !self.briefed
            && !c.items.is_empty()
            && let Some(brief) = self.sub.brief.clone()
        {
            self.briefed = true;
            let there = c.items.iter().take(3).any(|it| matches!(it.as_ref(), Item::You { text, .. } if same_start(text, &brief)));
            if !there {
                c.items.insert(0, Arc::new(Item::You { text: brief, images: 0, pictures: vec![], command: false, at_ms: self.sub.started_ms }));
                c.revision += 1;
            }
        }
        changed |= c.revision != before;
        self.changed |= changed;
        changed
    }

    /// Settle how it's getting on from everything known (`since`: when the session's process started), and what
    /// it reported.
    fn settle(&mut self, since: Option<SystemTime>) {
        let conv = self.conv.as_ref();
        let own = conv.and_then(|c| c.turn);
        let last = conv.and_then(|c| c.last_ms);
        let (told, told_at) = self.told;
        let mut state = if told.running() {
            match own {
                Some(s) if !s.running() => s,
                Some(_) => SubagentState::Running,
                None if self.finished => SubagentState::Done,
                None => SubagentState::Running,
            }
        } else if own == Some(SubagentState::Running) && last.zip(told_at).is_some_and(|(l, t)| l > t + 2000) {
            // it went on after it reported back (a message woke it)
            SubagentState::Running
        } else {
            told
        };
        // still running in a transcript last written before this session's process started: an earlier run of it
        // left it behind (subagents don't outlive their process)
        let quiet_since = self.written.or_else(|| self.sub.started_ms.map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)));
        if state.running()
            && let (Some(since), Some(w)) = (since, quiet_since)
            && w + Duration::from_secs(1) < since
        {
            state = SubagentState::Stopped;
        }
        let d = &mut self.sub;
        let mut c = put(&mut d.state, state);
        // what it said last, when the parent wasn't told it in so many words
        let said_last = conv.filter(|_| !state.running()).and_then(|c| {
            c.items.iter().rev().find_map(|it| match it.as_ref() {
                Item::Said { text, .. } => Some(text.clone()),
                _ => None,
            })
        });
        let result = if state.running() { self.told_result.clone().filter(|_| !told.running()) } else { self.told_result.clone().or(said_last) };
        c |= put(&mut d.result, result);
        let ended = if state.running() { None } else { told_at.filter(|_| !told.running()).or(last) };
        c |= put(&mut d.ended_ms, ended);
        if let Some(conv) = conv {
            c |= fill(&mut d.model, &conv.model);
            c |= fill(&mut d.effort, &conv.effort);
            if d.name.is_none() {
                c |= fill(&mut d.name, &conv.nickname);
            }
            if d.kind.is_none() {
                c |= fill(&mut d.kind, &kind_of(conv.role.as_deref()));
            }
            if d.started_ms.is_none() {
                c |= fill(&mut d.started_ms, &conv.first_ms);
            }
        }
        self.changed |= c;
    }

    /// What to show of it now (made again only when something changed).
    fn shown(&mut self) -> Arc<Subagent> {
        if self.changed || self.shown.is_none() {
            let mut s = self.sub.clone();
            s.conversation = self.conv.as_ref().map(|c| Arc::new(c.clone()));
            self.shown = Some(Arc::new(s));
            self.changed = false;
        }
        self.shown.clone().unwrap_or_default()
    }
}

impl Follower {
    /// Follow the subagents of the session whose transcript is `parent` (`since`: when its process started).
    pub fn new(harness: Harness, parent: impl Into<PathBuf>, since: Option<SystemTime>) -> Follower {
        Follower { harness, parent: parent.into(), since, followed: vec![], listed: None, metas: HashMap::new(), out: Arc::default() }
    }

    /// The session's transcript this follows the subagents of.
    pub fn parent(&self) -> &Path {
        &self.parent
    }

    /// Every subagent followed, the oldest first.
    pub fn subagents(&self) -> Arc<Vec<Arc<Subagent>>> {
        self.out.clone()
    }

    /// One poll: `spawned` is what the session's conversation says of its own subagents. Finds new transcripts and
    /// reads what's new in each; true when anything changed ([`Follower::subagents`] has it).
    pub fn poll(&mut self, spawned: &[Subagent], now: Instant) -> bool {
        let mut changed = false;
        for s in spawned {
            changed |= self.learn(s, None, 1);
        }
        if self.harness == Harness::Claude && self.listed.is_none_or(|t| now.saturating_duration_since(t) >= LOOK_EVERY) {
            self.listed = Some(now);
            changed |= self.list_claude();
        }
        let mut nested: Vec<(Subagent, String, u8)> = vec![];
        for f in &mut self.followed {
            changed |= f.read(self.harness, &self.parent, now);
            // Codex's nested subagents are known from the conversation of the one that started them
            if self.harness == Harness::Codex
                && let Some(c) = &f.conv
            {
                nested.extend(c.subagents.iter().map(|s| (s.clone(), f.sub.call.clone(), f.sub.depth.saturating_add(1))));
            }
        }
        for (s, parent, depth) in nested {
            changed |= self.learn(&s, Some(parent), depth);
        }
        if changed || self.out.len() != self.followed.len() {
            self.publish();
            return true;
        }
        // a subagent can go quiet past the session's start without a new line: settle again now and then
        let before: Vec<SubagentState> = self.followed.iter().map(|f| f.sub.state).collect();
        for f in &mut self.followed {
            f.settle(self.since);
        }
        if self.followed.iter().map(|f| f.sub.state).ne(before) {
            self.publish();
            return true;
        }
        false
    }

    /// Learn of subagent `s` (started by `parent`, a call; None for the session's own).
    fn learn(&mut self, s: &Subagent, parent: Option<String>, depth: u8) -> bool {
        if s.call.is_empty() {
            return false;
        }
        if let Some(f) = self.followed.iter_mut().find(|f| f.sub.call == s.call || (s.id.is_some() && f.sub.id == s.id)) {
            // the call its conversation names it by wins over one made up from its folder
            let mut c = put(&mut f.sub.call, s.call.clone());
            c |= f.learn(s);
            if parent.is_some() {
                c |= put(&mut f.sub.parent, parent);
                c |= put(&mut f.sub.depth, depth.max(1));
                f.changed |= c;
            }
            return c;
        }
        let mut sub = s.clone();
        sub.parent = parent;
        sub.depth = depth.max(1);
        sub.transcript = None;
        sub.conversation = None;
        self.followed.push(Followed::new(sub));
        // the oldest finished ones go first
        while self.followed.len() > FOLLOW_MAX {
            let i = self.followed.iter().position(|f| !f.sub.state.running()).unwrap_or(0);
            self.followed.remove(i);
        }
        true
    }

    /// Claude: read the meta files that changed in the session's subagents folder (new subagents, nested ones, a
    /// subagent that finished).
    fn list_claude(&mut self) -> bool {
        let Ok(dir) = std::fs::read_dir(claude_dir(&self.parent)) else { return false };
        let mut changed = false;
        for e in dir.flatten() {
            let path = e.path();
            let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("agent-")).and_then(|n| n.strip_suffix(".meta.json")).map(str::to_string) else { continue };
            let modified = e.metadata().ok().and_then(|m| m.modified().ok());
            if modified.is_some() && self.metas.get(&path) == modified.as_ref() {
                continue;
            }
            let Ok(meta) = std::fs::read_to_string(&path).map(|t| serde_json::from_str::<ClaudeMeta>(&t).unwrap_or_default()) else { continue };
            if let Some(m) = modified {
                self.metas.insert(path, m);
            }
            let call = meta.tool_use_id.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| format!("agent:{id}"));
            let found = self.followed.iter().position(|f| f.sub.call == call || f.sub.id.as_deref() == Some(id.as_str()));
            let i = match found {
                Some(i) => i,
                None => {
                    let title = meta.description.clone().or_else(|| meta.name.clone()).unwrap_or_default();
                    self.learn(&Subagent { id: Some(id.clone()), ..Subagent::started(&call, title, None) }, None, meta.spawn_depth.unwrap_or(1));
                    match self.followed.iter().position(|f| f.sub.call == call) {
                        Some(i) => i,
                        None => continue,
                    }
                }
            };
            changed |= self.followed[i].meta(&id, &meta);
        }
        changed
    }

    fn publish(&mut self) {
        // a nested Claude subagent names the one that started it by its agentId: that one's call
        let calls: HashMap<String, String> = self.followed.iter().filter_map(|f| f.sub.id.clone().map(|id| (id, f.sub.call.clone()))).collect();
        let since = self.since;
        for f in &mut self.followed {
            if let Some(pid) = &f.parent_id {
                let parent = calls.get(pid).cloned();
                if parent.is_some() && f.sub.parent != parent {
                    f.sub.parent = parent;
                    f.changed = true;
                }
            }
            f.settle(since);
        }
        let mut out: Vec<Arc<Subagent>> = self.followed.iter_mut().map(Followed::shown).collect();
        // in the order they started (those that don't say yet go last, as they came)
        out.sort_by_key(|s| s.started_ms.unwrap_or(i64::MAX));
        self.out = Arc::new(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A real session's subagents, read-only: `BRO_REAL_SESSION=<transcript> [BRO_REAL_HARNESS=codex] cargo test -p
    /// bro-core real_session_subagents -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads a real transcript named by BRO_REAL_SESSION"]
    fn real_session_subagents() {
        let Ok(path) = std::env::var("BRO_REAL_SESSION") else { return };
        let harness = if std::env::var("BRO_REAL_HARNESS").as_deref() == Ok("codex") { Harness::Codex } else { Harness::Claude };
        let path = PathBuf::from(path);
        let end = std::fs::metadata(&path).unwrap().len();
        let started = Instant::now();
        let conv = Conversation::history(harness, &path, end).unwrap();
        let mut f = Follower::new(harness, &path, None);
        f.poll(&conv.subagents, Instant::now());
        println!("read in {:?}: {} items, {} subagents in the conversation", started.elapsed(), conv.items.len(), conv.subagents.len());
        for s in f.subagents().iter() {
            let items = s.conversation.as_ref().map_or(0, |c| c.items.len());
            let result = s.result.as_deref().unwrap_or_default().chars().take(70).collect::<String>().replace('\n', " ");
            println!("{:>2} {:<8} {:<40} name={:?} kind={:?} model={:?} items={items} parent={:?} | {result}", s.depth, s.state.word(), s.title.chars().take(40).collect::<String>(), s.name, s.kind, s.model, s.parent);
        }
        println!("{} background tasks", conv.tasks.len());
        for t in &conv.tasks {
            println!("   {:<8} {:<9} {:<50} news={:?} took={:?}", t.state.word(), format!("{:?}", t.kind), t.title.chars().take(50).collect::<String>(), t.news, t.elapsed_ms(crate::util::now_ms()).map(|ms| ms / 1000));
        }
    }

    #[test]
    fn task_notifications_say_which_subagent_stopped_how_and_what_it_said() {
        let text = "<task-notification>\n<task-id>a0b84a07a3579d7f4</task-id>\n<tool-use-id>toolu_01TN</tool-use-id>\n<output-file>C:\\x.output</output-file>\n<status>completed</status>\n<summary>Agent \"Add swipe between media\" finished</summary>\n<result>Swiping works.\n\n<b>Typecheck</b> passes.</result>\n</task-notification>";
        let n = notices(text);
        assert_eq!(n.len(), 1);
        assert_eq!((n[0].task.as_deref(), n[0].call.as_deref(), n[0].state), (Some("a0b84a07a3579d7f4"), Some("toolu_01TN"), SubagentState::Done));
        assert_eq!(n[0].result.as_deref(), Some("Swiping works.\n\n<b>Typecheck</b> passes."), "a result keeps the tags it holds");
        let two = format!("{}\n{}", text.replace("completed", "killed"), "<task-notification><task-id>b</task-id><status>failed</status></task-notification>");
        let states: Vec<SubagentState> = notices(&two).iter().map(|n| n.state).collect();
        assert_eq!(states, [SubagentState::Stopped, SubagentState::Failed]);
        assert!(notices("nothing here").is_empty());
    }

    #[test]
    fn codex_final_answers_name_their_sender() {
        let msg = json!({"type":"agent_message","author":"/root/strength_tracking","recipient":"/root","content":[{"type":"input_text","text":"Message Type: FINAL_ANSWER\nTask name: /root\nSender: /root/strength_tracking\nPayload:\nCompleted all 15 strength competitors."}]});
        assert_eq!(codex_answer(&msg), Some(("/root/strength_tracking".to_string(), "Completed all 15 strength competitors.".to_string())));
        let encrypted = json!({"type":"agent_message","author":"/root/x","content":[{"type":"input_text","text":"Message Type: MESSAGE\nTask name: /root\nSender: /root/x\nPayload:\n"},{"type":"encrypted_content","encrypted_content":"gAAAAABq"}]});
        assert_eq!(codex_answer(&encrypted), None, "only its final answer");
        assert!(!readable("gAAAAABq6nrqHd8lBGIjTFNg2H8Mz4tzXwjjo5hOG4C"));
        assert!(readable("Audit the strength trackers"));
    }

    #[test]
    fn a_note_holds_the_call_and_how_it_ended() {
        let t = note_text("toolu_1", SubagentState::Failed);
        assert_eq!(read_note(&t), Some(("toolu_1", SubagentState::Failed)));
        assert_eq!(read_note("4m 5s"), None);
    }

    #[test]
    fn codex_thread_ids_say_when_they_were_made() {
        // 2026-11-04T00:23:26.058Z
        assert_eq!(uuid7_ms("01a1a3f0-706a-7022-9df5-c1fb12300ee8"), Some(0x01a1_a3f0_706a));
        assert_eq!(uuid7_ms("short"), None);
    }

    fn write(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    }

    fn claude_spawn(call: &str, title: &str, at: i64) -> Subagent {
        Subagent { background: true, ..Subagent::started(call, title.into(), Some(at)) }
    }

    #[test]
    fn claude_subagents_are_found_beside_the_session_with_their_own_conversations() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("projects").join("F--app").join("s1.jsonl");
        write(&session, &[json!({"type":"user","message":{"content":"hi"}})]);
        let sub = claude_dir(&session);
        // a fork: its parent's call first, then the boilerplate, then its own work
        write(&sub.join("agent-a1.jsonl"), &[
            json!({"type":"fork-context-ref","agentId":"a1","parentSessionId":"s1"}),
            json!({"type":"assistant","isSidechain":true,"agentId":"a1","message":{"id":"m0","content":[{"type":"tool_use","id":"toolu_A","name":"Agent","input":{"description":"Add swipe","prompt":"swipe please"}}]}}),
            json!({"type":"user","isSidechain":true,"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_A","content":"Fork started"},{"type":"text","text":"<fork-boilerplate>\nYou are a worker fork.</fork-boilerplate>"}]}}),
            json!({"type":"assistant","isSidechain":true,"timestamp":"2026-11-04T00:10:00Z","message":{"id":"m1","stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"F:\\app\\FileViews.tsx"}}]}}),
        ]);
        std::fs::write(sub.join("agent-a1.meta.json"), json!({"agentType":"fork","isFork":true,"description":"Add swipe between media","name":"media-swipe","toolUseId":"toolu_A","spawnDepth":1,"requestShape":"background","model":"inherit"}).to_string()).unwrap();
        // one it started in turn, known only from the folder
        write(&sub.join("agent-a2.jsonl"), &[json!({"type":"user","isSidechain":true,"message":{"content":"Find the swipe handlers"}})]);
        std::fs::write(sub.join("agent-a2.meta.json"), json!({"agentType":"Explore","description":"Find the swipe handlers","toolUseId":"toolu_B","parentAgentId":"a1","spawnDepth":2}).to_string()).unwrap();

        let mut f = Follower::new(Harness::Claude, &session, None);
        let now = Instant::now();
        let briefed = Subagent { brief: Some("swipe please".into()), ..claude_spawn("toolu_A", "Add swipe between media", 1) };
        assert!(f.poll(std::slice::from_ref(&briefed), now));
        let subs = f.subagents();
        assert_eq!(subs.len(), 2);
        let a = subs.iter().find(|s| s.call == "toolu_A").unwrap();
        assert_eq!((a.id.as_deref(), a.name.as_deref(), a.kind.as_deref(), a.model.as_deref()), (Some("a1"), Some("media-swipe"), Some("fork"), None), "inherit isn't a model");
        assert_eq!(a.state, SubagentState::Running);
        let conv = a.conversation.as_ref().expect("its own conversation");
        assert_eq!(conv.items.len(), 2, "its brief, then its work (the fork's preamble isn't): {:?}", conv.items);
        assert!(matches!(conv.items[0].as_ref(), Item::You { text, .. } if text == "swipe please"), "a fork's brief goes first");
        assert!(matches!(conv.items[1].as_ref(), Item::Did { steps } if steps[0].label == "Read FileViews.tsx"));
        let b = subs.iter().find(|s| s.call == "toolu_B").unwrap();
        assert_eq!((b.parent.as_deref(), b.depth, b.kind.as_deref()), (Some("toolu_A"), 2, Some("Explore")));
        assert!(matches!(b.conversation.as_ref().unwrap().items[0].as_ref(), Item::You { text, .. } if text == "Find the swipe handlers"), "a subagent's brief is the first thing in its conversation");
        // nothing new: nothing to publish
        assert!(!f.poll(std::slice::from_ref(&briefed), now));

        // it finishes: its last words are its report, and its meta says so
        let mut lines: Vec<Value> = std::fs::read_to_string(sub.join("agent-a1.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        lines.push(json!({"type":"assistant","isSidechain":true,"timestamp":"2026-11-04T00:12:00Z","message":{"id":"m2","stop_reason":"end_turn","content":[{"type":"text","text":"Swiping works now."}]}}));
        write(&sub.join("agent-a1.jsonl"), &lines);
        assert!(f.poll(std::slice::from_ref(&briefed), now + Duration::from_secs(1)));
        let a = f.subagents().iter().find(|s| s.call == "toolu_A").cloned().unwrap();
        assert_eq!(a.state, SubagentState::Done);
        assert_eq!(a.result.as_deref(), Some("Swiping works now."));
        assert!(a.ended_ms.is_some());
    }

    #[test]
    fn a_subagent_left_running_by_an_earlier_run_of_the_session_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("p").join("s1.jsonl");
        write(&session, &[]);
        let since = SystemTime::now() + Duration::from_secs(60);
        let mut f = Follower::new(Harness::Claude, &session, Some(since));
        f.poll(&[claude_spawn("toolu_A", "Old work", crate::util::now_ms() - 3_600_000)], Instant::now());
        assert_eq!(f.subagents()[0].state, SubagentState::Stopped);
        // told it finished: that stands
        let mut done = claude_spawn("toolu_A", "Old work", crate::util::now_ms() - 3_600_000);
        done.state = SubagentState::Done;
        done.result = Some("All done".into());
        f.poll(&[done], Instant::now());
        assert_eq!((f.subagents()[0].state, f.subagents()[0].result.as_deref()), (SubagentState::Done, Some("All done")));
    }

    #[test]
    fn codex_subagents_are_their_own_threads_and_a_fork_leaves_its_parents_history_out() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions").join("2026").join("11").join("03");
        let parent = day.join("rollout-2026-11-03T19-21-09-01a1a3ee-5bf3-7672-8844-d431a703aa92.jsonl");
        write(&parent, &[json!({"type":"session_meta","payload":{"id":"01a1a3ee-5bf3-7672-8844-d431a703aa92"}})]);
        let thread = "01a1a3f0-706a-7022-9df5-c1fb12300ee8";
        let event = |p: Value| json!({"type":"event_msg","timestamp":"2026-11-04T00:25:00Z","payload":p});
        let reply = |t: &str| json!({"type":"response_item","timestamp":"2026-11-04T00:30:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":t}]}});
        write(&day.join(format!("rollout-2026-11-03T19-23-26-{thread}.jsonl")), &[
            json!({"type":"session_meta","payload":{"id":thread,"agent_nickname":"Nash","agent_role":"worker","agent_path":"/root/nutrition_coaching_tracking","forked_from_id":"01a1a3ee","subagent_history_start_ordinal":4}}),
            json!({"type":"session_meta","payload":{"id":"01a1a3ee-5bf3-7672-8844-d431a703aa92"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"the parent's own prompt"}]}}),
            reply("The parent's own words."),
            event(json!({"type":"thread_settings_applied","thread_id":thread,"thread_settings":{"model":"gpt-6.1-sol","reasoning_effort":"max"}})),
            event(json!({"type":"task_started"})),
            event(json!({"type":"item_completed","item":{"type":"CommandExecution","id":"x1","command":["bash","-lc","cat a.md"],"parsed_cmd":[{"type":"read","cmd":"cat a.md","name":"a.md"}],"status":"completed","exit_code":0}})),
        ]);
        let mut spawn = Subagent::started("call_7e", "Nutrition coaching tracking".into(), Some(1_793_751_806_058));
        spawn.id = Some(thread.into());
        spawn.background = true;
        let mut f = Follower::new(Harness::Codex, &parent, None);
        assert!(f.poll(std::slice::from_ref(&spawn), Instant::now()));
        let s = f.subagents()[0].clone();
        assert_eq!((s.name.as_deref(), s.kind.as_deref(), s.model.as_deref(), s.effort.as_deref()), (Some("Nash"), Some("worker"), Some("gpt-6.1-sol"), Some("max")));
        let conv = s.conversation.as_ref().unwrap();
        assert_eq!(conv.items.len(), 1, "only its own work: {:?}", conv.items);
        assert_eq!(s.state, SubagentState::Running);
        // its turn ends
        let path = s.transcript.clone().unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&format!("{}\n{}\n", reply("Saved research for all 13."), event(json!({"type":"task_complete","last_agent_message":"Saved research for all 13."}))));
        std::fs::write(&path, text).unwrap();
        assert!(f.poll(std::slice::from_ref(&spawn), Instant::now()));
        let s = f.subagents()[0].clone();
        assert_eq!((s.state, s.result.as_deref()), (SubagentState::Done, Some("Saved research for all 13.")));
    }
}
