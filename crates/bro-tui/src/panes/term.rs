//! A real terminal in a pane: an agent CLI or shell on a pseudo-terminal (ConPTY / Unix pty), parsed by vt100
//! and drawn cell by cell. Ported from z4-oriel with: lazy spawn at the real pane size, the DSR reply written
//! without holding the parser lock, output teed to the bridge, OSC titles, and session metadata for the sidebar.

use super::keys::{encode_key, encode_mouse};
use crate::pane::{Activity, Cx, Pane, Waker};
use crate::services::Services;
use crate::theme::Theme;
use bro_core::Harness;
use bro_core::projects::ProjectKey;
use crossterm::event::{KeyEvent, MouseEvent, MouseEventKind};
use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Environment variables a parent Claude Code session sets that must not leak into child sessions.
pub const CLAUDE_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

/// Who is running in this pane (for the sidebar, titles and the bridge).
#[derive(Clone, Debug)]
pub struct Meta {
    /// Session id shared with the bridge.
    pub sid: String,
    pub harness: Option<Harness>,
    pub profile: Option<String>,
    /// The login whose folder holds this session's transcript ("claude:work", "codex:local"); None for
    /// shells and pi/omp. Used to move a running session to another login.
    pub store: Option<String>,
    pub model: Option<String>,
    /// "claude · work · opus"
    pub label: String,
    /// A name for it: yours (sidebar `r`, prefix `,`) when `renamed`, else a starting title (a resumed
    /// session's first prompt) that the agent's own title replaces once it sets one.
    pub name: Option<String>,
    /// `name` is one you typed
    pub renamed: bool,
    pub cwd: PathBuf,
    pub project: ProjectKey,
    pub started: Instant,
    /// Proxy route to drop when it exits.
    pub route_id: Option<String>,
    /// Staged files to delete when it exits.
    pub cleanup: Vec<PathBuf>,
    /// The caveman level it was launched with (the header's badge).
    pub caveman: Option<bro_core::caveman::Level>,
}

/// What to run.
#[derive(Clone, Debug, Default)]
pub struct Spawn {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
}

/// vt100 callbacks: remember the program's window title.
#[derive(Default)]
pub struct Cb {
    pub title: Option<String>,
}

impl vt100::Callbacks for Cb {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        let t = String::from_utf8_lossy(title).trim().to_string();
        self.title = (!t.is_empty()).then_some(t);
    }
}

type Parser = vt100::Parser<Cb>;

/// Small metadata published by the parser worker, independent of its screen lock.
struct ProgramState {
    title: Option<String>,
    app_cursor: bool,
    bracketed_paste: bool,
    mouse_mode: vt100::MouseProtocolMode,
    mouse_encoding: vt100::MouseProtocolEncoding,
}

impl ProgramState {
    fn of(parser: &Parser) -> Self {
        let screen = parser.screen();
        ProgramState {
            title: parser.callbacks().title.clone(),
            app_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            mouse_mode: screen.mouse_protocol_mode(),
            mouse_encoding: screen.mouse_protocol_encoding(),
        }
    }
}

struct Live {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

/// The terminal pane.
pub struct Term {
    pub meta: Meta,
    spawn: Option<Spawn>,
    live: Option<Live>,
    parser: Arc<Mutex<Parser>>,
    program_state: Arc<Mutex<ProgramState>>,
    /// The last painted viewport, used while the output worker owns the parser.
    frame_cache: Option<ratatui::buffer::Buffer>,
    cached_cursor: Option<(u16, u16)>,
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    exited: Arc<AtomicBool>,
    /// Preserve the process status independently of whether the PTY output pipe has closed.
    exit_status: Option<i32>,
    /// rows, cols
    size: (u16, u16),
    /// What the pty would be without the remote minimum: the tile it was last drawn in, or
    /// what a remote client asked for while it was hidden. (rows, cols)
    base: (u16, u16),
    /// When a remote client last watched (the minimum outlives it by a grace period).
    hold: super::remote_min::RemoteHold,
    /// Screen row shown on the tile's top row. Above 0 while the remote minimum makes the pty
    /// taller than the tile: the tile then shows the bottom rows (pinned bottom-left).
    view_top: u16,
    /// Tests: remote viewers instead of asking the bridge.
    #[cfg(test)]
    pub test_viewers: Option<usize>,
    /// Tests: added to the clock (grace periods).
    #[cfg(test)]
    pub test_skew: Duration,
    /// Tests: a fixed pane sized by its tile like a live one.
    #[cfg(test)]
    pub test_sized: bool,
    /// lines scrolled back (0 = live)
    scroll: usize,
    /// ms since `born` of the last output (set by the reader thread)
    last_output: Arc<AtomicU64>,
    born: Instant,
    status: Option<Activity>,
    agent: bool,
    last_scan: Instant,
    error: Option<String>,
    /// Demo panes: fixed activity, never spawned when `spawn` is None.
    pub demo_activity: Option<Activity>,
    /// Bytes `send` delivered (tests: fixed panes have no program to write to).
    #[cfg(test)]
    pub sent: Vec<u8>,
    svc: Services,
    title_sent: Option<String>,
    /// Targets for the last displayed cells, including the current scrollback viewport.
    links: super::links::Links,
    /// Which path-like words on screen are files (resolved off the UI thread).
    files: super::file_cache::FileCache,
    /// The agent's transcript, followed on a worker (Claude / Codex panes).
    insights: Option<super::insights::Insights>,
    /// What was launched (a `/recap` runs the same program with the same env).
    launch: Option<Spawn>,
    /// While a prompt is on screen: its question line ("Do you want to proceed?").
    prompt_title: Option<String>,
    /// Types `continue` for an agent that stopped on an API error.
    resume: super::auto_resume::AutoResume,
    /// Input from you so far (auto-resume backs off when this moves).
    typed: u64,
    /// Auto-resume typed its prompt: press enter at this time (a separate write, so it isn't read as a paste).
    enter_at: Option<Instant>,
    /// When auto-approve last answered a prompt (one answer per prompt while the screen catches up).
    approved_at: Option<Instant>,
}

impl Term {
    #[cfg(test)]
    pub(crate) fn set_test_writer(&mut self, writer: Box<dyn Write + Send>) {
        *self.writer.lock() = Some(writer);
    }
    /// A session that spawns lazily at its first render (when the real size is known).
    pub fn new(meta: Meta, spawn: Spawn, svc: Services) -> Term {
        let agent = meta.harness.is_some();
        let files = super::file_cache::FileCache::new(bro_bridge::file_preview::ResolveRoots { cwd: meta.cwd.clone(), project: Some(meta.project.root.clone()) });
        let parser = vt100::Parser::new_with_callbacks(24, 80, 10_000, Cb::default());
        let program_state = ProgramState::of(&parser);
        Term {
            meta,
            spawn: Some(spawn),
            live: None,
            parser: Arc::new(Mutex::new(parser)),
            program_state: Arc::new(Mutex::new(program_state)),
            frame_cache: None,
            cached_cursor: None,
            writer: Arc::new(Mutex::new(None)),
            exited: Arc::new(AtomicBool::new(false)),
            exit_status: None,
            size: (24, 80),
            base: (24, 80),
            hold: Default::default(),
            view_top: 0,
            #[cfg(test)]
            test_viewers: None,
            #[cfg(test)]
            test_skew: Duration::ZERO,
            #[cfg(test)]
            test_sized: false,
            scroll: 0,
            last_output: Arc::new(AtomicU64::new(0)),
            born: Instant::now(),
            status: agent.then_some(Activity::Idle),
            agent,
            last_scan: Instant::now(),
            error: None,
            demo_activity: None,
            #[cfg(test)]
            sent: Vec::new(),
            svc,
            title_sent: None,
            links: Default::default(),
            files,
            insights: None,
            launch: None,
            prompt_title: None,
            resume: Default::default(),
            typed: 0,
            enter_at: None,
            approved_at: None,
        }
    }

    /// A pane that never spawns: shows `transcript` (ANSI) as its screen. For snapshots and tests.
    pub fn fixed(meta: Meta, transcript: &str, svc: Services) -> Term {
        let mut t = Term::new(meta, Spawn::default(), svc);
        t.spawn = None;
        {
            let mut parser = t.parser.lock();
            parser.process(transcript.as_bytes());
            *t.program_state.lock() = ProgramState::of(&parser);
        }
        t
    }

    /// Spawn now if not yet (panes in hidden tabs, bridge-created sessions).
    pub fn ensure_started(&mut self, rows: u16, cols: u16, waker: Waker) {
        if self.live.is_some() || self.error.is_some() {
            return;
        }
        let Some(spawn) = self.spawn.take() else { return };
        let (rows, cols) = (rows.max(2), cols.max(10));
        if let Err(e) = self.start(&spawn, rows, cols, waker) {
            self.error = Some(e);
        }
    }

    fn start(&mut self, s: &Spawn, rows: u16, cols: u16, waker: Waker) -> Result<(), String> {
        let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| format!("couldn't open a pty: {e}"))?;
        let (prog, pre) = resolve(&s.program);
        let mut cmd = CommandBuilder::new(&prog);
        cmd.args(&pre);
        cmd.args(&s.args);
        let cwd = if self.meta.cwd.is_dir() { self.meta.cwd.clone() } else { dirs::home_dir().unwrap_or_else(std::env::temp_dir) };
        cmd.cwd(&cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env_remove("NO_COLOR");
        for k in CLAUDE_SESSION_ENV {
            cmd.env_remove(k);
        }
        cmd.env("BRO", "1");
        cmd.env("BRO_SESSION_ID", &self.meta.sid);
        // `bro open` finds the bridge here (else via its discovery file)
        match self.svc.bridge_local_url() {
            Some(url) => cmd.env("BRO_BRIDGE_URL", url),
            None => cmd.env_remove("BRO_BRIDGE_URL"),
        }
        for k in &s.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in &s.env {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("couldn't start {}: {e}", s.program))?;
        drop(pair.slave);
        let writer = pair.master.take_writer().map_err(|e| format!("pty writer: {e}"))?;
        let reader = pair.master.try_clone_reader().map_err(|e| format!("pty reader: {e}"))?;
        *self.writer.lock() = Some(Box::new(super::writer::Writer::new(writer).map_err(|e| format!("pty input worker: {e}"))?));
        self.parser.lock().screen_mut().set_size(rows, cols);
        self.size = (rows, cols);
        self.base = (rows, cols);
        let pid = child.process_id();
        // so closing the window (not just quitting) still ends this session's processes
        if let Some(p) = pid.filter(|_| !self.svc.is_demo()) {
            bro_core::proc_tree::track(p);
        }
        self.live = Some(Live { master: pair.master, child });
        self.svc.bridge_register(self.bridge_meta(s, pid));
        self.launch = Some(s.clone());
        if let Some(spec) = self.bind_spec(s, pid)
            && !self.svc.is_demo()
        {
            self.insights = Some(super::insights::Insights::start(spec, waker.clone()));
        }
        self.start_reader(reader, waker);
        Ok(())
    }

    /// How to find this pane's transcript (Claude / Codex only).
    fn bind_spec(&self, s: &Spawn, pid: Option<u32>) -> Option<bro_core::sessions::live::BindSpec> {
        let harness = self.meta.harness.filter(|h| matches!(h, Harness::Claude | Harness::Codex))?;
        let var = if harness == Harness::Claude { "CLAUDE_CONFIG_DIR" } else { "CODEX_HOME" };
        let config_dir = s.env.iter().find(|(k, _)| k == var).map(|(_, v)| PathBuf::from(v)).filter(|p| !p.as_os_str().is_empty());
        let started = std::time::SystemTime::now().checked_sub(self.meta.started.elapsed()).unwrap_or_else(std::time::SystemTime::now);
        Some(bro_core::sessions::live::BindSpec {
            harness,
            profile_id: self.meta.store.clone(),
            config_dir,
            pid,
            cwd: self.meta.cwd.clone(),
            started,
            resume_id: super::insights::resume_id(harness, &s.args),
        })
    }

    /// What the transcript says so far (None for shells, pi/omp, and until the worker has read anything).
    pub fn insights(&self) -> Option<super::insights::Snapshot> {
        self.insights.as_ref().map(|i| i.snapshot())
    }

    #[cfg(test)]
    pub fn set_screen_status(&mut self, status: Option<Activity>) {
        self.demo_activity = None;
        self.status = status;
    }

    /// The transcript shows a question tool (AskUserQuestion, ExitPlanMode, Codex
    /// request_user_input_async) still waiting for its answer.
    fn asking_question(&self) -> bool {
        const QUESTION_TOOLS: [&str; 3] = ["AskUserQuestion", "ExitPlanMode", "request_user_input_async"];
        self.insights()
            .and_then(|s| s.facts)
            .and_then(|f| f.pending.as_ref().map(|p| QUESTION_TOOLS.contains(&p.tool.as_str())))
            .unwrap_or(false)
    }

    /// The transcript worker's handle (a finished `/recap` is handed to it).
    pub fn insights_handle(&self) -> Option<&super::insights::Insights> {
        self.insights.as_ref()
    }

    /// Tests: give a fixed pane transcript facts.
    #[cfg(test)]
    pub fn set_insights(&mut self, insights: super::insights::Insights) {
        self.insights = Some(insights);
    }

    /// Tests: what the program was launched with.
    #[cfg(test)]
    pub fn set_launch(&mut self, spawn: Spawn) {
        self.launch = Some(spawn);
    }

    /// The question on screen while the agent waits on a prompt.
    pub fn prompt_title(&self) -> Option<&str> {
        self.prompt_title.as_deref()
    }

    /// The program and environment this session was launched with.
    pub fn launch_spawn(&self) -> Option<&Spawn> {
        self.launch.as_ref()
    }

    /// What the bridge is told about this session.
    pub fn bridge_meta(&self, s: &Spawn, pid: Option<u32>) -> bro_bridge::SessionMeta {
        bro_bridge::SessionMeta {
            id: self.meta.sid.clone(),
            title: self.display_name(),
            shell: s.program.clone(),
            args: s.args.clone(),
            cwd: Some(self.meta.cwd.clone()),
            project: Some(PathBuf::from(&self.meta.project.key)),
            pid,
            cols: self.size.1,
            rows: self.size.0,
            agent: self.meta.harness.map(|h| h.label().to_string()),
        }
    }

    /// Re-announce to a bridge that started after this session.
    pub fn reregister(&self) {
        if let Some(l) = &self.live {
            let s = Spawn { program: self.meta.harness.map(|h| h.label().to_string()).unwrap_or_else(|| "shell".into()), ..Spawn::default() };
            self.svc.bridge_register(self.bridge_meta(&s, l.child.process_id()));
        }
    }

    fn start_reader(&mut self, mut reader: Box<dyn Read + Send>, waker: Waker) {
        let parser = self.parser.clone();
        let program_state = self.program_state.clone();
        let writer = self.writer.clone();
        let exited = self.exited.clone();
        let (last_output, born) = (self.last_output.clone(), self.born);
        let svc = self.svc.clone();
        let sid = self.meta.sid.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16384];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        let reply = {
                            let mut p = parser.lock();
                            p.process(data);
                            *program_state.lock() = ProgramState::of(&p);
                            // Device Status Report: ConPTY asks where the cursor is and waits for the answer
                            contains(data, b"\x1b[6n").then(|| {
                                let (r, c) = p.screen().cursor_position();
                                format!("\x1b[{};{}R", r + 1, c + 1)
                            })
                        }; // parser lock released before writing the reply
                        if let Some(reply) = reply
                            && let Some(w) = writer.lock().as_mut() {
                                let _ = w.write_all(reply.as_bytes());
                                let _ = w.flush();
                            }
                        svc.bridge_output(&sid, data);
                        last_output.store(born.elapsed().as_millis() as u64, Ordering::Relaxed);
                        waker.wake();
                    }
                }
            }
            exited.store(true, Ordering::SeqCst);
            waker.wake();
        });
    }

    /// Write raw bytes to the program (keys, paste, bridge input).
    pub fn send(&mut self, bytes: &[u8]) {
        self.typed += 1;
        self.scroll = 0;
        self.write(bytes);
    }

    /// Write to the program on bro's own behalf.
    fn write(&mut self, bytes: &[u8]) {
        #[cfg(test)]
        self.sent.extend_from_slice(bytes);
        if let Some(w) = self.writer.lock().as_mut() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    /// While the agent waits on a numbered choice ("❯ 1. Yes"), the key that picks the option on screen row
    /// `row`. Only live at the bottom (not scrolled back) and only while bro sees a prompt, so ordinary
    /// numbered output never becomes clickable.
    pub fn prompt_option_at(&self, row: u16) -> Option<String> {
        if self.scroll != 0 || crate::pane::Pane::activity(self) != Some(Activity::Blocked) {
            return None;
        }
        let p = self.parser.lock();
        let s = p.screen();
        let (_, cols) = s.size();
        let line = s.rows(0, cols).nth(row.saturating_add(self.view_top) as usize)?;
        prompt_option(&line)
    }

    /// The local size for the pty (the tile, or a hidden pane's remote request). The pty gets
    /// this grown to the remote minimum while a phone or web client watches the session.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 {
            return;
        }
        self.base = (rows, cols);
        self.apply_size();
    }

    /// Size the pty for `base` plus the remote minimum (held for its grace period).
    fn apply_size(&mut self) {
        let min = self.svc.remote_min();
        let viewers = self.remote_viewers();
        let active = self.hold.active(viewers, self.now(), min.grace);
        let (rows, cols) = super::remote_min::effective(self.base, min, active);
        self.set_pty_size(rows, cols);
    }

    fn remote_viewers(&self) -> usize {
        #[cfg(test)]
        if let Some(n) = self.test_viewers {
            return n;
        }
        self.svc.bridge_remote_viewers(&self.meta.sid)
    }

    fn now(&self) -> Instant {
        #[cfg(test)]
        return Instant::now() + self.test_skew;
        #[cfg(not(test))]
        Instant::now()
    }

    /// Sized by its tile: a running program (fixed panes keep the transcript's size).
    fn sized(&self) -> bool {
        #[cfg(test)]
        if self.test_sized {
            return true;
        }
        self.live.is_some()
    }

    /// Tests: more output for a fixed pane.
    #[cfg(test)]
    pub fn feed(&self, bytes: &[u8]) {
        let mut parser = self.parser.lock();
        parser.process(bytes);
        *self.program_state.lock() = ProgramState::of(&parser);
    }

    #[cfg(test)]
    pub(crate) fn parser_for_test(&self) -> Arc<Mutex<Parser>> {
        self.parser.clone()
    }

    /// The pty is bigger than its local size because a remote client watches (or did within the grace).
    pub fn remote_grown(&self) -> bool {
        self.size.0 > self.base.0 || self.size.1 > self.base.1
    }

    /// Resize the pty and the parser (no-op if unchanged).
    fn set_pty_size(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == self.size || rows == 0 || cols == 0 {
            return;
        }
        self.size = (rows, cols);
        if let Some(l) = &self.live {
            let _ = l.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
        self.parser.lock().screen_mut().set_size(rows, cols);
        self.svc.bridge_resize(&self.meta.sid, cols, rows);
    }

    /// (rows, cols) of the pty.
    #[cfg(test)]
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Kill the program and what it started (bridge kill, close, quit): console programs in its
    /// process tree end with it; GUI apps it opened stay (see `bro_core::proc_tree`).
    pub fn kill(&mut self) {
        if self.exit_code().is_some() {
            return;
        }
        if let Some(l) = &mut self.live {
            if let Some(pid) = l.child.process_id() {
                bro_core::proc_tree::kill_tree(pid);
            }
            let _ = l.child.kill();
        }
    }

    /// Exit code, once exited.
    pub fn exit_code(&mut self) -> Option<i32> {
        if self.exit_status.is_none() {
            self.exit_status = self.live.as_mut().and_then(|l| l.child.try_wait().ok().flatten()).map(|s| s.exit_code() as i32);
            if self.exit_status.is_some()
                && let Some(pid) = self.live.as_ref().and_then(|l| l.child.process_id())
            {
                bro_core::proc_tree::untrack(pid);
            }
        }
        self.exit_status
    }

    /// A program that failed right after starting closes before you can read why: its last lines on screen.
    pub fn early_failure(&mut self) -> Option<String> {
        if self.born.elapsed() > Duration::from_secs(15) || self.exit_code().is_none_or(|c| c == 0) {
            return None;
        }
        let p = self.parser.lock();
        let s = p.screen();
        let rows: Vec<String> = s.rows(0, s.size().1).map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
        let tail = rows[rows.len().saturating_sub(2)..].join(" ");
        (!tail.is_empty()).then(|| crate::ui::fit(&tail, 200))
    }

    /// Where this session's file links resolve: its cwd, then its project root.
    pub fn file_roots(&self) -> bro_bridge::file_preview::ResolveRoots {
        self.files.roots().clone()
    }

    /// Tests: answer the file-link candidates seen so far without the worker thread.
    #[cfg(test)]
    pub fn resolve_file_links_now(&self) {
        self.files.resolve_queued_now();
    }

    /// The name shown everywhere: your rename, else the launch label.
    pub fn display_name(&self) -> String {
        self.display_title().unwrap_or_else(|| self.meta.label.clone())
    }

    /// What to call this session: your name for it, else the title the program set (Claude Code keeps it
    /// on the current task), else its starting title. None = fall back to login · model.
    pub fn display_title(&self) -> Option<String> {
        if self.meta.renamed {
            return self.meta.name.clone();
        }
        self.agent_title().or_else(|| self.meta.name.clone())
    }

    /// The program's window title (OSC 0/2), cleaned: spinner / status glyphs off the front, and nothing
    /// that's just the program's own name or path.
    pub fn agent_title(&self) -> Option<String> {
        clean_title(&self.program_title()?)
    }

    /// The program's own window title (OSC 0/2), e.g. Claude Code's task summary.
    pub fn program_title(&self) -> Option<String> {
        self.program_state.lock().title.clone()
    }

    /// Read the bottom of the screen for an agent's tell-tale lines: "esc to interrupt" while it works, a
    /// permission prompt or question when it's waiting on you.
    fn scan(&mut self) {
        let (text, question) = {
            let Some(p) = self.parser.try_lock() else { return };
            let s = p.screen();
            let (_, cols) = s.size();
            let rows: Vec<String> = s.rows(0, cols).collect();
            let tail: Vec<&String> = rows.iter().rev().filter(|r| !r.trim().is_empty()).take(14).collect();
            (tail.iter().rev().map(|r| r.to_lowercase()).collect::<Vec<_>>().join("\n"), prompt_question(&tail))
        };
        let working = ["esc to interrupt", "esc to cancel", "ctrl+c to interrupt", "ctrl-c to interrupt"].iter().any(|m| text.contains(m));
        let blocked = ["do you want to", "do you want me to", "allow this", "allow once", "approve this", "(y/n)", "[y/n]", "❯ 1. yes", "› 1. yes", "enter to confirm", "enter to select", "waiting for your approval"]
            .iter()
            .any(|m| text.contains(m));
        if working || blocked {
            self.agent = true;
        }
        if !self.agent {
            return;
        }
        let quiet_ms = (self.born.elapsed().as_millis() as u64).saturating_sub(self.last_output.load(Ordering::Relaxed));
        self.prompt_title = if blocked { question } else { None };
        self.status = Some(if blocked {
            Activity::Blocked
        } else if working || quiet_ms < 1500 {
            Activity::Working
        } else {
            Activity::Idle
        });
    }

    /// Retry an agent that stopped on an API error: see [`super::auto_resume`].
    fn auto_resume(&mut self, cx: &mut Cx) {
        if let Some(at) = self.enter_at
            && Instant::now() >= at
        {
            self.enter_at = None;
            self.write(b"\r");
        }
        // agents only: a shell that printed "(y/n)" once never gets `continue` typed into it
        if !self.agent || self.meta.harness.is_none() || self.svc.is_demo() || self.enter_at.is_some() {
            return;
        }
        let cfg = self.svc.settings().auto_resume;
        let now = chrono::Local::now();
        let quiet_ms = (self.born.elapsed().as_millis() as u64).saturating_sub(self.last_output.load(Ordering::Relaxed));
        let seen = super::auto_resume::Seen { activity: self.activity(), quiet: Duration::from_millis(quiet_ms), stall: self.stall(now), typed: self.typed };
        for step in self.resume.step(now, &seen, &cfg) {
            match step {
                super::auto_resume::Step::Send(text) => {
                    self.write(text.as_bytes());
                    self.enter_at = Some(Instant::now() + Duration::from_millis(250));
                }
                super::auto_resume::Step::Toast(kind, text) => cx.toast(kind, format!("{} · {text}", self.display_name())),
            }
        }
    }

    /// Answer Yes to a Claude permission prompt ("Do you want to proceed?") when the setting is on.
    fn auto_approve(&mut self, cx: &mut Cx) {
        if self.meta.harness != Some(Harness::Claude) || self.svc.is_demo() || self.status != Some(Activity::Blocked) || !self.svc.settings().auto_approve {
            return;
        }
        // let the prompt finish drawing, and give the last answer time to clear it
        let quiet_ms = (self.born.elapsed().as_millis() as u64).saturating_sub(self.last_output.load(Ordering::Relaxed));
        if quiet_ms < 300 || self.approved_at.is_some_and(|t| t.elapsed() < Duration::from_millis(1500)) {
            return;
        }
        let rows: Vec<String> = {
            let Some(p) = self.parser.try_lock() else { return };
            let (_, cols) = p.screen().size();
            p.screen().rows(0, cols).collect()
        };
        let Some(what) = permission_prompt(&rows) else { return };
        self.approved_at = Some(Instant::now());
        self.write(b"1\r");
        cx.toast(crate::alerts::Kind::Info, format!("{} · approved: {what}", self.display_name()));
    }

    /// The API error the agent stopped on: from the transcript when bro reads one (Codex doesn't always
    /// record them, so its screen counts too), else from the screen.
    fn stall(&self, now: chrono::DateTime<chrono::Local>) -> Option<super::auto_resume::Stall> {
        use super::auto_resume::{Stall, classify, screen_stall};
        let facts = self.insights().and_then(|s| s.facts);
        if let Some(f) = &facts {
            // an error left over from before this pane started (a resumed session) isn't this pane's to retry
            let born_ms = chrono::Utc::now().timestamp_millis() - self.meta.started.elapsed().as_millis() as i64;
            if let Some(e) = f.api_error.as_ref().filter(|e| e.at_ms.is_none_or(|t| t >= born_ms)) {
                return Some(Stall { key: format!("t:{}", e.id), text: e.text.clone(), cause: classify(&e.text, e.code.as_deref(), now)?, below: None });
            }
            if f.harness == Harness::Claude {
                return None;
            }
        }
        let rows: Vec<String> = {
            let p = self.parser.try_lock()?;
            let (_, cols) = p.screen().size();
            p.screen().rows(0, cols).collect()
        };
        screen_stall(&rows, now)
    }

    /// What auto-resume is waiting on, for headers and the sidebar ("↻ retry 2 in 14s").
    pub fn resume_note(&self) -> Option<String> {
        self.resume.note(chrono::Local::now())
    }

    /// Cleanup to run when the session ends: bridge exit/unregister. (Route + staged files: see the app.)
    pub fn on_exit(&mut self) {
        let code = self.exit_code();
        self.svc.bridge_exit(&self.meta.sid, code);
        self.svc.bridge_unregister(&self.meta.sid);
    }
}

/// `.cmd`/`.bat` shims (npm installs on Windows) run through cmd.exe; everything else resolves on PATH.
pub(crate) fn resolve(program: &str) -> (String, Vec<String>) {
    let path = crate::util::which(program).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| program.to_string());
    let lower = path.to_lowercase();
    if cfg!(windows) && (lower.ends_with(".cmd") || lower.ends_with(".bat")) {
        return ("cmd.exe".into(), vec!["/c".into(), path]);
    }
    (path, vec![])
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// A session colour in the current theme: default fg/bg follow a painted theme background, the 16 ANSI colours
/// come from its palette, ANSI white text is darkened on light themes, and on painted dark themes the greys CLIs
/// shade with lean toward the theme's own background and text.
fn themed(c: vt100::Color, fg: bool, t: &crate::theme::Theme) -> Color {
    let painted = t.painted();
    let dark_painted = painted && !t.is_light();
    // a neutral grey (r ≈ g ≈ b) as 0..1 lightness
    let grey = |r: u8, g: u8, b: u8| (r.max(g).max(b) - r.min(g).min(b) < 18).then(|| (r as f32 + g as f32 + b as f32) / 765.0);
    match c {
        vt100::Color::Default if painted => if fg { t.fg } else { t.bg },
        vt100::Color::Idx(7 | 15) if fg && t.is_light() => t.fg,
        vt100::Color::Rgb(r, g, b) if fg && t.is_light() && (r as u32 + g as u32 + b as u32) > 690 => t.fg,
        // dark-mode CLIs shade blocks with dark greys / dark tints: flip those to light tints
        vt100::Color::Idx(i @ 232..=243) if !fg && t.is_light() => Color::Indexed(255 - (i - 232)),
        vt100::Color::Idx(0 | 8) if !fg && t.is_light() => crate::theme::mix(t.bg, t.fg, 0.08),
        vt100::Color::Rgb(r, g, b) if !fg && t.is_light() && (r as u32 + g as u32 + b as u32) < 240 => {
            crate::theme::mix(Color::Rgb(r, g, b), t.bg, 0.82)
        }
        vt100::Color::Idx(i @ 0..=15) if t.ansi.is_some() => t.ansi.map_or(Color::Indexed(i), |a| a[i as usize]),
        vt100::Color::Idx(i @ 232..=255) if dark_painted => toward_theme((8.0 + 10.0 * (i - 232) as f32) / 255.0, t),
        vt100::Color::Rgb(r, g, b) if dark_painted && grey(r, g, b).is_some() => toward_theme(grey(r, g, b).unwrap_or(0.0), t),
        other => color(other),
    }
}

/// A grey of `lightness` (0..1, as on a black terminal) re-cast between the theme's background and text.
fn toward_theme(lightness: f32, t: &crate::theme::Theme) -> Color {
    crate::theme::mix(t.bg, t.fg, ((lightness - 0.04) / 0.86).clamp(0.0, 1.0))
}

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        if let Some(i) = &self.insights {
            i.stop();
        }
        self.kill();
    }
}

/// The prompt's question among the bottom rows (newest first): the nearest line asking something, box
/// drawing and option markers trimmed.
fn prompt_question(tail_newest_first: &[&String]) -> Option<String> {
    tail_newest_first
        .iter()
        .map(|r| r.trim_matches(|c: char| c.is_whitespace() || matches!(c, '│' | '╭' | '╰' | '─' | '╮' | '╯' | '┃')))
        .find(|r| r.ends_with('?') && prompt_option(r).is_none())
        .map(str::to_string)
}

/// Status glyph + word + colour for an activity.
pub fn status_look(a: Option<Activity>, done: bool, t: &Theme, time: f64) -> Option<(&'static str, &'static str, Color)> {
    Some(match a? {
        Activity::Blocked => ("●", "needs you", t.danger),
        Activity::Working => (["◐", "◓", "◑", "◒"][(time * 6.0) as usize % 4], "working", t.accent),
        Activity::Idle if done => ("●", "done", t.good),
        Activity::Idle => ("○", "idle", t.muted),
    })
}

impl Pane for Term {
    fn title(&self) -> String {
        self.display_name()
    }
    fn icon(&self) -> &'static str {
        "term"
    }
    fn header(&self, t: &Theme, time: f64) -> (Line<'static>, Option<Line<'static>>) {
        let brand = crate::ui::harness_color(self.meta.harness, t);
        let cwd = crate::util::short_path(&self.meta.cwd, 32);
        let mut spans = vec![Span::raw(" "), Span::styled(format!("{} ", crate::ui::harness_glyph(self.meta.harness)), Style::default().fg(brand))];
        if let Some(n) = self.display_title() {
            spans.push(Span::styled(format!("{n}  "), Style::default().add_modifier(Modifier::BOLD)));
        }
        // "claude · work · opus-5" → "work · opus-5": the logo already says which agent
        let label = match self.meta.harness {
            Some(h) => self.meta.label.strip_prefix(&format!("{} · ", h.label())).unwrap_or(&self.meta.label).to_string(),
            None => self.meta.label.clone(),
        };
        spans.push(Span::styled(label, Style::default().fg(brand).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" — {cwd} "), Style::default().fg(t.muted)));
        let mut right = vec![];
        if let Some(l) = self.meta.caveman {
            right.push(Span::styled(format!(" {} {} ", crate::ui::icon("caveman"), l.id()), Style::default().fg(t.inline)));
        }
        if self.scroll > 0 {
            right.push(Span::styled(format!(" ↑{} ", self.scroll), Style::default().fg(t.inline)));
        }
        if self.remote_grown() {
            // the tile shows the bottom-left of a bigger screen while a phone / web client watches
            right.push(Span::styled(format!(" ⇲ {}×{} remote ", self.size.1, self.size.0), Style::default().fg(t.muted)));
        }
        if let Some(note) = self.resume_note() {
            right.push(Span::styled(format!(" {note} "), Style::default().fg(t.inline)));
        }
        if let Some(d) = self.svc.watchdog(&self.meta.sid) {
            const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];
            let (done, total) = d.plan.progress();
            let (text, c) = match (&d.running, &d.finished, d.paused) {
                (Some(_), _, _) => (format!("{} watchdog reviewing", SPIN[(time * 6.0) as usize % 4]), t.accent),
                (None, Some(_), _) => ("✓ watchdog done".to_string(), t.good),
                (None, None, true) => ("‖ watchdog paused".to_string(), t.muted),
                (None, None, false) => (format!("{} watchdog {done}/{total}", crate::ui::icon("watchdog")), t.shine),
            };
            right.push(Span::styled(format!(" {text} "), Style::default().fg(c)));
        }
        if let Some((g, word, c)) = status_look(self.activity(), false, t, time) {
            right.push(Span::styled(format!(" {g} {word} "), Style::default().fg(c).add_modifier(Modifier::BOLD)));
        }
        (Line::from(spans), (!right.is_empty()).then(|| Line::from(right)))
    }
    fn is_terminal(&self) -> bool {
        true
    }
    fn alive(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }
    fn activity(&self) -> Option<Activity> {
        let screen = self.demo_activity.or(self.status);
        // A question tool waiting for its answer means the agent needs you, whatever the screen
        // markers say (Claude's question menu shows "esc to cancel", which reads as working).
        if self.demo_activity.is_none() && screen.is_some() && screen != Some(Activity::Blocked) && self.asking_question() {
            return Some(Activity::Blocked);
        }
        screen
    }
    fn busy(&self) -> bool {
        // Claude / Codex say so themselves; otherwise the screen markers and the output timeout
        if self.demo_activity.is_none()
            && let Some(b) = self.insights.as_ref().and_then(|i| i.busy())
        {
            return b && self.activity() != Some(Activity::Blocked);
        }
        self.activity() == Some(Activity::Working)
    }
    fn wants_mouse(&self) -> bool {
        self.program_state.lock().mouse_mode != vt100::MouseProtocolMode::None
    }
    fn tick_every(&self) -> Option<Duration> {
        // Watch every running process, including hidden shell tabs. ConPTY may keep its
        // output pipe open after the child exits, so EOF alone cannot drive tab cleanup.
        (self.agent || self.live.is_some()).then_some(Duration::from_millis(400))
    }
    fn poll(&mut self, cx: &mut Cx) {
        if self.exit_code().is_some() {
            self.exited.store(true, Ordering::SeqCst);
            return;
        }
        // hidden panes are never rendered: start and end the remote minimum from here too
        if self.sized() {
            self.apply_size();
        }
        if self.last_scan.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_scan = Instant::now();
        self.scan();
        self.auto_approve(cx);
        self.auto_resume(cx);
        // shells title their window with their own exe path — that's noise, keep bro's name for them
        let title = Some(self.display_name());
        if title.is_some() && title != self.title_sent {
            if let Some(t) = &title {
                self.svc.bridge_title(&self.meta.sid, t);
            }
            self.title_sent = title;
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        if area.width == 0 || area.height == 0 {
            self.links.clear();
            return;
        }
        self.ensure_started(area.height, area.width, cx.waker());
        if let Some(e) = &self.error {
            let lines = [format!("  {e}"), String::new(), "  enter or esc closes this pane".to_string()];
            for (i, l) in lines.iter().enumerate().take(area.height as usize) {
                let st = if i == 0 { Style::default().fg(cx.theme.danger) } else { Style::default().fg(cx.theme.muted) };
                crate::ui::line(f, Rect { y: area.y + i as u16, height: 1, ..area }, vec![Span::styled(l.clone(), st)]);
            }
            return;
        }
        if self.sized() {
            self.resize(area.height, area.width);
        }
        // A pty grown past the tile for a remote viewer shows its bottom-left (pinned to the end of
        // the content: right after growing, the rows below it are still blank until the program
        // redraws). Scrolling up first reveals the screen rows above, then the scrollback.
        let Some(mut p) = self.parser.try_lock() else {
            if let Some(cache) = &self.frame_cache {
                let buf = f.buffer_mut();
                for row in 0..area.height.min(cache.area.height) {
                    for col in 0..area.width.min(cache.area.width) {
                        buf[(area.x + col, area.y + row)] = cache[(col, row)].clone();
                    }
                }
                if cx.focused && self.scroll == 0
                    && let Some((row, col)) = self.cached_cursor.filter(|(r, c)| *r < area.height && *c < area.width)
                {
                    f.set_cursor_position(Position { x: area.x + col, y: area.y + row });
                }
            }
            return;
        };
        let hidden = if self.size.0 > self.base.0 && self.size.0 > area.height {
            p.screen_mut().set_scrollback(0);
            let s = p.screen();
            let (rows, cols) = s.size();
            let last_text = s.rows(0, cols).enumerate().filter(|(_, r)| !r.trim().is_empty()).map(|(i, _)| i as u16).last().unwrap_or(0);
            let end = last_text.max(s.cursor_position().0) + 1;
            end.saturating_sub(area.height).min(rows.saturating_sub(area.height)) as usize
        } else {
            0
        };
        p.screen_mut().set_scrollback(self.scroll.saturating_sub(hidden));
        if self.scroll > hidden {
            self.scroll = hidden + p.screen().scrollback(); // clamped to what exists
        }
        self.view_top = (hidden - self.scroll.min(hidden)) as u16;
        let top = self.view_top;
        let screen = p.screen();
        let mut files = self.files.lookup();
        self.links = super::links::detect_with(screen, &mut |m| files.get(m));
        files.finish(Some(cx.waker()));
        let buf = f.buffer_mut();
        for row in 0..area.height {
            for col in 0..area.width {
                let Some(cell) = screen.cell(top + row, col) else { continue };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut m = Modifier::empty();
                if cell.bold() {
                    m |= Modifier::BOLD;
                }
                if cell.dim() {
                    m |= Modifier::DIM;
                }
                if cell.italic() {
                    m |= Modifier::ITALIC;
                }
                if cell.underline() || self.links.contains_key(&(top + row, col)) {
                    m |= Modifier::UNDERLINED;
                }
                if cell.inverse() {
                    m |= Modifier::REVERSED;
                }
                let style = Style::default().fg(themed(cell.fgcolor(), true, cx.theme)).bg(themed(cell.bgcolor(), false, cx.theme)).add_modifier(m);
                let s = cell.contents();
                if let Some(bc) = buf.cell_mut(Position { x: area.x + col, y: area.y + row }) {
                    bc.set_symbol(if s.is_empty() { " " } else { s });
                    bc.set_style(style);
                }
            }
        }
        self.cached_cursor = None;
        if !screen.hide_cursor() && self.scroll == 0 && self.live.is_some() {
            let (r, c) = screen.cursor_position();
            if r >= top && r - top < area.height && c < area.width {
                self.cached_cursor = Some((r - top, c));
                if cx.focused {
                    f.set_cursor_position(Position { x: area.x + c, y: area.y + r - top });
                }
            }
        }
        drop(p);
        let local = Rect { x: 0, y: 0, ..area };
        if self.frame_cache.as_ref().is_none_or(|cache| cache.area != local) {
            self.frame_cache = Some(ratatui::buffer::Buffer::empty(local));
        }
        let cache = self.frame_cache.as_mut().unwrap();
        let buf = f.buffer_mut();
        for row in 0..area.height {
            for col in 0..area.width {
                cache[(col, row)] = buf[(area.x + col, area.y + row)].clone();
            }
        }
    }

    fn link_at(&self, row: u16, col: u16) -> Option<String> {
        self.links.get(&(row.saturating_add(self.view_top), col)).map(|uri| uri.to_string())
    }

    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        if self.error.is_some() {
            if matches!(key.code, crossterm::event::KeyCode::Enter | crossterm::event::KeyCode::Esc) {
                cx.act(crate::pane::Action::Close);
            }
            return true;
        }
        let app_cursor = self.program_state.lock().app_cursor;
        if let Some(bytes) = encode_key(key, app_cursor) {
            self.send(&bytes);
        }
        true
    }

    fn paste(&mut self, text: &str, _cx: &mut Cx) {
        let bracketed = self.program_state.lock().bracketed_paste;
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            // an end marker inside the text would close the paste early and the rest would be typed
            let text = text.replace("\x1b[201~", "");
            self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.send(text.as_bytes());
        }
    }

    fn mouse(&mut self, ev: MouseEvent, area: Rect, _cx: &mut Cx) {
        let (mode, enc) = {
            let state = self.program_state.lock();
            (state.mouse_mode, state.mouse_encoding)
        };
        if mode == vt100::MouseProtocolMode::None {
            // the program doesn't want the mouse: the wheel scrolls our scrollback
            match ev.kind {
                MouseEventKind::ScrollUp => self.scroll += 3,
                MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(3),
                _ => {}
            }
            return;
        }
        // the program sees screen rows: add the rows hidden above a remote-grown tile
        let ev = MouseEvent { row: ev.row.saturating_add(self.view_top), ..ev };
        if let Some(bytes) = encode_mouse(ev, area, mode, enc) {
            self.send(&bytes);
        }
    }

    fn as_term(&mut self) -> Option<&mut Term> {
        Some(self)
    }
    fn as_term_ref(&self) -> Option<&Term> {
        Some(self)
    }
}

/// A window title worth showing: leading spinner / status glyphs ("✳ ", "⠂ ") stripped; a bare program name
/// or path (what shells and freshly started agents set) is not.
fn clean_title(raw: &str) -> Option<String> {
    let t = raw.trim().trim_start_matches(|c: char| !c.is_alphanumeric() && c != '(' && c != '[' && c != '"' && c != '\'' && c != '~' && c != '/').trim();
    if t.is_empty() || looks_like_exe_path(raw.trim()) || looks_like_exe_path(t) {
        return None;
    }
    const GENERIC: [&str; 12] = ["claude", "claude code", "codex", "openai codex", "pi", "omp", "pwsh", "powershell", "windows powershell", "cmd", "bash", "zsh"];
    let lower = t.to_lowercase();
    if GENERIC.contains(&lower.as_str()) || lower.starts_with("administrator:") {
        return None;
    }
    Some(t.to_string())
}

/// "C:\Program Files\PowerShell\pwsh.exe", "/usr/bin/bash" — a program path, not a useful title.
fn looks_like_exe_path(t: &str) -> bool {
    let t = t.trim();
    let lower = t.to_ascii_lowercase();
    (lower.ends_with(".exe") || t.starts_with('/')) && !t.contains(' ') || lower.ends_with(".exe") && (t.contains(":\\") || t.contains(":/"))
}

#[cfg(test)]
mod title_tests {
    #[test]
    fn titles_are_cleaned() {
        assert_eq!(super::clean_title("✳ Fix the flaky workout timer").as_deref(), Some("Fix the flaky workout timer"));
        assert_eq!(super::clean_title("⠂ Refactor launcher").as_deref(), Some("Refactor launcher"));
        assert_eq!(super::clean_title("✳ Claude Code"), None);
        assert_eq!(super::clean_title(r"C:\Program Files\PowerShell\7\pwsh.exe"), None);
        assert_eq!(super::clean_title("codex"), None);
        assert_eq!(super::clean_title("  "), None);
    }

    #[test]
    fn exe_paths_are_not_titles() {
        assert!(super::looks_like_exe_path(r"C:\Program Files\PowerShell\7\pwsh.exe"));
        assert!(super::looks_like_exe_path("/usr/bin/bash"));
        assert!(!super::looks_like_exe_path("✳ Fix the flaky test"));
    }
}

/// A Claude permission prompt at the bottom of the screen: a "Do you want to …?" question with "1. Yes"
/// under it. Returns what it's about (the line above the question, else the question). Numbered questions
/// Claude asks you (AskUserQuestion) don't offer a bare "Yes" first, so they're left alone.
pub fn permission_prompt(rows: &[String]) -> Option<String> {
    let clean = |r: &String| r.trim_matches(|c: char| c.is_whitespace() || matches!(c, '│' | '╭' | '╰' | '─' | '╮' | '╯' | '┃')).to_string();
    let tail: Vec<String> = rows.iter().rev().map(clean).filter(|r| !r.is_empty()).take(14).collect();
    let q = tail.iter().position(|r| r.to_lowercase().starts_with("do you want to") && r.ends_with('?'))?;
    let yes = tail[..q].iter().any(|r| {
        let rest = r.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '❯' | '>' | '›'));
        rest.strip_prefix("1.").is_some_and(|l| l.trim() == "Yes")
    });
    yes.then(|| tail.get(q + 1).filter(|r| prompt_option(r).is_none()).unwrap_or(&tail[q]).clone())
}

/// The option number on a prompt row: "❯ 1. Yes", "  2. Yes, and don't ask again", "› 3. No" -> "1"/"2"/"3".
/// The same rows the bridge offers the phone as prompt options.
pub fn prompt_option(line: &str) -> Option<String> {
    let rest = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '❯' | '>' | '›' | '●' | '○' | '│'));
    let (number, label) = rest.split_once('.')?;
    (!number.is_empty() && number.len() <= 2 && number.chars().all(|c| c.is_ascii_digit()) && !label.trim().is_empty()).then(|| number.to_string())
}

#[cfg(test)]
mod prompt_option_tests {
    use super::{permission_prompt, prompt_option};

    #[test]
    fn permission_prompts_are_told_from_questions() {
        let rows = |s: &str| s.lines().map(str::to_string).collect::<Vec<_>>();
        let rm = rows(" Bash command\n\n   rm -rf F:\\X\\*\n\n Dangerous rm operation on statically-unresolvable target: F:\\X\\*\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently (esc)\n\n");
        assert_eq!(permission_prompt(&rm).as_deref(), Some(r"Dangerous rm operation on statically-unresolvable target: F:\X\*"));
        let ask = rows(" Which database?\n ❯ 1. Postgres\n   2. SQLite\n");
        assert_eq!(permission_prompt(&ask), None);
        let me = rows(" Do you want me to keep going?\n ❯ 1. Yes\n   2. No\n");
        assert_eq!(permission_prompt(&me), None, "Claude asking, not a permission check");
        assert_eq!(permission_prompt(&rows(" Do you want to proceed?\n")), None, "no options drawn yet");
    }

    #[test]
    fn option_rows_give_their_number() {
        assert_eq!(prompt_option(" ❯ 1. Yes").as_deref(), Some("1"));
        assert_eq!(prompt_option("   2. Yes, and don't ask again this session").as_deref(), Some("2"));
        assert_eq!(prompt_option("│ › 3. No, and tell Claude what to do differently │").as_deref(), Some("3"));
        assert_eq!(prompt_option("Do you want to proceed?"), None);
        assert_eq!(prompt_option("  1."), None);
        assert_eq!(prompt_option("v1.2 released"), None);
        assert_eq!(prompt_option("2024. A year"), None, "long numbers are not options");
    }
}
