//! Modal overlays: launcher, palette, help, the resume-in-which-login picker and yes/no confirmations.

use super::App;
use crate::help::Help;
use crate::launcher::{self, Launcher};
use crate::layout::PaneId;
use crate::palette::Palette;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::sessions::SessionInfo;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};

/// At most one overlay is open.
#[derive(Default)]
pub enum Overlay {
    #[default]
    None,
    Launcher(Box<Launcher>),
    Palette(Box<Palette>),
    Help(Help),
    Resume(Box<ResumePicker>),
    Folder(Box<crate::folder::FolderPicker>),
    Confirm(Confirm),
}

impl Overlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, Overlay::None)
    }
}

/// What's being resumed.
pub enum ResumeFrom {
    /// an earlier session from the sidebar
    Past(SessionInfo),
    /// a running session being moved to another login (closed, then resumed there)
    Live { pane: PaneId, harness: bro_core::Harness, store: String, cwd: std::path::PathBuf, since: std::time::SystemTime, title: String },
}

impl ResumeFrom {
    pub fn harness(&self) -> bro_core::Harness {
        match self {
            ResumeFrom::Past(s) => s.harness,
            ResumeFrom::Live { harness, .. } => *harness,
        }
    }
    pub fn title(&self) -> &str {
        match self {
            ResumeFrom::Past(s) => &s.title,
            ResumeFrom::Live { title, .. } => title,
        }
    }
}

/// A login the session can continue in.
pub struct ResumeTarget {
    pub profile_id: String,
    pub name: String,
    pub detail: String,
    /// % left of the tighter window
    pub left: Option<f64>,
    /// where the session lives now
    pub current: bool,
}

/// "Resume this in which login?"
pub struct ResumePicker {
    pub from: ResumeFrom,
    pub targets: Vec<ResumeTarget>,
    pub sel: usize,
    /// row rects from the last draw (for clicks)
    pub hits: Vec<Rect>,
}

/// What a confirmation does on `y`.
pub enum ConfirmAction {
    Quit,
    Close(PaneId),
}

/// A yes/no question.
pub struct Confirm {
    pub text: String,
    pub detail: String,
    pub action: ConfirmAction,
}

impl App {
    /// Draw whatever overlay is open.
    pub(super) fn draw_overlay(&mut self, f: &mut Frame, area: Rect) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Launcher(l) => launcher::view::draw(f, area, l, &t, time),
            Overlay::Palette(p) => p.draw(f, area, &t),
            Overlay::Help(h) => h.draw(f, area, &self.keymap, &t, time),
            Overlay::Resume(p) => draw_resume(f, area, p, &t),
            Overlay::Folder(p) => crate::folder::draw(f, area, p, &t),
            Overlay::Confirm(c) => draw_confirm(f, area, c, &t),
        }
    }

    /// Keys for the resume picker.
    pub(super) fn resume_key(&mut self, k: KeyEvent) {
        let Overlay::Resume(p) = &mut self.overlay else { return };
        match k.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Down | KeyCode::Char('j') => p.sel = (p.sel + 1).min(p.targets.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => p.sel = p.sel.saturating_sub(1),
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < p.targets.len() {
                    p.sel = i;
                    self.resume_chosen();
                }
            }
            KeyCode::Enter => self.resume_chosen(),
            _ => {}
        }
    }

    pub(super) fn confirm_key(&mut self, k: KeyEvent) {
        let Overlay::Confirm(c) = std::mem::take(&mut self.overlay) else { return };
        if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
            match c.action {
                ConfirmAction::Quit => self.quit = true,
                ConfirmAction::Close(id) => self.close(id),
            }
        }
    }
}

fn draw_resume(f: &mut Frame, area: Rect, p: &mut ResumePicker, t: &Theme) {
    p.hits.clear();
    let h = (p.targets.len() as u16 + 8).min(22);
    let title = match p.from {
        ResumeFrom::Past(_) => format!("{}resume in", ui::lead("history")),
        ResumeFrom::Live { .. } => format!("{}move to another login", ui::lead("history")),
    };
    let inner = ui::popup(f, area, 72, h, &title, t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let brand = ui::harness_color(Some(p.from.harness()), t);
    ui::line(f, Rect { height: 1, ..inner }, vec![
        Span::styled(format!("{} ", ui::harness_glyph(Some(p.from.harness()))), fg(brand)),
        Span::styled(ui::fit(p.from.title(), inner.width as usize - 4), ui::bold()),
    ]);
    let note = match p.from {
        ResumeFrom::Past(_) => "another login continues from a copy of the conversation",
        ResumeFrom::Live { .. } => "the running session closes and continues in the login you pick",
    };
    ui::line(f, Rect { y: inner.y + 1, height: 1, ..inner }, vec![Span::styled(format!("  {note}"), muted(t))]);
    for (i, tg) in p.targets.iter().enumerate() {
        let y = inner.y + 3 + i as u16;
        if y >= inner.bottom().saturating_sub(1) {
            break;
        }
        let on = i == p.sel;
        let mut left = vec![
            Span::styled(if on { "▌" } else { " " }, ui::accent(t)),
            Span::styled(format!("{} ", i + 1), muted(t)),
            Span::styled(ui::pad(&tg.name, 14), if on { ui::bold_accent(t) } else { ui::bold() }),
            Span::styled(tg.detail.clone(), muted(t)),
        ];
        if tg.current {
            left.push(Span::styled("  current", fg(t.shine)));
        }
        let right = match tg.left {
            Some(l) => vec![Span::styled(format!("{l:.0}% left "), fg(ui::left_color(l, t)))],
            None => vec![],
        };
        ui::line_lr(f, Rect { y, height: 1, ..inner }, left, right);
        p.hits.push(Rect { y, height: 1, ..inner });
    }
    ui::hint_line(f, inner, &[("⏎", "resume"), ("1-9", "pick + go"), ("↑↓", "move"), ("esc", "cancel")], t);
}

fn draw_confirm(f: &mut Frame, area: Rect, c: &Confirm, t: &Theme) {
    let w = (ui::width(&c.text).max(ui::width(&c.detail)) as u16 + 8).clamp(40, 100);
    let inner = ui::popup(f, area, w, 7, "confirm", t);
    let inner = Rect { x: inner.x + 2, width: inner.width.saturating_sub(4), y: inner.y + 1, ..inner };
    ui::line(f, Rect { height: 1, ..inner }, vec![Span::styled(c.text.clone(), fg(t.danger).add_modifier(Modifier::BOLD))]);
    ui::line(f, Rect { y: inner.y + 1, height: 1, ..inner }, vec![Span::styled(c.detail.clone(), muted(t))]);
    ui::line(f, Rect { y: inner.y + 3, height: 1, ..inner }, ui::hints(&[("y", "yes"), ("any other key", "no")], t));
}

/// Build launcher data from services + app state (dirs: the preferred cwd first).
pub(super) fn launcher_data(app: &App) -> launcher::Data {
    let st = app.svc.state();
    let installed = bro_core::Harness::ALL.iter().map(|h| (*h, app.svc.is_demo() || cfg!(test) || crate::util::which(h.label()).is_some())).collect();
    launcher::Data {
        profiles: st.profiles.ready().cloned().unwrap_or_default(),
        providers: st.providers.ready().cloned().unwrap_or_default(),
        usage: st.usage.iter().filter_map(|(k, e)| e.usage.as_ref().and_then(|u| u.five_hour.as_ref()).map(|w| (k.clone(), w.used_pct))).collect(),
        installed,
        recents: app.recents.clone(),
        models: st.models.clone(),
        keyed: {
            let cfg = st.config.clone().unwrap_or_default();
            st.providers.ready().map(|ps| ps.iter().filter(|p| app.svc.is_demo() || p.no_key || cfg.key_for(&p.id, p.key_env.as_deref()).is_some()).map(|p| p.id.clone()).collect()).unwrap_or_default()
        },
    }
}
