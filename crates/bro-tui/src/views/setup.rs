//! Setup guide: the three steps from a fresh machine to a first agent. 1) install Claude Code and / or Codex
//! (each installer runs in a tab), 2) sign in: what a profile is, the machine logins and every other one with
//! its state, plus adding more, 3) launch. Opens by itself at startup while nothing is signed in, until
//! dismissed; the palette (and prefix e) opens it any time.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

use super::profiles::{ProfilesView, open_login};
use crate::alerts::Kind;
use crate::keymap::Act;
use crate::pane::{Action, Cx, Pane, Place};
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use bro_core::agent_install;
use bro_core::profiles::{Profile, ProfileKind};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use parking_lot::Mutex;
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The agents setup offers (the ones profiles log in to).
const AGENTS: [Harness; 2] = [Harness::Claude, Harness::Codex];
/// Profiles listed before "more in profiles".
const SHOWN: usize = 6;
/// How often to look again while an install started here may still be running, and for how long.
const RECHECK: Duration = Duration::from_secs(3);
const WATCH_FOR: Duration = Duration::from_secs(15 * 60);

/// Where each agent's executable is (None: not installed).
type Found = Vec<(Harness, Option<PathBuf>)>;

/// A selectable row.
#[derive(Clone, PartialEq, Debug)]
enum Row {
    Agent(Harness),
    Login(String),
    More,
    Add(ProfileKind),
    Launch,
    Done,
}

#[derive(Default)]
pub struct SetupView {
    sel: usize,
    scroll: usize,
    /// (screen row, selectable row) from the last render, for clicks
    hits: Vec<(u16, usize)>,
    /// the rows of the last render
    rows: Vec<Row>,
    found: Option<Found>,
    /// a background look is running; its result lands here
    pending: Option<Arc<Mutex<Option<Found>>>>,
    checked_at: Option<Instant>,
    /// an install was started here at this moment: keep looking for a while
    watching: Option<Instant>,
}

fn harness_of(p: &Profile) -> Harness {
    if p.is_codex() { Harness::Codex } else { Harness::Claude }
}

impl SetupView {
    pub fn new() -> SetupView {
        SetupView::default()
    }

    /// Look for the agents on a background thread.
    fn check(&mut self, cx: &Cx) {
        if self.pending.is_some() {
            return;
        }
        self.checked_at = Some(Instant::now());
        if cx.svc.is_demo() {
            self.found = Some(AGENTS.iter().map(|h| (*h, Some(PathBuf::from(h.label())))).collect());
            return;
        }
        let slot = Arc::new(Mutex::new(None));
        self.pending = Some(slot.clone());
        let waker = cx.waker();
        std::thread::spawn(move || {
            *slot.lock() = Some(AGENTS.iter().map(|h| (*h, agent_install::installed(*h))).collect());
            waker.wake();
        });
    }

    /// Installed, not installed, or not looked yet (None).
    fn installed(&self, h: Harness) -> Option<bool> {
        self.found.as_ref().map(|f| f.iter().any(|(x, p)| *x == h && p.is_some()))
    }

    fn activate(&mut self, cx: &mut Cx) {
        let Some(row) = self.rows.get(self.sel).cloned() else { return };
        match row {
            Row::Agent(h) => self.install(h, cx),
            Row::Login(id) => {
                let p = cx.svc.state().profiles.ready().and_then(|ps| ps.iter().find(|p| p.id == id).cloned());
                match p {
                    Some(p) if self.installed(harness_of(&p)) == Some(false) => self.install(harness_of(&p), cx),
                    Some(p) => open_login(&p, cx),
                    None => cx.toast(Kind::Info, format!("{id} is gone: look again with r")),
                }
            }
            Row::More => cx.act(Action::Run(Act::Profiles)),
            Row::Add(kind) => cx.act(Action::Open(Box::new(ProfilesView::adding(kind)), Place::Tab)),
            Row::Launch => cx.act(Action::Run(Act::NewSession)),
            Row::Done => {
                let mut s = cx.svc.settings();
                if !s.setup_done {
                    s.setup_done = true;
                    cx.svc.save_settings(s);
                }
                cx.toast(Kind::Info, "setup won't open by itself again · prefix e or the palette brings it back");
                cx.act(Action::Close);
            }
        }
    }

    fn install(&mut self, h: Harness, cx: &mut Cx) {
        let name = agent_install::display_name(h);
        if self.installed(h) == Some(true) {
            return cx.toast(Kind::Info, format!("{name} is installed · `bro install --force {}` reinstalls it", h.label()));
        }
        if cx.svc.is_demo() {
            return cx.toast(Kind::Info, "the demo doesn't install anything");
        }
        let cwd = std::env::current_dir().unwrap_or_default();
        cx.act(Action::Open(Box::new(super::quick_install::install_term(cx.svc, cwd, &[h.label()])), Place::Tab));
        cx.toast(Kind::Info, format!("installing {name} in a new tab · come back here when it's done"));
        self.watching = Some(Instant::now());
    }
}

fn heading(text: &str, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    vec![Span::styled(format!("──── {text} "), muted(t).add_modifier(Modifier::BOLD))]
}

fn note(text: &str, t: &crate::theme::Theme) -> Vec<Span<'static>> {
    vec![Span::styled(format!("    {text}"), muted(t))]
}

impl Pane for SetupView {
    fn title(&self) -> String {
        "setup".into()
    }
    fn icon(&self) -> &'static str {
        "rocket"
    }
    fn view(&self) -> Option<&'static str> {
        Some("setup")
    }
    fn poll(&mut self, cx: &mut Cx) {
        if let Some(found) = self.pending.as_ref().and_then(|p| p.lock().take()) {
            self.pending = None;
            let before = self.found.replace(found);
            // an agent just landed: say so, and stop looking once all of them are there
            for h in AGENTS {
                let was = before.as_ref().is_some_and(|f| f.iter().any(|(x, p)| *x == h && p.is_some()));
                if self.watching.is_some() && !was && self.installed(h) == Some(true) {
                    cx.toast(Kind::Info, format!("{} is installed · next: sign in", agent_install::display_name(h)));
                }
            }
            if AGENTS.iter().all(|h| self.installed(*h) == Some(true)) {
                self.watching = None;
            }
        }
        if self.watching.is_some_and(|w| w.elapsed() > WATCH_FOR) {
            self.watching = None;
        }
        if self.watching.is_some() && self.checked_at.is_none_or(|c| c.elapsed() >= RECHECK) {
            self.check(cx);
        }
    }
    fn tick_every(&self) -> Option<Duration> {
        self.watching.map(|_| RECHECK)
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        let t = cx.theme;
        if self.checked_at.is_none() {
            self.check(cx);
        }
        let area = ui::hint_line(f, area, &[("j/k", "move"), ("enter", "do it"), ("r", "look again")], t);
        let st = cx.svc.state();
        let profiles: Option<Vec<Profile>> = st.profiles.ready().cloned();
        let why = st.profiles.why();
        drop(st);
        let any_agent = AGENTS.iter().any(|h| self.installed(*h) == Some(true));
        let any_login = profiles.as_ref().is_some_and(|ps| ps.iter().any(|p| p.authenticated));

        let mut rows: Vec<Row> = vec![];
        let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = vec![];
        let push_row = |rows: &mut Vec<Row>, lines: &mut Vec<(Option<usize>, Vec<Span<'static>>)>, row: Row, spans: Vec<Span<'static>>| {
            lines.push((Some(rows.len()), spans));
            rows.push(row);
        };
        let status = match (any_agent, any_login) {
            (true, true) => Span::styled("  ✓ ready: launch an agent below", fg(t.good)),
            (false, _) => Span::styled("  start here: install an agent", fg(t.inline)),
            (true, false) => Span::styled("  next: sign in", fg(t.inline)),
        };
        lines.push((None, vec![Span::styled(" SET UP BRO ", ui::bold_accent(t)), status]));
        lines.push((None, vec![]));

        // 1 · agents
        lines.push((None, heading("1 · install an agent", t)));
        lines.push((None, note("bro runs coding agents in tabs, side by side. Install at least one.", t)));
        for h in AGENTS {
            let installed = self.installed(h);
            let (mark, mark_style, detail) = match installed {
                None => ("…", muted(t), "looking…".to_string()),
                Some(true) => {
                    let at = self.found.as_ref().and_then(|f| f.iter().find(|(x, _)| *x == h)).and_then(|(_, p)| p.clone()).unwrap_or_default();
                    ("✓", fg(t.good), crate::util::short_path(&at, 48))
                }
                Some(false) if self.watching.is_some() => ("…", fg(t.shine), "installing in another tab…".to_string()),
                Some(false) => ("✗", fg(t.danger), "not installed · enter installs it".to_string()),
            };
            let blurb = if h == Harness::Claude { "Anthropic · needs a Claude subscription or API key" } else { "OpenAI · needs a ChatGPT plan or API key" };
            push_row(&mut rows, &mut lines, Row::Agent(h), vec![
                Span::styled(format!("{mark} "), mark_style),
                Span::styled(format!("{:<13}", agent_install::display_name(h)), fg(ui::harness_color(Some(h), t)).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:<36} ", detail), if installed == Some(true) { fg(t.inline) } else { muted(t) }),
                Span::styled(blurb, muted(t)),
            ]);
        }
        lines.push((None, note("Pi, omp, Git, Node.js and more: quick install, in the palette.", t)));
        lines.push((None, vec![]));

        // 2 · logins
        lines.push((None, heading("2 · sign in", t)));
        lines.push((None, note("A profile is one login bro launches agents with. This machine's own logins (~/.claude,", t)));
        lines.push((None, note("~/.codex) are used as they are. Add more Claude accounts or Codex profiles to see every", t)));
        lines.push((None, note("meter in the sidebar and move a session to another login when one runs out.", t)));
        match &profiles {
            None => lines.push((None, note(&why, t))),
            Some(ps) => {
                for p in ps.iter().take(SHOWN) {
                    let h = harness_of(p);
                    let (mark, mark_style, detail) = match () {
                        _ if p.authenticated => ("●", fg(t.good), p.email.clone().unwrap_or_else(|| "signed in".into())),
                        _ if self.installed(h) == Some(false) => ("○", muted(t), format!("needs {} first · enter installs it", agent_install::display_name(h))),
                        _ => ("○", muted(t), "not signed in · enter opens its login".to_string()),
                    };
                    let kind = if p.is_local() { "  machine login" } else { "" };
                    push_row(&mut rows, &mut lines, Row::Login(p.id.clone()), vec![
                        Span::styled(format!("{mark} "), mark_style),
                        Span::styled(format!("{:<20}", p.id), fg(ui::harness_color(Some(h), t))),
                        Span::styled(detail, if p.authenticated { fg(t.inline) } else { muted(t) }),
                        Span::styled(kind, muted(t)),
                    ]);
                }
                if ps.len() > SHOWN {
                    push_row(&mut rows, &mut lines, Row::More, vec![Span::styled(format!("  {} more in profiles", ps.len() - SHOWN), muted(t))]);
                }
            }
        }
        push_row(&mut rows, &mut lines, Row::Add(ProfileKind::ClaudeAccount), vec![Span::styled("+ Add a Claude account", ui::accent(t))]);
        push_row(&mut rows, &mut lines, Row::Add(ProfileKind::CodexProfile), vec![Span::styled("+ Add a Codex profile", ui::accent(t))]);
        lines.push((None, note("Each new profile opens its agent's login in a tab; it shows up here once signed in.", t)));
        lines.push((None, vec![]));

        // 3 · go
        lines.push((None, heading("3 · go", t)));
        push_row(&mut rows, &mut lines, Row::Launch, vec![Span::styled("▶ Launch an agent", ui::bold_accent(t))]);
        push_row(&mut rows, &mut lines, Row::Done, vec![Span::styled("✓ Done · don't open this at startup again", muted(t))]);

        self.sel = self.sel.min(rows.len().saturating_sub(1));
        // keep the selection on screen
        let room = area.height as usize;
        let at = lines.iter().position(|(s, _)| *s == Some(self.sel)).unwrap_or(0);
        if at < self.scroll {
            self.scroll = at.saturating_sub(2);
        } else if at >= self.scroll + room {
            self.scroll = at + 1 - room;
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(room));
        self.hits.clear();
        for (n, (sel, spans)) in lines.into_iter().skip(self.scroll).take(room).enumerate() {
            let r = Rect { y: area.y + n as u16, height: 1, ..area };
            let spans = match sel {
                Some(s) => {
                    self.hits.push((r.y, s));
                    let on = s == self.sel;
                    let lead = Span::raw(if on { "  ▌ " } else { "    " });
                    let spans: Vec<Span<'static>> = std::iter::once(lead).chain(spans).collect();
                    if on { spans.into_iter().map(|s| Span::styled(s.content, s.style.bg(t.frame))).collect() } else { spans }
                }
                None => spans,
            };
            ui::line(f, r, spans);
        }
        self.rows = rows;
    }
    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        let last = self.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.sel = (self.sel + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.sel = last,
            KeyCode::Char(' ') | KeyCode::Enter => self.activate(cx),
            KeyCode::Char('r') => {
                self.check(cx);
                cx.svc.refresh_data();
            }
            _ => return false,
        }
        true
    }
    fn mouse(&mut self, ev: MouseEvent, _area: Rect, cx: &mut Cx) {
        let last = self.rows.len().saturating_sub(1);
        match ev.kind {
            MouseEventKind::ScrollDown => self.sel = (self.sel + 1).min(last),
            MouseEventKind::ScrollUp => self.sel = self.sel.saturating_sub(1),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, s)) = self.hits.iter().find(|(y, _)| *y == ev.row) {
                    self.sel = s;
                    self.activate(cx);
                }
            }
            _ => {}
        }
    }
}
