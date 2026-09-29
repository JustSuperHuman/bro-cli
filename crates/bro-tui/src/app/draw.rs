//! Drawing the whole screen: sidebar, tab bar, panes in their frames, the selection, toasts, the armed-prefix
//! hint and overlays.

use super::App;
use crate::alerts::Kind;
use crate::pane::Cx;
use crate::ui::{self, fg};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear},
};

impl App {
    /// Where panes go: the whole body (sessions are switched from the sidebar, there's no tab bar).
    pub(super) fn body_panes(&self) -> Rect {
        self.body
    }

    pub(crate) fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let t = self.theme.clone();
        if !matches!(t.bg, ratatui::style::Color::Reset) {
            f.render_widget(Block::default().style(Style::default().bg(t.bg).fg(t.fg)), area);
        }
        let side_w = if self.sidebar && area.width >= 72 { (area.width / 4).clamp(30, 40) } else { 0 };
        self.body = Rect { x: area.x + side_w, width: area.width - side_w, ..area };
        self.side_hits.clear();
        if side_w > 0 {
            self.draw_sidebar(f, Rect { width: side_w, ..area });
        }
        self.pane_close.clear();
        self.outer.clear();
        self.inner.clear();
        if self.tabs.is_empty() {
            self.draw_welcome(f, self.body);
        } else {
            self.draw_panes(f);
        }
        self.draw_selection(f);
        self.draw_toasts(f, area);
        self.draw_overlay(f, area);
    }

    fn draw_panes(&mut self, f: &mut Frame) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let body = self.body_panes();
        let tab = &self.tabs[self.cur];
        let mut rects = vec![];
        if tab.zoom {
            rects.push((tab.focus, body));
        } else {
            tab.root.rects(body, &mut rects);
        }
        let focus = tab.focus;
        let zoomed = tab.zoom;
        self.outer = rects.clone();
        for (id, r) in rects {
            let Some(p) = self.panes.get(&id) else { continue };
            let focused = id == focus && !self.side_focus && !self.overlay.is_open();
            let (mut title, status) = p.header(&t, time);
            if !focused {
                title = Line::from(title.spans.into_iter().map(|s| Span::styled(s.content, s.style.remove_modifier(Modifier::BOLD).fg(t.muted))).collect::<Vec<_>>());
            }
            let sub = zoomed.then_some("zoomed · alt+z");
            let inner = ui::frame_ex(f, r, title, status, sub, focused, &t);
            self.inner.push((id, inner));
            if r.width > 24 {
                let b = Rect { x: r.right() - 4, y: r.bottom().saturating_sub(1), width: 3, height: 1 };
                if b.x > r.x + 2 && !zoomed {
                    let hot = b.contains(self.hover);
                    let style = if hot { fg(t.danger).add_modifier(Modifier::BOLD | Modifier::REVERSED) } else { fg(if focused { t.accent } else { t.frame }) };
                    ui::line(f, b, vec![Span::styled("×", style)]);
                    self.pane_close.push((b, id));
                }
            }
            let mut actions = vec![];
            if let Some(p) = self.panes.get_mut(&id) {
                let mut cx = Cx { id, theme: &t, svc: &self.svc, tx: &self.tx, actions: &mut actions, focused };
                p.render(f, inner, &mut cx);
            }
            if !actions.is_empty() {
                self.apply(id, actions);
                let _ = self.tx.send(crate::pane::Event::Tick);
            }
        }
    }

    /// Highlight the mouse selection and, after a release, copy it from the rendered cells.
    fn draw_selection(&mut self, f: &mut Frame) {
        let Some(sel) = self.sel.filter(|s| s.active) else { return };
        if !self.outer.iter().any(|(id, _)| *id == sel.pane) {
            self.sel = None;
            return;
        }
        let buf = f.buffer_mut();
        let (s, e) = sel.ordered();
        let mut text = String::new();
        for y in s.y..=e.y.min(sel.area.bottom().saturating_sub(1)) {
            let mut line = String::new();
            for x in sel.area.x..sel.area.right() {
                if !sel.contains(x, y) {
                    continue;
                }
                if let Some(c) = buf.cell_mut(Position { x, y }) {
                    line.push_str(c.symbol());
                    let st = c.style().add_modifier(Modifier::REVERSED);
                    c.set_style(st);
                }
            }
            if y > s.y {
                text.push('\n');
            }
            text.push_str(line.trim_end());
        }
        if self.copy_pending {
            self.copy_pending = false;
            let text = text.trim_end_matches('\n').to_string();
            if !text.trim().is_empty() {
                crate::clip::copy(&text);
                let n = text.chars().count();
                self.toast(Kind::Info, format!("copied {n} character{}", if n == 1 { "" } else { "s" }));
            }
        }
    }

    /// Toasts stack bottom-right; the armed prefix shows its table instead.
    fn draw_toasts(&mut self, f: &mut Frame, area: Rect) {
        let t = self.theme.clone();
        let mut y = area.bottom().saturating_sub(1);
        if self.prefix_armed {
            let text = format!(" {} ", self.keymap.prefix_hint());
            let w = (ui::width(&text) as u16 + 4).min(area.width);
            let r = Rect { x: area.right().saturating_sub(w + 1), y: y.saturating_sub(3), width: w, height: 3 };
            f.render_widget(Clear, r);
            let inner = ui::frame(f, r, &format!("{} …", self.keymap.prefix.display()), None, true, &t);
            ui::line(f, inner, vec![Span::styled(text, ui::accent(&t))]);
            return;
        }
        for toast in self.toasts.items.iter().rev() {
            let c = match toast.kind {
                Kind::Error | Kind::NeedsYou => t.danger,
                Kind::AgentDone => t.good,
                Kind::Usage => t.inline,
                Kind::Bridge => t.shine,
                Kind::Info => t.accent,
            };
            let text = ui::fit(&toast.text, (area.width as usize).saturating_sub(12).min(90));
            let w = (ui::width(&text) as u16 + 7).min(area.width);
            if y < 3 {
                break;
            }
            let r = Rect { x: area.right().saturating_sub(w + 1), y: y - 3, width: w, height: 3 };
            f.render_widget(Clear, r);
            let inner = ui::frame_ex(f, r, Line::default(), None, None, true, &crate::theme::Theme { accent: c, ..t.clone() });
            ui::line(f, inner, vec![Span::styled(format!(" {} ", toast.kind.glyph()), fg(c).add_modifier(Modifier::BOLD)), Span::styled(text, ui::bold())]);
            y -= 3;
        }
    }
}
