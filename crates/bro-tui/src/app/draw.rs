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

    pub(super) fn clamp_sidebar_width(width: u16, screen_width: u16) -> u16 {
        width.clamp(26, (screen_width / 2).max(26))
    }

    pub(super) fn project_color(&self, key: &str) -> ratatui::style::Color {
        crate::theme::project_color(*self.project_colors.get(key).unwrap_or(&crate::theme::project_color_slot(key)), &self.theme)
    }

    /// The tiled sessions by project: (key, name, panes), in tile order, each project where it first appears.
    pub(crate) fn tile_groups(&self) -> Vec<(String, String, Vec<crate::pane::PaneId>)> {
        let mut groups: Vec<(String, String, Vec<crate::pane::PaneId>)> = vec![];
        for &id in &self.stack {
            let (key, name) = self.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| (t.meta.project.key.clone(), t.meta.project.name.clone())).unwrap_or_default();
            match groups.iter_mut().find(|(k, ..)| *k == key) {
                Some((.., ids)) => ids.push(id),
                None => groups.push((key, name, vec![id])),
            }
        }
        groups
    }

    fn sync_project_colors(&mut self) {
        let mut live = self.live_infos();
        live.sort_by_key(|p| p.seq);
        let keys = self.open_infos().into_iter().map(|p| p.key).chain(live.into_iter().map(|p| p.project_key));
        for key in keys {
            if self.project_colors.contains_key(&key) {
                continue;
            }
            let preferred = crate::theme::project_color_slot(&key);
            let slot = (0..crate::theme::PROJECT_COLORS).map(|n| (preferred + n) % crate::theme::PROJECT_COLORS)
                .find(|slot| !self.project_colors.values().any(|used| used == slot)).unwrap_or(preferred);
            // Keep assignments when sessions close, so existing projects never change color mid-use.
            self.project_colors.insert(key, slot);
        }
    }

    pub(crate) fn draw(&mut self, f: &mut Frame) {
        self.sync_project_colors();
        let area = f.area();
        let t = self.theme.clone();
        if !matches!(t.bg, ratatui::style::Color::Reset) {
            f.render_widget(Block::default().style(Style::default().bg(t.bg).fg(t.fg)), area);
        }
        let side_w = if self.sidebar && area.width >= 72 {
            Self::clamp_sidebar_width(self.sidebar_width.unwrap_or((area.width / 4).clamp(30, 40)), area.width)
        } else { 0 };
        self.body = Rect { x: area.x + side_w, width: area.width - side_w, ..area };
        self.side_area = Rect { width: side_w, ..area };
        self.side_hits.clear();
        if side_w > 0 {
            self.draw_sidebar(f, self.side_area);
            let edge = Rect { x: self.body.x - 1, width: 1, ..area };
            let hot = self.side_drag || edge.contains(self.hover);
            if hot {
                f.buffer_mut().set_style(edge, ui::bold_accent(&t));
            }
            if area.height > 2 {
                ui::line(f, Rect { y: area.y + area.height / 2, height: 1, ..edge }, vec![Span::styled("↔", fg(if hot { t.accent } else { t.muted }))]);
            }
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
        let mut rects = vec![];
        // Tiled sessions from two or more projects: one colored frame per project around its sessions.
        let mut groups: Vec<(Rect, String, String, usize)> = vec![];
        let (focus, zoomed) = if self.stacked() {
            let focus = self.focused().unwrap_or(self.stack[0]);
            let by_project = if self.stack_zoom { vec![] } else { self.tile_groups() };
            rects = if self.stack_zoom {
                vec![(focus, body)]
            } else if by_project.len() >= 2 {
                let weights: Vec<usize> = by_project.iter().map(|(_, _, ids)| ids.len()).collect();
                let mut out = vec![];
                for ((key, name, ids), region) in by_project.into_iter().zip(crate::layout::group_regions(body, &weights)) {
                    // too small to spare a ring of cells: the panes keep the whole region
                    let framed = region.width >= 14 && region.height >= 6;
                    let inside = if framed { Rect { x: region.x + 1, y: region.y + 1, width: region.width - 2, height: region.height - 2 } } else { region };
                    out.extend(crate::layout::stack_rects(inside, &ids));
                    if framed {
                        groups.push((region, key, name, ids.len()));
                    }
                }
                out
            } else {
                crate::layout::stack_rects(body, &self.stack)
            };
            (focus, self.stack_zoom)
        } else {
            let tab = &self.tabs[self.cur];
            if tab.zoom {
                rects.push((tab.focus, body));
            } else {
                tab.root.rects(body, &mut rects);
            }
            (tab.focus, tab.zoom)
        };
        self.outer = rects.clone();
        for (region, key, name, count) in &groups {
            let color = self.project_color(key);
            let holds_focus = rects.iter().any(|(id, r)| *id == focus && region.contains(r.as_position())) && !self.side_focus && !self.overlay.is_open();
            ui::group_frame(f, *region, name, *count, color, holds_focus, &t);
        }
        let grouped = !groups.is_empty();
        for (id, r) in rects {
            let Some(p) = self.panes.get(&id) else { continue };
            let focused = id == focus && !self.side_focus && !self.overlay.is_open();
            let (mut title, status) = p.header(&t, time);
            if !focused {
                title = Line::from(title.spans.into_iter().map(|s| Span::styled(s.content, s.style.remove_modifier(Modifier::BOLD).fg(t.muted))).collect::<Vec<_>>());
            }
            let sub = zoomed.then(|| format!("zoomed · {}", self.keymap.primary(crate::keymap::Act::Zoom)));
            let project = p.as_term_ref().map(|p| &p.meta.project);
            let color = project.map(|p| self.project_color(&p.key)).unwrap_or(t.accent);
            let inner = if let Some(project) = project {
                // inside a project frame the group title already names the project
                let label = if grouped && groups.iter().any(|(region, ..)| region.contains(r.as_position())) { "" } else { project.name.as_str() };
                ui::session_frame(f, r, title, status, sub.as_deref(), label, color, focused, &t)
            } else {
                ui::frame_ex(f, r, title, status, sub.as_deref(), focused, &t)
            };
            self.inner.push((id, inner));
            if r.width > 24 {
                let b = Rect { x: r.right() - 4, y: r.bottom().saturating_sub(1), width: 3, height: 1 };
                if b.x > r.x + 2 && !zoomed {
                    let hot = b.contains(self.hover);
                    let style = if hot { fg(t.danger).add_modifier(Modifier::BOLD | Modifier::REVERSED) } else { fg(color) };
                    ui::line(f, b, vec![Span::styled("×", style)]);
                    self.pane_close.push((b, id));
                }
            }
            let mut actions = vec![];
            if let Some(p) = self.panes.get_mut(&id) {
                let mut cx = Cx { id, theme: &t, svc: &self.svc, tx: &self.tx, actions: &mut actions, focused };
                p.render(f, inner, &mut cx);
            }
            // a prompt option under the mouse lights up: it can be clicked
            if inner.contains(self.hover)
                && let Some(term) = self.panes.get(&id).and_then(|p| p.as_term_ref())
                && !self.panes.get(&id).is_some_and(|p| p.wants_mouse())
                && term.prompt_option_at(self.hover.y - inner.y).is_some()
            {
                let row = Rect { x: inner.x, y: self.hover.y, width: inner.width, height: 1 };
                f.buffer_mut().set_style(row, ratatui::style::Style::default().bg(crate::theme::project_tint(color, &t)).add_modifier(Modifier::BOLD));
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

    /// Compact toasts stack top-right; the armed prefix keeps its table at the bottom.
    pub(super) fn draw_toasts(&mut self, f: &mut Frame, area: Rect) {
        let t = self.theme.clone();
        if self.prefix_armed {
            let text = format!(" {} ", self.keymap.prefix_hint());
            let w = (ui::width(&text) as u16 + 4).min(area.width);
            let r = Rect { x: area.right().saturating_sub(w + 1), y: area.bottom().saturating_sub(4), width: w, height: 3 };
            f.render_widget(Clear, r);
            let inner = ui::frame(f, r, &format!("{} …", self.keymap.prefix.display()), None, true, &t);
            ui::line(f, inner, vec![Span::styled(text, ui::accent(&t))]);
            return;
        }
        if area.width < 8 || area.height < 3 {
            return;
        }
        let mut y = area.y + 1;
        for toast in self.toasts.items.iter().rev() {
            let c = match toast.kind {
                Kind::Error | Kind::NeedsYou => t.danger,
                Kind::AgentDone => t.good,
                Kind::Usage => t.inline,
                Kind::Bridge => t.shine,
                Kind::Info => t.accent,
            };
            let text = toast.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let text = ui::fit(&text, (area.width as usize).saturating_sub(6).min(60));
            let w = ui::width(&text) as u16 + 4;
            if y >= area.bottom().saturating_sub(1) {
                break;
            }
            let r = Rect { x: area.right() - w - 1, y, width: w, height: 1 };
            f.render_widget(Clear, r);
            f.render_widget(Block::default().style(Style::default().bg(t.bg).fg(t.fg)), r);
            ui::line(f, r, vec![Span::styled(format!(" {} ", toast.kind.glyph()), fg(c)), Span::styled(text, fg(t.fg))]);
            y += 1;
        }
    }
}
