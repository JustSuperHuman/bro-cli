//! Modal overlays: launcher, palette, help, the fork-into-profile picker and yes/no confirmations.

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
    Fork(Box<ForkPicker>),
    Confirm(Confirm),
}

impl Overlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, Overlay::None)
    }
}

/// "Fork this past session into which profile?"
pub struct ForkPicker {
    pub session: SessionInfo,
    /// (profile id, detail)
    pub targets: Vec<(String, String)>,
    pub sel: usize,
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
            Overlay::Fork(p) => draw_fork(f, area, p, &t),
            Overlay::Confirm(c) => draw_confirm(f, area, c, &t),
        }
    }

    /// Keys for the fork picker. Returns true when it should close.
    pub(super) fn fork_key(&mut self, k: KeyEvent) {
        let Overlay::Fork(p) = &mut self.overlay else { return };
        match k.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Down | KeyCode::Char('j') => p.sel = (p.sel + 1).min(p.targets.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => p.sel = p.sel.saturating_sub(1),
            KeyCode::Enter => {
                let target = p.targets.get(p.sel).map(|x| x.0.clone());
                let Overlay::Fork(p) = std::mem::take(&mut self.overlay) else { return };
                if let Some(target) = target {
                    self.fork_into(p.session, target);
                }
            }
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

fn draw_fork(f: &mut Frame, area: Rect, p: &ForkPicker, t: &Theme) {
    let h = (p.targets.len() as u16 + 7).min(20);
    let inner = ui::popup(f, area, 70, h, &format!("{}fork into profile", ui::lead("history")), t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let brand = ui::harness_color(Some(p.session.harness), t);
    ui::line(f, Rect { height: 1, ..inner }, vec![
        Span::styled(format!("{} ", ui::harness_glyph(Some(p.session.harness))), fg(brand)),
        Span::styled(ui::fit(&p.session.title, inner.width as usize - 4), ui::bold()),
    ]);
    ui::line(f, Rect { y: inner.y + 1, height: 1, ..inner }, vec![Span::styled(format!("  from {} — the copy is resumed with fork semantics and cleaned up on exit", p.session.profile_id.as_deref().unwrap_or("?")), muted(t))]);
    for (i, (id, detail)) in p.targets.iter().enumerate() {
        let y = inner.y + 3 + i as u16;
        if y >= inner.bottom().saturating_sub(1) {
            break;
        }
        let on = i == p.sel;
        ui::line(f, Rect { y, height: 1, ..inner }, vec![
            Span::styled(if on { "▌ " } else { "  " }, ui::accent(t)),
            Span::styled(ui::pad(id, 24), if on { ui::bold_accent(t) } else { ui::bold() }),
            Span::styled(detail.clone(), muted(t)),
        ]);
    }
    if p.targets.is_empty() {
        ui::line(f, Rect { y: inner.y + 3, height: 1, ..inner }, vec![Span::styled("  no other logged-in profile of this kind", muted(t))]);
    }
    ui::hint_line(f, inner, &[("⏎", "fork + resume"), ("↑↓", "pick"), ("esc", "cancel")], t);
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
    let mut live_dirs = vec![];
    for p in app.panes.values() {
        if let Some(t) = p.as_term_ref() {
            live_dirs.push(t.meta.project.root.clone());
        }
    }
    let past_dirs: Vec<_> = st.past.ready().map(|v| v.iter().filter_map(|s| s.project.as_ref().map(|p| p.root.clone()).or_else(|| s.cwd.clone())).collect()).unwrap_or_default();
    let here = std::env::current_dir().ok().into_iter().collect::<Vec<_>>();
    let dirs = launcher::dedup_dirs(vec![live_dirs, crate::recents::dirs(&app.recents), past_dirs, here]);
    let installed = bro_core::Harness::ALL.iter().map(|h| (*h, app.svc.is_demo() || cfg!(test) || crate::util::which(h.label()).is_some())).collect();
    launcher::Data {
        profiles: st.profiles.ready().cloned().unwrap_or_default(),
        providers: st.providers.ready().cloned().unwrap_or_default(),
        usage: st.usage.iter().filter_map(|(k, e)| e.usage.as_ref().and_then(|u| u.five_hour.as_ref()).map(|w| (k.clone(), w.used_pct))).collect(),
        installed,
        dirs,
        recents: app.recents.clone(),
        models: st.models.clone(),
        keyed: {
            let cfg = st.config.clone().unwrap_or_default();
            st.providers.ready().map(|ps| ps.iter().filter(|p| app.svc.is_demo() || p.no_key || cfg.key_for(&p.id, p.key_env.as_deref()).is_some()).map(|p| p.id.clone()).collect()).unwrap_or_default()
        },
    }
}
