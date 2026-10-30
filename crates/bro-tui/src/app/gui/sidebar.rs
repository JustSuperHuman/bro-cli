//! The sidebar and the project tabs.
//!
//! Sidebar, top to bottom: the wordmark with settings and the clock; the filter; the orchestrator, pinned;
//! projects with their sessions (status, agent, name, login · model, age, a note on why one waits or what it
//! did); then, pinned to the bottom, the tile switch, New / Continue with one-click agents, the usage left on
//! each login, and the proxy and phone bridge. Rows, order, numbering and every click do what the terminal UI's
//! sidebar does (they're the same [`crate::sidebar`] rows and [`SideHit`]s).

use super::actions::Action;
use super::chrome::{harness_mark, session_badge, session_mark, status_dot};
use super::kit::{Cursor, Style, Ui, radius, size, weight, widgets};
use crate::app::App;
use crate::app::SideHit;
use crate::app::project_tabs::{StripHit, StripTab};
use crate::app::side_view::{Status, Tally};
use crate::pane::Activity;
use crate::services::Avail;
use crate::sidebar::Row;
use bro_gfx::text::Align;
use bro_gfx::{Color, Corners, Point, Rect};

/// Scrolling and what was drawn where (keyboard moves scroll the selection into view).
#[derive(Default)]
pub struct SideState {
    pub scroll: f32,
    pub content: f32,
    pub view: f32,
    /// the list's area last frame
    pub list: Rect,
    /// rows' rects, by index into `App::rows()`
    pub rows: Vec<(usize, Rect)>,
    last_sel: Option<usize>,
    last_focus: bool,
    /// the wheel moved the list: keep it until the keyboard moves the selection
    pub manual: bool,
    /// the filter as last drawn: a change scrolls back to the top
    filter: String,
    /// the scrollbar last frame, when the list overflows: (track, thumb top, thumb height)
    bar: Option<(Rect, f32, f32)>,
    /// the scrollbar's thumb is held: (pointer y, scroll) where it was taken
    pub bar_drag: Option<(f32, f32)>,
    /// the right edge is being dragged (it stays lit until it's let go)
    pub resizing: bool,
}

impl SideState {
    /// Take hold of the scrollbar at height `y`: on the thumb it drags from there; elsewhere on the track the thumb
    /// first jumps to sit under the pointer.
    pub fn grab_bar(&mut self, y: f32) {
        let Some((track, top, h)) = self.bar else { return };
        let max = (self.content - self.view).max(0.0);
        if y < top || y > top + h {
            let room = (track.h - h).max(1.0);
            self.scroll = ((y - track.y - h / 2.0) / room).clamp(0.0, 1.0) * max;
        }
        self.bar_drag = Some((y, self.scroll));
        self.manual = true;
    }

    /// The held scrollbar moved to height `y`.
    pub fn drag_bar(&mut self, y: f32) {
        let (Some((y0, s0)), Some((track, _, h))) = (self.bar_drag, self.bar) else { return };
        let max = (self.content - self.view).max(0.0);
        let room = (track.h - h).max(1.0);
        self.scroll = (s0 + (y - y0) * max / room).clamp(0.0, max);
        self.manual = true;
    }
}

/// The scrollbar's track and thumb for a list `view` px tall over `content` px scrolled `scroll` down (the same
/// geometry [`widgets::scrollbar`] draws): (track, thumb top, thumb height).
fn bar_geometry(view: Rect, content: f32, scroll: f32) -> Option<(Rect, f32, f32)> {
    if content <= view.h + 0.5 {
        return None;
    }
    let track = Rect::new(view.right() - 5.0, view.y + 3.0, 2.0, view.h - 6.0);
    let h = (track.h * view.h / content).max(24.0).min(track.h);
    let top = track.y + (track.h - h) * (scroll / (content - view.h)).clamp(0.0, 1.0);
    Some((track, top, h))
}

const PAD: f32 = 10.0;
/// The header (wordmark, buttons, clock) ends this far below the sidebar's top.
pub const HEAD_BOTTOM: f32 = 44.0;

pub fn draw(app: &mut App, ui: &mut Ui, r: Rect, s: &mut SideState, _focused_window: bool, media_open: bool, media_unseen: usize) {
    let t = ui.t;
    let focused = app.side_focus && !app.overlay.is_open();
    // resize edge: lit under the pointer and while it's dragged
    let edge = Rect::new(r.right() - 3.0, r.y, 6.0, r.h);
    let edge_hot = ui.hover_amount("side-edge", edge).max(if s.resizing && ui.pressing { 1.0 } else { 0.0 });
    ui.vline(r.right() - 1.0, r.y, r.h, t.hairline.mix(t.accent, edge_hot * 0.8));
    ui.hit_with(edge, Action::SideResize, Cursor::ResizeH);
    let inner = Rect::new(r.x + PAD, r.y, r.w - 2.0 * PAD - 1.0, r.h);

    // ---- header: wordmark, settings, clock
    let head = Rect::new(inner.x, r.y + 8.0, inner.w, HEAD_BOTTOM - 8.0);
    let mark = ui.label(Point::new(head.x + 6.0, head.y + 8.0), "bro", Style::new(17.0, weight::BOLD, t.text));
    ui.circle(Point::new(head.x + 6.0 + mark.w + 4.0, head.y + 24.0), 2.2, t.accent);
    let clock = crate::util::clock();
    let cs = Style::new(size::SMALL, weight::MEDIUM, t.faint);
    let cw = ui.measure(&clock, cs);
    ui.label(Point::new(head.right() - cw - 4.0, head.y + 10.0), &clock, cs);
    let gear = Rect::new(head.right() - cw - 38.0, head.y + 5.0, 26.0, 26.0);
    widgets::icon_button(ui, gear, "\u{F0493}", t.muted, Action::Side(SideHit::Settings));
    let media = Rect::new(gear.x - 30.0, gear.y, 26.0, 26.0);
    let fresh = media_unseen > 0 && !media_open;
    if widgets::icon_button(ui, media, "\u{F02E9}", if media_open || fresh { t.accent } else { t.muted }, Action::MediaToggle) {
        let key = app.keymap.primary(crate::keymap::Act::Media);
        let what = if fresh { format!("{media_unseen} new · pictures and videos · {key}") } else { format!("Pictures and videos · {key}") };
        widgets::tooltip(ui, media, &what);
    }

    // ---- filter
    let filter_r = Rect::new(inner.x, head.bottom() + 4.0, inner.w, 32.0);
    let filtering = app.side_filtering || !app.side.filter.is_empty();
    let placeholder = if focused { "Type to filter" } else { "Filter" };
    let cursor = filtering.then_some(app.side.filter.len());
    widgets::field(ui, filter_r, &app.side.filter, cursor, placeholder, focused && filtering, Action::SideFilter);
    if !filtering && !app.alt_hints() {
        // the terminal UI's frame subtitle, from the live keymap: with the keyboard here, where the keys are listed;
        // else how to come here
        let k = if focused {
            Some(app.keymap.primary(crate::keymap::Act::Help)).filter(|k| !k.is_empty()).map(|k| format!("{k} keys")).unwrap_or_default()
        } else {
            app.keymap.primary(crate::keymap::Act::FocusSidebar)
        };
        if !k.is_empty() {
            let st = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
            let kw = ui.measure(&k, st);
            ui.label(Point::new(filter_r.right() - kw - 10.0, filter_r.y + 8.0), &k, st);
        }
    }

    // ---- footer (measured first, pinned to the bottom)
    let usage = app.usage_lines();
    let sessions = app.panes.values().filter(|p| p.is_terminal()).count();
    let tile_row = app.strip.height == 0 && sessions >= 2;
    let status_h = if app.usage_expanded { STATUS_H } else { 0.0 };
    let usage_h = if usage.is_empty() { 0.0 } else { 30.0 + usage.len() as f32 * 26.0 };
    let foot_h = 12.0 + if tile_row { 34.0 } else { 0.0 } + 44.0 + usage_h + status_h + 8.0;
    let foot_h = foot_h.min((r.h - filter_r.bottom() - 120.0).max(60.0));
    let foot = Rect::new(inner.x, r.bottom() - foot_h, inner.w, foot_h);
    draw_footer(app, ui, foot, &usage, tile_row, sessions);

    // ---- list
    let list = Rect::new(r.x, filter_r.bottom() + 8.0, r.w - 1.0, (foot.y - filter_r.bottom() - 12.0).max(0.0));
    draw_list(app, ui, list, s, focused);
}

/// (gap above, height) of a row: a project gets a gap above it (none for the first) and two lines; a session one
/// line, two with a note, and room for the field and its hint while it's being renamed; a lead its team under that.
fn row_height(row: &Row, renaming: Option<crate::layout::PaneId>) -> (f32, f32) {
    match row {
        Row::Project { .. } => (10.0, 48.0),
        Row::Live { info, .. } => {
            let base = if info.note.is_some() || renaming == Some(info.pane) { 48.0 } else { 32.0 };
            (0.0, base + info.team.as_deref().map_or(0.0, team_height))
        }
    }
}

/// One line of a lead's team under its row.
const TEAM_LINE: f32 = 19.0;

/// The room a lead's team takes under its row.
fn team_height(team: &bro_core::agent_teams::Team) -> f32 {
    2.0 + TEAM_LINE * crate::sidebar::team_lines(team) as f32 + 4.0
}

/// A lead's team under its row, from `top`: how many are working and the tasks done (a thin bar), then a line per
/// teammate on a rail — its status (a spinner in its colour while it works), its name in Claude's colour for it and
/// what it's doing — a bigger team ending in "+N more". A click anywhere on it opens the team board.
fn draw_team(ui: &mut Ui, r: Rect, top: f32, x: f32, team: &bro_core::agent_teams::Team, pane: crate::layout::PaneId) {
    let t = ui.t;
    let area = Rect::new(r.x, top, r.w, r.bottom() - top);
    let hot = ui.hover_amount(("side-team", pane), area);
    if hot > 0.0 {
        ui.round(Rect::new(x - 6.0, top, r.right() - 6.0 - (x - 6.0), area.h - 3.0), radius::CONTROL, t.hover.fade(hot));
    }
    ui.hit(area, Action::Side(SideHit::Team(pane)));
    let mates: Vec<_> = team.teammates().collect();
    let working = team.working();
    let c = team.counts();
    let right = r.right() - 12.0;
    // the summary: the team mark, who's working, the tasks done
    let mut y = top + 2.0;
    let mid = y + TEAM_LINE / 2.0;
    ui.icon(Rect::new(x - 1.0, y, 14.0, TEAM_LINE), "\u{F0849}", size::SMALL, t.accent);
    let (text, tc) = match working {
        0 => (format!("{} idle", mates.len()), t.muted),
        n => (format!("{n} of {} working", mates.len()), t.accent),
    };
    let mut room = right;
    if c.total > 0 {
        let all = c.done == c.total;
        let label = format!("{}/{}", c.done, c.total);
        let st = Style::new(size::CAPTION, weight::SEMIBOLD, if all { t.good } else { t.muted });
        let lw = ui.measure(&label, st);
        ui.label(Point::new(right - lw, mid - 7.5), &label, st);
        let bar = Rect::new(right - lw - 6.0 - 34.0, mid - 1.5, 34.0, 3.0);
        ui.round(bar, 1.5, t.text.with_alpha(0.1));
        if c.done > 0 {
            ui.round(Rect::new(bar.x, bar.y, (bar.w * c.done as f32 / c.total as f32).max(3.0), bar.h), 1.5, t.good);
        }
        room = bar.x - 8.0;
    }
    ui.label_in(Rect::new(x + 16.0, y, (room - x - 16.0).max(10.0), TEAM_LINE), &text, Style::new(size::SMALL, weight::MEDIUM, tc), Align::Left);
    y += TEAM_LINE;
    // the teammates, on a rail down from the mark
    let more = if mates.len() > crate::sidebar::TEAM_ROWS { mates.len() + 1 - crate::sidebar::TEAM_ROWS } else { 0 };
    let shown = if more > 0 { crate::sidebar::TEAM_ROWS - 1 } else { mates.len() };
    let lines = shown + usize::from(more > 0);
    if lines > 0 {
        ui.vline(x + 5.5, y - 3.0, TEAM_LINE * (lines as f32 - 0.5) + 3.0, t.hairline);
    }
    for m in mates.iter().take(shown) {
        let mid = y + TEAM_LINE / 2.0;
        ui.hline(x + 5.5, mid, 6.0, t.hairline);
        let mc = match m.rgb() {
            Some((r, g, b)) => Color::rgb8(r, g, b),
            None => t.inline,
        };
        let dot = Point::new(x + 17.0, mid);
        if m.active {
            ui.spinner(dot, 3.6, 1.3, mc);
        } else {
            ui.ring(dot, 3.2, 1.2, mc.with_alpha(0.7));
        }
        let name_st = Style::new(size::SMALL, weight::SEMIBOLD, if m.active { mc } else { mc.mix(t.muted, 0.45) });
        let doing = crate::team_board::doing(team, m);
        let spans = vec![(m.name.clone(), name_st), (format!("  {doing}"), Style::new(size::CAPTION, weight::REGULAR, if m.active { t.muted } else { t.faint }))];
        let b = ui.rich(&spans, size::SMALL, Some((right - x - 26.0).max(10.0)), false, Some(1));
        ui.draw(&b, Point::new(x + 26.0, (mid - b.h / 2.0).round()));
        y += TEAM_LINE;
    }
    if more > 0 {
        let mid = y + TEAM_LINE / 2.0;
        ui.hline(x + 5.5, mid, 6.0, t.hairline);
        ui.label_in(Rect::new(x + 14.0, y, right - x - 14.0, TEAM_LINE), &format!("+{more} more"), Style::new(size::CAPTION, weight::MEDIUM, t.faint), Align::Left);
    }
}

fn draw_list(app: &mut App, ui: &mut Ui, list: Rect, s: &mut SideState, focused: bool) {
    let t = ui.t;
    let rows = app.rows();
    app.side_sel = app.side_sel.min(rows.len().saturating_sub(1));
    let focus_pane = app.focused();
    let cur_key = app.current_project().map(|r| app.svc.project_for(&r).key);
    let renaming = app.renaming.as_ref().map(|(id, _)| *id);
    s.rows.clear();
    s.list = list;
    // each change to the filter starts the list from the top
    if s.filter != app.side.filter {
        s.filter = app.side.filter.clone();
        s.scroll = 0.0;
        s.manual = false;
    }
    if rows.is_empty() {
        s.bar = None;
        if !app.side.filter.is_empty() {
            ui.label_in(Rect::new(list.x + 18.0, list.y + 6.0, list.w - 36.0, 24.0), "Nothing matches", Style::new(size::BODY, weight::MEDIUM, t.muted), Align::Left);
        } else {
            let p = ui.para("Open a folder or start a session: projects you work in appear here.", Style::new(size::SMALL, weight::REGULAR, t.faint), list.w - 36.0, Some(3));
            ui.draw(&p, Point::new(list.x + 18.0, list.y + 8.0));
        }
        return;
    }
    let mut y = list.y;
    // the orchestrator is pinned above the scrolling list
    let pinned = matches!(rows.first(), Some(Row::Live { info, .. }) if info.orchestrator && info.project_key == bro_core::orchestrator::GLOBAL_KEY);
    if pinned {
        let (_, h) = row_height(&rows[0], renaming);
        let r = Rect::new(list.x + PAD, y, list.w - 2.0 * PAD, h);
        draw_row(app, ui, r, 0, &rows[0], focused, focus_pane, cur_key.as_deref());
        s.rows.push((0, r));
        y += h + 6.0;
        ui.hline(list.x + PAD + 8.0, y - 3.0, list.w - 2.0 * PAD - 16.0, t.hairline);
    }
    let view = Rect::new(list.x, y, list.w, (list.bottom() - y).max(0.0));
    let first = usize::from(pinned);
    // heights
    let mut tops = vec![];
    let mut acc = 0.0;
    for (i, row) in rows.iter().enumerate().skip(first) {
        let (gap, h) = row_height(row, renaming);
        let gap = if i == first { 0.0 } else { gap };
        tops.push((i, acc + gap, h));
        acc += gap + h;
    }
    s.content = acc + 8.0;
    s.view = view.h;
    // keep the keyboard selection in view
    let sel_changed = s.last_sel != Some(app.side_sel) || s.last_focus != app.side_focus;
    if sel_changed {
        s.manual = false;
    }
    // (or the session being renamed: its field is where your typing goes)
    let renamed_row = renaming.and_then(|id| rows.iter().position(|r| matches!(r, Row::Live { info, .. } if info.pane == id)));
    let keep = renamed_row.or((app.side_focus && !s.manual).then_some(app.side_sel));
    if let Some(&(_, top, h)) = keep.and_then(|k| tops.iter().find(|(i, ..)| *i == k)) {
        if top < s.scroll {
            s.scroll = top;
        } else if top + h > s.scroll + view.h {
            s.scroll = top + h - view.h;
        }
    }
    s.last_sel = Some(app.side_sel);
    s.last_focus = app.side_focus;
    s.scroll = s.scroll.clamp(0.0, (s.content - view.h).max(0.0));
    ui.clip(view, 0.0);
    ui.hit_with(view, Action::SideList, Cursor::Default);
    for (i, top, h) in tops {
        let ry = view.y + top - s.scroll;
        if ry + h < view.y || ry > view.bottom() {
            continue;
        }
        let r = Rect::new(list.x + PAD, ry, list.w - 2.0 * PAD, h);
        draw_row(app, ui, r, i, &rows[i], focused, focus_pane, cur_key.as_deref());
        s.rows.push((i, r));
    }
    ui.unclip();
    // scrollbar while scrolled, moving, under the pointer or held; it can be dragged (the track jumps)
    let moving = app.side_nav_at.is_some_and(|at| at.elapsed().as_millis() < 1050);
    let vis = ui.ease("side-bar", if moving || ui.hovered(view) || s.bar_drag.is_some() { 1.0 } else { 0.0 }, 200.0);
    widgets::scrollbar(ui, view, s.content, view.h, s.scroll, vis);
    s.bar = bar_geometry(view, s.content, s.scroll);
    if s.bar.is_some() {
        // clear of the rows' buttons and of the resize edge
        ui.hit_with(Rect::new(view.right() - 12.0, view.y, 9.0, view.h), Action::SideScroll, Cursor::Default);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(app: &mut App, ui: &mut Ui, r: Rect, i: usize, row: &Row, focused: bool, focus_pane: Option<crate::layout::PaneId>, cur_key: Option<&str>) {
    let t = ui.t;
    let selected = focused && i == app.side_sel;
    let hot = ui.hover_amount(("side-row", i, row.project_key()), r);
    match row {
        Row::Project { key, name, root, live, attention, last_age } => {
            let current = cur_key == Some(key.as_str());
            let tiled = matches!(&app.drawn_tab, Some(StripTab::Project(k)) if k == key);
            if selected {
                ui.round(r, radius::CONTROL, t.selected);
                ui.outline(r, radius::CONTROL, 1.0, t.accent.with_alpha(0.5));
            } else if current {
                ui.round(r, radius::CONTROL, t.text.with_alpha(if t.light { 0.04 } else { 0.045 }));
            } else if hot > 0.0 {
                ui.round(r, radius::CONTROL, t.hover.fade(hot));
            }
            ui.hit(r, Action::Side(SideHit::Row(i)));
            let l1 = Rect::new(r.x + 12.0, r.y + 6.0, r.w - 22.0, 18.0);
            // right side
            let mut right = l1.right();
            let close = Rect::new(right - 20.0, l1.y - 1.0, 20.0, 20.0);
            if hot > 0.3 || selected {
                widgets::icon_button(ui, close, "\u{F0156}", if selected { t.danger } else { t.muted }, Action::Side(SideHit::CloseProject(root.clone(), *live)));
                right = close.x - 4.0;
            } else if *live == 0
                && let Some(age) = last_age
            {
                let a = crate::util::short_dur(*age);
                let st = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
                let w = ui.measure(&a, st);
                ui.label(Point::new(right - w, l1.y + 2.0), &a, st);
                right -= w + 6.0;
            }
            if *attention {
                ui.ticking = true;
                let k = 0.7 + 0.3 * ((ui.time * 4.0).sin() as f32 * 0.5 + 0.5);
                ui.icon(Rect::new(right - 16.0, l1.y, 16.0, 18.0), "\u{F0028}", size::SMALL, t.danger.fade(k));
                right -= 20.0;
            }
            // the name in the project's own colour, as the terminal UI shows it
            let name_x = l1.x;
            let name_w = (right - name_x - if tiled { 22.0 } else { 4.0 }).max(10.0);
            let nb = ui.line(name, Style::new(size::BODY, weight::SEMIBOLD, t.project(key, &app.theme)), Some(name_w));
            ui.draw(&nb, Point::new(name_x, l1.y + (18.0 - nb.h) / 2.0));
            if tiled {
                ui.icon(Rect::new(name_x + nb.w + 4.0, l1.y, 16.0, 18.0), "\u{F05AF}", size::SMALL, t.muted);
            }
            // second line: path and branch (the branch tinted like the terminal UI's)
            let l2 = Rect::new(name_x, r.y + 26.0, r.right() - 12.0 - name_x, 16.0);
            let branch = bro_core::projects::git_branch(root);
            let mut path_w = l2.w;
            if let Some(b) = &branch {
                let st = Style::new(size::CAPTION, weight::MEDIUM, t.shine.mix(t.muted, 0.45));
                let text = crate::ui::fit(b, 18);
                let bw = ui.measure(&text, st);
                let bx = l2.right() - bw;
                ui.icon(Rect::new(bx - 15.0, l2.y, 13.0, 16.0), "\u{E725}", size::CAPTION, t.faint);
                ui.label(Point::new(bx, l2.y + 1.0), &text, st);
                path_w = (bx - 22.0 - l2.x).max(20.0);
            }
            let chars = (path_w / 6.2).max(8.0) as usize;
            ui.label_in(Rect { w: path_w, ..l2 }, &crate::util::short_path(root, chars), Style::new(size::CAPTION, weight::REGULAR, t.muted), Align::Left);
        }
        Row::Live { info, n } => {
            let notice = app.session_notice(info.pane);
            let is_focus = focus_pane == Some(info.pane);
            let stacked = app.stack.contains(&info.pane);
            let renaming = app.renaming.as_ref().filter(|(id, _)| *id == info.pane).map(|(_, s)| s.clone());
            // its project's colour (the orchestrator's is gold)
            let color = t.project(&info.project_key, &app.theme);
            if selected {
                ui.round(r, radius::CONTROL, t.selected);
                ui.outline(r, radius::CONTROL, 1.0, t.accent.with_alpha(0.5));
            } else if is_focus && !app.side_focus {
                // the session you're in, washed in its project's colour; while the sidebar has the keyboard only
                // its selection shows
                ui.round(r, radius::CONTROL, color.with_alpha(if t.light { 0.1 } else { 0.11 }));
            } else if stacked {
                ui.round(r, radius::CONTROL, t.text.with_alpha(0.035));
            } else if hot > 0.0 {
                ui.round(r, radius::CONTROL, t.hover.fade(hot));
            }
            if let Some(notice) = notice.filter(|n| n.unread) {
                let ink = match notice.kind { bro_bridge::insights::NoticeKind::Input => t.warn, bro_bridge::insights::NoticeKind::Finished => t.good };
                ui.round(r, radius::CONTROL, ink.with_alpha(if t.light { 0.065 } else { 0.08 }));
            }
            ui.hit(r, Action::Side(SideHit::Row(i)));
            let line = Rect::new(r.x + 8.0, r.y + 6.0, r.w - 16.0, 20.0);
            let mid = line.y + 10.0;
            // number (alt+1–9), the agent's mark, the indicator
            let indent = if info.orchestrator && info.project_key == bro_core::orchestrator::GLOBAL_KEY { 0.0 } else { 10.0 };
            let x0 = line.x + indent;
            if let Some(n) = n {
                let st = Style::new(size::CAPTION, weight::SEMIBOLD, if is_focus { color } else { t.faint });
                ui.label_in(Rect::new(x0, line.y, 10.0, 20.0), &n.to_string(), st, Align::Center);
            }
            let mark = Rect::new(x0 + 14.0, mid - 9.0, 18.0, 18.0);
            let m = session_mark(app, info.pane, t);
            ui.mark_badged(m, session_badge(app, info.pane), mark, if info.orchestrator { 16.0 } else { 15.0 }, None);
            let text_x = mark.right() + 8.0;
            if let Some(text) = renaming {
                let fr = Rect::new(text_x - 4.0, r.y + 3.0, r.right() - 8.0 - text_x + 4.0, 26.0);
                widgets::field(ui, fr, &text, Some(text.len()), "a name for it", true, Action::None);
                ui.label(Point::new(fr.x + 6.0, fr.bottom() + 2.0), "enter saves · esc cancels", Style::new(size::CAPTION, weight::REGULAR, t.faint));
                return;
            }
            // right: the indicator and age, or close
            let mut right = line.right();
            let close = Rect::new(right - 20.0, mid - 10.0, 20.0, 20.0);
            if hot > 0.3 || selected {
                widgets::icon_button(ui, close, "\u{F0156}", if selected { t.danger } else { t.muted }, Action::Side(SideHit::Close(info.pane)));
                right = close.x - 2.0;
            } else {
                let a = crate::util::short_dur(info.age_secs);
                let st = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
                let w = ui.measure(&a, st);
                ui.label(Point::new(right - w, mid - 7.5), &a, st);
                right -= w + 6.0;
            }
            if let Some(notice) = notice {
                let (glyph, ink) = if !notice.unread { ("\u{F012C}", t.faint) } else { match notice.kind {
                    bro_bridge::insights::NoticeKind::Input => ("\u{F0026}", t.warn),
                    bro_bridge::insights::NoticeKind::Finished => ("\u{F012C}", t.good),
                }};
                ui.icon(Rect::new(right - 15.0, mid - 8.0, 16.0, 16.0), glyph, 14.0, ink);
                right -= 18.0;
            } else if matches!(info.activity, Some(Activity::Working | Activity::Blocked)) || info.done {
                status_dot(ui, Point::new(right - 7.0, mid), info.activity, info.done);
                right -= 18.0;
            }
            // primary + secondary text
            let who = info.profile.as_deref().map(|p| p.split(':').next_back().unwrap_or(p).to_string()).unwrap_or_default();
            let model = info.model.as_deref().map(|m| crate::services::launch::short_model(m.rsplit('/').next().unwrap_or(m))).unwrap_or_default();
            let detail = [who, model].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let (primary, secondary) = match (&info.name, info.harness) {
                _ if info.orchestrator => {
                    let mut rest = vec![];
                    rest.extend(info.name.clone().filter(|n| !n.is_empty()));
                    if !detail.is_empty() {
                        rest.push(detail);
                    }
                    ("Orchestrator".to_string(), rest.join(" · "))
                }
                (Some(name), _) => (name.clone(), detail),
                (None, Some(_)) if !detail.is_empty() => (detail, String::new()),
                (None, Some(h)) => (h.label().to_string(), String::new()),
                (None, None) => ("shell".to_string(), String::new()),
            };
            // the session you're in (or picked) reads in its project's colour, as in the terminal UI
            let strong = is_focus || selected || info.orchestrator;
            let pc = if info.orchestrator { t.gold } else if strong { color } else { t.text.mix(t.muted, 0.2) };
            let mut spans = vec![(primary, Style::new(size::BODY, if strong { weight::SEMIBOLD } else { weight::MEDIUM }, pc))];
            if !secondary.is_empty() {
                spans.push((format!("  {secondary}"), Style::new(size::SMALL, weight::REGULAR, t.faint)));
            }
            let tb = ui.rich(&spans, size::BODY, Some((right - text_x - 4.0).max(10.0)), false, Some(1));
            ui.draw(&tb, Point::new(text_x, mid - tb.h / 2.0));
            // why it waits / what it did
            if let Some(note) = info.note.as_deref().filter(|_| r.h > 36.0) {
                let waiting = note.starts_with('⏸');
                let text = note.trim_start_matches(['⏸', '↳']).trim();
                let c = if waiting { t.danger.mix(t.muted, 0.35) } else { t.muted };
                let nr = Rect::new(text_x, r.y + 28.0, r.right() - 10.0 - text_x, 16.0);
                ui.icon(Rect::new(nr.x - 1.0, nr.y, 12.0, 16.0), if waiting { "\u{F03E4}" } else { "\u{F0045}" }, size::CAPTION, c);
                ui.label_in(Rect { x: nr.x + 14.0, w: nr.w - 14.0, ..nr }, text, Style::new(size::SMALL, weight::REGULAR, c), Align::Left);
            }
            // the team it leads, under that
            if let Some(team) = info.team.as_deref() {
                let top = r.y + if info.note.is_some() { 46.0 } else { 30.0 };
                draw_team(ui, r, top, text_x, team, info.pane);
            }
        }
    }
}

fn draw_footer(app: &mut App, ui: &mut Ui, r: Rect, usage: &[crate::app::side_view::UsageLine], tile_row: bool, sessions: usize) {
    let t = ui.t;
    let mut y = r.y + 4.0;
    ui.hline(r.x, r.y, r.w, t.hairline);
    y += 6.0;
    // tile all / tiled · minimized
    if tile_row {
        let row = Rect::new(r.x, y, r.w, 28.0);
        if app.tile_all {
            // "▸ minimized 2" / "▾ minimized 2" (the terminal UI's words): show or hide the minimized sessions in
            // the grid; there only while some are minimized
            let shown = app.show_minimized;
            let has_min = !app.minimized.is_empty() || shown;
            let min_text = format!("minimized {}", app.minimized.len());
            let st = Style::new(size::SMALL, weight::MEDIUM, if shown { t.accent } else { t.muted });
            let mw = if has_min { ui.measure(&min_text, st) + 34.0 } else { 0.0 };
            let min_r = Rect::new(row.right() - mw, row.y, mw, row.h);
            let tile_r = Rect::new(row.x, row.y, row.w - if has_min { mw + 6.0 } else { 0.0 }, row.h);
            let h1 = ui.hover_amount("tile-row", tile_r);
            ui.round(tile_r, radius::CONTROL, t.hover.fade(h1));
            ui.icon(Rect::new(tile_r.x + 6.0, tile_r.y, 18.0, tile_r.h), "\u{F05AF}", size::BODY, t.text);
            ui.label_in(Rect::new(tile_r.x + 28.0, tile_r.y, tile_r.w - 30.0, tile_r.h), &format!("Tiled · {}", app.stack.len()), Style::new(size::SMALL, weight::SEMIBOLD, t.text), Align::Left);
            ui.hit(tile_r, Action::Side(SideHit::TileAll));
            if has_min {
                let h2 = ui.hover_amount("min-row", min_r);
                ui.round(min_r, radius::CONTROL, t.hover.fade(h2));
                ui.icon(Rect::new(min_r.x + 6.0, min_r.y, 16.0, min_r.h), if shown { "\u{F0140}" } else { "\u{F0142}" }, size::SMALL, st.color);
                ui.label_in(Rect::new(min_r.x + 24.0, min_r.y, min_r.w - 28.0, min_r.h), &min_text, st, Align::Left);
                ui.hit(min_r, Action::Side(SideHit::Minimized));
            }
        } else {
            let h = ui.hover_amount("tile-row", row);
            ui.round(row, radius::CONTROL, t.hover.fade(h));
            ui.icon(Rect::new(row.x + 6.0, row.y, 18.0, row.h), "\u{F05AF}", size::BODY, t.muted.mix(t.text, h));
            ui.label_in(Rect::new(row.x + 28.0, row.y, row.w - 100.0, row.h), &format!("Tile all · {sessions}"), Style::new(size::SMALL, weight::MEDIUM, t.text.mix(t.muted, 0.3)), Align::Left);
            let k = app.keymap.primary(crate::keymap::Act::TileAll);
            if !k.is_empty() && !app.alt_hints() {
                let st = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
                let kw = ui.measure(&k, st);
                ui.label(Point::new(row.right() - kw - 8.0, row.y + 7.0), &k, st);
            }
            ui.hit(row, Action::Side(SideHit::TileAll));
        }
        y += 34.0;
    }
    // new session strip
    let strip = Rect::new(r.x, y, r.w, 34.0);
    let icons = [(Some(bro_core::Harness::Claude), "Claude"), (Some(bro_core::Harness::Codex), "Codex"), (None, "Terminal")];
    let icon_w = 28.0;
    let icons_w = icons.len() as f32 * icon_w;
    let show_icons = strip.w >= 210.0;
    // Continue keeps its word when there's room for "New session" beside it
    let rest = strip.w - if show_icons { icons_w + 6.0 } else { 0.0 };
    let cont_w = if rest >= 230.0 { 96.0 } else { 34.0 };
    let new_w = (rest - cont_w - 6.0).max(34.0);
    let new_r = Rect::new(strip.x, strip.y, new_w, strip.h);
    let label = if new_w >= 116.0 { "New session" } else { "New" };
    widgets::button(ui, new_r, "", widgets::Kind::Primary, Action::Side(SideHit::Launcher));
    ui.icon(Rect::new(new_r.x + 8.0, new_r.y, 18.0, new_r.h), "\u{F0415}", size::TITLE, t.on_accent);
    ui.label_in(Rect::new(new_r.x + 28.0, new_r.y, new_r.w - 30.0, new_r.h), label, Style::new(size::BODY, weight::SEMIBOLD, t.on_accent), Align::Left);
    let cont_r = Rect::new(new_r.right() + 6.0, strip.y, cont_w, strip.h);
    let ch = ui.hover_amount("side-cont", cont_r);
    widgets::control(ui, cont_r, t.control.mix(t.control_hover, ch));
    if cont_w > 40.0 {
        ui.icon(Rect::new(cont_r.x + 6.0, cont_r.y, 18.0, cont_r.h), "\u{F02DA}", size::BODY, t.text);
        ui.label_in(Rect::new(cont_r.x + 26.0, cont_r.y, cont_r.w - 28.0, cont_r.h), "Continue", Style::new(size::SMALL, weight::SEMIBOLD, t.text), Align::Left);
    } else {
        ui.icon(cont_r, "\u{F02DA}", size::BODY, t.text);
        if ch > 0.5 {
            widgets::tooltip(ui, cont_r, "Continue an earlier session");
        }
    }
    ui.hit(cont_r, Action::Side(SideHit::Continue));
    if show_icons {
        let mut x = strip.right() - icons_w;
        for (h, label) in icons {
            let b = Rect::new(x, strip.y + 3.0, icon_w, 28.0);
            if widgets::icon_button(ui, b, "", t.muted, Action::Side(SideHit::Quick(h))) {
                widgets::tooltip(ui, b, label);
            }
            ui.mark(harness_mark(h), b, 15.0, None);
            x += icon_w;
        }
    }
    y += 44.0;
    // usage left: lines while there's room over the status rows (and in a window too short for its heading too, as
    // in the terminal UI, only those rows)
    let status_h = if app.usage_expanded { STATUS_H } else { 0.0 };
    if !usage.is_empty() && y + 22.0 <= r.bottom() - status_h {
        let head = Rect::new(r.x, y, r.w, 22.0);
        let arrow = if app.usage_expanded { "\u{F0140}" } else { "\u{F0142}" };
        let hh = ui.hover_amount("usage-head", head);
        ui.round(head, radius::CONTROL, t.hover.fade(hh * 0.7));
        ui.icon(Rect::new(head.x + 2.0, head.y, 14.0, head.h), arrow, size::SMALL, t.faint);
        let lb = ui.label(Point::new(head.x + 18.0, head.y + 3.0), "Usage left", Style::new(size::SMALL, weight::SEMIBOLD, t.text.mix(t.muted, 0.35)));
        let info = Rect::new(head.x + 18.0 + lb.w + 4.0, head.y + 2.0, 18.0, 18.0);
        ui.hit(head, Action::Side(SideHit::UsageToggle));
        widgets::icon_button(ui, info, "\u{F02FD}", t.faint, Action::Side(SideHit::Stats));
        // the figures' columns (right edges), clear of the + a launchable row shows at its end on hover
        const PLUS: f32 = 16.0;
        let wk_r = head.right() - PLUS - 4.0;
        let h5_r = wk_r - 40.0;
        let resets = app.alt_hints();
        let hs = Style::new(size::CAPTION, weight::SEMIBOLD, t.faint);
        if resets {
            // clear of the key tip at the header's end
            let w = ui.measure("resets", hs);
            ui.label(Point::new(wk_r - 14.0 - w, head.y + 4.0), "resets", hs);
        } else {
            for (text, right) in [("5h", h5_r), ("wk", wk_r)] {
                let w = ui.measure(text, hs);
                ui.label(Point::new(right - w, head.y + 4.0), text, hs);
            }
        }
        y += 26.0;
        let now = crate::util::now_secs();
        for u in usage {
            if y + 24.0 > r.bottom() - status_h {
                break;
            }
            let row = Rect::new(r.x, y, r.w, 24.0);
            let launchable = u.harness.is_some();
            let rh = if launchable { ui.hover_amount(("usage", &u.label), row) } else { 0.0 };
            ui.round(row, radius::CONTROL, t.hover.fade(rh));
            // the row launches (a family's line: your usual login; a profile's: that login); the Fable arrow on it,
            // registered after, wins its own spot
            ui.hit(row, match u.harness {
                Some(h) => Action::Side(SideHit::LaunchUsage(h, u.profile.clone())),
                None => Action::Side(SideHit::Usage),
            });
            if let Some(h) = u.harness {
                let mark = if u.profile.as_deref().is_some_and(|p| bro_core::profiles::is_gemini_id(&p)) { super::kit::brand::provider("gemini").unwrap() } else { harness_mark(Some(h)) };
                ui.mark(mark, Rect::new(row.x + 4.0, row.y, 16.0, row.h), 13.0, None);
            }
            let label_x = row.x + 24.0;
            let ls = Style::new(size::SMALL, if launchable { weight::SEMIBOLD } else { weight::REGULAR }, if launchable { t.text } else { t.muted });
            let label_room = if u.profile.as_deref().is_some_and(bro_core::profiles::is_gemini_id) {
                let note_w = u.note.as_ref().map(|note| ui.measure(note, Style::new(size::SMALL, weight::MEDIUM, t.faint))).unwrap_or(0.0);
                (wk_r - label_x - note_w - 12.0).max(32.0)
            } else { 66.0 };
            let lb = ui.line(&u.label, ls, Some(label_room));
            ui.draw(&lb, Point::new(label_x, row.y + (24.0 - lb.h) / 2.0));
            if let Some(open) = u.fable {
                let fr = Rect::new(label_x + lb.w + 2.0, row.y + 3.0, 16.0, 18.0);
                ui.icon(fr, if open { "\u{F0140}" } else { "\u{F0142}" }, size::SMALL, t.faint);
                ui.hit(fr, Action::Side(SideHit::Fable));
            }
            let tight = [u.h5, u.wk].into_iter().flatten().fold(f64::INFINITY, f64::min);
            match &u.note {
                Some(n) if u.h5.is_none() && u.wk.is_none() => {
                    let st = Style::new(size::SMALL, weight::MEDIUM, t.faint);
                    let w = ui.measure(n, st);
                    ui.label(Point::new(wk_r - w, row.y + 4.0), n, st);
                }
                _ if resets => {
                    let c = if tight.is_finite() { left_color(tight, t) } else { t.muted };
                    let when = u.resets_at.map(|at| format!("Resets @ {}", crate::util::reset_time(at))).unwrap_or_else(|| "no reset reported".into());
                    let st = Style::new(size::SMALL, weight::MEDIUM, c);
                    let w = ui.measure(&when, st);
                    ui.label(Point::new(wk_r - w, row.y + 4.0), &when, st);
                }
                _ => {
                    // the meter shows the tighter window: up to 8 segments, as the terminal UI's ▰▱
                    let meter_x = label_x + 72.0;
                    let meter_w = (h5_r - 44.0 - meter_x).max(0.0);
                    if meter_w >= 24.0 && tight.is_finite() {
                        let n = widgets::segment_count(meter_w, 6.0).min(8);
                        let w = (n as f32 * 6.0).min(meter_w);
                        widgets::segments(ui, Rect::new(meter_x, row.y + 9.0, w, 6.0), (tight / 100.0) as f32, n, left_color(tight, t));
                    } else if app.usage_expanded && let Some(at) = u.resets_at {
                        ui.label(Point::new(meter_x, row.y + 4.0), &crate::util::countdown(at - now), Style::new(size::CAPTION, weight::REGULAR, t.faint));
                    }
                    let fig = |v: Option<f64>, approx: bool| v.map(|v| (format!("{}{v:.0}%", if approx { "≈" } else { "" }), left_color(v, t))).unwrap_or(("—".into(), t.faint));
                    let (a, ac) = fig(u.h5, u.approx);
                    let (b, bc) = fig(u.wk, false);
                    let st = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
                    let bw = ui.measure(&b, st);
                    ui.label(Point::new(wk_r - bw, row.y + 4.0), &b, st.color(bc));
                    let aw = ui.measure(&a, st);
                    ui.label(Point::new(h5_r - aw, row.y + 4.0), &a, st.color(ac));
                }
            }
            if rh > 0.05 {
                ui.icon(Rect::new(row.right() - PLUS - 2.0, row.y, PLUS, row.h), "\u{F0415}", size::SMALL, t.accent.fade(rh));
            }
            y += 26.0;
        }
    }
    // the session counter, proxy and phone bridge (expanded usage only)
    if app.usage_expanded {
        let tally = app.session_tally();
        let st = app.svc.state();
        let proxy = match &st.proxy.status {
            Avail::Ready(i) => {
                let n = st.proxy.routes.len();
                let use_ = match n {
                    0 => "idle".to_string(),
                    1 => "1 session".to_string(),
                    n => format!("{n} sessions"),
                };
                (t.good, format!("{use_} · port {}", i.port), n > 0)
            }
            Avail::Loading => (t.muted, "starting".into(), false),
            Avail::Unavailable(_) => (t.danger, "off".into(), false),
        };
        let bridge = match &st.bridge.status {
            Avail::Ready(s) if s.running => {
                let who = match s.clients {
                    0 => "no phones".to_string(),
                    1 => "1 connected".to_string(),
                    n => format!("{n} connected"),
                };
                (t.good, format!("{who} · port {}", s.port), s.clients > 0)
            }
            Avail::Loading if st.bridge.enabled => (t.muted, "starting".into(), false),
            _ => (t.danger, "off".into(), false),
        };
        drop(st);
        let base = r.bottom() - STATUS_H + 2.0;
        session_counter(ui, Rect::new(r.x, base, r.w, 22.0), tally);
        for (k, (icon, name, (c, text, busy), hit)) in [("\u{F04E1}", "Proxy", proxy, SideHit::Proxy), ("\u{F05A9}", "Phone", bridge, SideHit::Bridge)].into_iter().enumerate() {
            let row = Rect::new(r.x, base + (k + 1) as f32 * STATUS_ROW, r.w, 22.0);
            let h = ui.hover_amount(("status-row", k), row);
            ui.round(row, radius::CONTROL, t.hover.fade(h));
            ui.icon(Rect::new(row.x + 4.0, row.y, 16.0, row.h), icon, size::SMALL, t.faint);
            ui.label(Point::new(row.x + 24.0, row.y + 3.0), name, Style::new(size::SMALL, weight::MEDIUM, t.muted));
            // on, starting or off: a check, a spinner or a cross, never a coloured dot
            let state = Rect::new(row.x + 72.0, row.y, 16.0, row.h);
            if c == t.good {
                ui.icon(state, "\u{F012C}", size::SMALL, t.good);
            } else if c == t.danger {
                ui.icon(state, "\u{F0156}", size::SMALL, t.danger);
            } else {
                ui.spinner(state.center(), 4.0, 1.4, t.muted);
            }
            ui.label_in(Rect::new(row.x + 90.0, row.y, row.w - 92.0, row.h), &text, Style::new(size::SMALL, weight::MEDIUM, if busy { t.text } else { t.muted }), Align::Left);
            ui.hit(row, Action::Side(hit));
        }
    }
}

/// The status rows under usage (sessions, proxy, phone; expanded usage only) sit this far apart…
const STATUS_ROW: f32 = 24.0;
/// …at the footer's foot, in this much of it.
const STATUS_H: f32 = 3.0 * STATUS_ROW + 4.0;

/// Room between two of the session counter's statuses…
const STATUS_GAP: f32 = 8.0;
/// …and between a status's icon and its count.
const COUNT_GAP: f32 = 3.0;

/// The width of a status's icon as drawn (the spinner is narrower than the alert), so each count sits as far from
/// its icon.
fn status_icon_w(s: Status) -> f32 {
    match s {
        Status::Working => 10.0,
        Status::NeedsYou => 13.0,
        Status::Done => 12.0,
    }
}

/// What of the session counter fits `room` px, from the widths of its number, " running" and the separator and of
/// each status (icon and count): whether "running" stays, and which statuses do. Short of room "running" goes
/// first, then done, then working; what needs you always stays.
fn counter_fit(room: f32, (num, word, sep): (f32, f32, f32), statuses: &[(Status, f32)]) -> (bool, Vec<Status>) {
    let mut kept = statuses.to_vec();
    let width = |running: bool, kept: &[(Status, f32)]| {
        let rest = kept.iter().map(|s| s.1).sum::<f32>() + STATUS_GAP * kept.len().saturating_sub(1) as f32;
        num + if running { word } else { 0.0 } + if kept.is_empty() { 0.0 } else { sep + rest }
    };
    let mut running = true;
    while width(running, &kept) > room {
        if running {
            running = false;
        } else if let Some(i) = [Status::Done, Status::Working].iter().find_map(|d| kept.iter().position(|s| s.0 == *d)) {
            kept.remove(i);
        } else {
            break;
        }
    }
    (running, kept.into_iter().map(|s| s.0).collect())
}

/// The session counter over the proxy and phone rows: how many sessions run, the number lined up with what those
/// rows say, then how many are working, need you or are done, each the icon its row shows with its count (as the
/// project tabs show them; the words are in a tooltip). A click shows them all, or with none starts one.
fn session_counter(ui: &mut Ui, row: Rect, n: Tally) {
    const SEP: &str = " · ";
    let t = ui.t;
    let h = ui.hover_amount("status-sessions", row);
    ui.round(row, radius::CONTROL, t.hover.fade(h));
    ui.icon(Rect::new(row.x + 4.0, row.y, 16.0, row.h), "\u{F018D}", size::SMALL, t.faint);
    ui.label(Point::new(row.x + 24.0, row.y + 3.0), "Sessions", Style::new(size::SMALL, weight::MEDIUM, t.muted));
    ui.hit(row, Action::Side(SideHit::Sessions));
    let x = row.x + 90.0;
    let room = row.right() - 4.0 - x;
    if n.total == 0 {
        ui.label_in(Rect::new(x, row.y, room, row.h), "none running", Style::new(size::SMALL, weight::MEDIUM, t.muted), Align::Left);
        if h > 0.5 {
            widgets::tooltip(ui, row, "New session");
        }
        return;
    }
    let num = n.total.to_string();
    let ns = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let ws = Style::new(size::SMALL, weight::MEDIUM, t.text);
    let nw = ui.measure(&num, ns);
    let ww = ui.measure(" running", ws);
    let sw = ui.measure(SEP, ws);
    let statuses: Vec<(Status, String, f32)> = n
        .statuses()
        .map(|(s, k)| {
            let k = k.to_string();
            let w = status_icon_w(s) + COUNT_GAP + ui.measure(&k, ns);
            (s, k, w)
        })
        .collect();
    let (running, kept) = counter_fit(room, (nw, ww, sw), &statuses.iter().map(|s| (s.0, s.2)).collect::<Vec<_>>());
    let y = row.y + 3.0;
    ui.label(Point::new(x, y), &num, ns);
    if running {
        ui.label(Point::new(x + nw, y), " running", ws);
    }
    let mut cx = x + nw + if running { ww } else { 0.0 };
    if !kept.is_empty() {
        ui.label(Point::new(cx, y), SEP, ws.color(t.muted));
        cx += sw;
    }
    for (s, k, w) in statuses.iter().filter(|s| kept.contains(&s.0)) {
        let iw = status_icon_w(*s);
        let icon = Rect::new(cx, row.y + (row.h - 13.0) / 2.0, iw, 13.0);
        let c = match s {
            Status::Working => t.accent,
            Status::NeedsYou => t.danger,
            Status::Done => t.good,
        };
        match s {
            Status::Working => ui.spinner(icon.center(), 4.0, 1.4, c),
            Status::NeedsYou => ui.icon(icon, "\u{F0028}", size::SMALL, c),
            Status::Done => ui.icon(icon, "\u{F012C}", size::SMALL, c),
        }
        ui.label(Point::new(cx + iw + COUNT_GAP, y), k, ns.color(c));
        cx += w + STATUS_GAP;
    }
    if h > 0.5 {
        widgets::tooltip(ui, row, &n.words());
    }
}

/// Colour for "% left": the terminal UI's thresholds (≤20 danger, ≤50 amber, else good).
pub fn left_color(left: f64, t: &super::kit::Tokens) -> Color {
    if left <= 20.0 {
        t.danger
    } else if left <= 50.0 {
        t.warn
    } else {
        t.good
    }
}

// ------------------------------------------------------------------ project tabs

/// A project's name never shrinks below this to make room for the others, nor takes more than this (logical px;
/// the terminal UI's 6 and 24 columns).
const MIN_NAME: f32 = 44.0;
const MAX_NAME: f32 = 176.0;
/// Room a "‹ 2" / "3 ›" fold takes.
const FOLD_W: f32 = 48.0;

/// One tab of the strip, as it reads.
struct StripEntry {
    tab: StripTab,
    label: String,
    /// sessions in its grid (the orchestrator's tab shows none)
    count: Option<usize>,
    /// an agent in it is mid-turn
    working: bool,
    /// sessions waiting on you
    blocked: usize,
    /// sessions that finished while you weren't looking
    done: usize,
}

impl StripEntry {
    /// A project's tab: it has a × that closes the project.
    fn closable(&self) -> bool {
        matches!(self.tab, StripTab::Project(_))
    }
    /// The orchestrator's tab shows a spinner after its name while it works.
    fn spins_after(&self) -> bool {
        matches!(self.tab, StripTab::Orchestrator) && self.working
    }
    /// Room the tab takes besides its name: padding, the icon All and the orchestrator lead with (a project's
    /// tab has none: its name is in its colour), the count, the attention badges and the ×.
    fn chrome(&self) -> f32 {
        let mut w = 12.0 + 12.0 + if self.closable() { 0.0 } else { 16.0 + 8.0 };
        if self.count.is_some() || self.spins_after() {
            w += 22.0;
        }
        if self.blocked > 0 {
            w += 26.0;
        }
        if self.done > 0 {
            w += 26.0;
        }
        if self.closable() {
            w += 26.0;
        }
        w
    }
}

/// The tabs that fit `room` around tab `anchor` (each `widths[i]` wide; a fold on either side takes [`FOLD_W`]):
/// the range shown, the rest folded. The anchor always shows.
fn strip_window(widths: &[f32], anchor: usize, room: f32) -> (usize, usize) {
    if widths.is_empty() {
        return (0, 0);
    }
    let anchor = anchor.min(widths.len() - 1);
    let fits = |lo: usize, hi: usize| widths[lo..hi].iter().sum::<f32>() + if lo > 0 { FOLD_W } else { 0.0 } + if hi < widths.len() { FOLD_W } else { 0.0 } <= room;
    let (mut lo, mut hi) = (anchor, anchor + 1);
    loop {
        let mut grew = false;
        if hi < widths.len() && fits(lo, hi + 1) {
            hi += 1;
            grew = true;
        }
        if lo > 0 && fits(lo - 1, hi) {
            lo -= 1;
            grew = true;
        }
        if !grew {
            return (lo, hi);
        }
    }
}

/// The widest name cap (from the longest name, at most [`MAX_NAME`]) that lets every tab fit `room`, never below
/// [`MIN_NAME`]: names shorten before tabs fold.
fn strip_cap(names: &[f32], chrome: &[f32], room: f32) -> f32 {
    let total = |cap: f32| names.iter().zip(chrome).map(|(n, c)| c + n.min(cap)).sum::<f32>();
    let mut cap = names.iter().copied().fold(0.0, f32::max).min(MAX_NAME);
    while cap > MIN_NAME && total(cap) > room {
        cap = (cap - 4.0).max(MIN_NAME);
    }
    cap
}

/// The project tabs, WinUI's TabView: tabs on the window colour; the one showing filled with the content
/// layer's colour, its top corners rounded and its sides outlined, open into the sessions below; the rest are a
/// label with a hover fill and hairline separators. All, ★, one per project with running sessions, then + and
/// (at the far end) the minimized switch. Names shorten before tabs that still don't fit fold into "‹ n" / "n ›"
/// around the tab the keys picked (else the one showing). The keys' pick (alt+b with no sidebar) has a focus
/// outline. Draws the line under the tabs too (it stops at the selected tab).
pub fn draw_strip(app: &mut App, ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let tabs = app.project_tabs();
    let items = app.strip_items(&tabs);
    if let Some(sel) = app.strip_sel {
        app.strip_sel = Some(sel.min(items.len().saturating_sub(1)));
    }
    let picked = app.strip_sel.and_then(|i| items.get(i)).cloned();
    let showing = app.drawn_tab.clone();
    // the focused session's project reads bold
    let focus_key = app.focused().and_then(|id| app.panes.get(&id)).and_then(|p| p.as_term_ref()).map(|term| term.meta.project.key.clone());
    let top = r.y + 6.0;
    let h = r.bottom() - top;
    let mid = top + h / 2.0;
    let mut limit = r.right() - 8.0;
    // far end: the minimized sessions' switch, "▸ 2 minimized" / "▾ 2 minimized"
    if app.tile_all && !app.minimized.is_empty() {
        let shown = app.show_minimized;
        let text = format!("{} minimized", app.minimized.len());
        let st = Style::new(size::SMALL, weight::MEDIUM, if shown { t.accent } else { t.muted });
        let w = ui.measure(&text, st) + 38.0;
        let mr = Rect::new(limit - w, mid - 14.0, w, 28.0);
        let hot = ui.hover_amount("strip-min", mr);
        ui.round(mr, radius::CONTROL, t.text.with_alpha(0.06 * hot));
        ui.icon(Rect::new(mr.x + 8.0, mr.y, 16.0, mr.h), if shown { "\u{F0140}" } else { "\u{F0142}" }, size::SMALL, st.color);
        ui.label_in(Rect::new(mr.x + 28.0, mr.y, mr.w - 32.0, mr.h), &text, st, Align::Left);
        ui.hit(mr, Action::Side(SideHit::Strip(StripHit::Minimized)));
        limit = mr.x - 8.0;
    }
    limit -= 36.0; // the + after the tabs
    // what each tab says: All counts the sessions its grid shows (minimized ones aside, as the terminal UI's)
    let entries: Vec<StripEntry> = items
        .iter()
        .filter_map(|tab| {
            Some(match tab {
                StripTab::All => StripEntry { tab: tab.clone(), label: "All".into(), count: Some(app.tile_ids(None).len()), working: false, blocked: 0, done: 0 },
                StripTab::Orchestrator => {
                    let ids = app.orchestrator_panes();
                    let activity = |id: &crate::pane::PaneId| app.panes.get(id).and_then(|p| p.activity());
                    let blocked = ids.iter().filter(|id| activity(id) == Some(Activity::Blocked)).count();
                    let done = ids.iter().filter(|id| activity(id) != Some(Activity::Blocked) && app.done.contains(id)).count();
                    StripEntry { tab: tab.clone(), label: "Orchestrators".into(), count: Some(ids.len()), working: ids.iter().any(|id| activity(id) == Some(Activity::Working)), blocked, done }
                }
                StripTab::Project(key) => {
                    let p = tabs.iter().find(|p| &p.key == key)?;
                    StripEntry { tab: tab.clone(), label: p.name.clone(), count: Some(p.live), working: p.working > 0, blocked: p.blocked, done: p.done }
                }
            })
        })
        .collect();
    let label_st = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let names: Vec<f32> = entries.iter().map(|e| ui.measure(&e.label, label_st)).collect();
    let chrome: Vec<f32> = entries.iter().map(StripEntry::chrome).collect();
    // All and ★ always show; the project tabs share what's left
    let fixed = entries.iter().take_while(|e| !e.closable()).count();
    let x0 = r.x + 8.0;
    let fixed_w: f32 = (0..fixed).map(|i| chrome[i] + names[i].min(MAX_NAME)).sum();
    let room = (limit - x0 - fixed_w).max(0.0);
    let cap = strip_cap(&names[fixed..], &chrome[fixed..], room);
    let widths: Vec<f32> = (fixed..entries.len()).map(|i| chrome[i] + names[i].min(cap)).collect();
    let at = |tab: &Option<StripTab>| entries[fixed..].iter().position(|e| tab.as_ref() == Some(&e.tab));
    let (lo, hi) = strip_window(&widths, at(&picked).or_else(|| at(&showing)).unwrap_or(0), room);
    // lay them out: All and ★, the left fold, the tabs that fit, the right fold
    let mut x = x0;
    let mut placed: Vec<(usize, Rect, f32)> = vec![];
    for i in 0..fixed {
        let w = chrome[i] + names[i].min(MAX_NAME);
        placed.push((i, Rect::new(x, top, w, h), MAX_NAME));
        x += w;
    }
    let mut folds: Vec<(Rect, bool, &[StripEntry])> = vec![];
    if lo > 0 {
        folds.push((Rect::new(x + 4.0, mid - 14.0, FOLD_W - 8.0, 28.0), true, &entries[fixed..fixed + lo]));
        x += FOLD_W;
    }
    for (j, &w) in widths.iter().enumerate().take(hi).skip(lo) {
        placed.push((fixed + j, Rect::new(x, top, w, h), cap));
        x += w;
    }
    if hi < widths.len() {
        folds.push((Rect::new(x + 4.0, mid - 14.0, FOLD_W - 8.0, 28.0), false, &entries[fixed + hi..]));
        x += FOLD_W;
    }
    let selected = placed.iter().position(|(i, ..)| showing.as_ref() == Some(&entries[*i].tab));
    let hot: Vec<f32> = placed.iter().map(|(i, tr, _)| ui.hover_amount(("strip", format!("{:?}", entries[*i].tab)), *tr)).collect();
    // the line under the tabs, broken by the one showing
    let line_y = r.bottom() - ui.hair();
    match selected.map(|k| placed[k].1) {
        Some(sr) => {
            ui.hline(r.x + radius::OVERLAY, line_y, (sr.x - r.x - radius::OVERLAY).max(0.0), t.stroke);
            ui.hline(sr.right(), line_y, (r.right() - sr.right()).max(0.0), t.stroke);
        }
        None => ui.hline(r.x + radius::OVERLAY, line_y, (r.w - radius::OVERLAY).max(0.0), t.stroke),
    }
    // an anchor too wide for the strip is cut at its end
    ui.clip(Rect::new(r.x, r.y, (limit + 36.0 - r.x).max(0.0), r.h), 0.0);
    for (k, &(i, tr, cap)) in placed.iter().enumerate() {
        let e = &entries[i];
        let on = selected == Some(k);
        let kb = picked.as_ref() == Some(&e.tab);
        if on {
            // filled like the content below, outlined on three sides, open at the bottom
            ui.glass(Rect { h: tr.h + 1.0, ..tr }, Corners::top(radius::OVERLAY), t.layer);
            let o = Rect { h: tr.h + radius::OVERLAY, ..tr };
            ui.clip(Rect { h: tr.h, ..tr }, 0.0);
            ui.outline(o, radius::OVERLAY, 1.0, t.stroke);
            ui.unclip();
        } else {
            if hot[k] > 0.0 {
                ui.round(Rect::new(tr.x + 3.0, tr.y + 4.0, tr.w - 6.0, tr.h - 8.0), radius::CONTROL, t.text.with_alpha(0.05 * hot[k]));
            }
            // a hairline between two quiet tabs
            let next_quiet = k + 1 < placed.len() && selected != Some(k + 1) && hot[k + 1] < 0.05;
            if next_quiet && hot[k] < 0.05 {
                ui.vline(tr.right(), tr.y + 9.0, tr.h - 18.0, t.hairline);
            }
        }
        // the keys' pick: keyboard focus, a full outline
        if kb {
            ui.outline(Rect::new(tr.x + 3.0, tr.y + 3.0, tr.w - 6.0, tr.h - 7.0), radius::CONTROL, 1.0, t.accent);
        }
        ui.hit(tr, Action::Side(SideHit::Strip(StripHit::Tab(e.tab.clone()))));
        let mut cx = tr.x + 12.0;
        let icon_r = Rect::new(cx, mid - 8.0, 16.0, 16.0);
        let quiet = if on || kb { t.text } else { t.text.mix(t.muted, 0.3) };
        // All and the orchestrator lead with an icon; a project's name is in its colour, as in the sidebar
        let ink = match &e.tab {
            StripTab::All => {
                ui.icon(icon_r, "\u{F05AF}", size::BODY, quiet);
                cx += 16.0 + 8.0;
                quiet
            }
            StripTab::Orchestrator => {
                ui.icon(icon_r, "\u{F04CE}", size::BODY, t.gold);
                cx += 16.0 + 8.0;
                quiet
            }
            StripTab::Project(key) => t.project(key, &app.theme),
        };
        let bold = on || matches!(&e.tab, StripTab::Project(key) if focus_key.as_deref() == Some(key.as_str()));
        let st = Style::new(size::SMALL, if bold { weight::SEMIBOLD } else { weight::MEDIUM }, ink);
        let lb = ui.line(&e.label, st, Some(cap));
        ui.draw(&lb, Point::new(cx, mid - lb.h / 2.0));
        cx += lb.w + 8.0;
        let cs = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
        if let Some(n) = e.count {
            // while one of its agents works, the count's place spins
            if e.working && (e.closable() || e.spins_after()) {
                ui.spinner(Point::new(cx + 5.0, mid), 4.0, 1.4, if e.spins_after() { t.gold } else { t.accent });
            } else {
                ui.label_in(Rect::new(cx, tr.y, 14.0, tr.h), &n.to_string(), cs, Align::Left);
            }
            cx += 18.0;
        } else if e.spins_after() {
            ui.spinner(Point::new(cx + 6.0, mid), 4.0, 1.4, t.gold);
            cx += 18.0;
        }
        if e.blocked > 0 {
            ui.icon(Rect::new(cx, mid - 7.0, 14.0, 14.0), "\u{F0028}", size::SMALL, t.danger);
            ui.label_in(Rect::new(cx + 15.0, tr.y, 12.0, tr.h), &e.blocked.to_string(), cs.color(t.danger), Align::Left);
            cx += 26.0;
        }
        if e.done > 0 {
            ui.icon(Rect::new(cx, mid - 7.0, 14.0, 14.0), "\u{F012C}", size::SMALL, t.good);
            ui.label_in(Rect::new(cx + 15.0, tr.y, 12.0, tr.h), &e.done.to_string(), cs.color(t.good), Align::Left);
        }
        // the × shows under the pointer, on the tab showing and on the keys' pick (delete closes it); its room is
        // always kept, so the strip doesn't shift as you move along it
        if let StripTab::Project(key) = &e.tab
            && (hot[k] > 0.3 || on || kb)
        {
            let xr = Rect::new(tr.right() - 30.0, mid - 10.0, 20.0, 20.0);
            widgets::icon_button(ui, xr, "\u{F0156}", t.muted, Action::Side(SideHit::Strip(StripHit::Close(key.clone()))));
        }
    }
    // the tabs that don't fit: "‹ n" before, "n ›" after, with an alert when one of them wants you; a click lists
    // every tab in the palette
    for &(fr, left, folded) in &folds {
        let mh = ui.hover_amount(("strip-more", left), fr);
        ui.round(fr, radius::CONTROL, t.text.with_alpha(0.05 * mh));
        let wants = folded.iter().any(|e| e.blocked + e.done > 0);
        let n = folded.len().to_string();
        let st = Style::new(size::SMALL, weight::SEMIBOLD, t.muted);
        let nw = ui.measure(&n, st);
        let inner_w = 14.0 + 2.0 + nw + if wants { 14.0 } else { 0.0 };
        let mut fx = fr.x + (fr.w - inner_w) / 2.0;
        if left {
            ui.icon(Rect::new(fx, fr.y, 14.0, fr.h), "\u{F0141}", size::SMALL, t.muted);
            fx += 16.0;
            ui.label_in(Rect::new(fx, fr.y, nw + 1.0, fr.h), &n, st, Align::Left);
            fx += nw;
        } else {
            ui.label_in(Rect::new(fx, fr.y, nw + 1.0, fr.h), &n, st, Align::Left);
            fx += nw + 2.0;
            ui.icon(Rect::new(fx, fr.y, 14.0, fr.h), "\u{F0142}", size::SMALL, t.muted);
            fx += 14.0;
        }
        if wants {
            ui.icon(Rect::new(fx, fr.y, 14.0, fr.h), "\u{F0028}", size::CAPTION, t.danger);
        }
        ui.hit(fr, Action::Side(SideHit::Strip(StripHit::More)));
    }
    ui.unclip();
    // then +, a new session
    let plus = Rect::new(x.min(limit) + 4.0, mid - 14.0, 28.0, 28.0);
    if widgets::icon_button(ui, plus, "\u{F0415}", t.muted, Action::Side(SideHit::Strip(StripHit::New))) {
        widgets::tooltip(ui, plus, "New session");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_names_shorten_before_tabs_fold() {
        // three 100px names with 80px of chrome each: 540 fits whole; at 420 the names shorten to 60
        let names = [100.0, 100.0, 100.0];
        let chrome = [80.0, 80.0, 80.0];
        assert_eq!(strip_cap(&names, &chrome, 540.0), 100.0);
        assert_eq!(strip_cap(&names, &chrome, 420.0), 60.0);
        // never below the minimum, however tight
        assert_eq!(strip_cap(&names, &chrome, 10.0), MIN_NAME);
        // no wider than the longest name, nor the maximum
        assert_eq!(strip_cap(&[300.0], &[80.0], 1000.0), MAX_NAME);
    }

    #[test]
    fn sidebar_scrollbar_drags_and_jumps() {
        // a 400px list over 1200px of rows: no bar when everything fits
        let view = Rect::new(0.0, 100.0, 260.0, 400.0);
        assert!(bar_geometry(view, 380.0, 0.0).is_none());
        let mut s = SideState { content: 1200.0, view: 400.0, ..Default::default() };
        s.bar = bar_geometry(view, s.content, 0.0);
        let (track, top, h) = s.bar.expect("a bar");
        assert_eq!(top, track.y);
        // dragging the thumb by its whole travel scrolls to the end, and no further
        s.grab_bar(top + 5.0);
        assert_eq!(s.scroll, 0.0, "a grab on the thumb doesn't move it");
        s.drag_bar(top + 5.0 + (track.h - h));
        assert!((s.scroll - 800.0).abs() < 0.01, "{}", s.scroll);
        s.drag_bar(top + 5.0 + (track.h - h) / 2.0);
        assert!((s.scroll - 400.0).abs() < 0.01, "{}", s.scroll);
        s.drag_bar(top + 5000.0);
        assert_eq!(s.scroll, 800.0);
        assert!(s.manual, "the list then holds until a key moves the selection");
        // a press on the track below the thumb brings the thumb under the pointer
        let mut s = SideState { content: 1200.0, view: 400.0, ..Default::default() };
        s.bar = bar_geometry(view, s.content, 0.0);
        s.grab_bar(track.bottom());
        assert_eq!(s.scroll, 800.0);
    }

    #[test]
    fn the_session_counter_gives_way_word_first_and_keeps_what_needs_you() {
        use Status::*;
        // "5" 7, " running" 49, " · " 10, statuses 23 each with 8 between: 151 in all
        let widths = (7.0, 49.0, 10.0);
        let all = [(Working, 23.0), (NeedsYou, 23.0), (Done, 23.0)];
        assert_eq!(counter_fit(160.0, widths, &all), (true, vec![Working, NeedsYou, Done]));
        assert_eq!(counter_fit(120.0, widths, &all), (false, vec![Working, NeedsYou, Done]));
        assert_eq!(counter_fit(80.0, widths, &all), (false, vec![Working, NeedsYou]));
        assert_eq!(counter_fit(30.0, widths, &all), (false, vec![NeedsYou]));
        assert_eq!(counter_fit(0.0, widths, &all), (false, vec![NeedsYou]));
        // all idle: the count and its word
        assert_eq!(counter_fit(160.0, widths, &[]), (true, vec![]));
    }

    #[test]
    fn strip_folds_around_the_anchor() {
        let w = [100.0; 6];
        // everything fits: no folds
        assert_eq!(strip_window(&w, 0, 600.0), (0, 6));
        // room for three tabs and two folds around the 4th
        assert_eq!(strip_window(&w, 3, 3.0 * 100.0 + 2.0 * FOLD_W), (2, 5));
        // at the start: only a fold on the right
        let (lo, hi) = strip_window(&w, 0, 3.0 * 100.0 + FOLD_W);
        assert_eq!((lo, hi), (0, 3));
        // the anchor shows even when nothing fits
        assert_eq!(strip_window(&w, 5, 10.0), (5, 6));
        assert_eq!(strip_window(&[], 0, 100.0), (0, 0));
    }
}
