//! Profiles view: every Claude account / Codex profile with its login state; add (then log in inside a PTY
//! pane), log in again, and remove with a confirmation that names the exact directory.

use crate::alerts::Kind;
use crate::pane::{Action, Cx, Pane, Place};
use crate::panes::term::{Meta, Spawn, Term};
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use bro_core::profiles::{Profile, ProfileKind};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

/// The profiles pane.
pub struct ProfilesView {
    sel: usize,
    mode: Mode,
}

impl Default for ProfilesView {
    fn default() -> Self {
        ProfilesView { sel: 0, mode: Mode::List }
    }
}

impl ProfilesView {
    pub fn new() -> ProfilesView {
        ProfilesView::default()
    }

    fn selected(&self, cx: &Cx) -> Option<Profile> {
        cx.svc.state().profiles.ready().and_then(|p| p.get(self.sel).cloned())
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
                };
                let term = Term::new(meta, Spawn { program: c.program, args: c.args, env: c.env, env_remove: c.env_remove }, cx.svc.clone());
                cx.act(Action::Open(Box::new(term), Place::Tab));
            }
            Err(e) => cx.toast(Kind::Error, e),
        }
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
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
        let t = cx.theme;
        let st = cx.svc.state();
        let area = ui::hint_line(f, area, &[("a", "add claude account"), ("c", "add codex profile"), ("l/⏎", "log in"), ("d", "remove"), ("r", "reload")], t);
        ui::line(f, Rect { height: 1, ..area }, vec![Span::styled(" PROFILES ", ui::bold_accent(t)), Span::styled(" logins bro can launch as", muted(t))]);
        let Some(profiles) = st.profiles.ready() else {
            ui::line(f, Rect { y: area.y + 2, height: 1, ..area }, vec![Span::styled(format!("  {}", st.profiles.why()), muted(t))]);
            return;
        };
        self.sel = self.sel.min(profiles.len().saturating_sub(1));
        let w = area.width as usize;
        let mut y = area.y + 2;
        for (i, p) in profiles.iter().enumerate() {
            if y >= area.bottom().saturating_sub(3) {
                break;
            }
            let on = i == self.sel;
            let h = if p.is_codex() { Some(Harness::Codex) } else { Some(Harness::Claude) };
            let brand = ui::harness_color(h, t);
            let (dot, dc) = if p.authenticated { ("●", t.good) } else { ("○", t.danger) };
            let kind = match p.kind {
                ProfileKind::ClaudeLocal => "claude · ~/.claude",
                ProfileKind::ClaudeAccount => "claude account",
                ProfileKind::CodexLocal => "codex · ~/.codex",
                ProfileKind::CodexProfile => "codex profile",
            };
            let who = if p.authenticated { p.email.clone().unwrap_or_default() } else { "not logged in".into() };
            let left = vec![
                Span::styled(if on { "▌" } else { " " }, ui::accent(t)),
                Span::styled(format!("{dot} "), fg(dc)),
                Span::styled(format!("{} ", ui::harness_glyph(h)), fg(brand)),
                Span::styled(ui::pad(&p.id, 20), if on { Style::default().fg(brand).add_modifier(Modifier::BOLD) } else { ui::bold() }),
                Span::styled(ui::pad(p.plan.as_deref().unwrap_or("—"), 6), fg(t.shine)),
                Span::styled(ui::pad(&who, 28), if p.authenticated { Style::default() } else { fg(t.danger) }),
                Span::styled(ui::pad(kind, 18), muted(t)),
            ];
            let right = vec![Span::styled(crate::util::short_path(&p.dir, w.saturating_sub(84).max(12)), muted(t))];
            ui::line_lr(f, Rect { y, height: 1, ..area }, left, right);
            y += 1;
        }
        if profiles.is_empty() {
            ui::line(f, Rect { y, height: 1, ..area }, vec![Span::styled("  no profiles yet — a adds a Claude account, c a Codex profile", muted(t))]);
        }
        // the prompt / confirmation sits at the bottom
        let prompt = Rect { y: area.bottom().saturating_sub(2), height: 1, ..area };
        match &self.mode {
            Mode::List => {}
            Mode::Add(kind, name) => {
                let what = if *kind == ProfileKind::ClaudeAccount { "new Claude account name" } else { "new Codex profile name" };
                let ok = name.is_empty() || valid_name(name);
                ui::line(f, prompt, vec![
                    Span::styled(format!(" {what}: "), ui::bold_accent(t)),
                    Span::styled(name.clone(), if ok { ui::bold() } else { fg(t.danger) }),
                    Span::styled("▏", ui::accent(t)),
                    Span::styled("   ⏎ create + log in · esc cancel · letters, digits, . _ -", muted(t)),
                ]);
            }
            Mode::Confirm(p) => {
                ui::line(f, prompt, vec![
                    Span::styled(format!(" remove {}? ", p.id), fg(t.danger).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("this deletes {} ", p.dir.display()), Style::default().fg(t.danger)),
                    Span::styled("  y remove · any other key cancels", muted(t)),
                ]);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, cx: &mut Cx) -> bool {
        match std::mem::replace(&mut self.mode, Mode::List) {
            Mode::Add(kind, mut name) => {
                match key.code {
                    KeyCode::Enter if valid_name(&name) => {
                        cx.svc.create_profile(kind, name.clone());
                        cx.toast(Kind::Info, format!("creating {name}… then l to log in"));
                    }
                    KeyCode::Esc => {}
                    KeyCode::Backspace => {
                        name.pop();
                        self.mode = Mode::Add(kind, name);
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && name.len() < 40 => {
                        name.push(c);
                        self.mode = Mode::Add(kind, name);
                    }
                    _ => self.mode = Mode::Add(kind, name),
                }
                return true;
            }
            Mode::Confirm(p) => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    cx.svc.remove_profile(p.id.clone());
                } else {
                    cx.toast(Kind::Info, "kept it");
                }
                return true;
            }
            Mode::List => {}
        }
        let n = cx.svc.state().profiles.ready().map(|p| p.len()).unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('a') => self.mode = Mode::Add(ProfileKind::ClaudeAccount, String::new()),
            KeyCode::Char('c') => self.mode = Mode::Add(ProfileKind::CodexProfile, String::new()),
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
        true
    }
}
