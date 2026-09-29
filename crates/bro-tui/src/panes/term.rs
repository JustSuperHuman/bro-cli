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
    /// A name you gave it (sidebar `r`, prefix `,`).
    pub name: Option<String>,
    pub cwd: PathBuf,
    pub project: ProjectKey,
    pub started: Instant,
    /// Proxy route to drop when it exits.
    pub route_id: Option<String>,
    /// Staged files to delete when it exits.
    pub cleanup: Vec<PathBuf>,
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
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    exited: Arc<AtomicBool>,
    /// rows, cols
    size: (u16, u16),
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
    svc: Services,
    title_sent: Option<String>,
}

impl Term {
    /// A session that spawns lazily at its first render (when the real size is known).
    pub fn new(meta: Meta, spawn: Spawn, svc: Services) -> Term {
        let agent = meta.harness.is_some();
        Term {
            meta,
            spawn: Some(spawn),
            live: None,
            parser: Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(24, 80, 10_000, Cb::default()))),
            writer: Arc::new(Mutex::new(None)),
            exited: Arc::new(AtomicBool::new(false)),
            size: (24, 80),
            scroll: 0,
            last_output: Arc::new(AtomicU64::new(0)),
            born: Instant::now(),
            status: agent.then_some(Activity::Idle),
            agent,
            last_scan: Instant::now(),
            error: None,
            demo_activity: None,
            svc,
            title_sent: None,
        }
    }

    /// A pane that never spawns: shows `transcript` (ANSI) as its screen. For snapshots and tests.
    pub fn fixed(meta: Meta, transcript: &str, svc: Services) -> Term {
        let mut t = Term::new(meta, Spawn::default(), svc);
        t.spawn = None;
        t.parser.lock().process(transcript.as_bytes());
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
        *self.writer.lock() = Some(writer);
        self.parser.lock().screen_mut().set_size(rows, cols);
        self.size = (rows, cols);
        let pid = child.process_id();
        self.live = Some(Live { master: pair.master, child });
        self.svc.bridge_register(self.bridge_meta(s, pid));
        self.start_reader(reader, waker);
        Ok(())
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
        self.scroll = 0;
        if let Some(w) = self.writer.lock().as_mut() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    /// Resize the pty and the parser (no-op if unchanged).
    pub fn resize(&mut self, rows: u16, cols: u16) {
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

    /// Kill the program (bridge kill, close).
    pub fn kill(&mut self) {
        if let Some(l) = &mut self.live {
            let _ = l.child.kill();
        }
    }

    /// Exit code, once exited.
    pub fn exit_code(&mut self) -> Option<i32> {
        self.live.as_mut().and_then(|l| l.child.try_wait().ok().flatten()).map(|s| s.exit_code() as i32)
    }

    /// The name shown everywhere: your rename, else the launch label.
    pub fn display_name(&self) -> String {
        self.meta.name.clone().unwrap_or_else(|| self.meta.label.clone())
    }

    /// The program's own window title (OSC 0/2), e.g. Claude Code's task summary.
    pub fn program_title(&self) -> Option<String> {
        self.parser.lock().callbacks().title.clone()
    }

    /// Read the bottom of the screen for an agent's tell-tale lines: "esc to interrupt" while it works, a
    /// permission prompt or question when it's waiting on you.
    fn scan(&mut self) {
        let text = {
            let p = self.parser.lock();
            let s = p.screen();
            let (_, cols) = s.size();
            let rows: Vec<String> = s.rows(0, cols).collect();
            let tail: Vec<&String> = rows.iter().rev().filter(|r| !r.trim().is_empty()).take(14).collect();
            tail.iter().rev().map(|r| r.to_lowercase()).collect::<Vec<_>>().join("\n")
        };
        let working = ["esc to interrupt", "esc to cancel", "ctrl+c to interrupt", "ctrl-c to interrupt"].iter().any(|m| text.contains(m));
        let blocked = ["do you want to", "do you want me to", "allow this", "allow once", "approve this", "(y/n)", "[y/n]", "❯ 1. yes", "› 1. yes", "press enter to confirm", "waiting for your approval"]
            .iter()
            .any(|m| text.contains(m));
        if working || blocked {
            self.agent = true;
        }
        if !self.agent {
            return;
        }
        let quiet_ms = (self.born.elapsed().as_millis() as u64).saturating_sub(self.last_output.load(Ordering::Relaxed));
        self.status = Some(if blocked {
            Activity::Blocked
        } else if working || quiet_ms < 1500 {
            Activity::Working
        } else {
            Activity::Idle
        });
    }

    /// Cleanup to run when the session ends: bridge exit/unregister. (Route + staged files: see the app.)
    pub fn on_exit(&mut self) {
        let code = self.exit_code();
        self.svc.bridge_exit(&self.meta.sid, code);
        self.svc.bridge_unregister(&self.meta.sid);
    }
}

/// `.cmd`/`.bat` shims (npm installs on Windows) run through cmd.exe; everything else resolves on PATH.
fn resolve(program: &str) -> (String, Vec<String>) {
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

/// A session colour in the current theme: default fg/bg follow a painted theme background, and ANSI white text
/// is darkened on light themes.
fn themed(c: vt100::Color, fg: bool, t: &crate::theme::Theme) -> Color {
    let painted = !matches!(t.bg, Color::Reset);
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
        other => color(other),
    }
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
        self.kill();
    }
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
        if let Some(n) = &self.meta.name {
            spans.push(Span::styled(format!("{n} · "), Style::default().add_modifier(Modifier::BOLD)));
        }
        // "claude · work · opus-5" → "work · opus-5": the logo already says which agent
        let label = match self.meta.harness {
            Some(h) => self.meta.label.strip_prefix(&format!("{} · ", h.label())).unwrap_or(&self.meta.label).to_string(),
            None => self.meta.label.clone(),
        };
        spans.push(Span::styled(label, Style::default().fg(brand).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" — {cwd} "), Style::default().fg(t.muted)));
        let mut right = vec![];
        if self.scroll > 0 {
            right.push(Span::styled(format!(" ↑{} ", self.scroll), Style::default().fg(t.inline)));
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
        self.demo_activity.or(self.status)
    }
    fn wants_mouse(&self) -> bool {
        self.parser.lock().screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }
    fn tick_every(&self) -> Option<Duration> {
        // agents get re-checked so "working" turns into "idle/done" when they go quiet
        self.agent.then_some(Duration::from_millis(400))
    }
    fn poll(&mut self, _cx: &mut Cx) {
        if self.last_scan.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_scan = Instant::now();
        self.scan();
        // shells title their window with their own exe path — that's noise, keep bro's name for them
        let title = self.meta.name.clone().or_else(|| self.program_title().filter(|t| !looks_like_exe_path(t))).or_else(|| Some(self.meta.label.clone()));
        if title.is_some() && title != self.title_sent {
            if let Some(t) = &title {
                self.svc.bridge_title(&self.meta.sid, t);
            }
            self.title_sent = title;
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        if area.width == 0 || area.height == 0 {
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
        if self.live.is_some() {
            self.resize(area.height, area.width);
        }
        let mut p = self.parser.lock();
        p.screen_mut().set_scrollback(self.scroll);
        self.scroll = p.screen().scrollback(); // clamped to what exists
        let screen = p.screen();
        let buf = f.buffer_mut();
        for row in 0..area.height {
            for col in 0..area.width {
                let Some(cell) = screen.cell(row, col) else { continue };
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
                if cell.underline() {
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
        if cx.focused && !screen.hide_cursor() && self.scroll == 0 && self.live.is_some() {
            let (r, c) = screen.cursor_position();
            if r < area.height && c < area.width {
                f.set_cursor_position(Position { x: area.x + c, y: area.y + r });
            }
        }
    }

    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        if self.error.is_some() {
            if matches!(key.code, crossterm::event::KeyCode::Enter | crossterm::event::KeyCode::Esc) {
                cx.act(crate::pane::Action::Close);
            }
            return true;
        }
        let app_cursor = self.parser.lock().screen().application_cursor();
        if let Some(bytes) = encode_key(key, app_cursor) {
            self.send(&bytes);
        }
        true
    }

    fn paste(&mut self, text: &str, _cx: &mut Cx) {
        let bracketed = self.parser.lock().screen().bracketed_paste();
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.send(text.as_bytes());
        }
    }

    fn mouse(&mut self, ev: MouseEvent, area: Rect, _cx: &mut Cx) {
        let (mode, enc) = {
            let p = self.parser.lock();
            (p.screen().mouse_protocol_mode(), p.screen().mouse_protocol_encoding())
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

/// "C:\Program Files\PowerShell\pwsh.exe", "/usr/bin/bash" — a program path, not a useful title.
fn looks_like_exe_path(t: &str) -> bool {
    let t = t.trim();
    let lower = t.to_ascii_lowercase();
    (lower.ends_with(".exe") || t.starts_with('/')) && !t.contains(' ') || lower.ends_with(".exe") && (t.contains(":\\") || t.contains(":/"))
}

#[cfg(test)]
mod title_tests {
    #[test]
    fn exe_paths_are_not_titles() {
        assert!(super::looks_like_exe_path(r"C:\Program Files\PowerShell\7\pwsh.exe"));
        assert!(super::looks_like_exe_path("/usr/bin/bash"));
        assert!(!super::looks_like_exe_path("✳ Fix the flaky test"));
    }
}
