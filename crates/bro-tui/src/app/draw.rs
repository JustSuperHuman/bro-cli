//! Drawing the whole screen: sidebar, tab bar, panes in their frames, the selection, toasts, the armed-prefix
//! hint and overlays.

use super::App;
use crate::alerts::Kind;
use crate::pane::Cx;
use crate::ui::{self, fg, muted};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear},
};

impl App {
    /// Where panes go: the body minus the tab bar (always shown once there's a tab).
    pub(super) fn body_panes(&self) -> Rect {
        if !self.tabs.is_empty() && self.body.height > 3 { Rect { y: self.body.y + 1, height: self.body.height - 1, ..self.body } } else { self.body }
    }

    pub(crate) fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let t = self.theme.clone();
        if !matches!(t.bg, ratatui::style::Color::Reset) {
            f.render_widget(Block::default().style(Style::default().bg(t.bg)), area);
        }
        let side_w = if self.sidebar && area.width >= 72 { (area.width / 4).clamp(30, 40) } else { 0 };
        self.body = Rect { x: area.x + side_w, width: area.width - side_w, ..area };
        self.side_hits.clear();
        if side_w > 0 {
            self.draw_sidebar(f, Rect { width: side_w, ..area });
        }
        self.tab_hits.clear();
        self.new_tab_hit = None;
        self.pane_close.clear();
        self.outer.clear();
        self.inner.clear();
        if self.tabs.is_empty() {
            self.draw_welcome(f, self.body);
        } else {
            if self.body.height > 3 {
                self.draw_tab_bar(f, Rect { height: 1, ..self.body });
            }
            self.draw_panes(f);
        }
        self.draw_selection(f);
        self.draw_toasts(f, area);
        self.draw_overlay(f, area);
    }

    fn draw_tab_bar(&mut self, f: &mut Frame, r: Rect) {
        let t = self.theme.clone();
        let label_of = |app: &App, i: usize| ui::fit(&app.tab_label(i), 20);
        // scroll so the current tab is visible
        let new_key = self.keymap.primary(crate::keymap::Act::NewSession);
        let new_label = format!(" + new  {new_key} ");
        let new_w = ui::width(&new_label) as u16;
        let r = Rect { width: r.width.saturating_sub(new_w + 1), ..r };
        let widths: Vec<u16> = (0..self.tabs.len()).map(|i| ui::width(&label_of(self, i)) as u16 + 8).collect();
        let mut start = 0;
        while start < self.cur && widths[start..=self.cur].iter().sum::<u16>() + 4 > r.width {
            start += 1;
        }
        let mut x = r.x + 1;
        if start > 0 {
            ui::line(f, Rect { x, y: r.y, width: 2, height: 1 }, vec![Span::styled("‹ ", muted(&t))]);
            x += 2;
        }
        for i in start..self.tabs.len() {
            let tab = &self.tabs[i];
            let p = self.panes.get(&tab.focus);
            let term = p.and_then(|p| p.as_term_ref());
            let glyph = match term {
                Some(tm) => ui::harness_glyph(tm.meta.harness).to_string(),
                None => ui::icon(p.map(|p| p.icon()).unwrap_or("window")).to_string(),
            };
            let brand = term.map(|tm| ui::harness_color(tm.meta.harness, &t)).unwrap_or(t.shine);
            let n = tab.root.leaf_ids().len();
            let label = label_of(self, i);
            let extra = if n > 1 { format!(" ⊞{n}") } else { String::new() };
            let on = i == self.cur;
            let text_w = ui::width(&format!(" {glyph} {label}{extra} ")) as u16;
            if x + text_w > r.right() {
                ui::line(f, Rect { x, y: r.y, width: r.right().saturating_sub(x), height: 1 }, vec![Span::styled(" …", muted(&t))]);
                break;
            }
            let base = if on { Style::default().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED) } else { muted(&t) };
            let spans = vec![
                Span::styled(" ", base),
                Span::styled(format!("{glyph} "), if on { base } else { fg(brand) }),
                Span::styled(label, base),
                Span::styled(extra, base),
                Span::styled(" ", base),
            ];
            let cell = Rect { x, y: r.y, width: text_w, height: 1 };
            ui::line(f, cell, spans);
            self.tab_hits.push((cell, i));
            x += text_w + 1;
        }
        // "+ new" right after the last tab: the obvious way to start another session
        let cell = Rect { x: x.min(r.right() + 1), y: r.y, width: new_w, height: 1 };
        let key_style = muted(&t);
        ui::line(f, cell, vec![Span::styled(" + new ", fg(t.shine).add_modifier(Modifier::BOLD)), Span::styled(format!(" {new_key} "), key_style)]);
        self.new_tab_hit = Some(cell);
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
