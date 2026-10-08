//! Live agent panes: which transcript a running Claude/Codex pane is writing
//! ([`Binder`]), reading it incrementally ([`Tail`]) and what bro shows about it
//! ([`AgentFacts`]: files sent to the user, files touched, why it is waiting, its last words
//! and Claude's own `away_summary` recap). See `docs/agent-insights.md`.
//!
//! Everything here does blocking file I/O: call it from a worker thread, never the TUI thread.
//! Pi/omp panes are not bound (their insights stay empty).
use super::{find_in_profile, latest_for};
use crate::util::{parse_iso_ms, path_key, read_json, same_path};
use crate::{Harness, paths, profiles};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// A first read starts at most this far from the end of the file.
pub const FIRST_READ_MAX: u64 = 4 * 1024 * 1024;
/// Reasons and "asked" lines are cut to this many characters (with `…`).
pub const REASON_MAX: usize = 80;
/// `last_assistant` is cut to this many characters.
pub const LAST_ASSISTANT_MAX: usize = 160;
/// Lists in [`AgentFacts`] keep at most this many entries.
pub const LIST_MAX: usize = 500;
/// How much of the final assistant text the "asked" rule looks at.
const ASKED_WINDOW: usize = 700;

// ---------------------------------------------------------------------------------------
// Binding a pane to its transcript
// ---------------------------------------------------------------------------------------

/// `<CLAUDE_CONFIG_DIR>/sessions/<pid>.json`, which Claude Code keeps current for each
/// running process (it follows `/clear` and `/resume`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSessionFile {
    pub pid: u32,
    pub session_id: String,
    pub cwd: Option<PathBuf>,
    /// Claude's own "busy" / "idle"
    pub status: Option<String>,
    /// The session's display name (`/rename` or derived)
    pub name: Option<String>,
    pub started_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
}

fn parse_claude_session_file(v: &Value) -> Option<ClaudeSessionFile> {
    let session_id = v.get("sessionId")?.as_str()?.to_string();
    if !super::is_uuid(&session_id) {
        return None;
    }
    Some(ClaudeSessionFile {
        pid: v
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok())
            .unwrap_or(0),
        session_id,
        cwd: v
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
        status: v.get("status").and_then(Value::as_str).map(str::to_string),
        name: v
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        started_at_ms: v.get("startedAt").and_then(Value::as_i64),
        updated_at_ms: v.get("updatedAt").and_then(Value::as_i64),
    })
}

/// Read `<config_dir>/sessions/<pid>.json`; `None` when missing or malformed.
pub fn read_claude_session_file(config_dir: &Path, pid: u32) -> Option<ClaudeSessionFile> {
    read_json(&config_dir.join("sessions").join(format!("{pid}.json")))
        .as_ref()
        .and_then(parse_claude_session_file)
}

/// Every readable `<config_dir>/sessions/*.json`.
pub fn list_claude_session_files(config_dir: &Path) -> Vec<ClaudeSessionFile> {
    let Ok(dir) = std::fs::read_dir(config_dir.join("sessions")) else {
        return vec![];
    };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            read_json(&e.path())
                .as_ref()
                .and_then(parse_claude_session_file)
        })
        .collect()
}

/// Claude's project directory name for a cwd: every non-alphanumeric character becomes `-`.
pub fn claude_project_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `<config_dir>/projects/<slug>/<id>.jsonl`, looking in the cwd's slug first and then
/// every project directory. `None` until Claude has written the file.
pub fn claude_transcript_path(
    config_dir: &Path,
    session_id: &str,
    cwd: Option<&Path>,
) -> Option<PathBuf> {
    if !super::is_uuid(session_id) {
        return None;
    }
    let projects = config_dir.join("projects");
    let file = format!("{session_id}.jsonl");
    if let Some(cwd) = cwd {
        let p = projects.join(claude_project_slug(cwd)).join(&file);
        if p.is_file() {
            return Some(p);
        }
    }
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(&file))
        .find(|p| p.is_file())
}

/// What bro knows about a pane when it starts.
#[derive(Debug, Clone)]
pub struct BindSpec {
    pub harness: Harness,
    /// "claude:work", "codex:local", … (None = the machine's own login)
    pub profile_id: Option<String>,
    /// Claude's `CLAUDE_CONFIG_DIR` / Codex's `CODEX_HOME`; derived from the profile when None
    pub config_dir: Option<PathBuf>,
    /// The agent process bro spawned
    pub pid: Option<u32>,
    pub cwd: PathBuf,
    pub started: SystemTime,
    /// `--resume`/`resume <id>` the pane was launched with
    pub resume_id: Option<String>,
}

/// Finds and follows the transcript a live pane is writing. Poll it from a worker thread;
/// it rate-limits itself.
#[derive(Debug)]
pub struct Binder {
    spec: BindSpec,
    path: Option<PathBuf>,
    session_id: Option<String>,
    next: Option<Instant>,
    attempts: u32,
}

/// Slack for clocks and file systems that round mtimes when matching "started after".
const START_SLACK: Duration = Duration::from_secs(5);
const CLAUDE_EVERY: Duration = Duration::from_secs(5);
const CODEX_SEARCH_EVERY: Duration = Duration::from_secs(3);
const CODEX_BOUND_EVERY: Duration = Duration::from_secs(15);
/// After this many unbound Codex searches, slow down to [`CODEX_BOUND_EVERY`].
const CODEX_FAST_ATTEMPTS: u32 = 20;

impl Binder {
    pub fn new(spec: BindSpec) -> Self {
        Binder {
            spec,
            path: None,
            session_id: None,
            next: None,
            attempts: 0,
        }
    }

    pub fn spec(&self) -> &BindSpec {
        &self.spec
    }

    /// The transcript currently bound (None until found).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The agent's own session id, when known (Claude: from `sessions/<pid>.json`).
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// The pid the binder looks up (it can change if the pane restarts its agent).
    pub fn set_pid(&mut self, pid: Option<u32>) {
        if self.spec.pid != pid {
            self.spec.pid = pid;
            self.next = None;
        }
    }

    /// Re-check when due. Returns true when the bound transcript changed (the caller then
    /// starts a fresh [`Tail`] and [`AgentFacts`]).
    pub fn poll(&mut self, now: Instant) -> bool {
        if self.next.is_some_and(|n| now < n) {
            return false;
        }
        let (found, every) = match self.spec.harness {
            Harness::Claude => (self.find_claude(), CLAUDE_EVERY),
            Harness::Codex => {
                let found = self.find_codex();
                self.attempts += 1;
                let every = if found.is_some() || self.attempts > CODEX_FAST_ATTEMPTS {
                    CODEX_BOUND_EVERY
                } else {
                    CODEX_SEARCH_EVERY
                };
                (found, every)
            }
            Harness::Pi | Harness::Omp => (None, Duration::from_secs(3600)),
        };
        self.next = Some(now + every);
        let Some((path, id)) = found else {
            return false;
        };
        if id.is_some() {
            self.session_id = id;
        }
        let changed = !self.path.as_deref().is_some_and(|p| same_path(p, &path));
        if changed {
            self.path = Some(path);
        }
        changed
    }

    fn config_dir(&self) -> PathBuf {
        if let Some(d) = &self.spec.config_dir {
            return d.clone();
        }
        if let Some(p) = self.spec.profile_id.as_deref().and_then(profiles::get) {
            return p.dir;
        }
        match self.spec.harness {
            Harness::Codex => paths::codex_local_dir(),
            _ => paths::claude_local_dir(),
        }
    }

    fn since(&self) -> SystemTime {
        self.spec
            .started
            .checked_sub(START_SLACK)
            .unwrap_or(self.spec.started)
    }

    fn find_claude(&mut self) -> Option<(PathBuf, Option<String>)> {
        let dir = self.config_dir();
        // 1. the process's own session file
        let by_pid = self
            .spec
            .pid
            .and_then(|pid| read_claude_session_file(&dir, pid));
        // 2. a session file for this cwd started with the pane (bro may have spawned a shim)
        let file = by_pid.or_else(|| {
            let since_ms = crate::util::system_time_ms(self.since()) as i64;
            list_claude_session_files(&dir)
                .into_iter()
                .filter(|f| {
                    f.cwd
                        .as_deref()
                        .is_some_and(|c| same_path(c, &self.spec.cwd))
                })
                .filter(|f| f.started_at_ms.is_some_and(|t| t >= since_ms))
                .max_by_key(|f| f.started_at_ms)
        });
        if let Some(f) = file {
            let cwd = f.cwd.clone().unwrap_or_else(|| self.spec.cwd.clone());
            if let Some(p) = claude_transcript_path(&dir, &f.session_id, Some(&cwd)) {
                return Some((p, Some(f.session_id)));
            }
            // Not written yet (no message sent): remember the id, keep the old binding.
            self.session_id = Some(f.session_id);
            return None;
        }
        // 3. the resumed session, 4. the newest transcript for this cwd
        let resumed = self.spec.resume_id.as_deref().and_then(|id| {
            claude_transcript_path(&dir, id, Some(&self.spec.cwd))
                .map(|p| (p, Some(id.to_string())))
        });
        resumed.or_else(|| {
            latest_for(
                Harness::Claude,
                self.spec.profile_id.as_deref(),
                &self.spec.cwd,
                self.since(),
            )
            .map(|s| (s.path, Some(s.id)))
        })
    }

    fn find_codex(&mut self) -> Option<(PathBuf, Option<String>)> {
        latest_for(
            Harness::Codex,
            self.spec.profile_id.as_deref(),
            &self.spec.cwd,
            self.since(),
        )
        .map(|s| (s.path, Some(s.id)))
        .or_else(|| {
            let id = self.spec.resume_id.as_deref()?;
            find_in_profile(Harness::Codex, id, self.spec.profile_id.as_deref())
                .map(|s| (s.path, Some(s.id)))
        })
    }
}

// ---------------------------------------------------------------------------------------
// Incremental reading
// ---------------------------------------------------------------------------------------

/// Reads new complete lines from a growing JSONL file.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    created: Option<SystemTime>,
    started: bool,
}

/// What one [`Tail::read`] produced.
#[derive(Debug, Default, PartialEq)]
pub struct TailRead {
    /// The file shrank or was replaced: drop everything derived from earlier lines
    /// (`lines` then start from the new file).
    pub reset: bool,
    pub lines: Vec<String>,
}

impl Tail {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Tail {
            path: path.into(),
            offset: 0,
            partial: Vec::new(),
            created: None,
            started: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes consumed so far (including a pending partial line).
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// New complete lines since the last call. The first call starts at most
    /// [`FIRST_READ_MAX`] from the end (dropping the cut-off first line).
    pub fn read(&mut self) -> std::io::Result<TailRead> {
        let mut file = std::fs::File::open(&self.path)?;
        let meta = file.metadata()?;
        let len = meta.len();
        let created = meta.created().ok();
        let mut out = TailRead::default();
        let replaced = self.started
            && (len < self.offset
                || (created.is_some() && self.created.is_some() && created != self.created));
        if replaced {
            out.reset = true;
            self.offset = 0;
            self.partial.clear();
            self.started = false;
        }
        let mut skip_first = false;
        if !self.started {
            self.started = true;
            self.created = created;
            if len > FIRST_READ_MAX {
                self.offset = len - FIRST_READ_MAX;
                skip_first = true;
            }
        }
        if len <= self.offset {
            return Ok(out);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::with_capacity((len - self.offset) as usize);
        file.take(len - self.offset).read_to_end(&mut buf)?;
        self.offset += buf.len() as u64;
        let mut data = std::mem::take(&mut self.partial);
        data.extend_from_slice(&buf);
        let mut start = 0;
        if skip_first {
            match data.iter().position(|&b| b == b'\n') {
                Some(i) => start = i + 1,
                None => {
                    self.partial = data;
                    // still inside the cut-off line: it never becomes a line of its own
                    self.partial.clear();
                    return Ok(out);
                }
            }
        }
        let mut rest = &data[start..];
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&rest[..i]);
            let line = line.trim_end_matches('\r');
            if !line.trim().is_empty() {
                out.lines.push(line.to_string());
            }
            rest = &rest[i + 1..];
        }
        self.partial = rest.to_vec();
        Ok(out)
    }
}

// ---------------------------------------------------------------------------------------
// Facts
// ---------------------------------------------------------------------------------------

/// A file the agent deliberately sent to the user (Claude's `SendUserFile`).
#[derive(Debug, Clone, PartialEq)]
pub struct SentFile {
    pub path: PathBuf,
    pub caption: Option<String>,
    pub at_ms: Option<i64>,
    pub size: Option<u64>,
    pub is_image: bool,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchKind {
    Edit,
    Write,
    Create,
    Delete,
    Move,
    Read,
}

impl TouchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TouchKind::Edit => "edit",
            TouchKind::Write => "write",
            TouchKind::Create => "create",
            TouchKind::Delete => "delete",
            TouchKind::Move => "move",
            TouchKind::Read => "read",
        }
    }
}

/// A file the agent read or changed successfully.
#[derive(Debug, Clone, PartialEq)]
pub struct TouchedFile {
    pub path: PathBuf,
    pub kind: TouchKind,
    pub at_ms: Option<i64>,
    pub line: Option<u32>,
}

/// The last tool call still waiting for its result.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub tool: String,
    /// "Bash: npm publish", "Question: Which DB?", …
    pub reason: String,
    pub at_ms: Option<i64>,
}

/// The turn ended on an API error (Claude's synthetic `isApiErrorMessage` entry, Codex's `error` event) and
/// nothing has happened since.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    /// Claude's `error` code (`server_error`, `rate_limit`, `authentication_failed`, `unknown`, ...); None for Codex
    pub code: Option<String>,
    pub text: String,
    /// Identifies this occurrence (the record's uuid, else its timestamp)
    pub id: String,
    pub at_ms: Option<i64>,
}

/// Text with the time it was written.
#[derive(Debug, Clone, PartialEq)]
pub struct Stamped {
    pub text: String,
    pub at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct OpenCall {
    id: String,
    name: String,
    input: Value,
    at_ms: Option<i64>,
    /// Claude: the assistant message the call belongs to
    message_id: Option<String>,
}

/// Everything bro derives from one transcript. Feed it lines in order with [`ingest`](Self::ingest).
#[derive(Debug, Clone)]
pub struct AgentFacts {
    pub harness: Harness,
    /// Newest first, deduped by path
    pub sent: Vec<SentFile>,
    /// Newest first, deduped by path
    pub touched: Vec<TouchedFile>,
    pub pending: Option<Pending>,
    /// The turn ended on a question to the user: that line
    pub asked: Option<String>,
    /// Claude's own recap (`system`/`away_summary`)
    pub away_summary: Option<Stamped>,
    /// First sentence/line of the last assistant text
    pub last_assistant: Option<Stamped>,
    /// Newest record timestamp
    pub last_activity_ms: Option<i64>,
    /// The turn stopped on an API error, still unanswered
    pub api_error: Option<ApiError>,
    /// The agent's session id as recorded in the transcript
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    /// Bumped whenever anything above changed
    pub revision: u64,
    open: Vec<OpenCall>,
    /// Codex apply_patch calls waiting for their outcome: call id → parsed changes
    patches: HashMap<String, Vec<(PathBuf, TouchKind)>>,
}

impl AgentFacts {
    pub fn new(harness: Harness) -> Self {
        AgentFacts {
            harness,
            sent: vec![],
            touched: vec![],
            pending: None,
            asked: None,
            away_summary: None,
            last_assistant: None,
            last_activity_ms: None,
            api_error: None,
            session_id: None,
            cwd: None,
            revision: 0,
            open: vec![],
            patches: HashMap::new(),
        }
    }

    /// Ingest transcript lines (malformed lines are ignored). Returns true if anything changed.
    pub fn ingest<S: AsRef<str>>(&mut self, lines: &[S]) -> bool {
        let before = self.revision;
        for line in lines {
            self.ingest_line(line.as_ref());
        }
        self.revision != before
    }

    /// Ingest one JSONL line.
    pub fn ingest_line(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let snapshot = self.fingerprint();
        if let Some(ts) = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso_ms)
            && self.last_activity_ms.is_none_or(|t| ts > t)
        {
            self.last_activity_ms = Some(ts);
        }
        match self.harness {
            Harness::Claude => self.claude(&v),
            Harness::Codex => self.codex(&v),
            _ => {}
        }
        if self.fingerprint() != snapshot {
            self.revision += 1;
        }
    }

    /// Whether Claude's `away_summary` is at least as new as the last assistant text.
    pub fn away_summary_is_fresh(&self) -> bool {
        match (&self.away_summary, &self.last_assistant) {
            (Some(_), None) => true,
            (Some(a), Some(l)) => match (a.at_ms, l.at_ms) {
                (Some(a), Some(l)) => a >= l,
                _ => false,
            },
            _ => false,
        }
    }

    /// A cheap hash of everything observable, to bump `revision` only on real changes.
    fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            self.sent.len(),
            self.sent.first().map(|s| (path_key(&s.path), s.at_ms)),
        )
            .hash(&mut h);
        (
            self.touched.len(),
            self.touched
                .first()
                .map(|t| (path_key(&t.path), t.at_ms, t.kind as u8, t.line)),
        )
            .hash(&mut h);
        self.pending.as_ref().map(|p| &p.reason).hash(&mut h);
        self.asked.hash(&mut h);
        self.away_summary
            .as_ref()
            .map(|a| (&a.text, a.at_ms))
            .hash(&mut h);
        self.last_assistant
            .as_ref()
            .map(|a| (&a.text, a.at_ms))
            .hash(&mut h);
        (self.last_activity_ms, &self.session_id, &self.cwd).hash(&mut h);
        self.api_error.as_ref().map(|e| &e.id).hash(&mut h);
        h.finish()
    }

    fn absolute(&self, raw: &str, record_cwd: Option<&str>) -> PathBuf {
        let raw = raw
            .strip_prefix("file:///")
            .map(|s| {
                if cfg!(windows) {
                    s.to_string()
                } else {
                    format!("/{s}")
                }
            })
            .unwrap_or_else(|| raw.to_string());
        let p = PathBuf::from(raw);
        if p.is_absolute() {
            return p;
        }
        match record_cwd.map(PathBuf::from).or_else(|| self.cwd.clone()) {
            Some(cwd) => cwd.join(p),
            None => p,
        }
    }

    fn touch(&mut self, path: PathBuf, kind: TouchKind, at_ms: Option<i64>, line: Option<u32>) {
        let key = path_key(&path);
        self.touched.retain(|t| path_key(&t.path) != key);
        self.touched.insert(
            0,
            TouchedFile {
                path,
                kind,
                at_ms,
                line,
            },
        );
        self.touched.truncate(LIST_MAX);
    }

    fn send(&mut self, file: SentFile) {
        let key = path_key(&file.path);
        self.sent.retain(|s| path_key(&s.path) != key);
        self.sent.insert(0, file);
        self.sent.truncate(LIST_MAX);
    }

    fn refresh_pending(&mut self) {
        self.pending = self.open.last().map(|c| Pending {
            tool: c.name.clone(),
            reason: reason_for_tool(&c.name, &c.input),
            at_ms: c.at_ms,
        });
    }

    fn user_prompt(&mut self) {
        self.api_error = None;
        self.open.clear();
        self.asked = None;
        self.refresh_pending();
    }

    fn assistant_text(&mut self, text: &str, at_ms: Option<i64>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.last_assistant = Some(Stamped {
            text: first_sentence(text, LAST_ASSISTANT_MAX),
            at_ms,
        });
        self.asked = asked_line(text);
    }

    // ---- Claude ------------------------------------------------------------------------

    fn claude(&mut self, v: &Value) {
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        if let Some(id) = v.get("sessionId").and_then(Value::as_str) {
            self.session_id = Some(id.to_string());
        }
        if let Some(cwd) = v
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.cwd = Some(PathBuf::from(cwd));
        }
        let at = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso_ms);
        let cwd = v.get("cwd").and_then(Value::as_str);
        match v.get("type").and_then(Value::as_str) {
            Some("system") if v.get("subtype").and_then(Value::as_str) == Some("away_summary") => {
                if let Some(text) = v
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    self.away_summary = Some(Stamped {
                        text: text.to_string(),
                        at_ms: at,
                    });
                }
            }
            Some("assistant") => {
                let msg = v.get("message");
                self.api_error = (v.get("isApiErrorMessage").and_then(Value::as_bool)
                    == Some(true))
                .then(|| ApiError {
                    code: v.get("error").and_then(Value::as_str).map(str::to_string),
                    text: claude_text(msg),
                    id: v
                        .get("uuid")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| at.unwrap_or_default().to_string()),
                    at_ms: at,
                });
                let message_id = msg
                    .and_then(|m| m.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // A new assistant message means calls from an older one will never get results.
                if message_id.is_some()
                    && self
                        .open
                        .iter()
                        .any(|c| c.message_id.is_some() && c.message_id != message_id)
                {
                    self.open
                        .retain(|c| c.message_id.is_none() || c.message_id == message_id);
                    self.refresh_pending();
                }
                let Some(blocks) = msg.and_then(|m| m.get("content")).and_then(Value::as_array)
                else {
                    return;
                };
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(Value::as_str) {
                                self.assistant_text(t, at);
                            }
                        }
                        Some("tool_use") => {
                            let (Some(id), Some(name)) = (
                                b.get("id").and_then(Value::as_str),
                                b.get("name").and_then(Value::as_str),
                            ) else {
                                continue;
                            };
                            // The text before a tool call was not the end of the turn.
                            self.asked = None;
                            self.open.retain(|c| c.id != id);
                            self.open.push(OpenCall {
                                id: id.to_string(),
                                name: name.to_string(),
                                input: b.get("input").cloned().unwrap_or(Value::Null),
                                at_ms: at,
                                message_id: message_id.clone(),
                            });
                            self.refresh_pending();
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    return;
                }
                let content = v.get("message").and_then(|m| m.get("content"));
                let results: Vec<&Value> = content
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter(|b| {
                                b.get("type").and_then(Value::as_str) == Some("tool_result")
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if results.is_empty() {
                    // A real prompt (string or text blocks) starts a new turn.
                    if content.is_some() {
                        self.user_prompt();
                    }
                    return;
                }
                let tool_result = v.get("toolUseResult");
                for r in results {
                    let Some(id) = r.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(pos) = self.open.iter().position(|c| c.id == id) else {
                        continue;
                    };
                    let call = self.open.remove(pos);
                    let failed = r.get("is_error").and_then(Value::as_bool) == Some(true)
                        || tool_result
                            .and_then(Value::as_str)
                            .is_some_and(|s| s.trim_start().starts_with("Error"));
                    if !failed {
                        self.claude_success(&call, tool_result, at.or(call.at_ms), cwd);
                    }
                }
                self.refresh_pending();
            }
            _ => {}
        }
    }

    fn claude_success(
        &mut self,
        call: &OpenCall,
        result: Option<&Value>,
        at: Option<i64>,
        cwd: Option<&str>,
    ) {
        let input = &call.input;
        let file_path = || {
            input
                .get("file_path")
                .or_else(|| input.get("notebook_path"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        match call.name.as_str() {
            "SendUserFile" => {
                let caption = input
                    .get("caption")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let attachments = result
                    .and_then(|r| r.get("attachments"))
                    .and_then(Value::as_array);
                if let Some(list) = attachments.filter(|a| !a.is_empty()) {
                    for a in list {
                        let Some(p) = a.get("path").and_then(Value::as_str) else {
                            continue;
                        };
                        self.send(SentFile {
                            path: self.absolute(p, cwd),
                            caption: caption.clone(),
                            at_ms: at,
                            size: a.get("size").and_then(Value::as_u64),
                            is_image: a.get("isImage").and_then(Value::as_bool).unwrap_or(false),
                            mime_type: a
                                .get("media_type")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        });
                    }
                } else if let Some(files) = input.get("files").and_then(Value::as_array) {
                    for p in files.iter().filter_map(Value::as_str) {
                        let path = self.absolute(p, cwd);
                        let mime = guess_image_mime(&path);
                        self.send(SentFile {
                            path,
                            caption: caption.clone(),
                            at_ms: at,
                            size: None,
                            is_image: mime.is_some(),
                            mime_type: mime.map(str::to_string),
                        });
                    }
                }
            }
            "Edit" | "MultiEdit" | "NotebookEdit" => {
                if let Some(p) = file_path() {
                    let line = result
                        .and_then(|r| r.get("structuredPatch"))
                        .and_then(Value::as_array)
                        .and_then(|h| h.first())
                        .and_then(first_changed_line);
                    self.touch(self.absolute(p, cwd), TouchKind::Edit, at, line);
                }
            }
            "Write" => {
                if let Some(p) = file_path() {
                    let kind = if result.and_then(|r| r.get("type")).and_then(Value::as_str)
                        == Some("create")
                    {
                        TouchKind::Create
                    } else {
                        TouchKind::Write
                    };
                    self.touch(self.absolute(p, cwd), kind, at, None);
                }
            }
            "Read" => {
                if let Some(p) = file_path() {
                    let line = input
                        .get("offset")
                        .and_then(Value::as_u64)
                        .filter(|&n| n > 0)
                        .and_then(|n| u32::try_from(n).ok());
                    self.touch(self.absolute(p, cwd), TouchKind::Read, at, line);
                }
            }
            _ => {}
        }
    }

    // ---- Codex -------------------------------------------------------------------------

    fn codex(&mut self, v: &Value) {
        let at = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso_ms);
        let Some(payload) = v.get("payload") else {
            return;
        };
        let kind = payload.get("type").and_then(Value::as_str);
        match v.get("type").and_then(Value::as_str) {
            Some("session_meta") | Some("turn_context") => {
                if let Some(id) = payload.get("id").and_then(Value::as_str) {
                    self.session_id = Some(id.to_string());
                }
                if let Some(cwd) = payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    self.cwd = Some(PathBuf::from(cwd));
                }
            }
            Some("response_item") => match kind {
                Some("message")
                    if payload.get("role").and_then(Value::as_str) == Some("assistant") =>
                {
                    let text: String = payload
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|b| b.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    self.assistant_text(&text, at);
                }
                Some("function_call") | Some("custom_tool_call") => {
                    let (Some(id), Some(name)) = (
                        payload.get("call_id").and_then(Value::as_str),
                        payload.get("name").and_then(Value::as_str),
                    ) else {
                        return;
                    };
                    let input = match payload.get("arguments") {
                        Some(Value::String(s)) => {
                            serde_json::from_str(s).unwrap_or(Value::String(s.clone()))
                        }
                        Some(other) => other.clone(),
                        None => payload.get("input").cloned().unwrap_or(Value::Null),
                    };
                    if name == "apply_patch"
                        && let Some(text) = input
                            .as_str()
                            .or_else(|| input.get("input").and_then(Value::as_str))
                    {
                        let changes = parse_apply_patch(text)
                            .into_iter()
                            .map(|(p, k)| (self.absolute(&p, None), k))
                            .collect();
                        self.patches.insert(id.to_string(), changes);
                    }
                    self.asked = None;
                    self.open.retain(|c| c.id != id);
                    self.open.push(OpenCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        input,
                        at_ms: at,
                        message_id: None,
                    });
                    self.refresh_pending();
                }
                Some("function_call_output") | Some("custom_tool_call_output") => {
                    let Some(id) = payload.get("call_id").and_then(Value::as_str) else {
                        return;
                    };
                    self.open.retain(|c| c.id != id);
                    if let Some(changes) = self.patches.remove(id) {
                        let output = payload
                            .get("output")
                            .map(|o| o.to_string().to_lowercase())
                            .unwrap_or_default();
                        if !(output.contains("error") || output.contains("failed")) {
                            for (p, k) in changes {
                                self.touch(p, k, at, None);
                            }
                        }
                    }
                    self.refresh_pending();
                }
                _ => {}
            },
            Some("event_msg") => match kind {
                Some("user_message") => self.user_prompt(),
                Some("error") => {
                    let text = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    self.api_error = Some(ApiError {
                        code: None,
                        id: at.unwrap_or_default().to_string(),
                        text,
                        at_ms: at,
                    });
                }
                Some("task_started") | Some("agent_message") => self.api_error = None,
                Some("task_complete") | Some("turn_aborted") => {
                    self.open.clear();
                    self.refresh_pending();
                }
                Some("patch_apply_end") => {
                    let id = payload
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    self.patches.remove(&id);
                    if payload.get("success").and_then(Value::as_bool) == Some(false) {
                        return;
                    }
                    if let Some(changes) = payload.get("changes").and_then(Value::as_object) {
                        for (p, change) in changes {
                            self.codex_change(p, change, at);
                        }
                    }
                }
                Some("item_completed") => {
                    let Some(item) = payload.get("item") else {
                        return;
                    };
                    let status = item.get("status").and_then(Value::as_str);
                    if matches!(status, Some("failed") | Some("declined")) {
                        return;
                    }
                    match item.get("type").and_then(Value::as_str) {
                        Some("FileChange") => {
                            if let Some(changes) = item.get("changes").and_then(Value::as_object) {
                                for (p, change) in changes {
                                    self.codex_change(p, change, at);
                                }
                            }
                        }
                        Some("CommandExecution") => {
                            for c in item
                                .get("parsed_cmd")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                            {
                                if c.get("type").and_then(Value::as_str) == Some("read")
                                    && let Some(p) = c
                                        .get("path")
                                        .and_then(Value::as_str)
                                        .filter(|s| !s.is_empty())
                                {
                                    self.touch(self.absolute(p, None), TouchKind::Read, at, None);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn codex_change(&mut self, path: &str, change: &Value, at: Option<i64>) {
        let kind = match change.get("type").and_then(Value::as_str) {
            Some("add") => TouchKind::Create,
            Some("delete") => TouchKind::Delete,
            _ => TouchKind::Edit,
        };
        if let Some(to) = change
            .get("move_path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.touch(self.absolute(to, None), TouchKind::Move, at, None);
            return;
        }
        let line = change
            .get("unified_diff")
            .and_then(Value::as_str)
            .and_then(first_hunk_line);
        self.touch(self.absolute(path, None), kind, at, line);
    }
}

/// `+12` from the first `@@ -a,b +12,c @@` hunk header.
fn first_hunk_line(diff: &str) -> Option<u32> {
    let header = diff.lines().find(|l| l.starts_with("@@"))?;
    let plus = header.split_whitespace().find(|w| w.starts_with('+'))?;
    plus[1..]
        .split(',')
        .next()?
        .parse()
        .ok()
        .filter(|&n: &u32| n > 0)
}

/// The first added or removed line of a `structuredPatch` hunk. `newStart` is where the hunk
/// begins, which is up to three context lines before the change.
fn first_changed_line(hunk: &Value) -> Option<u32> {
    let start = hunk.get("newStart").and_then(Value::as_u64)?;
    let context = hunk
        .get("lines")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(Value::as_str)
                .take_while(|l| !l.starts_with('+') && !l.starts_with('-'))
                .count() as u64
        })
        .unwrap_or(0);
    u32::try_from(start + context).ok()
}

/// `*** Add|Update|Delete File: <path>` (and `*** Move to: <path>`) from an apply_patch body.
pub fn parse_apply_patch(text: &str) -> Vec<(String, TouchKind)> {
    let mut out: Vec<(String, TouchKind)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        let (kind, rest) = if let Some(r) = line.strip_prefix("*** Add File: ") {
            (TouchKind::Create, r)
        } else if let Some(r) = line.strip_prefix("*** Update File: ") {
            (TouchKind::Edit, r)
        } else if let Some(r) = line.strip_prefix("*** Delete File: ") {
            (TouchKind::Delete, r)
        } else if let Some(r) = line.strip_prefix("*** Move to: ") {
            out.pop();
            (TouchKind::Move, r)
        } else {
            continue;
        };
        out.push((rest.trim().to_string(), kind));
    }
    out
}

fn guess_image_mime(path: &Path) -> Option<&'static str> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Pure text rules
// ---------------------------------------------------------------------------------------

/// Cut to `max` characters, ending in `…` when cut. Whitespace runs collapse to one space.
pub fn truncate(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

fn basename(p: &str) -> &str {
    p.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(p)
}

/// Why a pending tool call is waiting, herdr-island style: `Bash: <description|command>`,
/// `Edit: <basename>`, `Question: <header|question>`, `Approve the plan?`, `MCP: <tool>`,
/// Codex `request_user_input_async` → `Question: <title>`, else the tool name.
/// Cut to [`REASON_MAX`].
pub fn reason_for_tool(name: &str, input: &Value) -> String {
    let s = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let first_question = || {
        input
            .get("questions")
            .and_then(Value::as_array)
            .and_then(|q| q.first())
    };
    let text = match name {
        "Bash" => {
            let what = s(input.get("description")).or_else(|| {
                s(input.get("command")).map(|c| c.lines().next().unwrap_or_default().to_string())
            });
            what.map_or_else(|| "Bash".to_string(), |w| format!("Bash: {w}"))
        }
        "Edit" | "MultiEdit" | "Write" | "Read" | "NotebookEdit" => {
            match s(input.get("file_path")).or_else(|| s(input.get("notebook_path"))) {
                Some(p) => format!("{name}: {}", basename(&p)),
                None => name.to_string(),
            }
        }
        "AskUserQuestion" => {
            let q = first_question();
            let what = s(q.and_then(|q| q.get("header")))
                .or_else(|| s(q.and_then(|q| q.get("question"))))
                .or_else(|| s(input.get("question")));
            what.map_or_else(|| "Question".to_string(), |w| format!("Question: {w}"))
        }
        "ExitPlanMode" => "Approve the plan?".to_string(),
        "request_user_input_async" | "request_user_input" => {
            let q = first_question();
            let what =
                s(q.and_then(|q| q.get("title"))).or_else(|| s(q.and_then(|q| q.get("question"))));
            what.map_or_else(|| "Question".to_string(), |w| format!("Question: {w}"))
        }
        "shell" | "local_shell" | "exec_command" | "shell_command" => {
            let cmd = match input.get("command").or_else(|| input.get("cmd")) {
                Some(Value::Array(parts)) => Some(
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                Some(Value::String(c)) => Some(c.clone()),
                _ => None,
            };
            cmd.filter(|c| !c.trim().is_empty()).map_or_else(
                || "Run a command".to_string(),
                |c| format!("Run: {}", c.lines().next().unwrap_or_default()),
            )
        }
        other if other.starts_with("mcp__") => {
            format!("MCP: {}", other.rsplit("__").next().unwrap_or(other))
        }
        other => other.to_string(),
    };
    truncate(&text, REASON_MAX)
}

/// The first sentence (or line) of `text`, cut to `max` characters. Markdown emphasis
/// and heading/bullet markers are dropped.
pub fn first_sentence(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(clean_markdown)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let mut end = line.len();
    let bytes = line.as_bytes();
    for (i, c) in line.char_indices() {
        if matches!(c, '.' | '!' | '?') && (i + 1 == bytes.len() || bytes[i + 1] == b' ') {
            end = i + 1;
            break;
        }
    }
    truncate(&line[..end], max)
}

fn clean_markdown(line: &str) -> String {
    let t = line.trim();
    let t = t.trim_start_matches(['#', '>', '-', '*', '+']).trim_start();
    t.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_string()
}

/// Remove fenced code blocks, inline code and quoted strings so a `?` inside them
/// doesn't count.
fn strip_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push('\n');
            continue;
        }
        let mut quote: Option<char> = None;
        for c in line.chars() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if matches!(c, '`' | '"' | '“' | '”') => {
                    quote = Some(if c == '“' { '”' } else { c })
                }
                None => out.push(c),
            }
        }
        out.push('\n');
    }
    out
}

const ASK_PHRASES: &[&str] = &[
    "let me know",
    "please confirm",
    "can you confirm",
    "could you confirm",
    "should i ",
    "shall i ",
    "want me to",
    "would you like",
    "do you want",
    "up to you",
    "your call",
    "go-ahead",
    "go ahead?",
    "which would you",
    "which do you",
];

/// The line a finished turn asks the user (attention-queue's rule): within the last
/// ~700 characters (code and quotes ignored), the last line ending in `?`, else the last
/// line with a "let me know / confirm / should I / want me to …" phrase.
/// Cut to [`REASON_MAX`].
pub fn asked_line(text: &str) -> Option<String> {
    let tail_start = text
        .char_indices()
        .rev()
        .nth(ASKED_WINDOW)
        .map_or(0, |(i, _)| i);
    let tail = strip_quoted(&text[tail_start..]);
    let lines: Vec<String> = tail
        .lines()
        .map(clean_markdown)
        .filter(|l| !l.is_empty())
        .collect();
    let ends_q = |l: &str| l.trim_end_matches([')', '*', '_', ' ', '.']).ends_with('?');
    let found = lines.iter().rev().find(|l| ends_q(l)).or_else(|| {
        lines.iter().rev().find(|l| {
            let lower = l.to_lowercase();
            ASK_PHRASES.iter().any(|p| lower.contains(p))
        })
    })?;
    // A long paragraph: keep the sentence holding the question.
    let sentence = found
        .split_inclusive(['.', '!', '?'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .rev()
        .find(|s| ends_q(s) || ASK_PHRASES.iter().any(|p| s.to_lowercase().contains(p)))
        .unwrap_or(found);
    Some(truncate(sentence, REASON_MAX))
}

/// Why a pane is waiting (section 3 of the doc): while it needs you, the pending call's
/// reason, else the screen prompt's title, else the question its turn ended on; when it
/// is done, only that question.
pub fn waiting_reason(
    facts: Option<&AgentFacts>,
    screen_prompt_title: Option<&str>,
    needs_you: bool,
) -> Option<String> {
    let asked = facts.and_then(|f| f.asked.clone());
    if !needs_you {
        return asked;
    }
    facts
        .and_then(|f| f.pending.as_ref().map(|p| p.reason.clone()))
        .or_else(|| {
            screen_prompt_title
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| truncate(t, REASON_MAX))
        })
        .or(asked)
}

/// The text blocks of a Claude message, joined.
fn claude_text(msg: Option<&Value>) -> String {
    msg.and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(
                    "
",
                )
        })
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;
    use serde_json::json;

    #[test]
    fn edit_line_skips_the_hunks_leading_context() {
        // Real Claude Code shape: the hunk starts two context lines before the change.
        let hunk = json!({"oldStart": 1, "newStart": 1, "lines": [" a", " b", "-c", "+d", " e"]});
        assert_eq!(first_changed_line(&hunk), Some(3));
        assert_eq!(
            first_changed_line(&json!({"newStart": 12, "lines": []})),
            Some(12)
        );
        assert_eq!(first_changed_line(&json!({"lines": ["+x"]})), None);
    }

    fn claude_facts(lines: &[Value]) -> AgentFacts {
        let mut f = AgentFacts::new(Harness::Claude);
        f.ingest(&lines.iter().map(Value::to_string).collect::<Vec<_>>());
        f
    }

    fn codex_facts(lines: &[Value]) -> AgentFacts {
        let mut f = AgentFacts::new(Harness::Codex);
        f.ingest(&lines.iter().map(Value::to_string).collect::<Vec<_>>());
        f
    }

    fn assistant(msg: &str, ts: &str, content: Value) -> Value {
        json!({"type": "assistant", "timestamp": ts, "sessionId": "s1", "cwd": "F:\\proj", "message": {"id": msg, "role": "assistant", "content": content}})
    }

    fn result(ts: &str, id: &str, is_error: bool, tool_result: Value) -> Value {
        let mut block = json!({"type": "tool_result", "tool_use_id": id, "content": "ok"});
        if is_error {
            block["is_error"] = json!(true);
        }
        json!({"type": "user", "timestamp": ts, "cwd": "F:\\proj", "message": {"role": "user", "content": [block]}, "toolUseResult": tool_result})
    }

    #[test]
    fn api_errors_are_remembered_until_the_next_prompt_or_reply() {
        let err = json!({"type": "assistant", "timestamp": "2026-10-07T14:35:44Z", "uuid": "u9", "isApiErrorMessage": true, "error": "server_error",
            "message": {"id": "x", "model": "<synthetic>", "role": "assistant", "content": [{"type": "text", "text": "API Error: Connection lost mid-response. The response above may be incomplete."}]}});
        let mut f = claude_facts(&[
            assistant(
                "m1",
                "2026-10-07T14:35:00Z",
                json!([{"type": "text", "text": "working on it"}]),
            ),
            err.clone(),
        ]);
        let e = f.api_error.clone().expect("api error");
        assert_eq!(
            (e.code.as_deref(), e.id.as_str()),
            (Some("server_error"), "u9")
        );
        assert!(e.text.starts_with("API Error: Connection lost"));
        f.ingest(&[json!({"type": "user", "timestamp": "2026-10-07T14:36:00Z", "message": {"role": "user", "content": "continue"}}).to_string()]);
        assert!(f.api_error.is_none());
        f.ingest(&[err.to_string()]);
        assert!(f.api_error.is_some());
        f.ingest(&[assistant(
            "m2",
            "2026-10-07T14:37:00Z",
            json!([{"type": "text", "text": "back"}]),
        )
        .to_string()]);
        assert!(f.api_error.is_none());

        let mut c = codex_facts(&[
            json!({"type": "event_msg", "timestamp": "2026-10-01T09:00:01Z", "payload": {"type": "error", "message": "stream disconnected before completion"}}),
        ]);
        assert_eq!(
            c.api_error.as_ref().map(|e| e.text.as_str()),
            Some("stream disconnected before completion")
        );
        c.ingest(&[json!({"type": "event_msg", "timestamp": "2026-10-01T09:00:05Z", "payload": {"type": "task_started"}}).to_string()]);
        assert!(c.api_error.is_none());
    }

    #[test]
    fn claude_sent_and_touched_files_follow_successful_results() {
        let f = claude_facts(&[
            json!({"type": "user", "timestamp": "2026-10-01T10:00:00Z", "cwd": "F:\\proj", "message": {"role": "user", "content": "make a chart"}}),
            assistant(
                "m1",
                "2026-10-01T10:00:01Z",
                json!([{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "F:\\proj\\src\\a.rs", "offset": 40, "limit": 20}}]),
            ),
            result(
                "2026-10-01T10:00:02Z",
                "t1",
                false,
                json!({"type": "text", "file": {"filePath": "F:\\proj\\src\\a.rs"}}),
            ),
            assistant(
                "m2",
                "2026-10-01T10:00:03Z",
                json!([{"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": "F:\\proj\\src\\a.rs", "old_string": "a", "new_string": "b"}}]),
            ),
            result(
                "2026-10-01T10:00:04Z",
                "t2",
                false,
                json!({"filePath": "F:\\proj\\src\\a.rs", "structuredPatch": [{"oldStart": 12, "newStart": 12, "lines": []}]}),
            ),
            assistant(
                "m3",
                "2026-10-01T10:00:05Z",
                json!([{"type": "tool_use", "id": "t3", "name": "Edit", "input": {"file_path": "F:\\proj\\src\\missing.rs", "old_string": "x", "new_string": "y"}}]),
            ),
            result(
                "2026-10-01T10:00:06Z",
                "t3",
                true,
                json!("Error: String to replace not found in file."),
            ),
            assistant(
                "m4",
                "2026-10-01T10:00:07Z",
                json!([
                    {"type": "tool_use", "id": "t4", "name": "Write", "input": {"file_path": "out/chart.py", "content": "print()"}},
                    {"type": "tool_use", "id": "t5", "name": "NotebookEdit", "input": {"notebook_path": "F:\\proj\\nb.ipynb"}},
                ]),
            ),
            result(
                "2026-10-01T10:00:08Z",
                "t4",
                false,
                json!({"type": "create", "filePath": "F:\\proj\\out\\chart.py"}),
            ),
            result("2026-10-01T10:00:08Z", "t5", false, json!({})),
            assistant(
                "m5",
                "2026-10-01T10:00:09Z",
                json!([{"type": "tool_use", "id": "t6", "name": "SendUserFile", "input": {"files": ["F:\\proj\\out\\chart.png"], "caption": "The chart", "status": "normal"}}]),
            ),
            result(
                "2026-10-01T10:00:10Z",
                "t6",
                false,
                json!({"caption": "The chart", "attachments": [{"path": "F:\\proj\\out\\chart.png", "size": 2048, "isImage": true, "media_type": "image/png", "pathValidated": true}]}),
            ),
            assistant(
                "m6",
                "2026-10-01T10:00:11Z",
                json!([{"type": "tool_use", "id": "t7", "name": "SendUserFile", "input": {"files": "[\"F:\\\\proj\\\\x.png\"]"}}]),
            ),
            result(
                "2026-10-01T10:00:12Z",
                "t7",
                true,
                json!("Error: files must be an array"),
            ),
            assistant(
                "m7",
                "2026-10-01T10:00:13Z",
                json!([{"type": "tool_use", "id": "t8", "name": "SendUserFile", "input": {"files": ["F:\\proj\\report.pdf"]}}]),
            ),
            result(
                "2026-10-01T10:00:14Z",
                "t8",
                false,
                json!("1 file delivered"),
            ),
        ]);
        let sent: Vec<_> = f
            .sent
            .iter()
            .map(|s| {
                (
                    s.path.to_string_lossy().into_owned(),
                    s.is_image,
                    s.caption.clone(),
                )
            })
            .collect();
        assert_eq!(
            sent,
            [
                ("F:\\proj\\report.pdf".to_string(), false, None),
                (
                    "F:\\proj\\out\\chart.png".to_string(),
                    true,
                    Some("The chart".to_string())
                ),
            ]
        );
        assert_eq!(f.sent[1].mime_type.as_deref(), Some("image/png"));
        assert_eq!(f.sent[1].size, Some(2048));
        let touched: Vec<_> = f
            .touched
            .iter()
            .map(|t| (t.path.to_string_lossy().replace('/', "\\"), t.kind, t.line))
            .collect();
        assert_eq!(
            touched,
            [
                ("F:\\proj\\nb.ipynb".to_string(), TouchKind::Edit, None),
                (
                    "F:\\proj\\out\\chart.py".to_string(),
                    TouchKind::Create,
                    None
                ),
                ("F:\\proj\\src\\a.rs".to_string(), TouchKind::Edit, Some(12)),
            ],
            "the failed edit is skipped and a.rs is deduped to its newest touch"
        );
        assert!(f.pending.is_none());
        assert_eq!(f.session_id.as_deref(), Some("s1"));
        assert_eq!(f.last_activity_ms, parse_iso_ms("2026-10-01T10:00:14Z"));
    }

    #[test]
    fn claude_pending_reasons_and_turn_boundaries() {
        let mut f = claude_facts(&[assistant(
            "m1",
            "2026-10-01T10:00:00Z",
            json!([{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "rm -rf dist", "description": "Remove build output"}}]),
        )]);
        assert_eq!(
            f.pending.as_ref().unwrap().reason,
            "Bash: Remove build output"
        );
        f.ingest_line(&result("2026-10-01T10:00:01Z", "t1", false, json!({})).to_string());
        assert!(f.pending.is_none());

        f.ingest_line(&assistant("m2", "2026-10-01T10:00:02Z", json!([{"type": "tool_use", "id": "t2", "name": "AskUserQuestion", "input": {"questions": [{"header": "Database", "question": "Which DB?", "options": []}]}}])).to_string());
        assert_eq!(f.pending.as_ref().unwrap().reason, "Question: Database");
        // The user interrupted: a real prompt clears it.
        f.ingest_line(
            &json!({"type": "user", "message": {"content": "[Request interrupted by user]"}})
                .to_string(),
        );
        assert!(f.pending.is_none());

        f.ingest_line(&assistant("m3", "2026-10-01T10:00:03Z", json!([{"type": "tool_use", "id": "t3", "name": "ExitPlanMode", "input": {"plan": "…"}}])).to_string());
        assert_eq!(f.pending.as_ref().unwrap().reason, "Approve the plan?");
        // A newer assistant message means t3 will never get a result.
        f.ingest_line(&assistant("m4", "2026-10-01T10:00:04Z", json!([{"type": "tool_use", "id": "t4", "name": "mcp__github__create_pr", "input": {}}])).to_string());
        assert_eq!(f.pending.as_ref().unwrap().reason, "MCP: create_pr");
        // Parallel calls in one message: the last unresolved one is shown.
        f.ingest_line(&assistant("m4", "2026-10-01T10:00:04Z", json!([{"type": "tool_use", "id": "t5", "name": "Edit", "input": {"file_path": "C:\\a\\b\\lib.rs"}}])).to_string());
        assert_eq!(f.pending.as_ref().unwrap().reason, "Edit: lib.rs");
        f.ingest_line(&result("2026-10-01T10:00:05Z", "t5", false, json!({})).to_string());
        assert_eq!(f.pending.as_ref().unwrap().reason, "MCP: create_pr");
        // Sidechain (subagent) records are ignored.
        f.ingest_line(
            &json!({"type": "user", "isSidechain": true, "message": {"content": "sub prompt"}})
                .to_string(),
        );
        assert_eq!(f.pending.as_ref().unwrap().reason, "MCP: create_pr");
    }

    #[test]
    fn claude_asked_last_assistant_and_away_summary() {
        let mut f = claude_facts(&[assistant(
            "m1",
            "2026-10-01T10:00:00Z",
            json!([{"type": "text", "text": "Let me look at the tests first?"}, {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "a"}}]),
        )]);
        assert!(
            f.asked.is_none(),
            "text followed by a tool call is not the end of the turn"
        );
        f.ingest_line(&result("2026-10-01T10:00:01Z", "t1", false, json!({})).to_string());
        f.ingest_line(
            &assistant(
                "m2",
                "2026-10-01T10:00:02Z",
                json!([{"type": "text", "text": "**Done.** I fixed the parser and added tests.\n\n```rust\nlet x = a?;\n```\nShould I also update the README, or leave it for now?"}]),
            )
            .to_string(),
        );
        assert_eq!(f.last_assistant.as_ref().unwrap().text, "Done.");
        assert_eq!(
            f.asked.as_deref(),
            Some("Should I also update the README, or leave it for now?")
        );
        assert!(!f.away_summary_is_fresh());
        f.ingest_line(&json!({"type": "system", "subtype": "away_summary", "content": "Parser fixed; README decision pending.", "timestamp": "2026-10-01T10:05:00Z"}).to_string());
        assert_eq!(
            f.away_summary.as_ref().unwrap().text,
            "Parser fixed; README decision pending."
        );
        assert!(f.away_summary_is_fresh());
        f.ingest_line(&json!({"type": "user", "timestamp": "2026-10-01T10:06:00Z", "message": {"content": [{"type": "text", "text": "yes"}]}}).to_string());
        assert!(f.asked.is_none());
        f.ingest_line(
            &assistant(
                "m3",
                "2026-10-01T10:07:00Z",
                json!([{"type": "text", "text": "Updated the README."}]),
            )
            .to_string(),
        );
        assert!(
            !f.away_summary_is_fresh(),
            "newer assistant text makes the summary stale"
        );
        assert!(f.asked.is_none());
    }

    #[test]
    fn codex_changes_reads_questions_and_old_apply_patch() {
        let f = codex_facts(&[
            json!({"type": "session_meta", "timestamp": "2026-10-01T09:00:00Z", "payload": {"id": "019a-codex", "cwd": "F:\\tools"}}),
            json!({"type": "event_msg", "timestamp": "2026-10-01T09:00:01Z", "payload": {"type": "user_message", "message": "fix it"}}),
            json!({"type": "event_msg", "timestamp": "2026-10-01T09:00:02Z", "payload": {"type": "item_completed", "item": {"type": "CommandExecution", "status": "completed", "parsed_cmd": [{"type": "read", "cmd": "Get-Content x", "name": "SKILL.md", "path": "C:/skills/SKILL.md"}, {"type": "unknown", "cmd": "ls"}]}}}),
            json!({"type": "event_msg", "timestamp": "2026-10-01T09:00:03Z", "payload": {"type": "item_completed", "item": {"type": "FileChange", "status": "completed", "changes": {
                "F:\\tools\\src\\context.rs": {"type": "update", "unified_diff": "@@ -512,2 +512,14 @@\n-a\n+b", "move_path": null},
                "F:\\tools\\new.rs": {"type": "add", "content": "x"},
                "F:\\tools\\old.rs": {"type": "delete", "content": "x"},
                "F:\\tools\\a.rs": {"type": "update", "unified_diff": "", "move_path": "F:\\tools\\b.rs"}
            }}}}),
            json!({"type": "event_msg", "timestamp": "2026-10-01T09:00:04Z", "payload": {"type": "item_completed", "item": {"type": "FileChange", "status": "failed", "stderr": "Failed to write file", "changes": {"F:\\tools\\nope.rs": {"type": "update"}}}}}),
            json!({"type": "response_item", "timestamp": "2026-10-01T09:00:05Z", "payload": {"type": "custom_tool_call", "name": "apply_patch", "call_id": "c1", "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-a\n+b\n*** Add File: docs/x.md\n+hi\n*** End Patch"}}),
            json!({"type": "response_item", "timestamp": "2026-10-01T09:00:06Z", "payload": {"type": "custom_tool_call_output", "call_id": "c1", "output": "Success. Updated the following files:\nM src/lib.rs"}}),
            json!({"type": "response_item", "timestamp": "2026-10-01T09:00:07Z", "payload": {"type": "function_call", "name": "request_user_input_async", "call_id": "c2", "arguments": "{\"questions\":[{\"title\":\"Ship it now?\",\"options\":[\"yes\",\"no\"]}]}"}}),
        ]);
        let touched: Vec<_> = f
            .touched
            .iter()
            .map(|t| (t.path.to_string_lossy().replace('/', "\\"), t.kind, t.line))
            .collect();
        assert!(touched.contains(&(
            "F:\\tools\\src\\context.rs".into(),
            TouchKind::Edit,
            Some(512)
        )));
        assert!(touched.contains(&("F:\\tools\\new.rs".into(), TouchKind::Create, None)));
        assert!(touched.contains(&("F:\\tools\\old.rs".into(), TouchKind::Delete, None)));
        assert!(touched.contains(&("F:\\tools\\b.rs".into(), TouchKind::Move, None)));
        assert!(touched.contains(&("C:\\skills\\SKILL.md".into(), TouchKind::Read, None)));
        assert!(
            touched.contains(&("F:\\tools\\src\\lib.rs".into(), TouchKind::Edit, None)),
            "relative apply_patch paths join the session cwd: {touched:?}"
        );
        assert!(touched.contains(&("F:\\tools\\docs\\x.md".into(), TouchKind::Create, None)));
        assert!(
            !touched.iter().any(|t| t.0.contains("nope")),
            "failed changes are skipped"
        );
        assert_eq!(f.pending.as_ref().unwrap().reason, "Question: Ship it now?");
        assert_eq!(f.session_id.as_deref(), Some("019a-codex"));
        assert!(f.sent.is_empty());
    }

    #[test]
    fn codex_patch_apply_end_failure_and_turn_end() {
        let mut f = codex_facts(&[
            json!({"type": "session_meta", "payload": {"id": "x", "cwd": "/w"}}),
            json!({"type": "response_item", "payload": {"type": "custom_tool_call", "name": "apply_patch", "call_id": "c1", "input": "*** Update File: a.rs\n"}}),
            json!({"type": "event_msg", "payload": {"type": "patch_apply_end", "call_id": "c1", "success": false, "changes": {"/w/a.rs": {"type": "update"}}}}),
            json!({"type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": "c1", "output": "ok"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Which option do you prefer?"}]}}),
        ]);
        assert!(
            f.touched.is_empty(),
            "a failed patch_apply_end wins over the later output"
        );
        assert_eq!(f.asked.as_deref(), Some("Which option do you prefer?"));
        f.ingest_line(&json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell", "call_id": "c3", "arguments": "{\"command\":[\"cargo\",\"test\"]}"}}).to_string());
        assert_eq!(f.pending.as_ref().unwrap().reason, "Run: cargo test");
        f.ingest_line(
            &json!({"type": "event_msg", "payload": {"type": "task_complete"}}).to_string(),
        );
        assert!(f.pending.is_none());
    }

    #[test]
    fn reason_and_text_rules() {
        assert_eq!(
            reason_for_tool("Bash", &json!({"command": "npm publish\n--tag next"})),
            "Bash: npm publish"
        );
        assert_eq!(
            reason_for_tool("Write", &json!({"file_path": "/a/b/c.txt"})),
            "Write: c.txt"
        );
        assert_eq!(
            reason_for_tool(
                "AskUserQuestion",
                &json!({"questions": [{"question": "Pick one?"}]})
            ),
            "Question: Pick one?"
        );
        assert_eq!(reason_for_tool("TodoWrite", &json!({})), "TodoWrite");
        let long = reason_for_tool("Bash", &json!({"description": "x".repeat(200)}));
        assert_eq!(long.chars().count(), REASON_MAX);
        assert!(long.ends_with('…'));

        assert_eq!(
            asked_line("All done. Let me know if you want the tests too."),
            Some("Let me know if you want the tests too.".into())
        );
        assert_eq!(
            asked_line("Run `cargo test?` then \"why?\" — finished."),
            None,
            "quoted/code ? doesn't count"
        );
        assert_eq!(
            asked_line("Done.\n\n- **Option A**: fast\n- **Option B**: safe\n\nWhich do you want?"),
            Some("Which do you want?".into())
        );
        assert_eq!(
            asked_line("Implemented the feature and all tests pass."),
            None
        );

        assert_eq!(
            first_sentence("## Summary\nFixed it. More text.", 160),
            "Summary"
        );
        assert_eq!(
            first_sentence("Fixed v1.2 parsing. Also more.", 160),
            "Fixed v1.2 parsing."
        );

        let mut f = AgentFacts::new(Harness::Claude);
        assert_eq!(
            waiting_reason(Some(&f), Some("Do you want to proceed?"), true).as_deref(),
            Some("Do you want to proceed?")
        );
        f.asked = Some("Ship it?".into());
        assert_eq!(
            waiting_reason(Some(&f), None, false).as_deref(),
            Some("Ship it?")
        );
        f.pending = Some(Pending {
            tool: "Bash".into(),
            reason: "Bash: deploy".into(),
            at_ms: None,
        });
        assert_eq!(
            waiting_reason(Some(&f), Some("Proceed?"), true).as_deref(),
            Some("Bash: deploy")
        );
        assert_eq!(waiting_reason(None, None, true), None);
    }

    #[test]
    fn tail_reads_incrementally_bounds_the_first_read_and_resets() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        // > 4 MB of old lines, then a marker
        let old_line = format!("{{\"pad\":\"{}\"}}\n", "x".repeat(1000));
        let mut body = old_line.repeat((FIRST_READ_MAX as usize / old_line.len()) + 200);
        body.push_str("{\"marker\":1}\n");
        std::fs::write(&p, &body).unwrap();
        let mut t = Tail::new(&p);
        let first = t.read().unwrap();
        assert!(!first.reset);
        assert!(first.lines.len() < 4200, "bounded first read");
        assert!(
            first
                .lines
                .iter()
                .all(|l| l.starts_with('{') && l.ends_with('}')),
            "the cut-off line is dropped"
        );
        assert_eq!(first.lines.last().unwrap(), "{\"marker\":1}");
        assert_eq!(t.offset(), body.len() as u64);

        // a partial line waits for its newline
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        use std::io::Write;
        f.write_all(b"{\"a\":").unwrap();
        assert!(t.read().unwrap().lines.is_empty());
        f.write_all(b"1}\r\n{\"b\":2}\n").unwrap();
        assert_eq!(t.read().unwrap().lines, ["{\"a\":1}", "{\"b\":2}"]);
        assert!(t.read().unwrap().lines.is_empty());
        drop(f);

        // shrink → reset and re-read from the start
        std::fs::write(&p, "{\"new\":1}\n").unwrap();
        let r = t.read().unwrap();
        assert!(r.reset);
        assert_eq!(r.lines, ["{\"new\":1}"]);
    }

    #[test]
    fn binder_follows_claude_session_files_and_falls_back() {
        let sb = sandbox();
        let cfg = sb.home().join(".claude");
        let cwd = PathBuf::from(if cfg!(windows) { "F:\\proj" } else { "/proj" });
        let id1 = "11111111-1111-4111-8111-111111111111";
        let id2 = "22222222-2222-4222-8222-222222222222";
        let slug = claude_project_slug(&cwd);
        let proj = cfg.join("projects").join(&slug);
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(cfg.join("sessions")).unwrap();
        std::fs::write(
            cfg.join("sessions").join("4242.json"),
            json!({"pid": 4242, "sessionId": id1, "cwd": cwd, "status": "busy", "startedAt": 1})
                .to_string(),
        )
        .unwrap();
        let spec = BindSpec {
            harness: Harness::Claude,
            profile_id: None,
            config_dir: Some(cfg.clone()),
            pid: Some(4242),
            cwd: cwd.clone(),
            started: SystemTime::now(),
            resume_id: None,
        };
        let mut b = Binder::new(spec);
        let t0 = Instant::now();
        assert!(!b.poll(t0), "no transcript yet");
        assert_eq!(b.session_id(), Some(id1));
        std::fs::write(proj.join(format!("{id1}.jsonl")), "{}\n").unwrap();
        assert!(!b.poll(t0 + Duration::from_secs(1)), "rate limited");
        assert!(b.poll(t0 + CLAUDE_EVERY));
        assert!(b.path().unwrap().ends_with(format!("{id1}.jsonl")));
        assert!(!b.poll(t0 + CLAUDE_EVERY * 2), "unchanged");
        // /clear → a new session id for the same pid
        std::fs::write(
            cfg.join("sessions").join("4242.json"),
            json!({"pid": 4242, "sessionId": id2, "cwd": cwd}).to_string(),
        )
        .unwrap();
        std::fs::write(proj.join(format!("{id2}.jsonl")), "{}\n").unwrap();
        assert!(b.poll(t0 + CLAUDE_EVERY * 3));
        assert!(b.path().unwrap().ends_with(format!("{id2}.jsonl")));
        assert_eq!(b.session_id(), Some(id2));

        // A shim pid: falls back to a session file for the cwd started with the pane.
        let started = SystemTime::now();
        let ms = crate::util::system_time_ms(started) as i64;
        std::fs::write(
            cfg.join("sessions").join("777.json"),
            json!({"pid": 777, "sessionId": id1, "cwd": cwd, "startedAt": ms}).to_string(),
        )
        .unwrap();
        let mut b2 = Binder::new(BindSpec {
            pid: Some(1),
            started,
            ..b.spec().clone()
        });
        assert!(b2.poll(Instant::now()));
        assert!(b2.path().unwrap().ends_with(format!("{id1}.jsonl")));

        // Pi panes are never bound.
        let mut b3 = Binder::new(BindSpec {
            harness: Harness::Pi,
            ..b.spec().clone()
        });
        assert!(!b3.poll(Instant::now()));
        assert!(b3.path().is_none());
    }

    #[test]
    fn binder_finds_new_codex_rollouts_for_the_cwd() {
        let sb = sandbox();
        let day = sb
            .home()
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("10")
            .join("02");
        std::fs::create_dir_all(&day).unwrap();
        let cwd = PathBuf::from(if cfg!(windows) { "F:\\tools" } else { "/tools" });
        let id = "019a0000-0000-7000-8000-000000000001";
        let lines = [
            json!({"type": "session_meta", "payload": {"id": id, "cwd": cwd, "originator": "codex-tui"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hello codex"}]}}),
            json!({"type": "pad", "x": "y".repeat(600)}),
        ];
        let mut b = Binder::new(BindSpec {
            harness: Harness::Codex,
            profile_id: None,
            config_dir: None,
            pid: None,
            cwd: cwd.clone(),
            started: SystemTime::now(),
            resume_id: None,
        });
        let t0 = Instant::now();
        assert!(!b.poll(t0));
        std::fs::write(
            day.join(format!("rollout-2026-10-02T09-00-00-{id}.jsonl")),
            lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
        )
        .unwrap();
        assert!(b.poll(t0 + CODEX_SEARCH_EVERY));
        assert_eq!(b.session_id(), Some(id));
    }

    /// Read-only sanity pass over the real local transcripts: parsing never panics and
    /// yields plausible counts. Prints counts only.
    /// `cargo test -p bro-core live_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_real_transcripts_sanity() {
        let home = crate::util::home_dir();
        let mut totals: HashMap<String, [usize; 7]> = HashMap::new();
        let mut all: Vec<(Harness, PathBuf, SystemTime)> = Vec::new();
        transcripts_newest_first(&home, &mut all);
        for h in [Harness::Claude, Harness::Codex] {
            for (_, p, _) in all.iter().filter(|f| f.0 == h).take(25) {
                let mut t = Tail::new(p);
                let started = Instant::now();
                let r = t.read().expect("read");
                let mut f = AgentFacts::new(h);
                f.ingest(&r.lines);
                assert!(started.elapsed() < Duration::from_secs(5));
                let e = totals.entry(h.label().to_string()).or_default();
                let counts = [
                    1,
                    f.sent.len(),
                    f.touched.len(),
                    usize::from(f.pending.is_some()),
                    usize::from(f.asked.is_some()),
                    usize::from(f.away_summary.is_some()),
                    usize::from(f.last_assistant.is_some()),
                ];
                for (total, n) in e.iter_mut().zip(counts) {
                    *total += n;
                }
            }
        }
        for (h, [n, sent, touched, pending, asked, away, last]) in &totals {
            println!(
                "{h}: files={n} sent={sent} touched={touched} pending={pending} asked={asked} away_summary={away} last_assistant={last}"
            );
        }
        // Transcripts that contain the rarer records.
        let (mut files, mut sent, mut images, mut away) = (0, 0, 0, 0);
        for (_, p, _) in all.iter().filter(|f| f.0 == Harness::Claude).take(600) {
            let r = Tail::new(p).read().expect("read");
            if !r
                .lines
                .iter()
                .any(|l| l.contains("\"SendUserFile\"") || l.contains("away_summary"))
            {
                continue;
            }
            let mut f = AgentFacts::new(Harness::Claude);
            f.ingest(&r.lines);
            files += 1;
            sent += f.sent.len();
            images += f.sent.iter().filter(|s| s.is_image).count();
            away += usize::from(f.away_summary.is_some());
        }
        println!(
            "claude with SendUserFile/away_summary: files={files} sent={sent} images={images} away_summary={away}"
        );
    }

    fn transcripts_newest_first(home: &Path, out: &mut Vec<(Harness, PathBuf, SystemTime)>) {
        let roots = [
            (
                Harness::Claude,
                home.join(".claude").join("projects"),
                1usize,
            ),
            (Harness::Codex, home.join(".codex").join("sessions"), 4),
            (Harness::Codex, home.join(".bro").join("codex-profiles"), 6),
        ];
        for (h, root, depth) in roots {
            let mut stack = vec![(root, 0usize)];
            while let Some((d, n)) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() && n < depth {
                        stack.push((p, n + 1));
                    } else if p.extension().is_some_and(|x| x == "jsonl") {
                        let m = e
                            .metadata()
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .unwrap_or(SystemTime::UNIX_EPOCH);
                        out.push((h, p, m));
                    }
                }
            }
        }
        out.sort_by_key(|f| std::cmp::Reverse(f.2));
    }
}
