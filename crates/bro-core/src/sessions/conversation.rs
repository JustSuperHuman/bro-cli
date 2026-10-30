//! The conversation in a live transcript, for the desktop app's focus mode: what you said, what the agent said
//! (its markdown, as written) and what it did in between, each tool call one step in plain words ("Read
//! live.rs", "Ran cargo test", "Edited chrome.rs") that says how it went. Built a line at a time as the
//! transcript grows ([`Conversation::ingest`]), from the lines [`super::live::AgentFacts`] reads, so it costs no
//! extra file I/O. Thinking, tool output, injected context and sub-agents' own turns are left out.
//!
//! Claude Code writes one content block per record (a reply's text, then its tool calls, under one message id);
//! Codex writes `response_item`s and, from 0.16x, `item_completed` events that describe each command, file change
//! and search once it's done. Both are read: Codex's items win once one is seen.

use super::describe::{CLAUDE_NOISE, CODEX_NOISE, command_title, is_noise, strip_images};
use super::live::{TouchKind, parse_apply_patch};
use crate::Harness;
use crate::util::parse_iso_ms;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

mod wording;

/// At most this many items are kept (the oldest go first).
pub const ITEMS_MAX: usize = 400;
/// [`Conversation::history`] reads at most this much of a transcript, from its end.
pub const HISTORY_MAX: u64 = 256 * 1024 * 1024;
/// A step's detail (a command, a path, a query) is cut to this many characters.
const DETAIL_MAX: usize = 600;
/// A step's label keeps this much of what it names.
const LABEL_MAX: usize = 90;

/// What a step did, roughly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepKind {
    Read,
    Edit,
    Create,
    Delete,
    Run,
    Search,
    Web,
    /// a helper agent (Claude's Task, Codex's sub-agents)
    Agent,
    /// its own task list, or a plan for you to approve
    Plan,
    /// a question for you
    Ask,
    /// a file sent to you
    Send,
    /// a skill it used: `label` is the skill's name in words ("Generate voice"), `detail` what it asked of it
    Skill,
    /// an MCP server's tool: `label` says what it did ("Navigate · figma.com"), `detail` names the server
    Mcp,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepState {
    Running,
    Done,
    Failed,
}

/// One tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// the call's id in the transcript
    pub id: String,
    pub kind: StepKind,
    /// what happened, in plain words: "Read live.rs", "Ran cargo test", "Searched for “focus”"
    pub label: String,
    /// what it was done to, in full: the command, the path, the query
    pub detail: Option<String>,
    pub state: StepState,
    /// lines added and removed (edits)
    pub added: u32,
    pub removed: u32,
    /// the line it was about: the first changed one of an edit, where a read started
    pub line: Option<u32>,
    pub at_ms: Option<i64>,
    /// the page its result shows in, when its MCP server made it an app
    pub app: Option<Arc<McpApp>>,
}

/// An MCP App: a tool call whose server gave its result a page of its own (a `ui://` resource, MCP Apps'
/// `_meta.ui.resourceUri` or the Apps SDK's `openai/outputTemplate`), drawn in the conversation with the call's
/// input and result. The page itself is the server's: only the session's harness can read it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpApp {
    /// the server as its tools' names spell it (`claude_ai_JustGains` of `mcp__claude_ai_JustGains__show_workout`)
    pub server: String,
    /// the tool's own name on it (`show_workout`)
    pub tool: String,
    /// the page (`ui://widget/workout.html`)
    pub resource: String,
    /// what the call was given
    pub input: Value,
    /// the result as the server sent it: `content`, `structuredContent`, `_meta`, `isError`
    pub result: Value,
}

/// Something about the conversation itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteKind {
    /// you stopped the turn
    Interrupted,
    /// the history was summarised to make room
    Compacted,
    /// the turn ended on an API error
    Error,
    /// how long a turn took (`text` is "4m 5s")
    Worked,
}

/// One thing in the conversation, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// What you sent: cleaned of what the harness wraps around it; a slash command (or a `!` shell command) as
    /// you typed it, with `command` set; pictures counted (`images`), with their paths when the transcript has
    /// them (`pictures`: a path you pasted, as bro's alt+v does, taken out of `text`).
    You { text: String, images: usize, pictures: Vec<String>, command: bool, at_ms: Option<i64> },
    /// The agent's words: markdown as it wrote them (text blocks with nothing between them joined)
    Said { text: String, at_ms: Option<i64> },
    /// What the agent did between two things it said: its tool calls, in order
    Did { steps: Vec<Step> },
    Note { kind: NoteKind, text: String, at_ms: Option<i64> },
}

/// A heading the agent gave its own reasoning (Codex's summaries: "Planning the fix").
#[derive(Debug, Clone, PartialEq)]
pub struct Thought {
    pub text: String,
    pub at_ms: Option<i64>,
}

/// The conversation so far. Cheap to clone: items are shared.
#[derive(Debug, Clone)]
pub struct Conversation {
    pub harness: Harness,
    pub items: Vec<Arc<Item>>,
    /// The model the agent last ran on, as its requests name it ("claude-opus-5-5", "gpt-6.1-sol")
    pub model: Option<String>,
    /// How hard it thinks: the effort its requests ask for ("max", "high", "medium", "low")
    pub effort: Option<String>,
    /// The folder it works in, and its git branch, as the transcript last said
    pub cwd: Option<String>,
    pub branch: Option<String>,
    /// Its latest reasoning heading this turn (gone when you next say something)
    pub thought: Option<Thought>,
    /// The agent's own version, as its transcript says ("2.1.312", "0.165.0")
    pub version: Option<String>,
    /// Bumped whenever anything changed
    pub revision: u64,
    /// Codex: `item_completed` events describe the steps (tool calls in `response_item`s are left alone)
    codex_items: bool,
    /// MCP calls waiting on their results, by id: what they were given (an app shows it)
    mcp_calls: HashMap<String, Value>,
}

impl Conversation {
    pub fn new(harness: Harness) -> Conversation {
        Conversation { harness, items: vec![], model: None, effort: None, cwd: None, branch: None, thought: None, version: None, revision: 0, codex_items: false, mcp_calls: HashMap::new() }
    }

    /// Set one of the session's facts (model, effort, cwd, branch) when it says something new.
    fn fact(slot: &mut Option<String>, value: Option<&str>, revision: &mut u64) {
        let Some(v) = value.map(str::trim).filter(|v| !v.is_empty() && *v != "<synthetic>" && *v != "HEAD") else { return };
        if slot.as_deref() != Some(v) {
            *slot = Some(v.to_string());
            *revision += 1;
        }
    }

    /// A reasoning summary: its heading (`**Planning the fix**`), else its first line.
    fn reasoned(&mut self, text: &str, at_ms: Option<i64>) {
        let t = text.trim();
        let heading = t.strip_prefix("**").and_then(|r| r.split("**").next()).unwrap_or_else(|| t.lines().next().unwrap_or_default());
        let heading = line(heading.trim_end_matches(['.', ':']), LABEL_MAX);
        if heading.is_empty() {
            return;
        }
        self.thought = Some(Thought { text: heading, at_ms });
        self.revision += 1;
    }

    /// The last thing you said, and where it is.
    pub fn last_you(&self) -> Option<(usize, &str)> {
        self.items.iter().enumerate().rev().find_map(|(i, it)| match it.as_ref() {
            Item::You { text, .. } => Some((i, text.as_str())),
            _ => None,
        })
    }

    /// The step that's still running last, if any.
    pub fn running(&self) -> Option<&Step> {
        self.items.iter().rev().take(4).find_map(|it| match it.as_ref() {
            Item::Did { steps } => steps.iter().rev().find(|s| s.state == StepState::Running),
            _ => None,
        })
    }

    /// Ingest one transcript record (anything that isn't part of the conversation is ignored). Returns true
    /// when the conversation changed.
    pub fn ingest(&mut self, v: &Value) -> bool {
        let before = self.revision;
        match self.harness {
            Harness::Claude => self.claude(v),
            Harness::Codex => self.codex(v),
            Harness::Pi | Harness::Omp => {}
        }
        if self.items.len() > ITEMS_MAX {
            let extra = self.items.len() - ITEMS_MAX;
            self.items.drain(..extra);
        }
        self.revision != before
    }

    /// Ingest JSONL lines (malformed ones are skipped). Returns true when the conversation changed.
    pub fn ingest_lines<S: AsRef<str>>(&mut self, lines: &[S]) -> bool {
        let mut changed = false;
        for line in lines {
            if let Ok(v) = serde_json::from_str::<Value>(line.as_ref()) {
                changed |= self.ingest(&v);
            }
        }
        changed
    }

    /// The conversation in the transcript at `path` up to byte `end` (where a [`Tail`](super::live::Tail) goes on
    /// from), read from the file's start, or [`HISTORY_MAX`] before `end`. A tail's first read of a long transcript
    /// holds only its last [`FIRST_READ_MAX`](super::live::FIRST_READ_MAX): most of a long session is tool output
    /// and pictures, so that can be its last few items. A line still being written at `end` is left to the tail.
    /// Blocking: call it from a worker thread (about a millisecond per megabyte).
    pub fn history(harness: Harness, path: &Path, end: u64) -> std::io::Result<Conversation> {
        Self::read_back(harness, path, end, HISTORY_MAX)
    }

    /// [`Conversation::history`] from at most `max` bytes before `end`.
    fn read_back(harness: Harness, path: &Path, end: u64, max: u64) -> std::io::Result<Conversation> {
        use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path)?;
        let end = end.min(file.metadata()?.len());
        let start = end.saturating_sub(max);
        // begun inside a line: that line is cut off
        let mut cut = false;
        if start > 0 {
            let mut before = [0u8];
            file.seek(SeekFrom::Start(start - 1))?;
            file.read_exact(&mut before)?;
            cut = before[0] != b'\n';
        }
        let mut reader = BufReader::with_capacity(1 << 20, file.take(end - start));
        let mut c = Conversation::new(harness);
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 || line.last() != Some(&b'\n') {
                break;
            }
            if std::mem::take(&mut cut) {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&line)) {
                c.ingest(&v);
            }
        }
        Ok(c)
    }

    // ---- building ------------------------------------------------------------------------

    fn push(&mut self, item: Item) {
        self.items.push(Arc::new(item));
        self.revision += 1;
    }

    fn you(&mut self, text: String, images: usize, command: bool, at_ms: Option<i64>) {
        self.you_in(text, images, command, at_ms, true);
    }

    /// `settle`: what ran before it is over (a message read mid-turn leaves the steps in flight running).
    fn you_in(&mut self, text: String, images: usize, command: bool, at_ms: Option<i64>, settle: bool) {
        let (text, pictures) = if command { (text, vec![]) } else { take_pictures(&text) };
        let text = text.trim().to_string();
        let images = images.max(pictures.len());
        if text.is_empty() && images == 0 {
            return;
        }
        // Codex reports a prompt twice in a row (a response item and an event): once is enough. The same words
        // sent again after anything else (an interruption) are a new prompt.
        let repeat = self.harness == Harness::Codex && self.items.last().is_some_and(|it| matches!(it.as_ref(), Item::You { text: t, .. } if *t == text));
        if repeat {
            return;
        }
        if settle {
            self.settle(StepState::Done);
        }
        self.thought = None;
        self.push(Item::You { text, images, pictures, command, at_ms });
    }

    fn said(&mut self, text: &str, at_ms: Option<i64>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.items.last_mut()
            && let Item::Said { text: prev, .. } = Arc::make_mut(last)
        {
            prev.push_str("\n\n");
            prev.push_str(text);
            self.revision += 1;
            return;
        }
        self.push(Item::Said { text: text.to_string(), at_ms });
    }

    fn step(&mut self, step: Step) {
        // a call reported again (Codex replays, Claude re-sends a block): keep the first
        if self.find_step(&step.id).is_some() {
            return;
        }
        if let Some(last) = self.items.last_mut()
            && let Item::Did { steps } = Arc::make_mut(last)
        {
            steps.push(step);
            self.revision += 1;
            return;
        }
        self.push(Item::Did { steps: vec![step] });
    }

    fn note(&mut self, kind: NoteKind, text: String, at_ms: Option<i64>) {
        // one compaction can be reported twice (a boundary and a summary, an event and a record)
        if kind == NoteKind::Compacted && self.items.last().is_some_and(|it| matches!(it.as_ref(), Item::Note { kind: NoteKind::Compacted, .. })) {
            return;
        }
        self.push(Item::Note { kind, text, at_ms });
    }

    /// Where the step with `id` is: (item, step).
    fn find_step(&self, id: &str) -> Option<(usize, usize)> {
        if id.is_empty() {
            return None;
        }
        self.items.iter().enumerate().rev().take(60).find_map(|(i, it)| match it.as_ref() {
            Item::Did { steps } => steps.iter().position(|s| s.id == id).map(|j| (i, j)),
            _ => None,
        })
    }

    /// Change the step with `id` (a result came back).
    fn update_step(&mut self, id: &str, f: impl FnOnce(&mut Step)) {
        let Some((i, j)) = self.find_step(id) else { return };
        if let Item::Did { steps } = Arc::make_mut(&mut self.items[i]) {
            let before = steps[j].clone();
            f(&mut steps[j]);
            if steps[j] != before {
                self.revision += 1;
            }
        }
    }

    /// Steps still running when the turn ended or moved on won't hear back: they end as `state`.
    fn settle(&mut self, state: StepState) {
        let open: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .rev()
            .take(8)
            .filter(|(_, it)| matches!(it.as_ref(), Item::Did { steps } if steps.iter().any(|s| s.state == StepState::Running)))
            .map(|(i, _)| i)
            .collect();
        for i in open {
            if let Item::Did { steps } = Arc::make_mut(&mut self.items[i]) {
                for s in steps.iter_mut().filter(|s| s.state == StepState::Running) {
                    s.state = state;
                }
                self.revision += 1;
            }
        }
    }

    // ---- Claude --------------------------------------------------------------------------

    fn claude(&mut self, v: &Value) {
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let at = stamp(v);
        let s = |v: &Value, path: &str| v.pointer(path).and_then(Value::as_str).map(str::to_string);
        Self::fact(&mut self.cwd, s(v, "/cwd").as_deref(), &mut self.revision);
        Self::fact(&mut self.branch, s(v, "/gitBranch").as_deref(), &mut self.revision);
        Self::fact(&mut self.version, s(v, "/version").as_deref(), &mut self.revision);
        match v.get("type").and_then(Value::as_str) {
            // a message you sent while it worked, read mid-turn (alongside a tool's result): it goes where it was
            // read, not as a turn of its own
            Some("attachment") => self.claude_queued(v.get("attachment"), at),
            // what each request asked for: the model and how hard it thinks
            Some("api-request") => {
                Self::fact(&mut self.model, s(v, "/params/model").as_deref(), &mut self.revision);
                Self::fact(&mut self.effort, s(v, "/params/output_config/effort").as_deref(), &mut self.revision);
            }
            Some("assistant") => {
                let msg = v.get("message");
                Self::fact(&mut self.model, s(v, "/message/model").as_deref(), &mut self.revision);
                if v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
                    let text = blocks_text(msg.and_then(|m| m.get("content")));
                    self.settle(StepState::Failed);
                    self.note(NoteKind::Error, text, at);
                    return;
                }
                let Some(blocks) = msg.and_then(|m| m.get("content")).and_then(Value::as_array) else { return };
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => self.said(b.get("text").and_then(Value::as_str).unwrap_or_default(), at),
                        Some("tool_use") => {
                            let id = b.get("id").and_then(Value::as_str).unwrap_or_default();
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = b.get("input").cloned().unwrap_or(Value::Null);
                            let (kind, label, detail) = claude_step(name, &input);
                            let line = (kind == StepKind::Read).then(|| input.get("offset").and_then(Value::as_u64)).flatten().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0);
                            if name.starts_with("mcp__") && !id.is_empty() {
                                // its server may give the result a page, which shows what it was given
                                if self.mcp_calls.len() >= 64 {
                                    self.mcp_calls.clear();
                                }
                                self.mcp_calls.insert(id.to_string(), serde_json::json!({ "name": name, "input": input }));
                            }
                            self.step(Step { id: id.to_string(), kind, label, detail, state: StepState::Running, added: 0, removed: 0, line, at_ms: at, app: None });
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    return;
                }
                // the summary a compaction leaves behind: the boundary already says so
                if v.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
                    self.note(NoteKind::Compacted, String::new(), at);
                    return;
                }
                let content = v.get("message").and_then(|m| m.get("content"));
                let results: Vec<&Value> = content.and_then(Value::as_array).map(|a| a.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")).collect()).unwrap_or_default();
                if !results.is_empty() {
                    let structured = v.get("toolUseResult");
                    for r in results {
                        let id = r.get("tool_use_id").and_then(Value::as_str).unwrap_or_default().to_string();
                        let failed = r.get("is_error").and_then(Value::as_bool) == Some(true) || structured.and_then(Value::as_str).is_some_and(|s| s.trim_start().starts_with("Error"));
                        let (added, removed) = structured.map(patch_counts).unwrap_or((0, 0));
                        let created = structured.and_then(|s| s.get("type")).and_then(Value::as_str) == Some("create");
                        let changed = structured.and_then(patch_line);
                        let app = self.mcp_calls.remove(&id).and_then(|call| mcp_app(&call, r, v.get("mcpMeta"), failed)).map(Arc::new);
                        self.update_step(&id, |s| {
                            s.state = if failed { StepState::Failed } else { StepState::Done };
                            s.added = added;
                            s.removed = removed;
                            if app.is_some() {
                                s.app = app;
                            }
                            if changed.is_some() {
                                s.line = changed;
                            }
                            if created && s.kind == StepKind::Edit && !failed {
                                s.kind = StepKind::Create;
                                if let Some(rest) = s.label.strip_prefix("Wrote ") {
                                    s.label = format!("Created {rest}");
                                }
                            }
                        });
                    }
                    return;
                }
                self.claude_prompt(content, at);
            }
            Some("system") => match v.get("subtype").and_then(Value::as_str) {
                Some("compact_boundary") => self.note(NoteKind::Compacted, String::new(), at),
                Some("turn_duration") => {
                    self.settle(StepState::Done);
                    if let Some(ms) = v.get("durationMs").and_then(Value::as_u64) {
                        self.note(NoteKind::Worked, duration(ms), at);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// A user record that isn't tool results: your prompt, a slash command, an interruption, or harness text.
    fn claude_prompt(&mut self, content: Option<&Value>, at: Option<i64>) {
        let mut texts: Vec<String> = vec![];
        let mut images = 0;
        match content {
            Some(Value::String(s)) => texts.push(s.clone()),
            Some(Value::Array(blocks)) => {
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => texts.push(b.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
                        Some("image") => images += 1,
                        _ => {}
                    }
                }
            }
            _ => return,
        }
        let mut kept: Vec<String> = vec![];
        for raw in texts {
            let t = raw.trim_start();
            if t.starts_with("[Request interrupted") {
                self.settle(StepState::Failed);
                self.note(NoteKind::Interrupted, String::new(), at);
                return;
            }
            if t.starts_with("<command-name>") || t.starts_with("<command-message>") {
                let cmd = command_title(t);
                self.command_facts(&cmd);
                if !cmd.is_empty() {
                    self.you(cmd, images, true, at);
                }
                return;
            }
            // what a command said back: the model or effort it set
            if let Some(out) = t.strip_prefix("<local-command-stdout>") {
                self.command_output(out.split("</local-command-stdout>").next().unwrap_or_default());
                continue;
            }
            if let Some(rest) = t.strip_prefix("<bash-input>") {
                let cmd = rest.split("</bash-input>").next().unwrap_or_default().trim();
                if !cmd.is_empty() {
                    self.you(format!("! {cmd}"), images, true, at);
                }
                return;
            }
            if is_noise(t, &CLAUDE_NOISE) || is_noise(t, CLAUDE_MORE_NOISE) {
                continue;
            }
            kept.push(strip_image_marks(&raw));
        }
        let text = kept.join("\n\n");
        if text.trim().is_empty() && images == 0 {
            return;
        }
        self.you(text, images, false, at);
    }

    /// A message Claude Code queued while it worked and read mid-turn (`attachment` of type `queued_command`).
    fn claude_queued(&mut self, a: Option<&Value>, at: Option<i64>) {
        let Some(a) = a.filter(|a| a.get("type").and_then(Value::as_str) == Some("queued_command")) else { return };
        // only what you typed (its own background work queues notifications the same way)
        if a.pointer("/origin/kind").and_then(Value::as_str).is_some_and(|k| k != "human") {
            return;
        }
        let at = a.get("timestamp").and_then(Value::as_str).and_then(parse_iso_ms).or(at);
        let (text, images) = match a.get("prompt") {
            Some(Value::String(s)) => (s.clone(), 0),
            Some(Value::Array(blocks)) => (
                blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
                blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("image")).count(),
            ),
            _ => return,
        };
        let images = images.max(text.matches("[Image #").count());
        let text = strip_image_marks(&text);
        match a.get("commandMode").and_then(Value::as_str) {
            Some("bash") => self.you_in(format!("! {}", text.trim()), 0, true, at, false),
            None | Some("prompt") => {
                let command = text.starts_with('/');
                self.you_in(text, images, command, at, false);
            }
            _ => {}
        }
    }

    /// `/model x` and `/effort x` set what its next requests ask for.
    fn command_facts(&mut self, cmd: &str) {
        let mut words = cmd.split_whitespace();
        let (Some(name), Some(arg)) = (words.next(), words.next()) else { return };
        match name {
            "/model" => Self::fact(&mut self.model, Some(arg), &mut self.revision),
            "/effort" if arg.chars().all(|c| c.is_ascii_alphabetic()) => Self::fact(&mut self.effort, Some(&arg.to_ascii_lowercase()), &mut self.revision),
            _ => {}
        }
    }

    /// What a command said back: "Set model to Opus 5.5", "Set effort level to max (this session only): …".
    fn command_output(&mut self, out: &str) {
        let out = strip_ansi(out);
        let out = out.trim();
        if let Some(rest) = out.strip_prefix("Set model to ").or_else(|| out.strip_prefix("Kept model as ")) {
            let name = rest.split(" and saved").next().unwrap_or(rest).lines().next().unwrap_or_default().trim();
            // "Default (Opus 5.5)", "Opus 5.5 (claude-opus-5-5)": the model in brackets, when that names it
            let name = match (name.rfind(" ("), name.ends_with(')')) {
                (Some(i), true) => {
                    let inside = &name[i + 2..name.len() - 1];
                    if name[..i].eq_ignore_ascii_case("default") || !inside.contains(' ') { inside } else { &name[..i] }
                }
                _ => name,
            };
            Self::fact(&mut self.model, Some(name.trim()), &mut self.revision);
        } else if let Some(rest) = out.strip_prefix("Set effort level to ").or_else(|| out.strip_prefix("Effort level set to ")) {
            let level: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            Self::fact(&mut self.effort, Some(&level.to_ascii_lowercase()), &mut self.revision);
        }
    }

    // ---- Codex ---------------------------------------------------------------------------

    fn codex(&mut self, v: &Value) {
        let at = stamp(v);
        let Some(p) = v.get("payload") else {
            return;
        };
        let kind = p.get("type").and_then(Value::as_str);
        let s = |path: &str| p.pointer(path).and_then(Value::as_str).map(str::to_string);
        match v.get("type").and_then(Value::as_str) {
            Some("compacted") => self.note(NoteKind::Compacted, String::new(), at),
            // the session and each turn say where it works, on what model and how hard it thinks
            Some("session_meta") => {
                Self::fact(&mut self.cwd, s("/cwd").as_deref(), &mut self.revision);
                Self::fact(&mut self.branch, s("/git/branch").as_deref(), &mut self.revision);
                Self::fact(&mut self.model, s("/model").as_deref(), &mut self.revision);
                Self::fact(&mut self.version, s("/cli_version").as_deref(), &mut self.revision);
            }
            Some("turn_context") => {
                Self::fact(&mut self.cwd, s("/cwd").as_deref(), &mut self.revision);
                Self::fact(&mut self.model, s("/model").as_deref(), &mut self.revision);
                Self::fact(&mut self.effort, s("/effort").or_else(|| s("/reasoning_effort")).as_deref(), &mut self.revision);
            }
            Some("response_item") if kind == Some("reasoning") => {
                let summary = p.get("summary").and_then(Value::as_array).and_then(|a| a.iter().rev().find_map(|b| b.get("text").and_then(Value::as_str)));
                if let Some(text) = summary {
                    self.reasoned(text, at);
                }
            }
            Some("response_item") => match kind {
                Some("message") => {
                    let role = p.get("role").and_then(Value::as_str);
                    let content = p.get("content").and_then(Value::as_array);
                    let text = content.map(|a| a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                    match role {
                        Some("assistant") => self.said(&text, at),
                        Some("user") => {
                            let images = content.map(|a| a.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("input_image")).count()).unwrap_or(0);
                            self.codex_prompt(&text, images, at);
                        }
                        _ => {}
                    }
                }
                Some("function_call" | "custom_tool_call" | "local_shell_call" | "web_search_call") if !self.codex_items => {
                    if let Some(step) = codex_call(p, at) {
                        self.step(step);
                    }
                }
                Some("function_call_output" | "custom_tool_call_output") if !self.codex_items => {
                    let id = p.get("call_id").and_then(Value::as_str).unwrap_or_default().to_string();
                    let failed = codex_output_failed(p.get("output"));
                    self.update_step(&id, |s| s.state = if failed { StepState::Failed } else { StepState::Done });
                }
                _ => {}
            },
            Some("event_msg") => match kind {
                Some("thread_settings_applied") => {
                    Self::fact(&mut self.model, s("/thread_settings/model").as_deref(), &mut self.revision);
                    Self::fact(&mut self.effort, s("/thread_settings/reasoning_effort").as_deref(), &mut self.revision);
                }
                Some("agent_reasoning") => {
                    if let Some(text) = s("/text") {
                        self.reasoned(&text, at);
                    }
                }
                Some("user_message") => {
                    let mut text = p.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
                    let images = p.get("images").and_then(Value::as_array).map_or(0, Vec::len);
                    // pictures from disk come as their paths: with the text, they're its pictures
                    for path in p.get("local_images").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                        text.push_str(&format!(" \"{path}\""));
                    }
                    self.codex_prompt(&text, images, at);
                }
                Some("item_completed") => {
                    self.codex_items = true;
                    if let Some(item) = p.get("item") {
                        self.codex_item(item, at);
                    }
                }
                Some("turn_aborted") => {
                    self.settle(StepState::Failed);
                    self.note(NoteKind::Interrupted, String::new(), at);
                }
                Some("task_complete") => self.settle(StepState::Done),
                Some("error") => {
                    let text = p.get("message").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                    self.settle(StepState::Failed);
                    self.note(NoteKind::Error, text, at);
                }
                Some("context_compacted") => self.note(NoteKind::Compacted, String::new(), at),
                _ => {}
            },
            _ => {}
        }
    }

    fn codex_prompt(&mut self, raw: &str, images: usize, at: Option<i64>) {
        let t = raw.trim_start();
        if t.starts_with("<turn_aborted>") {
            return;
        }
        if is_noise(t, &CODEX_NOISE) {
            let cmd = command_title(t);
            if !cmd.is_empty() {
                self.you(cmd, images, true, at);
            }
            return;
        }
        let text = strip_images(raw);
        self.you(strip_image_marks(&text), images, false, at);
    }

    /// One of Codex's finished items: a command, a file change, a search, a tool, your message.
    fn codex_item(&mut self, item: &Value, at: Option<i64>) {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let status = item.get("status").and_then(Value::as_str);
        let failed_status = matches!(status, Some("failed" | "declined"));
        let s = |k: &str| item.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let step = |kind: StepKind, label: String, detail: Option<String>, failed: bool, added: u32, removed: u32| Step {
            id: id.clone(),
            kind,
            label,
            detail: detail.map(|d| cut(&d, DETAIL_MAX)),
            state: if failed { StepState::Failed } else { StepState::Done },
            added,
            removed,
            line: None,
            at_ms: at,
            app: None,
        };
        match item.get("type").and_then(Value::as_str) {
            Some("UserMessage") => {
                let content = item.get("content").and_then(Value::as_array);
                let text = content.map(|a| a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                let images = content.map(|a| a.iter().filter(|b| b.get("type").and_then(Value::as_str).is_some_and(|t| t.contains("image"))).count()).unwrap_or(0);
                self.codex_prompt(&text, images, at);
            }
            Some("CommandExecution") => {
                let parsed: Vec<&Value> = item.get("parsed_cmd").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
                let cmd = parsed
                    .first()
                    .and_then(|c| c.get("cmd").and_then(Value::as_str))
                    .map(str::to_string)
                    .or_else(|| item.get("command").map(command_text))
                    .unwrap_or_default();
                let failed = failed_status || item.get("exit_code").and_then(Value::as_i64).is_some_and(|c| c != 0);
                let (kind, label) = parsed_label(&parsed, &cmd);
                let st = step(kind, label, Some(cmd), failed, 0, 0);
                self.step(st);
            }
            Some("FileChange") => {
                let Some(changes) = item.get("changes").and_then(Value::as_object) else { return };
                let (mut added, mut removed) = (0, 0);
                let mut kinds = vec![];
                for (path, change) in changes {
                    let kind = match change.get("type").and_then(Value::as_str) {
                        Some("add") => StepKind::Create,
                        Some("delete") => StepKind::Delete,
                        _ => StepKind::Edit,
                    };
                    if let Some(diff) = change.get("unified_diff").and_then(Value::as_str) {
                        let (a, r) = diff_counts(diff);
                        added += a;
                        removed += r;
                    } else if kind == StepKind::Create {
                        added += change.get("content").and_then(Value::as_str).map_or(0, |c| c.lines().count() as u32);
                    }
                    kinds.push((path.clone(), kind));
                }
                let (kind, label) = files_label(&kinds);
                let detail = kinds.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join("\n");
                let mut st = step(kind, label, Some(detail), failed_status, added, removed);
                st.line = changes.values().next().and_then(|c| c.get("unified_diff")).and_then(Value::as_str).and_then(diff_line);
                self.step(st);
            }
            Some("McpToolCall") => {
                let args = match item.get("arguments") {
                    Some(Value::String(a)) => serde_json::from_str(a).unwrap_or(Value::Null),
                    Some(a) => a.clone(),
                    None => Value::Null,
                };
                let st = step(StepKind::Mcp, wording::tool_label(s("tool").unwrap_or_default(), &args), s("server").map(wording::server_name), failed_status, 0, 0);
                self.step(st);
            }
            Some("ImageView") => {
                let path = s("path").unwrap_or_default().to_string();
                let st = step(StepKind::Read, format!("Looked at {}", basename(&path)), Some(path), failed_status, 0, 0);
                self.step(st);
            }
            Some("Extension") if s("kind").is_some_and(|k| k.contains("search")) => {
                let st = step(StepKind::Web, web(s("query").unwrap_or_default()), s("query").map(str::to_string), failed_status, 0, 0);
                self.step(st);
            }
            Some("WebSearch") => {
                let st = step(StepKind::Web, web(s("query").unwrap_or_default()), None, failed_status, 0, 0);
                self.step(st);
            }
            Some("SubAgentActivity") if s("kind") == Some("started") => {
                let who = s("agent_path").map(|p| wording::name(p.rsplit('/').next().unwrap_or(p))).unwrap_or_else(|| "a helper".to_string());
                let st = step(StepKind::Agent, format!("Started a helper: {who}"), s("model").map(str::to_string), failed_status, 0, 0);
                self.step(st);
            }
            Some("ContextCompaction") => self.note(NoteKind::Compacted, String::new(), at),
            Some("Reasoning") => {
                let summary = item.get("summary_text").and_then(Value::as_array).and_then(|a| a.iter().rev().find_map(Value::as_str));
                if let Some(text) = summary {
                    self.reasoned(text, at);
                }
            }
            _ => {}
        }
    }
}

/// Text Claude Code puts in user records that the user didn't type (beyond what titles already skip).
const CLAUDE_MORE_NOISE: &[&str] = &["<task-notification>", "<local-command-stderr>", "<bash-stderr>", "<user-memory-input>", "<hook-"];

fn stamp(v: &Value) -> Option<i64> {
    v.get("timestamp").and_then(Value::as_str).and_then(parse_iso_ms)
}

/// "4m 5s", "38s", "1h 12m".
fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

/// Text cut to `max` characters with an ellipsis (line breaks kept).
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

/// One line of `text`, whitespace collapsed, cut to `max`.
fn line(text: &str, max: usize) -> String {
    cut(&text.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

fn basename(p: &str) -> String {
    let p = p.strip_prefix("file:///").unwrap_or(p);
    p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(p).to_string()
}

/// "Ran cargo test": the command's first line, or "Ran a script" when that line says nothing (a PowerShell
/// here-string's `@'`, a heredoc's opener).
fn ran(cmd: &str) -> String {
    let first = cmd.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let opener = matches!(first, "@'" | "@\"") || first.len() < 2 || first.ends_with("<<'EOF'") || first.ends_with("<<EOF");
    if opener { "Ran a script".to_string() } else { format!("Ran {}", line(first, LABEL_MAX)) }
}

/// "Searched the web for “…”", or "Read docs.rs" when the query is an address it opened.
fn web(query: &str) -> String {
    let q = query.trim().trim_matches(['\'', '"']);
    if q.is_empty() {
        return "Searched the web".to_string();
    }
    if q.starts_with("http://") || q.starts_with("https://") {
        let host = url::Url::parse(q).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| line(q, LABEL_MAX));
        return format!("Read {host}");
    }
    format!("Searched the web for “{}”", line(q, LABEL_MAX))
}

/// The paths to pictures in what you wrote (quoted when they hold spaces, as bro pastes them), and the text
/// without them.
fn take_pictures(text: &str) -> (String, Vec<String>) {
    const EXT: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".heic", ".tif", ".tiff"];
    let picture = |t: &str| {
        let lower = t.to_ascii_lowercase();
        t.contains(['/', '\\']) && EXT.iter().any(|e| lower.ends_with(e))
    };
    let mut pictures: Vec<String> = vec![];
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        // a quoted path, spaces and all
        if let Some(after) = rest.strip_prefix('"')
            && let Some(end) = after.find('"')
            && picture(&after[..end])
        {
            pictures.push(after[..end].to_string());
            rest = &after[end + 1..];
            continue;
        }
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (word, tail) = rest.split_at(word_end);
        let bare = word.trim_matches(|c: char| matches!(c, ',' | ';' | '(' | ')' | '<' | '>' | '`' | '\'')).trim_start_matches('@');
        if !bare.is_empty() && picture(bare) {
            if !pictures.iter().any(|p| p == bare) {
                pictures.push(bare.to_string());
            }
        } else {
            kept.push_str(word);
        }
        let spaces = tail.len() - tail.trim_start().len();
        kept.push_str(&tail[..spaces]);
        rest = &tail[spaces..];
    }
    (kept, pictures)
}

/// Terminal colour and style codes (`\x1b[1m`) taken out.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// `[Image #1]` markers (the picture is counted separately) taken out.
fn strip_image_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("[Image #") {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// The text blocks of a message's content, joined.
fn blocks_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Array(a)) => a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n").trim().to_string(),
        _ => String::new(),
    }
}

/// Lines added and removed in a Claude edit's `structuredPatch`.
fn patch_counts(result: &Value) -> (u32, u32) {
    let Some(hunks) = result.get("structuredPatch").and_then(Value::as_array) else { return (0, 0) };
    let mut added = 0;
    let mut removed = 0;
    for h in hunks {
        for l in h.get("lines").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            if l.starts_with('+') {
                added += 1;
            } else if l.starts_with('-') {
                removed += 1;
            }
        }
    }
    (added, removed)
}

/// The first line a Claude edit changed (`structuredPatch`'s first hunk starts up to three lines of context
/// before it).
fn patch_line(result: &Value) -> Option<u32> {
    let hunk = result.get("structuredPatch").and_then(Value::as_array)?.first()?;
    let start = hunk.get("newStart").and_then(Value::as_u64)?;
    let context = hunk.get("lines").and_then(Value::as_array).map_or(0, |l| l.iter().filter_map(Value::as_str).take_while(|l| !l.starts_with('+') && !l.starts_with('-')).count() as u64);
    u32::try_from(start + context).ok().filter(|n| *n > 0)
}

/// The first line a unified diff changes: its first hunk's `+N` (the context before the change counted in).
fn diff_line(diff: &str) -> Option<u32> {
    let mut lines = diff.lines().skip_while(|l| !l.starts_with("@@"));
    let header = lines.next()?;
    let start: u32 = header.split_whitespace().find(|w| w.starts_with('+'))?[1..].split(',').next()?.parse().ok()?;
    let context = lines.take_while(|l| !l.starts_with('+') && !l.starts_with('-')).count() as u32;
    Some(start + context).filter(|n| *n > 0)
}

/// Lines added and removed in a unified diff (headers left out).
fn diff_counts(diff: &str) -> (u32, u32) {
    let mut added = 0;
    let mut removed = 0;
    for l in diff.lines() {
        if l.starts_with("+++") || l.starts_with("---") {
            continue;
        }
        if l.starts_with('+') {
            added += 1;
        } else if l.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

/// The step a Claude Code tool call is, in plain words.
fn claude_step(name: &str, input: &Value) -> (StepKind, String, Option<String>) {
    use StepKind::*;
    let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let file = s("file_path").or_else(|| s("notebook_path")).or_else(|| s("path"));
    let file_name = || file.map(basename).unwrap_or_else(|| "a file".to_string());
    let detail = |d: Option<&str>| d.map(|d| cut(d, DETAIL_MAX));
    match name {
        "Read" => (Read, format!("Read {}", file_name()), detail(file)),
        "Edit" | "MultiEdit" | "NotebookEdit" => (Edit, format!("Edited {}", file_name()), detail(file)),
        "Write" => (Edit, format!("Wrote {}", file_name()), detail(file)),
        "Bash" | "PowerShell" => {
            let cmd = s("command");
            // its own words for it, else the command's when it's one of the usual ones
            let (kind, label) = match s("description") {
                Some(d) => (Run, line(d, LABEL_MAX)),
                None => cmd.and_then(wording::command_label).unwrap_or_else(|| (Run, ran(cmd.unwrap_or("a command")))),
            };
            (kind, label, detail(cmd))
        }
        "Monitor" => (Run, s("description").map_or("Watching a command".to_string(), |d| line(&format!("Watching {d}"), LABEL_MAX)), detail(s("command"))),
        "BashOutput" => (Run, "Checked on a command".into(), None),
        "KillShell" | "KillBash" => (Run, "Stopped a command".into(), None),
        "TaskStop" => (Run, "Stopped a background task".into(), None),
        "Grep" => (Search, format!("Searched for “{}”", line(s("pattern").unwrap_or_default(), LABEL_MAX)), detail(s("path"))),
        "Glob" => (Search, format!("Looked for {}", line(s("pattern").unwrap_or("files"), LABEL_MAX)), detail(s("path"))),
        "LS" => (Read, format!("Listed {}", file_name()), detail(file)),
        "WebSearch" => (Web, format!("Searched the web for “{}”", line(s("query").unwrap_or_default(), LABEL_MAX)), None),
        "WebFetch" => {
            let url = s("url").unwrap_or_default();
            let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| line(url, LABEL_MAX));
            (Web, format!("Read {host}"), detail(Some(url)))
        }
        "Task" | "Agent" => {
            let what = s("description").map(|d| line(d, LABEL_MAX)).unwrap_or_else(|| "A helper".to_string());
            // a helper of a kind of its own says which
            let label = match s("subagent_type").filter(|t| !matches!(*t, "general-purpose" | "claude" | "fork")) {
                Some(t) => line(&format!("{what} · {}", wording::name(t)), LABEL_MAX),
                None => what,
            };
            (Agent, label, detail(s("prompt")))
        }
        "SendMessage" => {
            let to = s("to").map_or("a session".to_string(), |t| if t == "*" { "everyone".to_string() } else { line(t, 40) });
            (Agent, format!("Messaged {to}"), detail(s("message")))
        }
        "ListAgents" => (Agent, "Checked on the sessions".into(), None),
        "Workflow" => (Agent, s("name").map_or("Ran a workflow".to_string(), |n| format!("Ran the {} workflow", line(n, 40))), None),
        "TodoWrite" => {
            let (label, now) = wording::plan_progress(input.get("todos"), &["activeForm", "content"]);
            (Plan, label, detail(now.as_deref()))
        }
        "EnterPlanMode" => (Plan, "Started planning".into(), None),
        "ExitPlanMode" => (Plan, "Proposed a plan".into(), detail(s("plan"))),
        "ScheduleWakeup" => {
            let when = input.get("delaySeconds").and_then(Value::as_f64).map_or("later".to_string(), |secs| format!("in {}", wording::wait(secs)));
            (Plan, format!("Checks back {when}"), detail(s("reason")))
        }
        "CronCreate" => (Plan, s("prompt").map_or("Scheduled a prompt".to_string(), |p| format!("Scheduled “{}”", line(p, 50))), detail(s("cron"))),
        "CronDelete" => (Plan, "Removed a schedule".into(), None),
        "CronList" => (Plan, "Listed the schedules".into(), None),
        "ReportFindings" => {
            let label = match input.get("findings").and_then(Value::as_array).map_or(0, Vec::len) {
                0 => "Found nothing to report".to_string(),
                1 => "Reported a finding".to_string(),
                n => format!("Reported {n} findings"),
            };
            (Plan, label, None)
        }
        "AskUserQuestion" => {
            let q = input.get("questions").and_then(Value::as_array).and_then(|q| q.first()).and_then(|q| q.get("question")).and_then(Value::as_str).or_else(|| s("question"));
            (Ask, format!("Asked you: {}", line(q.unwrap_or("a question"), LABEL_MAX)), None)
        }
        "SendUserFile" => (Send, format!("Sent you {}", file_name()), detail(file)),
        "PushNotification" => (Other, "Notified you".into(), detail(s("message").or_else(|| s("title")))),
        "Artifact" => match s("action") {
            // a page it published opens from its file, as a file it sent you does
            None | Some("publish") => (Send, s("file_path").map_or("Published a page".to_string(), |f| format!("Published {}", basename(f))), detail(s("file_path"))),
            Some(action) => {
                let label = match action {
                    "read" => "Read a page",
                    "list" => "Listed its pages",
                    "open" => "Opened a page",
                    "delete" => "Deleted a page",
                    "quickstart" => "Looked up page types",
                    "pin" => "Pinned a page",
                    "unpin" => "Unpinned a page",
                    _ => "Used a page",
                };
                (Other, label.to_string(), None)
            }
        },
        "Skill" => (Skill, wording::skill_name(s("skill").or_else(|| s("command")).unwrap_or_default()), detail(s("args"))),
        "SlashCommand" => (Run, format!("Ran {}", line(s("command").unwrap_or("a command"), LABEL_MAX)), None),
        "ToolSearch" => (Other, wording::tool_search(s("query").unwrap_or_default()), None),
        "EnterWorktree" => (Other, "Moved into a worktree".into(), detail(s("name"))),
        "ExitWorktree" => (Other, "Left the worktree".into(), None),
        "StructuredOutput" => (Other, "Gave its answer".into(), None),
        n if n.starts_with("mcp__") => {
            let mut parts = n.splitn(3, "__").skip(1);
            let server = parts.next().unwrap_or_default();
            let tool = parts.next().unwrap_or(server);
            (Mcp, wording::tool_label(tool, input), (!server.is_empty()).then(|| wording::server_name(server)))
        }
        n => (Other, wording::name(n), None),
    }
}

/// The page an MCP tool's result names for itself, if any: MCP Apps' `ui.resourceUri` (or its draft's
/// `ui/resourceUri`), else the Apps SDK's `openai/outputTemplate`.
pub fn app_resource(meta: &Value) -> Option<&str> {
    [meta.pointer("/ui/resourceUri"), meta.get("ui/resourceUri"), meta.get("openai/outputTemplate")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|u| u.starts_with("ui://"))
}

/// An MCP call's result as an app, when its server gave it a page: `call` is what the call was (`name`, `input`),
/// `block` its `tool_result`, `mcp` the record's `mcpMeta` (where Claude Code keeps the result's `_meta` and
/// `structuredContent`).
fn mcp_app(call: &Value, block: &Value, mcp: Option<&Value>, failed: bool) -> Option<McpApp> {
    let meta = mcp?.get("_meta")?;
    let resource = app_resource(meta)?.to_string();
    let name = call.get("name").and_then(Value::as_str)?;
    let mut parts = name.strip_prefix("mcp__")?.splitn(2, "__");
    let (server, tool) = (parts.next()?.to_string(), parts.next()?.to_string());
    // what the model read, as MCP content blocks (Claude's pictures carry their bytes in a `source`)
    let content: Vec<Value> = match block.get("content") {
        Some(Value::String(s)) => vec![serde_json::json!({ "type": "text", "text": s })],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| match (b.get("type").and_then(Value::as_str), b.get("source")) {
                (Some("image"), Some(src)) => serde_json::json!({ "type": "image", "data": src.get("data"), "mimeType": src.get("media_type") }),
                _ => b.clone(),
            })
            .collect(),
        _ => vec![],
    };
    let mut result = serde_json::json!({ "content": content, "_meta": meta, "isError": failed });
    if let Some(sc) = mcp.and_then(|m| m.get("structuredContent")).filter(|v| !v.is_null()) {
        result["structuredContent"] = sc.clone();
    }
    Some(McpApp { server, tool, resource, input: call.get("input").cloned().unwrap_or(Value::Null), result })
}

/// A command as text: an argv array (a shell's `-c`/`-Command` script taken out of it) or a string.
fn command_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let parts: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
            // bash -lc "…", powershell -Command "…", cmd /c "…": the script is the command
            if let Some(i) = parts.iter().position(|p| matches!(p.to_ascii_lowercase().as_str(), "-c" | "-lc" | "-command" | "/c"))
                && let Some(script) = parts.get(i + 1)
            {
                return script.to_string();
            }
            parts.join(" ")
        }
        _ => String::new(),
    }
}

/// The skill whose instructions `text` (a path, or a command that reads one) names: `…/skills/justui/SKILL.md`.
fn skill_of(text: &str) -> Option<String> {
    let at = text.to_ascii_lowercase().find("skill.md")?;
    let dir = text[..at].trim_end_matches(['/', '\\']);
    let mut up = dir.rsplit(['/', '\\']);
    let name = up.next()?.trim_matches(['"', '\'']);
    // one of the skills folders' own, not any file of that name
    (!name.is_empty() && up.next().is_some_and(|p| p.eq_ignore_ascii_case("skills"))).then(|| name.to_string())
}

/// Codex's own reading of a command (`parsed_cmd`): reads and searches say so (a skill's instructions read are the
/// skill in use), the usual commands say what they did, the rest ran.
fn parsed_label(parsed: &[&Value], cmd: &str) -> (StepKind, String) {
    let types: Vec<&str> = parsed.iter().filter_map(|c| c.get("type").and_then(Value::as_str)).collect();
    let field = |c: &Value, k: &str| c.get(k).and_then(Value::as_str).map(str::to_string);
    if !types.is_empty() && types.iter().all(|t| *t == "read") {
        if let Some(skill) = parsed.iter().find_map(|c| field(c, "path").or_else(|| field(c, "cmd")).and_then(|p| skill_of(&p))) {
            return (StepKind::Skill, wording::skill_name(&skill));
        }
        let names: Vec<String> = parsed.iter().filter_map(|c| field(c, "name").or_else(|| field(c, "path").map(|p| basename(&p)))).collect();
        return (StepKind::Read, match names.as_slice() {
            [one] => format!("Read {one}"),
            [] => "Read a file".to_string(),
            many => format!("Read {} files", many.len()),
        });
    }
    if types.first() == Some(&"search") {
        let q = parsed.first().and_then(|c| field(c, "query"));
        return (StepKind::Search, match q {
            Some(q) => format!("Searched for “{}”", line(&q, LABEL_MAX)),
            None => "Searched the code".to_string(),
        });
    }
    if !types.is_empty() && types.iter().all(|t| *t == "list_files") {
        let path = parsed.first().and_then(|c| field(c, "path")).map(|p| basename(&p)).unwrap_or_else(|| "files".to_string());
        return (StepKind::Read, format!("Listed {path}"));
    }
    wording::command_label(cmd).unwrap_or_else(|| (StepKind::Run, ran(cmd)))
}

/// "Edited chrome.rs", "Created 2 files", "Changed 5 files" for one change or many.
fn files_label(files: &[(String, StepKind)]) -> (StepKind, String) {
    let verb = |k: StepKind| match k {
        StepKind::Create => "Created",
        StepKind::Delete => "Deleted",
        _ => "Edited",
    };
    match files {
        [] => (StepKind::Edit, "Edited files".to_string()),
        [(p, k)] => (*k, format!("{} {}", verb(*k), basename(p))),
        many => {
            let first = many[0].1;
            if many.iter().all(|(_, k)| *k == first) {
                (first, format!("{} {} files", verb(first), many.len()))
            } else {
                (StepKind::Edit, format!("Changed {} files", many.len()))
            }
        }
    }
}

/// A Codex tool call from a `response_item` (Codex before `item_completed` events).
fn codex_call(p: &Value, at: Option<i64>) -> Option<Step> {
    let id = p.get("call_id").and_then(Value::as_str).or_else(|| p.get("id").and_then(Value::as_str)).unwrap_or_default().to_string();
    let name = p.get("name").and_then(Value::as_str).unwrap_or_default();
    let input = match p.get("arguments") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
        Some(other) => other.clone(),
        None => p.get("input").cloned().unwrap_or(Value::Null),
    };
    let ptype = p.get("type").and_then(Value::as_str).unwrap_or_default();
    let state = match p.get("status").and_then(Value::as_str) {
        Some("failed" | "incomplete") => StepState::Failed,
        _ if ptype == "web_search_call" => StepState::Done,
        _ => StepState::Running,
    };
    let make = |kind, label: String, detail: Option<String>| Some(Step { id: id.clone(), kind, label, detail: detail.map(|d| cut(&d, DETAIL_MAX)), state, added: 0, removed: 0, line: None, at_ms: at, app: None });
    if ptype == "web_search_call" {
        let q = p.get("action").and_then(|a| a.get("query")).and_then(Value::as_str).unwrap_or_default();
        return make(StepKind::Web, web(q), None);
    }
    if ptype == "local_shell_call" {
        let cmd = p.get("action").and_then(|a| a.get("command")).map(command_text).unwrap_or_default();
        let (kind, label) = parsed_label(&[], &cmd);
        return make(kind, label, Some(cmd));
    }
    match name {
        "shell" | "local_shell" | "exec_command" | "shell_command" | "container.exec" => {
            let cmd = input.get("command").or_else(|| input.get("cmd")).map(command_text).unwrap_or_default();
            let (kind, label) = parsed_label(&[], &cmd);
            make(kind, label, Some(cmd))
        }
        "apply_patch" => {
            let text = input.as_str().or_else(|| input.get("input").and_then(Value::as_str)).unwrap_or_default();
            let files: Vec<(String, StepKind)> = parse_apply_patch(text)
                .into_iter()
                .map(|(path, k)| {
                    let kind = match k {
                        TouchKind::Create => StepKind::Create,
                        TouchKind::Delete => StepKind::Delete,
                        _ => StepKind::Edit,
                    };
                    (path, kind)
                })
                .collect();
            let (kind, label) = files_label(&files);
            let (added, removed) = diff_counts(text);
            let mut step = make(kind, label, Some(files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join("\n")))?;
            step.added = added;
            step.removed = removed;
            Some(step)
        }
        "update_plan" => {
            let (label, now) = wording::plan_progress(input.get("plan"), &["step"]);
            make(StepKind::Plan, label, now)
        }
        "view_image" => {
            let path = input.get("path").and_then(Value::as_str).unwrap_or_default();
            make(StepKind::Read, format!("Looked at {}", basename(path)), Some(path.to_string()))
        }
        "web_search" => {
            let q = input.get("query").and_then(Value::as_str).unwrap_or_default();
            make(StepKind::Web, web(q), None)
        }
        "request_user_input" | "request_user_input_async" => make(StepKind::Ask, "Asked you a question".into(), None),
        // a script that drives the other tools: the command it runs, when it says
        "exec" => {
            let script = input.as_str().unwrap_or_default();
            match script_command(script) {
                Some(cmd) => {
                    let (kind, label) = wording::command_label(&cmd).unwrap_or_else(|| (StepKind::Run, ran(&cmd)));
                    make(kind, label, Some(cmd))
                }
                None => make(StepKind::Run, "Ran a script".into(), Some(script.to_string())),
            }
        }
        "spawn_agent" => make(StepKind::Agent, "Started a helper".into(), None),
        "send_message" | "followup_task" | "wait" | "list_agents" => None,
        "" => None,
        n => match n.strip_prefix("mcp__").unwrap_or(n).split_once("__") {
            // an MCP server's tool, as Codex names them (`server__tool`)
            Some((server, tool)) => make(StepKind::Mcp, wording::tool_label(tool, &input), Some(wording::server_name(server))),
            None => make(StepKind::Other, wording::name(n), None),
        },
    }
}

/// The first `cmd:"…"` a Codex orchestration script passes to `exec_command`.
fn script_command(script: &str) -> Option<String> {
    let i = script.find("cmd:")?;
    let rest = script[i + 4..].trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let mut out = String::new();
    let mut chars = rest[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => break,
            },
            c if c == quote => return Some(out).filter(|s| !s.trim().is_empty()),
            c => out.push(c),
        }
    }
    None
}

/// A Codex tool output that says it failed: a non-zero exit code (`Exit code: 2`, or the JSON's
/// `metadata.exit_code`).
fn codex_output_failed(output: Option<&Value>) -> bool {
    let text = match output {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        Some(o) => o.to_string(),
        None => return false,
    };
    if let Ok(v) = serde_json::from_str::<Value>(&text)
        && let Some(code) = v.pointer("/metadata/exit_code").and_then(Value::as_i64)
    {
        return code != 0;
    }
    text.lines()
        .take(3)
        .find_map(|l| l.trim().strip_prefix("Exit code:").or_else(|| l.trim().strip_prefix("Process exited with code")).map(|c| c.trim().parse::<i64>().unwrap_or(0)))
        .is_some_and(|c| c != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claude(lines: &[Value]) -> Conversation {
        let mut c = Conversation::new(Harness::Claude);
        for l in lines {
            c.ingest(l);
        }
        c
    }

    fn kinds(c: &Conversation) -> Vec<&'static str> {
        c.items
            .iter()
            .map(|i| match i.as_ref() {
                Item::You { .. } => "you",
                Item::Said { .. } => "said",
                Item::Did { .. } => "did",
                Item::Note { .. } => "note",
            })
            .collect()
    }

    fn user(text: &str) -> Value {
        json!({"type":"user","timestamp":"2026-10-28T10:00:00Z","message":{"role":"user","content":text}})
    }

    fn text(id: &str, t: &str) -> Value {
        json!({"type":"assistant","timestamp":"2026-10-28T10:00:01Z","message":{"id":id,"content":[{"type":"text","text":t}]}})
    }

    fn tool(id: &str, call: &str, name: &str, input: Value) -> Value {
        json!({"type":"assistant","timestamp":"2026-10-28T10:00:02Z","message":{"id":id,"content":[{"type":"tool_use","id":call,"name":name,"input":input}]}})
    }

    fn result(call: &str, error: bool, structured: Value) -> Value {
        json!({"type":"user","timestamp":"2026-10-28T10:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":call,"content":"ok","is_error":error}]},"toolUseResult":structured})
    }

    fn app_of(c: &Conversation, call: &str) -> Option<Arc<McpApp>> {
        c.items.iter().find_map(|i| match i.as_ref() {
            Item::Did { steps } => steps.iter().find(|s| s.id == call).map(|s| s.app.clone()),
            _ => None,
        })?
    }

    #[test]
    fn an_mcp_result_with_a_page_is_an_app() {
        // as Claude Code 2.1.312 records a claude.ai connector's call (JustGains' generate_workout)
        let sc = json!({"workoutId": "w1", "title": "Upper Body"});
        let c = claude(&[
            tool("m1", "t1", "mcp__claude_ai_JustGains__generate_workout", json!({"prompt": "30 minute upper body"})),
            json!({"type":"user","timestamp":"2026-10-28T10:00:03Z",
                "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":sc.to_string()}]},
                "toolUseResult": sc.to_string(),
                "mcpMeta": {"_meta": {"ui": {"resourceUri": "ui://widget/workout-v84.html"}, "openai/outputTemplate": "ui://widget/workout-v84.html", "workoutId": "w1"}, "structuredContent": sc}}),
        ]);
        let app = app_of(&c, "t1").expect("an app");
        assert_eq!((app.server.as_str(), app.tool.as_str(), app.resource.as_str()), ("claude_ai_JustGains", "generate_workout", "ui://widget/workout-v84.html"));
        assert_eq!(app.input, json!({"prompt": "30 minute upper body"}));
        assert_eq!(app.result["structuredContent"], sc);
        assert_eq!(app.result["content"][0]["text"], sc.to_string());
        assert_eq!(app.result["_meta"]["workoutId"], "w1");
        assert_eq!(app.result["isError"], false);
    }

    #[test]
    fn only_a_ui_page_makes_an_app() {
        let meta = |m: Value| {
            let mut c = claude(&[tool("m1", "t1", "mcp__srv__tool", json!({}))]);
            c.ingest(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"hi"}]}]},"mcpMeta":{"_meta": m}}));
            app_of(&c, "t1").map(|a| a.resource.clone())
        };
        assert_eq!(meta(json!({"openai/outputTemplate": "ui://widget/a.html"})).as_deref(), Some("ui://widget/a.html"), "the Apps SDK's template");
        assert_eq!(meta(json!({"ui/resourceUri": "ui://b"})).as_deref(), Some("ui://b"), "the draft's key");
        assert_eq!(meta(json!({"ui": {"resourceUri": "https://example.com/page"}})), None, "only ui:// pages");
        assert_eq!(meta(json!({"progressToken": 1})), None);
        // a built-in tool's result never is one
        let mut c = claude(&[tool("m1", "t1", "Read", json!({"file_path": "a"}))]);
        c.ingest(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]},"mcpMeta":{"_meta":{"ui":{"resourceUri":"ui://x"}}}}));
        assert_eq!(app_of(&c, "t1"), None);
    }

    #[test]
    fn a_long_transcript_is_read_back_from_its_start() {
        use super::super::live::{FIRST_READ_MAX, Tail};
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("long.jsonl");
        // a turn whose tool output is more than a tail's first read takes, as screenshots and big files make it
        let output = "x".repeat(FIRST_READ_MAX as usize + 4096);
        let lines = [
            user("Make the logo pixel art"),
            tool("m1", "t1", "Read", json!({"file_path": "F:\\site\\logo.svg"})),
            result("t1", false, json!({ "file": { "content": output } })),
            text("m2", "Done: the logo is pixel art now."),
        ];
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        // and a line still being written
        std::fs::write(&p, format!("{body}{{\"type\":\"user\",")).unwrap();
        let mut tail = Tail::new(&p);
        let first = tail.read().unwrap();
        assert!(first.began.is_some_and(|b| b > 0), "the first read starts near the end");
        let mut latest = Conversation::new(Harness::Claude);
        latest.ingest_lines(&first.lines);
        assert_eq!(kinds(&latest), ["said"], "the last lines alone lose the turn");
        assert_eq!(tail.lines_end(), body.len() as u64);
        let c = Conversation::history(Harness::Claude, &p, tail.lines_end()).unwrap();
        assert_eq!(kinds(&c), ["you", "did", "said"]);
        let Item::Did { steps } = c.items[1].as_ref() else { panic!() };
        assert_eq!((steps[0].label.as_str(), steps[0].state), ("Read logo.svg", StepState::Done));
        // begun inside a line, it's left out; begun at one's start, it's in
        let answer = format!("{}\n", lines[3]);
        let c = Conversation::read_back(Harness::Claude, &p, tail.lines_end(), answer.len() as u64 + 10).unwrap();
        assert_eq!(kinds(&c), ["said"]);
        let c = Conversation::read_back(Harness::Claude, &p, tail.lines_end(), answer.len() as u64).unwrap();
        assert_eq!(kinds(&c), ["said"]);
        let c = Conversation::read_back(Harness::Claude, &p, tail.lines_end(), answer.len() as u64 - 1).unwrap();
        assert_eq!(kinds(&c), Vec::<&str>::new());
        // later reads aren't first ones
        assert_eq!(tail.read().unwrap().began, None);
    }

    #[test]
    fn a_claude_turn_reads_as_you_said_did_said() {
        let c = claude(&[
            user("fix the flaky timer test"),
            text("m1", "I'll look at the test first."),
            tool("m1", "t1", "Read", json!({"file_path":"F:\\app\\src\\timer.test.ts"})),
            tool("m1", "t2", "Bash", json!({"command":"npm test -- timer","description":"Run the timer tests"})),
            result("t1", false, json!({})),
            result("t2", true, json!("Error: Exit code 1")),
            tool("m2", "t3", "Edit", json!({"file_path":"F:\\app\\src\\timer.ts"})),
            result("t3", false, json!({"structuredPatch":[{"lines":[" a","-b","+c","+d"]}]})),
            text("m3", "Fixed: the timer read the wall clock."),
            text("m3", "Want me to ship it?"),
            json!({"type":"system","subtype":"turn_duration","durationMs":245_000,"timestamp":"2026-10-28T10:04:05Z"}),
        ]);
        assert_eq!(kinds(&c), ["you", "said", "did", "said", "note"]);
        let Item::Did { steps } = c.items[2].as_ref() else { panic!() };
        let got: Vec<(&str, StepState)> = steps.iter().map(|s| (s.label.as_str(), s.state)).collect();
        assert_eq!(got, [("Read timer.test.ts", StepState::Done), ("Run the timer tests", StepState::Failed), ("Edited timer.ts", StepState::Done)]);
        assert_eq!((steps[2].added, steps[2].removed), (2, 1));
        assert_eq!(steps[1].detail.as_deref(), Some("npm test -- timer"));
        let Item::Said { text, .. } = c.items[3].as_ref() else { panic!() };
        assert_eq!(text, "Fixed: the timer read the wall clock.\n\nWant me to ship it?", "text blocks with nothing between join");
        assert!(matches!(c.items[4].as_ref(), Item::Note { kind: NoteKind::Worked, text, .. } if text == "4m 5s"));
    }

    #[test]
    fn harness_text_is_left_out_and_commands_read_as_typed() {
        let c = claude(&[
            json!({"type":"user","isMeta":true,"message":{"content":"<local-command-caveat>Caveat…</local-command-caveat>"}}),
            user("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>"),
            json!({"type":"user","message":{"content":[{"type":"text","text":"<system-reminder>be nice</system-reminder>"},{"type":"text","text":"[Image #1] what's wrong here?"},{"type":"image","source":{}}]}}),
            user("<task-notification><status>completed</status></task-notification>"),
            user("<bash-input>git status</bash-input>"),
            json!({"type":"user","isSidechain":true,"message":{"content":"a helper's own prompt"}}),
        ]);
        let you: Vec<(String, usize, bool)> = c.items.iter().filter_map(|i| match i.as_ref() {
            Item::You { text, images, command, .. } => Some((text.clone(), *images, *command)),
            _ => None,
        }).collect();
        let pasted = claude(&[user(r#"C:\Users\me\.bro\paste\paste-1793204344367.png "C:\My Shots\a b.PNG" what's wrong here?"#)]);
        let Item::You { text, images, pictures, .. } = pasted.items[0].as_ref() else { panic!() };
        assert_eq!(text, "what's wrong here?");
        assert_eq!(*images, 2);
        assert_eq!(pictures, &[r"C:\Users\me\.bro\paste\paste-1793204344367.png".to_string(), r"C:\My Shots\a b.PNG".to_string()]);
        assert_eq!(you, [("/model opus".to_string(), 0, true), ("what's wrong here?".to_string(), 1, false), ("! git status".to_string(), 0, true)]);
    }

    #[test]
    fn what_other_sessions_send_isnt_you() {
        let c = claude(&[
            user("spawn the pop-* teammates"),
            text("m1", "Spawned six."),
            // a teammate's message: a plain user record, no isMeta and no origin
            user("Another Claude session sent a message:\n<teammate-message teammate_id=\"pop-held\" color=\"purple\" summary=\"heads-up\">\npop-held: someone else is editing held.ts\n</teammate-message>\n\nThis came from another Claude session — not typed by your user"),
            text("m2", "Noted."),
            user("<teammate-message teammate_id=\"pop-wear\" color=\"cyan\">\n{\"type\":\"idle_notification\"}\n</teammate-message>"),
            user("<agent-message from=\"a560080e21d258320\">\nfound it\n</agent-message>"),
            json!({"type":"user","message":{"content":[{"type":"text","text":"<system-reminder>\n<tools-used-before-summary>WebSearch</tools-used-before-summary>\n</system-reminder>\n"},{"type":"text","text":"Another Claude session sent a message:\n<agent-message from=\"popculture\">\n17 new pieces\n</agent-message>"}]}}),
            text("m3", "All in."),
        ]);
        assert_eq!(kinds(&c), ["you", "said"], "{:?}", c.items);
        let Item::Said { text, .. } = c.items[1].as_ref() else { panic!() };
        assert_eq!(text, "Spawned six.\n\nNoted.\n\nAll in.", "its replies to them read on from the last");
        assert_eq!(super::super::describe::claude_prompt(Some(&json!("Another Claude session sent a message:\n<cross-session-message from=\"x\">hi</cross-session-message>"))), None, "nor the session's last prompt");
    }

    #[test]
    fn a_message_read_mid_turn_lands_where_it_was_read() {
        let queued = |text: &str, kind: &str| json!({"type":"attachment","timestamp":"2026-10-28T10:00:04Z","attachment":{"type":"queued_command","prompt":text,"commandMode":"prompt","origin":{"kind":kind},"timestamp":"2026-10-28T10:00:04Z","humanTurn":true}});
        let c = claude(&[
            user("fix the flaky timer test"),
            tool("m1", "t1", "Read", json!({"file_path":"a.ts"})),
            result("t1", false, json!({})),
            tool("m1", "t2", "Bash", json!({"command":"npm test"})),
            queued("also check the docs [Image #1]", "human"),
            queued("<task-notification>done</task-notification>", "agent"),
            text("m2", "Will do."),
        ]);
        assert_eq!(kinds(&c), ["you", "did", "you", "said"], "the message goes where Claude read it, not at the end");
        let Item::You { text, images, command, .. } = c.items[2].as_ref() else { panic!() };
        assert_eq!((text.as_str(), *images, *command), ("also check the docs", 1, false));
        let Item::Did { steps } = c.items[1].as_ref() else { panic!() };
        assert_eq!(steps[1].state, StepState::Running, "the command in flight still runs: the turn goes on");
    }

    #[test]
    fn model_and_effort_commands_say_what_it_runs_on() {
        let mut c = claude(&[
            json!({"type":"user","version":"2.1.312","message":{"content":"hi"}}),
            json!({"type":"api-request","params":{"model":"claude-opus-5-5","output_config":{"effort":"high"}}}),
            user("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>claude-sonnet-5-5</command-args>"),
            user("<command-name>/effort</command-name>\n<command-message>effort</command-message>\n<command-args></command-args>"),
            user("<local-command-stdout>Set effort level to max (this session only): Maximum capability with deepest reasoning</local-command-stdout>"),
        ]);
        assert_eq!((c.model.as_deref(), c.effort.as_deref(), c.version.as_deref()), (Some("claude-sonnet-5-5"), Some("max"), Some("2.1.312")));
        c.ingest(&user("<local-command-stdout>Set model to \u{1b}[1mOpus 5.5\u{1b}[22m and saved as your default for new sessions</local-command-stdout>"));
        assert_eq!(c.model.as_deref(), Some("Opus 5.5"));
        c.ingest(&user("<local-command-stdout>Set model to Default (claude-fable-5-1)</local-command-stdout>"));
        assert_eq!(c.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(kinds(&c), ["you", "you", "you"], "what the commands said back isn't a message");
        let mut x = Conversation::new(Harness::Codex);
        x.ingest(&json!({"type":"session_meta","payload":{"cwd":"J:\\app","cli_version":"0.165.0"}}));
        assert_eq!(x.version.as_deref(), Some("0.165.0"));
    }

    #[test]
    fn interruptions_errors_and_compactions_are_notes() {
        let c = claude(&[
            user("go"),
            tool("m1", "t1", "Bash", json!({"command":"sleep 100"})),
            user("[Request interrupted by user for tool use]"),
            json!({"type":"assistant","isApiErrorMessage":true,"message":{"content":[{"type":"text","text":"API Error: 529 overloaded"}]}}),
            json!({"type":"system","subtype":"compact_boundary","content":"Conversation compacted"}),
            json!({"type":"user","isCompactSummary":true,"message":{"content":"This session is being continued…"}}),
        ]);
        assert_eq!(kinds(&c), ["you", "did", "note", "note", "note"]);
        let Item::Did { steps } = c.items[1].as_ref() else { panic!() };
        assert_eq!(steps[0].state, StepState::Failed, "the interrupted command never finished");
        assert_eq!(steps[0].label, "Waited 1m 40s");
        assert!(matches!(c.items[3].as_ref(), Item::Note { kind: NoteKind::Error, text, .. } if text.contains("529")));
        assert!(matches!(c.items[4].as_ref(), Item::Note { kind: NoteKind::Compacted, .. }), "one note for the boundary and its summary");
    }

    #[test]
    fn the_session_says_its_model_effort_and_folder() {
        let c = claude(&[
            json!({"type":"user","cwd":"F:\\app","gitBranch":"main","message":{"content":"hi"}}),
            json!({"type":"api-request","params":{"model":"claude-opus-5-5","thinking":{"type":"adaptive"},"output_config":{"effort":"max"}}}),
            json!({"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"hello"}]}}),
            json!({"type":"assistant","message":{"id":"m2","model":"<synthetic>","content":[]}}),
        ]);
        assert_eq!((c.model.as_deref(), c.effort.as_deref(), c.cwd.as_deref(), c.branch.as_deref()), (Some("claude-opus-5-5"), Some("max"), Some("F:\\app"), Some("main")));
        let mut x = Conversation::new(Harness::Codex);
        for l in [
            json!({"type":"session_meta","payload":{"cwd":"J:\\justgains","git":{"branch":"dev"}}}),
            json!({"type":"turn_context","payload":{"cwd":"J:\\justgains","model":"gpt-6.1-sol","effort":"high"}}),
            json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-6.1-sol","reasoning_effort":"max"}}}),
            json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"**Planning the fix**\n\nI'll read the hook first."}]}}),
        ] {
            x.ingest(&l);
        }
        assert_eq!((x.model.as_deref(), x.effort.as_deref(), x.branch.as_deref()), (Some("gpt-6.1-sol"), Some("max"), Some("dev")));
        assert_eq!(x.thought.as_ref().map(|t| t.text.as_str()), Some("Planning the fix"));
        x.ingest(&json!({"type":"event_msg","payload":{"type":"user_message","message":"next"}}));
        assert!(x.thought.is_none(), "a new prompt starts fresh");
    }

    #[test]
    fn edits_and_reads_know_their_line() {
        let c = claude(&[
            user("a"),
            tool("m1", "t1", "Read", json!({"file_path":"/x/a.rs","offset":120})),
            tool("m1", "t2", "Edit", json!({"file_path":"/x/b.rs"})),
            result("t2", false, json!({"structuredPatch":[{"newStart":40,"lines":[" a"," b","-c","+d"]}]})),
        ]);
        let Item::Did { steps } = c.items[1].as_ref() else { panic!() };
        assert_eq!((steps[0].line, steps[1].line), (Some(120), Some(42)), "the edit's first changed line, after its context");
        assert_eq!(diff_line("--- a\n+++ b\n@@ -1,3 +7,4 @@\n x\n-y\n+z\n"), Some(8));
    }

    #[test]
    fn a_prompt_sent_again_after_stopping_is_kept() {
        let c = claude(&[user("run the tests"), user("[Request interrupted by user]"), user("run the tests")]);
        assert_eq!(kinds(&c), ["you", "note", "you"]);
    }

    #[test]
    fn a_new_prompt_settles_steps_that_never_reported() {
        let c = claude(&[user("a"), tool("m1", "t1", "Read", json!({"file_path":"x.rs"})), user("b")]);
        let Item::Did { steps } = c.items[1].as_ref() else { panic!() };
        assert_eq!(steps[0].state, StepState::Done);
        assert!(c.running().is_none());
        let c = claude(&[user("a"), tool("m1", "t1", "Grep", json!({"pattern":"focus"}))]);
        assert_eq!(c.running().map(|s| s.label.as_str()), Some("Searched for “focus”"));
    }

    #[test]
    fn writes_that_create_a_file_say_so() {
        let c = claude(&[user("a"), tool("m1", "t1", "Write", json!({"file_path":"/x/notes.md"})), result("t1", false, json!({"type":"create","filePath":"/x/notes.md"}))]);
        let Item::Did { steps } = c.items[1].as_ref() else { panic!() };
        assert_eq!((steps[0].kind, steps[0].label.as_str()), (StepKind::Create, "Created notes.md"));
    }

    #[test]
    fn mcp_and_unknown_tools_read_as_words() {
        let navigate = claude_step("mcp__justbrowse__chrome_navigate", &json!({"url":"https://www.figma.com/design/gW6z/Just-Gains","background":true}));
        assert_eq!(navigate, (StepKind::Mcp, "Navigate · figma.com".to_string(), Some("JustBrowse".to_string())), "what it did and to what; its server by name");
        assert_eq!(claude_step("mcp__claude_ai_JustGains__generate_workout", &json!({"prompt":"30 minute upper body"})).2.as_deref(), Some("JustGains"));
        assert_eq!(claude_step("NotebookRead", &json!({})).1, "Notebook read");
        assert_eq!(claude_step("WebFetch", &json!({"url":"https://docs.rs/wgpu"})).1, "Read docs.rs");
    }

    #[test]
    fn a_skill_reads_as_its_name() {
        assert_eq!(claude_step("Skill", &json!({"skill":"generate-voice"})), (StepKind::Skill, "Generate voice".to_string(), None));
        let asked = claude_step("Skill", &json!({"skill":"justgains-app-deployment","args":"ship the beta"}));
        assert_eq!(asked, (StepKind::Skill, "JustGains app deployment".to_string(), Some("ship the beta".to_string())));
        // Codex reads a skill's instructions itself
        let read = json!({"type":"read","cmd":"cat /home/j/.codex/skills/generate-voice/SKILL.md","name":"SKILL.md"});
        assert_eq!(parsed_label(&[&read], "cat /home/j/.codex/skills/generate-voice/SKILL.md"), (StepKind::Skill, "Generate voice".to_string()));
        let win = json!({"type":"read","cmd":"Get-Content 'C:\\Users\\j\\.codex\\skills\\justui\\SKILL.md'","name":"SKILL.md"});
        assert_eq!(parsed_label(&[&win], "").1, "JustUI");
        let other = json!({"type":"read","cmd":"cat docs/SKILL.md","name":"SKILL.md"});
        assert_eq!(parsed_label(&[&other], "cat docs/SKILL.md"), (StepKind::Read, "Read SKILL.md".to_string()), "any file of that name isn't a skill");
    }

    #[test]
    fn claude_codes_other_tools_say_what_they_did() {
        // as Claude Code 2.1.312 records them
        let label = |n: &str, i: Value| claude_step(n, &i).1;
        assert_eq!(label("SendMessage", json!({"to":"pop-head","message":"Main session here: someone edited compose.ts"})), "Messaged pop-head");
        assert_eq!(label("ToolSearch", json!({"query":"select:WebSearch","max_results":1})), "Loaded WebSearch");
        assert_eq!(label("TaskStop", json!({"task_id":"bw0akmxbl"})), "Stopped a background task");
        assert_eq!(label("Monitor", json!({"description":"EAS builds: iOS production a716cf1b","command":"while true; do sleep 30; done"})), "Watching EAS builds: iOS production a716cf1b");
        assert_eq!(label("ScheduleWakeup", json!({"delaySeconds":1200,"reason":"fallback in case the emulator screenshot agent stalls"})), "Checks back in 20m");
        assert_eq!(label("Agent", json!({"description":"Arms: light-bend rubber hose","subagent_type":"general-purpose","prompt":"…"})), "Arms: light-bend rubber hose");
        assert_eq!(label("Agent", json!({"description":"Find the route code","subagent_type":"Explore"})), "Find the route code · Explore");
        let todos = json!({"todos":[{"content":"Write it","status":"completed"},{"content":"Test it","status":"in_progress","activeForm":"Testing it"}]});
        assert_eq!(claude_step("TodoWrite", &todos), (StepKind::Plan, "Updated the plan · 1/2 done".to_string(), Some("Testing it".to_string())));
        assert_eq!(claude_step("Artifact", &json!({"file_path":"C:/x/report.html","icon":"chart"})), (StepKind::Send, "Published report.html".to_string(), Some("C:/x/report.html".to_string())));
        assert_eq!(label("Artifact", json!({"action":"quickstart","intent":"other"})), "Looked up page types");
        // a command with no words of its own reads as what it did
        assert_eq!(claude_step("Bash", &json!({"command":"cargo test -p bro-tui"})), (StepKind::Run, "Tested bro-tui".to_string(), Some("cargo test -p bro-tui".to_string())));
        assert_eq!(label("Bash", json!({"command":"cargo test","description":"Run the launcher tests"})), "Run the launcher tests", "its own words win");
        assert_eq!(label("PowerShell", json!({"command":"./build.ps1 -Fast"})), "Ran ./build.ps1 -Fast");
    }

    #[test]
    fn codex_mcp_calls_and_commands_read_as_words() {
        let mut c = Conversation::new(Harness::Codex);
        for l in [
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u1","content":[{"type":"text","text":"check the page"}]}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"m1","server":"justbrowse","tool":"chrome_navigate","arguments":{"url":"https://example.com/a"},"status":"completed"}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"x1","command":["bash","-lc","git push origin main"],"parsed_cmd":[{"type":"unknown","cmd":"git push origin main"}],"status":"completed","exit_code":0}}}),
        ] {
            c.ingest(&l);
        }
        let Item::Did { steps } = c.items.last().unwrap().as_ref() else { panic!("{:?}", c.items) };
        assert_eq!((steps[0].kind, steps[0].label.as_str(), steps[0].detail.as_deref()), (StepKind::Mcp, "Navigate · example.com", Some("JustBrowse")));
        assert_eq!((steps[1].kind, steps[1].label.as_str()), (StepKind::Run, "Pushed main"));
    }

    #[test]
    fn codex_items_describe_its_steps() {
        let mut c = Conversation::new(Harness::Codex);
        let lines = [
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n<cwd>J:\\</cwd></environment_context>"}]}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u1","content":[{"type":"text","text":"add golden fixtures"}]}}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add golden fixtures"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I'll read the stream fixtures first."}]}}),
            json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"c1","name":"exec","input":"text(await tools.exec_command({cmd:\"cat a.jsonl\"}))"}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"x1","command":["bash","-lc","cat fixtures/a.jsonl"],"parsed_cmd":[{"type":"read","cmd":"cat fixtures/a.jsonl","name":"a.jsonl"}],"status":"completed","exit_code":0}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"x2","command":["bash","-lc","cargo test"],"parsed_cmd":[{"type":"unknown","cmd":"cargo test -p bro-proxy"}],"status":"completed","exit_code":101}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","id":"x3","changes":{"/r/tests/golden.rs":{"type":"update","unified_diff":"@@ -1,2 +1,3 @@\n a\n-b\n+c\n+d\n"}},"status":"completed"}}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"Extension","kind":"web.search","id":"x4","query":"responses api streaming"}}}),
            json!({"type":"event_msg","payload":{"type":"task_complete"}}),
        ];
        for l in &lines {
            c.ingest(l);
        }
        assert_eq!(kinds(&c), ["you", "said", "did"]);
        let Item::Did { steps } = c.items[2].as_ref() else { panic!() };
        let got: Vec<(&str, StepState)> = steps.iter().map(|s| (s.label.as_str(), s.state)).collect();
        assert_eq!(got, [("Read a.jsonl", StepState::Done), ("Tested bro-proxy", StepState::Failed), ("Edited golden.rs", StepState::Done), ("Searched the web for “responses api streaming”", StepState::Done)], "the script's own call is left to the items");
        assert_eq!((steps[2].added, steps[2].removed), (2, 1));
    }

    #[test]
    fn older_codex_tool_calls_are_steps() {
        let mut c = Conversation::new(Harness::Codex);
        c.ingest_lines(&[
            json!({"type":"event_msg","payload":{"type":"user_message","message":"run the tests","images":[]}}).to_string(),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}"}}).to_string(),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"{\"output\":\"ok\",\"metadata\":{\"exit_code\":0}}"}}).to_string(),
            json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"c2","name":"apply_patch","input":"*** Begin Patch\n*** Add File: src/new.rs\n+fn a() {}\n*** End Patch"}}).to_string(),
            json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c2","output":"Exit code: 1\nfailed"}}).to_string(),
            json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}).to_string(),
        ]);
        assert_eq!(kinds(&c), ["you", "did", "note"]);
        let Item::Did { steps } = c.items[1].as_ref() else { panic!() };
        let got: Vec<(&str, StepState)> = steps.iter().map(|s| (s.label.as_str(), s.state)).collect();
        assert_eq!(got, [("Ran the tests", StepState::Done), ("Created new.rs", StepState::Failed)]);
    }

    #[test]
    fn commands_and_searches_read_plainly() {
        assert_eq!(ran("@'\nWrite-Output hi\n'@ | Set-Content x"), "Ran a script");
        assert_eq!(ran("\n  cargo test -p bro-core\n"), "Ran cargo test -p bro-core");
        assert_eq!(web("https://developers.openai.com/api/docs"), "Read developers.openai.com");
        assert_eq!(web("'keywords'"), "Searched the web for “keywords”");
        assert_eq!(web(""), "Searched the web");
    }

    #[test]
    fn scripts_name_their_command() {
        assert_eq!(script_command("text(await tools.exec_command({cmd:\"Get-Content a.md; ls\",max:1}))").as_deref(), Some("Get-Content a.md; ls"));
        assert_eq!(script_command("await tools.view({path:'x'})"), None);
    }

    #[test]
    fn old_items_go_first() {
        let mut c = Conversation::new(Harness::Claude);
        for i in 0..ITEMS_MAX + 10 {
            c.ingest(&user(&format!("prompt {i}")));
            c.ingest(&text(&format!("m{i}"), "ok"));
        }
        assert_eq!(c.items.len(), ITEMS_MAX);
        assert!(matches!(c.items.last().map(|i| i.as_ref()), Some(Item::Said { .. })));
    }
}

