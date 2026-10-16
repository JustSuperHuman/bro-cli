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
        crate::theme::project_color(key, &self.theme)
    }

    /// The tiled sessions by project: (key, name, panes), in tile order, each project where it first appears.
    /// The orchestrator isn't in any: it has its own column (see [`App::orchestrator_column`]).
    pub(crate) fn tile_groups(&self) -> Vec<(String, String, Vec<crate::pane::PaneId>)> {
        let mut groups: Vec<(String, String, Vec<crate::pane::PaneId>)> = vec![];
        let column = self.orchestrator_column(self.body_panes()).map(|(id, _)| id);
        for &id in self.stack.iter().filter(|id| Some(**id) != column) {
            let (key, name) = self.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| (t.meta.project.key.clone(), t.meta.project.name.clone())).unwrap_or_default();
            match groups.iter_mut().find(|(k, ..)| *k == key) {
                Some((.., ids)) => ids.push(id),
                None => groups.push((key, name, vec![id])),
            }
        }
        groups
    }

    /// Tiled with other sessions, the orchestrator takes a full-height column on the right of `body`: (its pane,
    /// its column). The rest tile in what's left.
    pub(crate) fn orchestrator_column(&self, body: Rect) -> Option<(crate::pane::PaneId, Rect)> {
        if !self.stacked() || self.stack_zoom || self.stack.len() < 2 {
            return None;
        }
        let id = self.stack.iter().copied().find(|id| self.panes.get(id).and_then(|p| p.as_term_ref()).is_some_and(|t| t.meta.orchestrator))?;
        // a third of the width, never so narrow it can't work nor so wide the others starve
        let w = (body.width / 3).clamp(44.min(body.width / 2), 96).min(body.width / 2);
        (w >= 20).then(|| (id, Rect { x: body.right() - w, width: w, ..body }))
    }

    pub(crate) fn draw(&mut self, f: &mut Frame) {
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
            let handle = Rect { y: area.y + area.height / 2, height: 1, ..edge };
            // the scrollbar thumb passes over the resize handle
            if area.height > 2 && (hot || !self.side_thumb.intersects(handle)) {
                ui::line(f, handle, vec![Span::styled("↔", fg(if hot { t.accent } else { t.muted }))]);
            }
        }
        self.pane_close.clear();
        self.pane_minimize.clear();
        self.outer.clear();
        self.inner.clear();
        if self.tabs.is_empty() {
            self.draw_welcome(f, self.body);
        } else {
            self.draw_panes(f);
        }
        self.draw_selection(f);
        // the backdrop fills whatever is still empty; toasts and overlays then sit on top of it
        let time = self.start.elapsed().as_secs_f64();
        for (r, bd, skip) in self.backdrops(area) {
            crate::skin::backdrop(f.buffer_mut(), r, &t, time, bd, &skip);
        }
        self.draw_alt_hints(f);
        self.draw_toasts(f, area);
        self.draw_overlay(f, area);
        crate::skin::effects(f.buffer_mut(), area, &t);
    }

    /// Where backdrops go this frame, in order (earlier ones win the space), each with the rects it keeps out
    /// of: the rain setting over the theme's own. In "working" mode rain falls in the sidebar and around the
    /// sessions all the time, and inside a session only while its agent is mid-turn.
    pub(super) fn backdrops(&self, area: Rect) -> Vec<(Rect, crate::skin::Backdrop, Vec<Rect>)> {
        use crate::skin::Backdrop;
        use bro_core::config::RainMode;
        let own = self.theme.skin.backdrop;
        match self.svc.settings().rain {
            RainMode::Off => vec![(area, own, vec![])],
            RainMode::Everywhere => vec![(area, Backdrop::Rain, vec![])],
            RainMode::Working => {
                // whole panes, frame included, so nothing lands on a resting session's edges
                let resting: Vec<Rect> = self.outer.iter().filter(|(id, _)| !self.panes.get(id).is_some_and(|p| p.is_terminal() && p.busy())).map(|(_, r)| *r).collect();
                vec![(area, Backdrop::Rain, resting), (area, if own == Backdrop::Rain { Backdrop::None } else { own }, vec![])]
            }
        }
    }

    /// The visible sessions whose agent is mid-turn.
    pub(super) fn working_rects(&self) -> Vec<Rect> {
        self.inner.iter().filter(|(id, _)| self.panes.get(id).is_some_and(|p| p.is_terminal() && p.busy())).map(|(_, r)| *r).collect()
    }

    fn draw_panes(&mut self, f: &mut Frame) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let body = self.body_panes();
        let mut rects = vec![];
        // Tiled sessions from two or more projects: one colored frame per project around its sessions.
        let mut groups: Vec<(Rect, String, String, usize)> = vec![];
        // every project region (framed or not) and, per entry of `groups`, its index here: the priority grow
        // resizes the regions with the panes
        let mut regions: Vec<(Rect, bool)> = vec![];
        let mut group_tags: Vec<usize> = vec![];
        // the orchestrator's column on the right; the others tile beside it
        let column = self.orchestrator_column(body);
        let body = match column {
            Some((_, c)) => Rect { width: body.width - c.width, ..body },
            None => body,
        };
        let tiled: Vec<crate::pane::PaneId> = self.stack.iter().copied().filter(|id| column.is_none_or(|(c, _)| c != *id)).collect();
        let (focus, zoomed) = if self.stacked() {
            let focus = self.focused().unwrap_or(0);
            let by_project = if self.stack_zoom { vec![] } else { self.tile_groups() };
            rects = if self.stack_zoom {
                if focus == 0 { vec![] } else { vec![(focus, body)] }
            } else if by_project.len() >= 2 {
                let weights: Vec<usize> = by_project.iter().map(|(_, _, ids)| ids.len()).collect();
                let mut out = vec![];
                for ((key, name, ids), region) in by_project.into_iter().zip(crate::layout::group_regions(body, &weights)) {
                    // too small to spare a ring of cells: the panes keep the whole region
                    let framed = region.width >= 14 && region.height >= 6;
                    let inside = if framed { Rect { x: region.x + 1, y: region.y + 1, width: region.width - 2, height: region.height - 2 } } else { region };
                    out.extend(crate::layout::stack_rects(inside, &ids));
                    if framed {
                        group_tags.push(regions.len());
                        groups.push((region, key, name, ids.len()));
                    }
                    regions.push((region, framed));
                }
                out
            } else {
                crate::layout::stack_rects(body, &tiled)
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
        // A ctrl+clicked session grows to a working size inside this arrangement; its neighbours give way.
        let mut folded = vec![];
        let grown = self.priority.filter(|_| !zoomed).and_then(|id| crate::priority::prioritize(body, &rects, &regions, id, crate::priority::want(body)));
        self.grown = grown.is_some();
        if let Some(g) = grown {
            for (tag, r) in g.frames {
                if let Some(i) = group_tags.iter().position(|t| *t == tag) {
                    groups[i].0 = r;
                }
            }
            rects = g.panes;
            folded = g.folded;
        }
        // the column stays put while a tile beside it grows
        rects.extend(column);
        self.outer = rects.clone();
        if self.tile_all && rects.is_empty() && !self.minimized.is_empty() {
            let message = "All sessions minimized · show minimized in the sidebar";
            let width = ui::width(message) as u16;
            if body.width >= width && body.height > 0 {
                let row = Rect { x: body.x + (body.width - width) / 2, y: body.y + body.height / 2, width, height: 1 };
                ui::line(f, row, vec![Span::styled(message, ui::muted(&t))]);
            }
        }
        for (region, key, name, count) in &groups {
            let color = self.project_color(key);
            let holds_focus = rects.iter().any(|(id, r)| *id == focus && region.contains(r.as_position())) && !self.side_focus && !self.overlay.is_open();
            ui::group_frame(f, *region, name, *count, color, holds_focus, &t);
        }
        let grouped = !groups.is_empty();
        let now = std::time::Instant::now();
        self.card_links.clear();
        self.flips.retain(|id, flip| self.panes.contains_key(id) && (flip.back || flip.turning(now)));
        for (id, r) in rects {
            let Some(p) = self.panes.get(&id) else { continue };
            let focused = id == focus && !self.side_focus && !self.overlay.is_open();
            let (mut title, status) = p.header(&t, time);
            if !focused {
                title = Line::from(title.spans.into_iter().map(|s| Span::styled(s.content, s.style.remove_modifier(Modifier::BOLD).fg(t.muted))).collect::<Vec<_>>());
            }
            let sub = if self.flips.get(&id).is_some_and(|f| f.back) {
                Some("back · alt+double-click".to_string())
            } else if zoomed {
                Some(format!("zoomed · {}", self.keymap.primary(crate::keymap::Act::Zoom)))
            } else {
                (self.grown && self.priority == Some(id)).then(|| format!("grown · {} restores", self.keymap.primary(crate::keymap::Act::Shrink)))
            };
            let folded = folded.contains(&id);
            let label: String = title.spans.iter().map(|s| s.content.as_ref()).collect();
            let project = p.as_term_ref().map(|p| &p.meta.project);
            let color = project.map(|p| self.project_color(&p.key)).unwrap_or(t.accent);
            let inner = if let Some(project) = project {
                // inside a project frame the group title already names the project
                let label = if grouped && groups.iter().any(|(region, ..)| region.contains(r.as_position())) { "" } else { project.name.as_str() };
                let minimize = (self.tile_all && r.width >= 5).then_some(self.minimized.contains(&id));
                let button = Rect { x: r.x + 1, y: r.y, width: 3, height: 1 };
                let hot = minimize.is_some() && button.contains(self.hover);
                if minimize.is_some() {
                    self.pane_minimize.push((button, id));
                }
                let standout = p.as_term_ref().is_some_and(|t| t.meta.orchestrator);
                ui::session_frame_as(f, r, title, status, sub.as_deref(), label, minimize, hot, color, focused, standout, &t)
            } else {
                ui::frame_ex(f, r, title, status, sub.as_deref(), focused, &t)
            };
            // a folded pane takes no clicks inside: one focuses it, which unfolds it
            if !folded {
                self.inner.push((id, inner));
            }
            if r.width > 24 {
                let b = Rect { x: r.right() - 4, y: r.bottom().saturating_sub(1), width: 3, height: 1 };
                if b.x > r.x + 2 && !zoomed {
                    let hot = b.contains(self.hover);
                    let style = if hot { fg(t.danger).add_modifier(Modifier::BOLD | Modifier::REVERSED) } else { fg(color) };
                    ui::line(f, b, vec![Span::styled("×", style)]);
                    self.pane_close.push((b, id));
                }
            }
            // Folded out of the way of the grown session: just its title (down the strip when narrow); the program
            // keeps its size until it unfolds.
            if folded {
                if inner.width > 0 && inner.width < crate::priority::USEFUL[0] && inner.height > 1 {
                    for (i, c) in label.split(" — ").next().unwrap_or_default().trim().chars().filter(|c| !c.is_whitespace()).take(inner.height as usize).enumerate() {
                        let cell = Rect { x: inner.x + (inner.width - 1) / 2, y: inner.y + i as u16, width: 1, height: 1 };
                        ui::line(f, cell, vec![Span::styled(c.to_string(), fg(color))]);
                    }
                }
                continue;
            }
            let mut actions = vec![];
            if let Some(p) = self.panes.get_mut(&id) {
                let mut cx = Cx { id, theme: &t, svc: &self.svc, tx: &self.tx, actions: &mut actions, focused };
                let started = std::time::Instant::now();
                p.render(f, inner, &mut cx);
                self.perf.pane_rendered(id, started.elapsed());
            }
            // a prompt option under the mouse lights up: it can be clicked
            if inner.contains(self.hover)
                && !self.flips.contains_key(&id)
                && let Some(term) = self.panes.get(&id).and_then(|p| p.as_term_ref())
                && !self.panes.get(&id).is_some_and(|p| p.wants_mouse())
                && term.prompt_option_at(self.hover.y - inner.y).is_some()
            {
                let row = Rect { x: inner.x, y: self.hover.y, width: inner.width, height: 1 };
                f.buffer_mut().set_style(row, ratatui::style::Style::default().bg(crate::theme::project_tint(color, &t)).add_modifier(Modifier::BOLD));
            }
            // Turned over: the back covers the program (which keeps running and sized underneath).
            if let Some(flip) = self.flips.get(&id).copied() {
                let (back, cover) = flip.face(now);
                if back {
                    let links = self.draw_card_back(f, id, inner, color, time);
                    // a link or file under the mouse lights up: it opens on click
                    if let Some((r, _)) = links.iter().find(|(r, _)| r.contains(self.hover)) {
                        f.buffer_mut().set_style(*r, Style::default().bg(crate::theme::mix(color, t.bg, 0.6)));
                    }
                    self.card_links.extend(links.into_iter().map(|(r, uri)| (r, id, uri)));
                }
                crate::card::squash(f.buffer_mut(), inner, cover, color, &t);
            }
            if !actions.is_empty() {
                self.apply(id, actions);
                let _ = self.tx.send(crate::pane::Event::Tick);
            }
        }
    }

    /// The back of session `id`'s card, from what its transcript worker and the attention tracker already hold;
    /// returns the rows that open something when clicked.
    fn draw_card_back(&self, f: &mut Frame, id: crate::pane::PaneId, area: Rect, color: ratatui::style::Color, time: f64) -> Vec<(Rect, String)> {
        let Some(term) = self.panes.get(&id).and_then(|p| p.as_term_ref()) else { return vec![] };
        let snap = term.insights();
        let facts = snap.as_ref().and_then(|s| s.facts.as_deref());
        let view = self.pane_view(id).filter(|_| term.meta.harness.is_some());
        // your name for it, else the one the agent gave the session, else what the header shows
        let title = if term.meta.renamed { None } else { facts.and_then(|f| f.custom_title.clone().or_else(|| f.ai_title.clone())) };
        let card = crate::card::Card {
            title: title.unwrap_or_else(|| term.display_name()),
            who: term.meta.label.clone(),
            cwd: &term.meta.cwd,
            open_for: term.meta.started.elapsed(),
            state: view.as_ref().map(|v| v.state),
            reason: view.as_ref().and_then(|v| v.reason.clone()),
            facts,
            recap: view.and_then(|v| v.recap),
            now_ms: chrono::Utc::now().timestamp_millis(),
            time,
            links: self.card_links_for(term, facts),
        };
        crate::card::render(f, area, &card, color, &self.theme)
    }

    /// The addresses for a card: what the session mentioned, then what's on its screen (a dev server started in
    /// a shell), each with whether it answers when it's on this machine.
    fn card_links_for(&self, term: &crate::panes::term::Term, facts: Option<&bro_core::sessions::live::AgentFacts>) -> Vec<crate::card::Link> {
        let mut urls: Vec<String> = term.screen_urls();
        for u in facts.map(|f| f.urls.as_slice()).unwrap_or_default() {
            if !urls.contains(&u.url) {
                urls.push(u.url.clone());
            }
        }
        urls.into_iter()
            .map(|url| {
                let local = bro_core::sessions::live::SeenUrl { url: url.clone(), at_ms: None }.is_local();
                let up = if local { self.probe.up(&url) } else { None };
                crate::card::Link { url, local, up }
            })
            .collect()
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
            ui::clear(f, r, &t);
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
