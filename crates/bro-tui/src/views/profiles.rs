//! Profiles view: every Claude account / Codex profile with its login state; add (then log in inside a PTY
//! pane), log in again, and remove with a confirmation that names the exact directory.

use crate::alerts::Kind;
use crate::pane::{Action, Cx, Pane, Place};
use crate::panes::term::{Meta, Spawn, Term};
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use bro_core::profiles::{Profile, ProfileKind};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};

enum Mode {
    List,
    /// typing a name for a new profile of this kind
    Add(ProfileKind, String),
    /// confirm removing this profile
    Confirm(Profile),
}

enum Pending {
    Create(ProfileKind, String, std::sync::mpsc::Receiver<Result<Profile, String>>),
    Remove(String, std::sync::mpsc::Receiver<Result<(), String>>),
}

#[derive(Clone, Copy)]
enum Hit {
    Key(KeyCode),
    Row(usize),
}

/// The profiles pane.
pub struct ProfilesView {
    sel: usize,
    mode: Mode,
    scroll: usize,
    rows: usize,
    hits: Vec<(Rect, Hit)>,
    pending: Option<Pending>,
    selected_id: Option<String>,
    error: Option<String>,
}

impl Default for ProfilesView {
    fn default() -> Self {
        ProfilesView { sel: 0, mode: Mode::List, scroll: 0, rows: 1, hits: vec![], pending: None, selected_id: None, error: None }
    }
}

impl ProfilesView {
    pub fn new() -> ProfilesView {
        ProfilesView::default()
    }

    fn selected(&self, cx: &Cx) -> Option<Profile> {
        cx.svc.state().profiles.ready().and_then(|ps| {
            self.selected_id.as_ref().and_then(|id| ps.iter().find(|p| &p.id == id)).or_else(|| ps.get(self.sel)).cloned()
        })
    }

    fn button(&mut self, f: &mut Frame, area: Rect, text: &str, key: KeyCode, cx: &Cx) {
        ui::line(f, area, vec![Span::styled(text.to_string(), ui::bold_accent(cx.theme).bg(cx.theme.frame))]);
        self.hits.push((area, Hit::Key(key)));
    }

    fn sync_selection(&mut self, profiles: &[Profile]) {
        if let Some(i) = self.selected_id.as_ref().and_then(|id| profiles.iter().position(|p| &p.id == id)) {
            self.sel = i;
        }
        self.sel = self.sel.min(profiles.len().saturating_sub(1));
        self.selected_id = profiles.get(self.sel).map(|p| p.id.clone());
        self.scroll = self.scroll.min(self.sel);
        if self.sel >= self.scroll + self.rows {
            self.scroll = self.sel.saturating_sub(self.rows.saturating_sub(1));
        }
    }

    /// Open the profile's interactive login in a new terminal tab.
    fn login(&self, p: &Profile, cx: &mut Cx) {
        match cx.svc.login_command(p) {
            Ok(c) => {
                let harness = if p.is_codex() { Harness::Codex } else { Harness::Claude };
                let meta = Meta {
                    sid: uuid::Uuid::new_v4().to_string(),
                    harness: Some(harness),
                    profile: Some(p.id.clone()),
                    store: None,
                    model: None,
                    label: format!("login · {}", p.id),
                    name: None,
                    renamed: false,
                    project: cx.svc.project_for(&c.cwd),
                    cwd: c.cwd.clone(),
                    started: std::time::Instant::now(),
                    route_id: None,
                    cleanup: vec![],
                    caveman: None,
                    orchestrator: false,
                };
                let term = Term::new(meta, Spawn { program: c.program, args: c.args, env: c.env, env_remove: c.env_remove }, cx.svc.clone());
                cx.act(Action::Open(Box::new(term), Place::Tab));
            }
            Err(e) => cx.toast(Kind::Error, e),
        }
    }
}

fn valid_name(s: &str) -> bool {
    bro_core::profiles::valid_name(s)
}

impl Pane for ProfilesView {
    fn title(&self) -> String {
        "profiles".into()
    }
    fn icon(&self) -> &'static str {
        "account"
    }
    fn view(&self) -> Option<&'static str> {
        Some("profiles")
    }
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &mut Cx) {
        self.hits.clear();
        let t = cx.theme;
        if area.height < 8 || area.width < 30 {
            ui::line(f, area, vec![Span::styled("Enlarge this pane to manage profiles", muted(t))]);
            return;
        }
        let area = ui::hint_line(f, area, &[("↑↓", "select"), ("enter", "log in"), ("del", "remove"), ("r", "reload")], t);
        ui::line(f, Rect { height: 1, ..area }, vec![Span::styled(" Profiles", ui::bold_accent(t)), Span::styled(" · Claude and Codex logins", muted(t))]);
        let modal = !matches!(self.mode, Mode::List) || self.pending.is_some();
        if !modal {
            for (i, (text, key)) in [(" + Claude (a) ", 'a'), (" + Codex (c) ", 'c')].iter().enumerate() {
                self.button(f, Rect { x: area.x + i as u16 * 15, y: area.y + 1, width: 14, height: 1 }, text, KeyCode::Char(*key), cx);
            }
            self.button(f, Rect { y: area.y + 2, width: 14, height: 1, ..area }, " Log in (l) ", KeyCode::Char('l'), cx);
            if self.selected(cx).is_some_and(|p| !p.is_local()) {
                self.button(f, Rect { x: area.x + 15, y: area.y + 2, width: 14, height: 1 }, " Remove (d) ", KeyCode::Char('d'), cx);
            }
        }
        let st = cx.svc.state();
        let Some(profiles) = st.profiles.ready() else {
            ui::line(f, Rect { y: area.y + 3, height: 1, ..area }, vec![Span::styled(st.profiles.why(), muted(t))]);
            return;
        };
        self.rows = area.height.saturating_sub(6).max(1) as usize;
        self.sync_selection(profiles);
        for (row, (i, p)) in profiles.iter().enumerate().skip(self.scroll).take(self.rows).enumerate() {
            let r = Rect { y: area.y + 3 + row as u16, height: 1, ..area };
            let brand = ui::harness_color(Some(if p.is_codex() { Harness::Codex } else { Harness::Claude }), t);
            let status = if p.authenticated { p.email.as_deref().unwrap_or("Logged in") } else { "Not logged in" };
            let local = if p.is_local() { " · machine login" } else { "" };
            let text = format!("{} {} · {}{}", if i == self.sel { "▌" } else { " " }, p.id, status, local);
            ui::line(f, r, vec![Span::styled(ui::fit(&text, r.width as usize), if i == self.sel { fg(brand).add_modifier(Modifier::BOLD).bg(t.frame) } else { Style::default() })]);
            if !modal { self.hits.push((r, Hit::Row(i))); }
        }
        let prompt = Rect { y: area.bottom().saturating_sub(3), height: 1, ..area };
        let detail = Rect { y: prompt.y + 1, ..prompt };
        let buttons = Rect { y: prompt.y + 2, ..prompt };
        if self.pending.is_some() {
            ui::line(f, prompt, vec![Span::styled(" Saving profile…", muted(t))]);
            return;
        }
        match &self.mode {
            Mode::List => {
                if let Some(p) = profiles.get(self.sel) {
                    ui::line(f, prompt, vec![Span::styled(crate::util::short_path(&p.dir, area.width as usize), muted(t))]);
                    let note = if p.is_local() { "Machine login: can be used here; cannot be removed by Bro." } else { "Remove deletes this profile's login, settings and saved sessions." };
                    ui::line(f, detail, vec![Span::styled(note, muted(t))]);
                }
            }
            Mode::Add(kind, name) => {
                let family = if *kind == ProfileKind::ClaudeAccount { "Claude" } else { "Codex" };
                ui::line(f, prompt, vec![Span::styled(format!(" {family} profile name: {name}▏"), ui::bold_accent(t))]);
                let message = self.error.as_deref().unwrap_or("1–40 letters, digits, . _ - (example: work). 'local' is reserved.");
                ui::line(f, detail, vec![Span::styled(message, if self.error.is_some() { fg(t.danger) } else { muted(t) })]);
                self.button(f, Rect { width: 22.min(area.width), ..buttons }, " Create + log in (↵) ", KeyCode::Enter, cx);
                if area.width >= 37 {
                    self.button(f, Rect { x: buttons.x + 23, width: 14, ..buttons }, " Cancel (esc) ", KeyCode::Esc, cx);
                }
            }
            Mode::Confirm(p) => {
                ui::line(f, prompt, vec![Span::styled(format!(" Remove {}? Deletes login, settings and saved sessions.", p.id), fg(t.danger))]);
                ui::line(f, detail, vec![Span::styled(p.dir.display().to_string(), muted(t))]);
                self.button(f, Rect { width: 14, ..buttons }, " Remove (y) ", KeyCode::Char('y'), cx);
                self.button(f, Rect { x: buttons.x + 15, width: 14, ..buttons }, " Cancel (esc) ", KeyCode::Esc, cx);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        if self.pending.is_some() { return true; }
        match std::mem::replace(&mut self.mode, Mode::List) {
            Mode::Add(kind, mut name) => {
                match key.code {
                    KeyCode::Enter if valid_name(&name) => {
                        let rx = cx.svc.create_profile(kind, name.clone());
                        self.pending = Some(Pending::Create(kind, name, rx));
                        self.error = None;
                    }
                    KeyCode::Enter => {
                        self.error = Some("Choose a valid name; avoid local, reserved device names and trailing dots.".into());
                        self.mode = Mode::Add(kind, name);
                    }
                    KeyCode::Esc => {}
                    KeyCode::Backspace => {
                        name.pop();
                        self.error = None;
                        self.mode = Mode::Add(kind, name);
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && name.len() < 40 => {
                        name.push(c);
                        self.error = None;
                        self.mode = Mode::Add(kind, name);
                    }
                    _ => self.mode = Mode::Add(kind, name),
                }
                return true;
            }
            Mode::Confirm(p) => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    let rx = cx.svc.remove_profile(p.id.clone());
                    self.pending = Some(Pending::Remove(p.id, rx));
                } else {
                    cx.toast(Kind::Info, "kept it");
                }
                return true;
            }
            Mode::List => {}
        }
        let profiles = cx.svc.state().profiles.ready().cloned().unwrap_or_default();
        self.sync_selection(&profiles);
        let n = profiles.len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = n.saturating_sub(1),
            KeyCode::PageDown => self.sel = (self.sel + self.rows).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(self.rows),
            KeyCode::Char('a') => { self.error = None; self.mode = Mode::Add(ProfileKind::ClaudeAccount, String::new()); }
            KeyCode::Char('c') => { self.error = None; self.mode = Mode::Add(ProfileKind::CodexProfile, String::new()); }
            KeyCode::Char('r') => {
                cx.svc.refresh_data();
                cx.toast(Kind::Info, "reloading profiles…");
            }
            KeyCode::Char('l') | KeyCode::Enter => {
                if let Some(p) = self.selected(cx) {
                    self.login(&p, cx);
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => match self.selected(cx) {
                Some(p) if matches!(p.kind, ProfileKind::ClaudeLocal | ProfileKind::CodexLocal) => cx.toast(Kind::Info, "the local logins (~/.claude, ~/.codex) aren't removed by bro"),
                Some(p) => self.mode = Mode::Confirm(p),
                None => {}
            },
            _ => return false,
        }
        self.selected_id = profiles.get(self.sel).map(|p| p.id.clone());
        true
    }

    fn paste(&mut self, text: &str, _cx: &mut Cx) {
        if let Mode::Add(_, name) = &mut self.mode {
            for c in text.trim().chars().filter(|c| !c.is_control()) {
                if name.len() + c.len_utf8() > 40 { break; }
                name.push(c);
            }
            self.error = None;
        }
    }

    fn mouse(&mut self, ev: MouseEvent, _area: Rect, cx: &mut Cx) {
        if self.pending.is_some() { return; }
        if matches!(self.mode, Mode::List) {
            match ev.kind {
                MouseEventKind::ScrollDown => { self.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), cx); return; }
                MouseEventKind::ScrollUp => { self.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), cx); return; }
                _ => {}
            }
        }
        if ev.kind != MouseEventKind::Down(MouseButton::Left) { return; }
        let hit = self.hits.iter().find(|(r, _)| r.contains((ev.column, ev.row).into())).map(|(_, h)| *h);
        match hit {
            Some(Hit::Key(key)) => { self.key(KeyEvent::new(key, KeyModifiers::NONE), cx); }
            Some(Hit::Row(i)) if matches!(self.mode, Mode::List) => {
                self.sel = i;
                self.selected_id = cx.svc.state().profiles.ready().and_then(|ps| ps.get(i).map(|p| p.id.clone()));
            }
            _ => {}
        }
    }

    fn tick_every(&self) -> Option<std::time::Duration> {
        self.pending.as_ref().map(|_| std::time::Duration::from_millis(50))
    }

    fn poll(&mut self, cx: &mut Cx) {
        use std::sync::mpsc::TryRecvError;
        let Some(pending) = self.pending.take() else { return };
        match pending {
            Pending::Create(kind, name, rx) => match rx.try_recv() {
                Ok(Ok(p)) => {
                    self.selected_id = Some(p.id.clone());
                    cx.toast(Kind::Info, format!("Created {}. Complete sign-in in the login tab.", p.id));
                    self.login(&p, cx);
                }
                Ok(Err(e)) => { self.error = Some(e); self.mode = Mode::Add(kind, name); }
                Err(TryRecvError::Empty) => self.pending = Some(Pending::Create(kind, name, rx)),
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("Could not create the profile. Try again.".into());
                    self.mode = Mode::Add(kind, name);
                }
            },
            Pending::Remove(id, rx) => match rx.try_recv() {
                Ok(Ok(())) => { self.selected_id = None; cx.toast(Kind::Info, format!("Removed {id}")); }
                Ok(Err(e)) => cx.toast(Kind::Error, e),
                Err(TryRecvError::Empty) => self.pending = Some(Pending::Remove(id, rx)),
                Err(TryRecvError::Disconnected) => cx.toast(Kind::Error, "Could not remove the profile. Try again."),
            },
        }
    }
}
