//! Drawing the sidebar: projects with live sessions, the New/Continue strip, then the footer
//! blocks (usage — Claude / Codex totals or every profile — proxy, bridge).

use super::{App, SideHit};
use crate::pane::Activity;
use crate::panes::term::status_look;
use crate::services::Avail;
use crate::sidebar::Row;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::usage::{AppSummary, LoginUsage};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::time::{Duration, Instant};

/// The sidebar scrollbar stays solid this long after the last scroll or move…
const BAR_HOLD: Duration = Duration::from_millis(800);
/// …then fades into the border over this long.
const BAR_FADE: Duration = Duration::from_millis(250);
/// Redraw this often while it fades.
const BAR_FRAME: Duration = Duration::from_millis(40);

/// How solid the scrollbar is `since` the sidebar last moved: 1 → 0 over the fade, None once it's gone.
pub(super) fn bar_alpha(since: Duration) -> Option<f32> {
    if since < BAR_HOLD {
        Some(1.0)
    } else if since < BAR_HOLD + BAR_FADE {
        Some(1.0 - (since - BAR_HOLD).as_secs_f32() / BAR_FADE.as_secs_f32())
    } else {
        None
    }
}

/// When the loop must wake to fade or hide the scrollbar.
pub(super) fn bar_wake(moved: Option<Instant>) -> Option<Duration> {
    let since = moved?.elapsed();
    if since < BAR_HOLD {
        Some(BAR_HOLD - since)
    } else if since < BAR_HOLD + BAR_FADE {
        Some(BAR_FRAME)
    } else {
        None
    }
}

/// The thumb on a track `track` cells long: (offset, length), for `room` of `total` lines scrolled `pos` of `max`.
fn thumb(track: u16, room: u16, total: u16, pos: u16, max: u16) -> (u16, u16) {
    let len = ((u32::from(track) * u32::from(room)) / u32::from(total.max(1))).clamp(1, u32::from(track)) as u16;
    let off = (u32::from(track - len) * u32::from(pos.min(max)) / u32::from(max.max(1))) as u16;
    (off, len)
}

/// One line of the usage block: a glyph + label, then the 5h / week figures (% left).
pub(super) struct UsageLine {
    pub(super) harness: Option<bro_core::Harness>,
    pub(super) profile: Option<String>,
    pub(super) label: String,
    pub(super) h5: Option<f64>,
    pub(super) wk: Option<f64>,
    /// the 5h figure was capped by the week
    pub(super) approx: bool,
    /// still loading / couldn't be read
    pub(super) note: Option<String>,
    /// countdown to the tighter window's reset (expanded view)
    pub(super) resets_at: Option<i64>,
    /// the Claude total with a Fable line folded under it: Some(open)
    pub(super) fable: Option<bool>,
}

/// The session counter over proxy and phone: every session running, and how many of them are working, waiting on
/// you or finished unseen (the rest are idle), as the list's own statuses add up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Tally {
    pub(super) total: usize,
    pub(super) working: usize,
    pub(super) blocked: usize,
    pub(super) done: usize,
}

/// A status the counter shows on its own (idle sessions only count in the total).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Working,
    NeedsYou,
    Done,
}

impl Status {
    /// The session rows' word for it.
    pub(super) fn word(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::NeedsYou => "needs you",
            Status::Done => "done",
        }
    }
}

impl Tally {
    /// The statuses there are, with how many, in the order the project tabs show them.
    pub(super) fn statuses(&self) -> impl Iterator<Item = (Status, usize)> {
        [(Status::Working, self.working), (Status::NeedsYou, self.blocked), (Status::Done, self.done)].into_iter().filter(|(_, n)| *n > 0)
    }

    /// All of it in words: "1 working · 1 needs you · 1 done · 2 idle".
    pub(super) fn words(&self) -> String {
        let idle = self.total.saturating_sub(self.working + self.blocked + self.done);
        let mut parts: Vec<String> = self.statuses().map(|(s, n)| format!("{n} {}", s.word())).collect();
        if idle > 0 {
            parts.push(format!("{idle} idle"));
        }
        parts.join(" · ")
    }
}

impl App {
    pub(super) fn draw_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let focused = self.side_focus && !self.overlay.is_open();
        let title = Line::from(vec![Span::raw(" "), Span::styled("bro", ui::bold_accent(&t)), Span::raw(" ")]);
        let clock = Line::from(Span::styled(format!(" {} ", crate::util::clock()), muted(&t)));
        let sub = if focused { "? keys" } else { "alt+b" };
        // the gear sits right after the title, however the skin dresses it
        let title_w = crate::skin::title(title.clone(), t.accent, &t).width() as u16;
        let inner = ui::frame_ex(f, area, title, Some(clock), Some(sub), focused, &t);
        let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(1), ..inner };
        self.side_thumb = Rect::default();
        // the settings gear sits in the top border, right after " bro "
        if area.width >= 20 + title_w && area.height > 0 {
            let gear = Rect { x: area.x + 1 + title_w, y: area.y, width: 3, height: 1 };
            let style = if gear.contains(self.hover) { ui::bold_accent(&t).bg(current_tint(&t)) } else { muted(&t) };
            ui::line(f, gear, vec![Span::styled(format!(" {} ", ui::icon("settings")), style)]);
            self.side_hits.push((gear, SideHit::Settings));
        }

        // footer first (fixed height), rows get the rest
        let usage = self.usage_lines();
        let foot_h = footer_height(usage.len() as u16, self.usage_expanded, inner.height);
        // the one-row session strip sits above the footer, with a blank line under it when there's room
        let strip_h = if inner.height >= foot_h + 10 {
            2
        } else if inner.height >= foot_h + 6 {
            1
        } else {
            0
        };
        let sessions = self.panes.values().filter(|p| p.is_terminal()).count();
        // the project tabs have All and the minimized switch: no tile row then
        let tile_h = u16::from(self.strip.height == 0 && sessions >= 2 && inner.height >= foot_h + strip_h + 10);
        let list = Rect { height: inner.height.saturating_sub(foot_h + strip_h + tile_h), ..inner };
        if foot_h > 0 {
            self.draw_footer(f, Rect { y: inner.bottom() - foot_h, height: foot_h, ..inner }, &usage, &t);
        }
        if strip_h > 0 {
            self.draw_strip(f, Rect { y: list.bottom() + tile_h, height: strip_h, ..inner }, &t);
        }
        if tile_h > 0 {
            let row = Rect { y: list.bottom(), height: 1, ..inner };
            if self.tile_all {
                let minimized = format!("{} minimized {}", if self.show_minimized { "▾" } else { "▸" }, self.minimized.len());
                let min_w = ui::width(&minimized) as u16;
                let min_hit = Rect { x: row.right().saturating_sub(min_w), width: min_w.min(row.width), ..row };
                let tile_hit = Rect { width: min_hit.x.saturating_sub(row.x), ..row };
                let tile_style = if tile_hit.contains(self.hover) { ui::bold_accent(&t) } else { muted(&t) };
                let min_style = if self.show_minimized || min_hit.contains(self.hover) { ui::bold_accent(&t) } else { muted(&t) };
                ui::line_lr(f, row, vec![Span::styled(format!("▦ tiled {}", self.stack.len()), tile_style)],
                    vec![Span::styled(minimized, min_style)]);
                self.side_hits.push((tile_hit, SideHit::TileAll));
                self.side_hits.push((min_hit, SideHit::Minimized));
            } else {
                let style = if row.contains(self.hover) { ui::bold_accent(&t) } else { muted(&t) };
                ui::line_lr(f, row, vec![Span::styled(format!("▦ tile all · {sessions}"), style)],
                    vec![Span::styled(format!("{} ", self.keymap.primary(crate::keymap::Act::TileAll)), muted(&t))]);
                self.side_hits.push((row, SideHit::TileAll));
            }
        }

        let rows = self.rows();
        self.side_sel = self.side_sel.min(rows.len().saturating_sub(1));
        let focus_pane = self.focused();
        let cur_key = self.current_project().map(|r| self.svc.project_for(&r).key);
        let mut y = list.y;
        if (self.side_filtering || !self.side.filter.is_empty()) && y < list.bottom() {
            let cursor = if self.side_filtering { "▏" } else { "" };
            ui::line(f, Rect { y, height: 1, ..list }, vec![Span::styled("/ ", ui::bold_accent(&t)), Span::styled(format!("{}{cursor}", self.side.filter), ui::bold())]);
            y += 1;
        }
        let projects = &rows[..];
        if projects.is_empty() {
            self.side_scroll = 0;
            if y < list.bottom() {
                let tasks = Rect { y, height: 1, ..list };
                ui::line_lr(f, tasks, vec![Span::styled(" Tasks · outcomes", ui::bold_accent(&t))], vec![Span::styled(format!(" {} ", self.keymap.primary(crate::keymap::Act::Tasks)), muted(&t))]);
                self.side_hits.push((tasks, SideHit::Tasks));
                y += 1;
            }
            if !self.side.filter.is_empty() && y < list.bottom() {
                ui::line(f, Rect { y, height: 1, ..list }, vec![Span::styled(" nothing matches", muted(&t))]);
            }
            return;
        }
        // rows have heights (projects: a gap line + two lines)
        let lines: Vec<(u16, u16)> = row_lines(projects);
        // The orchestrator is sticky: drawn above the list, which scrolls beneath a hairline under it.
        let pinned = usize::from(matches!(projects.first(), Some(Row::Live { info, .. }) if info.orchestrator));
        if pinned == 1 {
            let h = lines[0].1;
            if y + h > list.bottom() {
                return;
            }
            self.draw_side_row(f, Rect { y, height: h, ..list }, 0, &projects[0], focused, focus_pane, cur_key.as_deref(), &t, time);
            y += h;
            if y < list.bottom() {
                let gold = crate::theme::gold(&t);
                ui::line(f, Rect { y, height: 1, ..list }, vec![Span::styled("╌".repeat(list.width as usize), fg(crate::theme::mix(gold, t.frame, 0.7)))]);
                y += 1;
            }
        }
        if y < list.bottom() {
            let tasks = Rect { y, height: 1, ..list };
            ui::line_lr(f, tasks, vec![Span::styled(" Tasks · outcomes", ui::bold_accent(&t))], vec![Span::styled(format!(" {} ", self.keymap.primary(crate::keymap::Act::Tasks)), muted(&t))]);
            self.side_hits.push((tasks, SideHit::Tasks));
            y += 1;
        }
        let room = list.bottom().saturating_sub(y);
        if room == 0 || projects.len() == pinned {
            self.side_scroll = pinned;
            return;
        }
        // scroll the rest so the selection stays in view
        let total = |a: usize, b: usize| lines[a..=b].iter().map(|(g, h)| g + h).sum::<u16>();
        self.side_scroll = self.side_scroll.max(pinned);
        if self.side_focus && !self.side_manual_scroll && self.side_sel >= pinned {
            let selected = self.side_sel;
            if selected < self.side_scroll {
                self.side_scroll = selected;
            }
            while self.side_scroll < selected && total(self.side_scroll, selected) > room {
                self.side_scroll += 1;
            }
        }
        let mut last_start = projects.len();
        let mut acc = 0;
        while last_start > pinned && acc + lines[last_start - 1].0 + lines[last_start - 1].1 <= room {
            last_start -= 1;
            acc += lines[last_start].0 + lines[last_start].1;
        }
        self.side_scroll = self.side_scroll.min(last_start);
        if self.side_scroll != self.side_drawn_scroll {
            self.side_drawn_scroll = self.side_scroll;
            self.side_nav();
        }
        // only when the list overflows, over the right border (fading into it)
        if last_start > pinned
            && let Some(a) = self.side_nav_at.and_then(|at| bar_alpha(at.elapsed()))
            && area.width > 0
        {
            let span = |r: std::ops::Range<usize>| lines[r].iter().map(|(g, h)| g + h).sum::<u16>();
            let track = list.bottom().saturating_sub(y);
            let (off, len) = thumb(track, room, span(pinned..projects.len()), span(pinned..self.side_scroll), span(pinned..last_start));
            let border = if focused { t.accent } else { t.frame };
            self.side_thumb = Rect { x: area.right() - 1, y: y + off, width: 1, height: len };
            for cy in self.side_thumb.y..self.side_thumb.bottom() {
                if let Some(c) = f.buffer_mut().cell_mut((self.side_thumb.x, cy)) {
                    c.set_symbol("┃").set_fg(crate::theme::mix(t.muted, border, 1.0 - a));
                }
            }
        }
        for (offset, row) in projects.iter().enumerate().skip(self.side_scroll) {
            let (gap, h) = lines[offset];
            // no gap above the first row on screen
            let gap = if offset == self.side_scroll { 0 } else { gap };
            if y + gap + h > list.bottom() {
                break;
            }
            y += gap;
            self.draw_side_row(f, Rect { y, height: h, ..list }, offset, row, focused, focus_pane, cur_key.as_deref(), &t, time);
            y += h;
        }
    }

    /// One sidebar row at `r` (row `i`): its selection / current highlight, the row itself, and its click targets.
    #[allow(clippy::too_many_arguments)]
    fn draw_side_row(&mut self, f: &mut Frame, r: Rect, i: usize, row: &Row, focused: bool, focus_pane: Option<crate::layout::PaneId>, cur_key: Option<&str>, t: &Theme, time: f64) {
        let selected = focused && i == self.side_sel;
        let current = matches!(row, Row::Live { info, .. } if focus_pane == Some(info.pane) && !self.side_focus);
        if selected {
            f.buffer_mut().set_style(r, Style::default().bg(crate::theme::mix(t.user, ratatui::style::Color::Rgb(20, 20, 24), 0.35)));
        } else if current {
            let color = match row {
                Row::Live { info, .. } => self.project_color(&info.project_key),
                _ => t.accent,
            };
            f.buffer_mut().set_style(r, Style::default().bg(crate::theme::project_tint(color, t)));
        }
        self.draw_row(f, r, row, selected, focus_pane, cur_key, t, time);
        if let Row::Project { root, live, .. } = row {
            self.side_hits.push((Rect { x: r.right().saturating_sub(1), width: 1, height: 1, ..r }, SideHit::CloseProject(root.clone(), *live)));
        }
        if let Row::Live { info, .. } = row
            && self.renaming.as_ref().is_none_or(|(id, _)| *id != info.pane)
        {
            // the × cell wins over the row (hits are searched in order)
            self.side_hits.push((Rect { x: r.right().saturating_sub(1), width: 1, height: 1, ..r }, SideHit::Close(info.pane)));
            // so does its team, which opens the team board
            if info.team.is_some() {
                let top = r.y + 1 + u16::from(info.note.is_some());
                self.side_hits.push((Rect { y: top, height: r.bottom().saturating_sub(top), ..r }, SideHit::Team(info.pane)));
            }
        }
        self.side_hits.push((r, SideHit::Row(i)));
    }

    /// The sidebar scrolled or its selection moved: show the scrollbar for a moment.
    pub(super) fn side_nav(&mut self) {
        self.side_nav_at = Some(Instant::now());
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_row(&self, f: &mut Frame, r: Rect, row: &Row, selected: bool, focus_pane: Option<crate::layout::PaneId>, cur_key: Option<&str>, t: &Theme, time: f64) {
        let w = r.width as usize;
        match row {
            Row::Project { key, name, root, live, attention, last_age, .. } => {
                // the current project (where new sessions start) gets a bar down both lines
                let current = cur_key == Some(key.as_str());
                let color = self.project_color(key);
                // ▦ while the grid shows only this project
                let mark = if matches!(&self.drawn_tab, Some(super::project_tabs::StripTab::Project(k)) if k == key) { "▦" } else { " " };
                let mut right = vec![];
                if *attention {
                    right.push(Span::styled("● ", fg(t.danger)));
                }
                if *live > 0 {
                    right.push(Span::styled(format!("{live} live "), fg(t.shine)));
                } else if let Some(age) = last_age {
                    right.push(Span::styled(format!("{} ", crate::util::short_dur(*age)), muted(t)));
                }
                let name_style = fg(color).add_modifier(Modifier::BOLD);
                let bar = |c: bool| Span::styled(if c { "▍" } else { "▏" }, fg(color));
                let line1 = Rect { height: 1, ..r };
                right.push(Span::styled(" ×", if selected { fg(t.danger) } else { fg(t.frame) }));
                ui::line_lr(f, line1, vec![bar(current), Span::styled(format!("{mark} "), fg(color)), Span::styled(name.clone(), name_style)], right);
                if r.height > 1 {
                    let line2 = Rect { y: r.y + 1, height: 1, ..r };
                    let branch = bro_core::projects::git_branch(root);
                    let branch_w = branch.as_ref().map(|b| ui::width(b) + 3).unwrap_or(0);
                    let path_w = w.saturating_sub(4 + branch_w).max(8);
                    let mut spans = vec![bar(current), Span::raw("  "), Span::styled(crate::util::short_path(root, path_w), muted(t))];
                    let mut rside = vec![];
                    if let Some(b) = branch {
                        rside.push(Span::styled(format!("{} ", ui::icon("branch")), fg(t.frame)));
                        rside.push(Span::styled(format!("{} ", ui::fit(&b, 18)), fg(crate::theme::mix(t.shine, t.muted, 0.45))));
                    }
                    spans.push(Span::raw(""));
                    ui::line_lr(f, line2, spans, rside);
                }
            }
            Row::Live { info, n } => {
                let brand = ui::harness_color(info.harness, t);
                let color = self.project_color(&info.project_key);
                let (dot, dot_c) = match status_look(info.activity, info.done, t, time) {
                    Some((g, _, c)) => (g, c),
                    None => ("•", t.muted),
                };
                let is_focus = focus_pane == Some(info.pane);
                let num = n.map(|n| n.to_string()).unwrap_or_else(|| " ".into());
                let who = info.profile.as_deref().map(|p| p.split(':').next_back().unwrap_or(p).to_string()).unwrap_or_default();
                let model = info.model.as_deref().map(|m| crate::services::launch::short_model(m.rsplit('/').next().unwrap_or(m))).unwrap_or_default();
                let detail = [who, model].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                // the logo already says which agent it is: lead with your name for it, else login · model
                // the orchestrator says what it is first; its login (and its own title, if any) follow
                let (primary, secondary) = match (&info.name, info.harness) {
                    _ if info.orchestrator => {
                        let mut rest = vec![];
                        rest.extend(info.name.clone().filter(|n| !n.is_empty()));
                        if !detail.is_empty() {
                            rest.push(detail);
                        }
                        ("★ orchestrator".to_string(), rest.join(" · "))
                    }
                    (Some(name), _) => (name.clone(), detail),
                    (None, Some(_)) if !detail.is_empty() => (detail, String::new()),
                    (None, Some(h)) => (h.label().to_string(), String::new()),
                    (None, None) => ("shell".to_string(), String::new()),
                };
                if let Some((id, text)) = &self.renaming
                    && *id == info.pane
                {
                    ui::line(f, r, vec![Span::styled("   ✎ ", ui::accent(t)), Span::styled(format!("{text}▏"), ui::bold_accent(t).add_modifier(Modifier::UNDERLINED)), Span::styled("  ⏎ ok · esc", muted(t))]);
                    return;
                }
                let stacked = self.stack.contains(&info.pane);
                let (bar, bar_style) = if is_focus { ("▶", fg(color)) } else if stacked { ("┃", fg(color)) } else { ("│", fg(color)) };
                let primary_style = if is_focus || selected || info.orchestrator { fg(color).add_modifier(Modifier::BOLD) } else { Style::default().fg(t.fg) };
                // pinned and gold: a quiet gold ground even when it isn't the current session
                if info.orchestrator && !selected && !is_focus {
                    f.buffer_mut().set_style(r, Style::default().bg(crate::theme::project_tint(color, t)));
                }
                let mut left = vec![
                    Span::styled(bar, bar_style.add_modifier(Modifier::BOLD)),
                    Span::styled(num, if is_focus { fg(color).add_modifier(Modifier::BOLD) } else { muted(t) }),
                    Span::styled(format!(" {dot} "), fg(dot_c).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{} ", ui::harness_glyph(info.harness)), fg(brand)),
                ];
                // the maker of the model it runs, when that isn't the agent's own lab
                let badge = ui::model_badge(info.harness, info.profile.as_deref(), info.model.as_deref(), t);
                let badged = badge.is_some();
                left.extend(badge);
                left.push(Span::styled(primary, primary_style));
                if !secondary.is_empty() {
                    left.push(Span::styled(format!("  {secondary}"), muted(t)));
                }
                let right = vec![Span::styled(format!(" {}", crate::util::short_dur(info.age_secs)), muted(t)), Span::styled(" ×", if selected { fg(t.danger) } else { fg(t.frame) })];
                ui::line_lr(f, Rect { height: 1, ..r }, left, right);
                // why it waits / what it did, under the name
                if let Some(note) = info.note.as_deref().filter(|_| r.height > 1) {
                    // under the name, past the badge when there is one
                    let indent = if badged { 9 } else { 7 };
                    let style = if note.starts_with('⏸') { fg(crate::theme::mix(t.danger, t.muted, 0.45)) } else { muted(t) };
                    let text = ui::fit(note, w.saturating_sub(indent + 1));
                    ui::line(f, Rect { y: r.y + 1, height: 1, ..r }, vec![Span::styled(bar, bar_style), Span::raw(" ".repeat(indent - 1)), Span::styled(text, style)]);
                }
                // the team it leads, under that
                if let Some(team) = info.team.as_deref() {
                    let top = r.y + 1 + u16::from(info.note.is_some());
                    let rail = Span::styled(if is_focus { "│" } else { bar }, bar_style);
                    draw_team(f, Rect { y: top, height: r.bottom().saturating_sub(top), ..r }, team, rail, t, time);
                }
            }
        }
    }

    /// One quiet row: "+ New" and "↻ Continue" on the left, one-click claude / codex / terminal icons on the
    /// right. Everything tints on hover; a hovered icon names itself in the gap between.
    fn draw_strip(&mut self, f: &mut Frame, r: Rect, t: &Theme) {
        use bro_core::Harness;
        const ICON: u16 = 3;
        let row = Rect { height: 1, ..r };
        let hover_bg = current_tint(t);
        let agents: [(Option<Harness>, &str); 3] = [(Some(Harness::Claude), "Claude"), (Some(Harness::Codex), "Codex"), (None, "Terminal")];
        let icons_w = ICON * agents.len() as u16;
        // narrow sidebars drop the icons first, then Continue's word
        let continue_text = if row.width >= 15 { "↻ Continue" } else { "↻" };
        let left_w = ui::width("+ New") as u16 + 2 + ui::width(continue_text) as u16;
        let show_icons = row.width >= left_w + 2 + icons_w;

        let mut x = row.x;
        for (text, hit, base) in [("+ New", SideHit::Launcher, ui::bold_accent(t)), (continue_text, SideHit::Continue, ui::bold())] {
            let w = (ui::width(text) as u16 + 1).min(row.right().saturating_sub(x));
            if w == 0 {
                break;
            }
            let b = Rect { x, width: w, ..row };
            let hot = b.contains(self.hover);
            if hot {
                f.buffer_mut().set_style(b, Style::default().bg(hover_bg));
            }
            ui::line(f, b, vec![Span::styled(text, if hot { ui::bold_accent(t) } else { base })]);
            self.side_hits.push((b, hit));
            x += w + 1;
        }
        if !show_icons {
            return;
        }

        let mut ix = row.right() - icons_w;
        let mut hovered = None;
        for (h, label) in agents {
            let b = Rect { x: ix, width: ICON, ..row };
            let hot = b.contains(self.hover);
            if hot {
                f.buffer_mut().set_style(b, Style::default().bg(hover_bg));
                hovered = Some(label);
            }
            let color = if h.is_some() { ui::harness_color(h, t) } else { t.shine };
            ui::line(f, b, vec![Span::raw(" "), Span::styled(ui::harness_glyph(h), fg(color).add_modifier(Modifier::BOLD))]);
            self.side_hits.push((b, SideHit::Quick(h)));
            ix += ICON;
        }
        if let Some(label) = hovered {
            let w = ui::width(label) as u16;
            let end = row.right() - icons_w;
            if end > x + w {
                ui::line(f, Rect { x: end - w - 1, width: w, ..row }, vec![Span::styled(label, muted(t))]);
            }
        }
    }

    /// Every running session, added up by the status its row shows (the session counter).
    pub(super) fn session_tally(&self) -> Tally {
        let mut n = Tally::default();
        for (id, p) in self.panes.iter().filter(|(_, p)| p.is_terminal()) {
            n.total += 1;
            match p.activity() {
                Some(Activity::Working) => n.working += 1,
                Some(Activity::Blocked) => n.blocked += 1,
                Some(Activity::Idle) if self.done.contains(id) => n.done += 1,
                _ => {}
            }
        }
        n
    }

    /// The usage block's lines: the Claude and Codex totals (v1's Usage box), or one line per profile.
    pub(super) fn usage_lines(&self) -> Vec<UsageLine> {
        let st = self.svc.state();
        let Some(profiles) = st.profiles.ready() else { return vec![] };
        let gemini: Vec<_> = profiles.iter().filter(|p| p.authenticated && p.is_gemini()).map(|p| {
            let usage = st.usage.get(&p.id).and_then(|e| e.usage.as_ref());
            let (label, note) = if p.is_gemini_subscription() {
                let left = usage.and_then(|u| u.subscription.as_ref()).into_iter().flatten().filter_map(|m| m.remaining).reduce(f64::min);
                let label = if p.plan.as_deref().is_some_and(|s| s.to_lowercase().contains("ultra")) { "Gemini Ultra" } else { "Gemini plan" };
                (label.into(), left.map(|left| format!("{:.0}% left", left * 100.0)).unwrap_or_else(|| "checking…".into()))
            } else {
                let api = usage.and_then(|u| u.api.as_ref());
                (if p.name == "local" { "Gemini API".into() } else { format!("Gemini/{}", p.name) }, api.map(|u| format!("{} req", u.today.requests)).unwrap_or_else(|| "API key".into()))
            };
            UsageLine { harness: Some(bro_core::Harness::Claude), profile: Some(p.id.clone()), label, h5: None, wk: None, approx: false, note: Some(note), resets_at: None, fable: None }
        }).collect();
        let signed_in: Vec<_> = profiles.iter().filter(|p| p.authenticated && !p.is_gemini()).collect();
        // proxy mode runs each family on its subscription logins together (the launcher's one row for them)
        let shared = st.settings.proxy.shared;
        let family = |claude: bool| -> Vec<&bro_core::profiles::Profile> {
            if shared {
                bro_core::profiles::subscription_logins_in(profiles, claude)
            } else {
                signed_in.iter().copied().filter(|p| p.is_claude() == claude).collect()
            }
        };
        let state_of = |id: &str| match st.usage.get(id) {
            Some(e) => match (&e.usage, &e.error) {
                (Some(u), _) => LoginUsage::Ready(u),
                (None, Some(_)) => LoginUsage::Unavailable,
                (None, None) => LoginUsage::Loading,
            },
            None => LoginUsage::Loading,
        };
        let total = |claude: bool| -> Option<UsageLine> {
            let family = family(claude);
            if family.is_empty() {
                return None;
            }
            let s: AppSummary = bro_core::usage::app_summary(family.iter().map(|p| (*p, state_of(&p.id))));
            // the soonest any of them gets room back
            let resets_at = family.iter().filter_map(|p| st.usage.get(&p.id).and_then(|e| e.usage.as_ref()).and_then(|u| tight_reset(p, u))).min();
            let note = if s.h5.is_none() && s.wk.is_none() {
                Some(if s.loading > 0 { "…".to_string() } else { "unavailable".to_string() })
            } else if s.unavailable > 0 {
                Some(format!("{}✗", s.unavailable))
            } else {
                None
            };
            let h = if claude { bro_core::Harness::Claude } else { bro_core::Harness::Codex };
            Some(UsageLine { harness: Some(h), profile: None, label: h.label().into(), h5: s.h5, wk: s.wk, approx: s.h5_estimated, note, resets_at, fable: None })
        };
        let login = |p: &bro_core::profiles::Profile| -> UsageLine {
            let h = if p.is_codex() { bro_core::Harness::Codex } else { bro_core::Harness::Claude };
            let e = st.usage.get(&p.id);
            let (h5, wk, approx, resets_at, note) = match e.and_then(|e| e.usage.as_ref()) {
                Some(u) => {
                    let d = bro_core::usage::headroom_detail(p, u);
                    (d.h5, d.wk, d.h5_estimated, tight_reset(p, u), None)
                }
                None if e.is_some_and(|e| e.error.is_some()) => (None, None, false, None, Some("unavailable".to_string())),
                None => (None, None, false, None, Some("…".to_string())),
            };
            UsageLine { harness: Some(h), profile: Some(p.id.clone()), label: p.name.clone(), h5, wk, approx, note, resets_at, fable: None }
        };
        if !self.usage_expanded {
            let mut v: Vec<UsageLine> = [total(true), total(false)].into_iter().flatten().collect();
            // Fable has its own allowance: show it under Claude when any account has it
            let claude: Vec<_> = family(true).into_iter().map(|p| (p, state_of(&p.id))).collect();
            let s = bro_core::usage::app_summary(claude);
            if s.fable_wk.is_some() {
                // The separate disclosure arrow reveals the allowance; the row launches Claude.
                if let Some(c) = v.first_mut().filter(|l| l.harness == Some(bro_core::Harness::Claude)) {
                    c.fable = Some(self.show_fable);
                }
                if self.show_fable {
                    v.insert(1, UsageLine { harness: None, profile: None, label: "fable".into(), h5: s.fable_5h, wk: s.fable_wk, approx: false, note: None, resets_at: None, fable: None });
                }
            }
            v.extend(gemini);
            return v;
        }
        if shared {
            // no login to pick: each family's line starts a session on all of them, their meters folded under it
            // as the Fable line is (a click there opens the usage view)
            return [true, false]
                .into_iter()
                .flat_map(|claude| total(claude).into_iter().chain(family(claude).into_iter().map(|p| UsageLine { harness: None, ..login(p) })))
                .chain(gemini)
                .collect();
        }
        signed_in.iter().map(|p| login(p)).chain(gemini).collect()
    }

    fn draw_footer(&mut self, f: &mut Frame, area: Rect, usage: &[UsageLine], t: &Theme) {
        let mut y = area.y;
        let row = |y: u16| Rect { y, height: 1, ..area };
        let now = crate::util::now_secs();
        let status_rows = if self.usage_expanded { STATUS_ROWS } else { 0 };
        // (a footer of only the status rows has no header)
        if area.height > status_rows.max(2) && !usage.is_empty() {
            // header: "▸ usage left ⓘ ───  5h  wk" — click to switch totals ⇄ every profile; ⓘ opens the stats
            let head = row(y);
            let arrow = if self.usage_expanded { "▾" } else { "▸" };
            let label = format!("{arrow} usage left ");
            let info = format!("{} ", ui::icon("info"));
            let info_at = Rect { x: head.x + ui::width(&label) as u16, width: (ui::width(&info) as u16).min(head.width), ..head };
            let info_style = if info_at.contains(self.hover) { ui::bold_accent(t) } else { muted(t) };
            // column heads sit over the figures: " 46%" + " " + " 39%" + " "; holding alt, the rows say when they reset
            let resets = self.alt_hints();
            let heads = if resets { "resets " } else { " 5h   wk  " };
            let fill = (area.width as usize).saturating_sub(ui::width(&label) + ui::width(&info) + ui::width(heads) + 1);
            ui::line_lr(f, head, vec![Span::styled(label, ui::accent(t)), Span::styled(info, info_style), Span::styled("─".repeat(fill), fg(t.frame))], vec![Span::styled(heads, muted(t))]);
            // the ⓘ wins over the header (hits are searched in order)
            self.side_hits.push((info_at, SideHit::Stats));
            self.side_hits.push((head, SideHit::UsageToggle));
            y += 1;
            let name_w = usage.iter().map(|u| ui::width(&u.label)).max().unwrap_or(6).clamp(5, 10);
            let meter_n = (area.width as usize).saturating_sub(name_w + 3 + 12).clamp(0, 8);
            for u in usage {
                if y + status_rows >= area.bottom() {
                    break;
                }
                let r = row(y);
                let hot = r.contains(self.hover) && u.harness.is_some();
                if hot {
                    f.buffer_mut().set_style(r, Style::default().bg(current_tint(t)));
                }
                let is_gemini = u.profile.as_deref().is_some_and(|p| bro_core::profiles::is_gemini_id(&p));
                let brand = if is_gemini { ui::provider_glyph("gemini", t.shine, t).1 } else { u.harness.map(|h| ui::harness_color(Some(h), t)).unwrap_or(t.muted) };
                let glyph = if is_gemini { ui::provider_glyph("gemini", t.shine, t).0 } else { u.harness.map(|h| ui::harness_glyph(Some(h))).unwrap_or(" ") };
                let name_style = if u.harness.is_some() { ui::bold() } else { muted(t) };
                let label = match u.fable {
                    Some(open) => format!("{} {}", u.label, if open { "▾" } else { "▸" }),
                    None if u.harness.is_none() => format!(" {}", u.label),
                    None => u.label.clone(),
                };
                let mut left = vec![Span::styled(format!("{glyph} "), fg(brand)), Span::styled(ui::pad(&label, name_w + 2), name_style), Span::raw(" ")];
                let mut right = vec![];
                match &u.note {
                    Some(n) if u.h5.is_none() && u.wk.is_none() => right.push(Span::styled(format!("{n} "), muted(t))),
                    _ if resets => {
                        let tight = [u.h5, u.wk].into_iter().flatten().fold(f64::INFINITY, f64::min);
                        let color = if tight.is_finite() { ui::left_color(tight, t) } else { t.muted };
                        let when = u.resets_at.map(|at| format!("Resets @ {} ", crate::util::reset_time(at))).unwrap_or_else(|| "no reset reported ".into());
                        right.push(Span::styled(when, fg(color)));
                    }
                    _ => {
                        // the meter shows the tighter window
                        let tight = [u.h5, u.wk].into_iter().flatten().fold(f64::INFINITY, f64::min);
                        if meter_n >= 3 && tight.is_finite() {
                            left.extend(ui::left_meter(tight, meter_n, t));
                        }
                        if self.usage_expanded
                            && let Some(at) = u.resets_at
                            && meter_n < 3
                        {
                            left.push(Span::styled(crate::util::countdown(at - now), muted(t)));
                        }
                        right.extend(figure(u.h5, u.approx, t));
                        right.push(Span::raw(" "));
                        right.extend(figure(u.wk, false, t));
                        right.push(Span::raw(" "));
                    }
                }
                ui::line_lr(f, r, left, right);
                if u.fable.is_some() {
                    let x = r.x + ui::width(glyph) as u16 + 2 + ui::width(&u.label) as u16;
                    if x < r.right() {
                        self.side_hits.push((Rect { x, width: 1, ..r }, SideHit::Fable));
                    }
                }
                if hot && r.width > 0 {
                    ui::line(f, Rect { x: r.right() - 1, width: 1, ..r }, vec![Span::styled("+", ui::bold_accent(t))]);
                }
                self.side_hits.push((r, match u.harness {
                    Some(h) => SideHit::LaunchUsage(h, u.profile.clone()),
                    None => SideHit::Usage,
                }));
                y += 1;
            }
        }
        // the session counter, proxy and phone bridge, pinned to the bottom, in plain words (click any for more) —
        // only with the usage block expanded
        if !self.usage_expanded {
            return;
        }
        let label = |icon: &str, name: &str| Span::styled(format!("{} {:<8}", ui::icon(icon), name), muted(t));
        // no light of its own (its place keeps the long label clear): the count lines up with what the lights below say
        let sr = row(area.bottom() - 3);
        let mut sessions = vec![label("term", "sessions"), Span::raw("  ")];
        let room = (sr.width as usize).saturating_sub(spans_width(&sessions));
        sessions.extend(session_spans(self.session_tally(), room, t, self.start.elapsed().as_secs_f64()));
        ui::line(f, sr, sessions);
        let st = self.svc.state();
        let pr = row(area.bottom() - 2);
        let proxy = match &st.proxy.status {
            Avail::Ready(i) => {
                let n = st.proxy.routes.len();
                let use_ = match n {
                    0 => "idle".to_string(),
                    1 => "1 session".to_string(),
                    n => format!("{n} sessions"),
                };
                vec![label("proxy", "proxy"), Span::styled("● ", fg(t.good)), Span::styled(use_, if n > 0 { fg(t.shine) } else { muted(t) }), Span::styled(format!(" · port {}", i.port), muted(t))]
            }
            Avail::Loading => vec![label("proxy", "proxy"), Span::styled("◌ starting", muted(t))],
            Avail::Unavailable(_) => vec![label("proxy", "proxy"), Span::styled("○ off", fg(t.danger))],
        };
        ui::line(f, pr, proxy);
        let br = row(area.bottom() - 1);
        let bridge = match &st.bridge.status {
            Avail::Ready(s) if s.running => {
                let who = match s.clients {
                    0 => "no phones".to_string(),
                    1 => "1 connected".to_string(),
                    n => format!("{n} connected"),
                };
                vec![
                    label("bridge", "phone"),
                    Span::styled("● ", fg(t.good)),
                    Span::styled(who, if s.clients > 0 { fg(t.shine) } else { muted(t) }),
                    Span::styled(format!(" · port {}", s.port), muted(t)),
                ]
            }
            Avail::Loading if st.bridge.enabled => vec![label("bridge", "phone"), Span::styled("◌ starting", muted(t))],
            _ => vec![label("bridge", "phone"), Span::styled("○ off", fg(t.danger))],
        };
        ui::line(f, br, bridge);
        drop(st);
        self.side_hits.push((sr, SideHit::Sessions));
        self.side_hits.push((pr, SideHit::Proxy));
        self.side_hits.push((br, SideHit::Bridge));
    }
}

/// The status rows at the footer's foot while usage is expanded: sessions, proxy, phone.
const STATUS_ROWS: u16 = 3;

/// The session counter after its label, in `room` columns: "5 running" and what they're doing, in words while they
/// fit ("· 1 working · 1 needs you"), else as the project tabs' shapes with their counts ("· ◐1 ●1 ✓1"), then
/// without "running".
fn session_spans(n: Tally, room: usize, t: &Theme, time: f64) -> Vec<Span<'static>> {
    if n.total == 0 {
        return vec![Span::styled("none running", muted(t))];
    }
    let look = |s: Status| match s {
        Status::Working => (["◐", "◓", "◑", "◒"][(time * 6.0) as usize % 4], t.accent),
        Status::NeedsYou => ("●", t.danger),
        Status::Done => ("✓", t.good),
    };
    let count = |running: bool| {
        let mut v = vec![Span::styled(n.total.to_string(), fg(t.shine).add_modifier(Modifier::BOLD))];
        if running {
            v.push(Span::styled(" running", fg(t.shine)));
        }
        v
    };
    let mut words = count(true);
    for (s, k) in n.statuses() {
        words.extend([Span::styled(" ·", muted(t)), Span::styled(format!(" {k} {}", s.word()), fg(look(s).1))]);
    }
    let shapes = |running: bool| {
        let mut v = count(running);
        if n.statuses().next().is_some() {
            v.push(Span::styled(" ·", muted(t)));
        }
        for (s, k) in n.statuses() {
            let (glyph, c) = look(s);
            v.push(Span::styled(format!(" {glyph}{k}"), fg(c).add_modifier(Modifier::BOLD)));
        }
        v
    };
    [words, shapes(true)].into_iter().find(|v| spans_width(v) <= room).unwrap_or_else(|| shapes(false))
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| ui::width(&s.content)).sum()
}

/// A right-aligned "% left" figure, 4 columns + an optional ≈.
fn figure(v: Option<f64>, approx: bool, t: &Theme) -> Vec<Span<'static>> {
    match v {
        Some(v) => vec![Span::styled(format!("{}{:>3.0}%", if approx { "≈" } else { " " }, v), fg(ui::left_color(v, t)))],
        None => vec![Span::styled("    —", muted(t))],
    }
}

/// Footer rows: header + usage lines (+ sessions, proxy and bridge when expanded), or less when space is short.
fn footer_height(usage: u16, expanded: bool, avail: u16) -> u16 {
    let status = if expanded { STATUS_ROWS } else { 0 };
    let full = if usage > 0 { 1 + usage + status } else { status };
    if avail >= full + 8 {
        full
    } else if avail >= 12 {
        status
    } else {
        0
    }
}

/// (gap above, own height) per sidebar row: projects get a blank line above (except the first) and two lines.
fn row_lines(rows: &[Row]) -> Vec<(u16, u16)> {
    let mut seen_project = false;
    rows.iter()
        .map(|r| match r {
            Row::Project { .. } => {
                let gap = u16::from(seen_project);
                seen_project = true;
                (gap, 2)
            }
            // a running session with something to say gets a second line, a lead its team's under that (the sticky
            // orchestrator too: the hairline under it spaces the first project)
            Row::Live { info, .. } => (0, 1 + u16::from(info.note.is_some()) + info.team.as_deref().map_or(0, |t| crate::sidebar::team_lines(t) as u16)),
        })
        .collect()
}

/// A lead's team under its row: how many teammates are working and the tasks done, then a line per teammate (its
/// status, its name in Claude's colour for it, what it's doing), a bigger team ending in "+N more".
fn draw_team(f: &mut Frame, r: Rect, team: &bro_core::agent_teams::Team, rail: Span<'static>, t: &Theme, time: f64) {
    const INDENT: usize = 7;
    let w = r.width as usize;
    let mates: Vec<_> = team.teammates().collect();
    let working = team.working();
    let c = team.counts();
    let lead = || vec![rail.clone(), Span::raw(" ".repeat(INDENT - 1))];
    let mut left = lead();
    left.push(Span::styled(ui::lead("team"), fg(t.accent)));
    left.push(match working {
        0 => Span::styled(format!("{} idle", mates.len()), muted(t)),
        n => Span::styled(format!("{n} of {} working", mates.len()), fg(t.accent)),
    });
    let mut right = vec![];
    if c.total > 0 {
        let all = c.done == c.total;
        right.push(Span::styled(format!("{}/{} ", c.done, c.total), if all { fg(t.good) } else { muted(t) }));
        right.push(Span::styled("✓ ", if all { fg(t.good) } else { fg(t.frame) }));
    }
    let mut y = r.y;
    if y >= r.bottom() {
        return;
    }
    ui::line_lr(f, Rect { y, height: 1, ..r }, left, right);
    y += 1;
    let more = if mates.len() > crate::sidebar::TEAM_ROWS { mates.len() + 1 - crate::sidebar::TEAM_ROWS } else { 0 };
    let shown = if more > 0 { crate::sidebar::TEAM_ROWS - 1 } else { mates.len() };
    for (n, m) in mates.iter().take(shown).enumerate() {
        if y >= r.bottom() {
            return;
        }
        let last = n + 1 == shown && more == 0;
        let (dot, dot_c) = match m.active {
            true => status_look(Some(crate::pane::Activity::Working), false, t, time).map(|(g, _, c)| (g, c)).unwrap_or(("●", t.accent)),
            false => ("○", t.muted),
        };
        let name = ui::fit(&m.name, 14);
        let room = w.saturating_sub(INDENT + 6 + ui::width(&name));
        let mut spans = lead();
        spans.extend([
            Span::styled(if last { "└ " } else { "├ " }, fg(t.frame)),
            Span::styled(format!("{dot} "), fg(dot_c).add_modifier(Modifier::BOLD)),
            Span::styled(name, fg(crate::team_board::member_color(m, t))),
        ]);
        if room > 3 {
            spans.push(Span::styled(format!(" {}", ui::fit(&crate::team_board::doing(team, m), room)), muted(t)));
        }
        ui::line(f, Rect { y, height: 1, ..r }, spans);
        y += 1;
    }
    if more > 0 && y < r.bottom() {
        let mut spans = lead();
        spans.extend([Span::styled("└ ", fg(t.frame)), Span::styled(format!("+{more} more"), muted(t))]);
        ui::line(f, Rect { y, height: 1, ..r }, spans);
    }
}

/// When a login gets room back: the reset of whichever window is tighter (the week once it's the limit, else the
/// 5 hours), falling back to the other window's.
fn tight_reset(p: &bro_core::profiles::Profile, u: &bro_core::usage::Usage) -> Option<i64> {
    let d = bro_core::usage::headroom_detail(p, u);
    let h5 = u.five_hour.as_ref().and_then(|w| w.resets_at);
    let wk = u.weekly.as_ref().and_then(|w| w.resets_at);
    match (d.h5, d.wk) {
        (Some(h), Some(w)) if w <= h => wk.or(h5),
        _ => h5.or(wk),
    }
}

/// Hover background for sidebar actions: a quiet tint of the accent.
fn current_tint(t: &Theme) -> ratatui::style::Color {
    if t.is_light() {
        crate::theme::mix(t.accent, t.bg, 0.9)
    } else {
        crate::theme::mix(t.accent, t.deep(), 0.84)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_session_counter_says_as_much_as_fits() {
        let t = crate::theme::get("graphite");
        let n = Tally { total: 5, working: 1, blocked: 1, done: 1 };
        assert_eq!(text(&session_spans(n, 60, &t, 0.0)), "5 running · 1 working · 1 needs you · 1 done");
        // short of room, the project tabs' shapes (they read without colour too), then the count alone with them
        assert_eq!(text(&session_spans(n, 24, &t, 0.0)), "5 running · ◐1 ●1 ✓1");
        assert_eq!(text(&session_spans(n, 12, &t, 0.0)), "5 · ◐1 ●1 ✓1");
        // idle sessions only count in the total
        assert_eq!(text(&session_spans(Tally { total: 2, ..Tally::default() }, 24, &t, 0.0)), "2 running");
        assert_eq!(text(&session_spans(Tally::default(), 24, &t, 0.0)), "none running");
        assert_eq!(n.words(), "1 working · 1 needs you · 1 done · 2 idle");
    }
}
