//! The session tabs in the window (the terminal UI draws its own in `app/project_tabs.rs`): the sessions of what the
//! sidebar picked (a project, or the orchestrators), headed by that project's icon and name, then its All, then a
//! tab per session, then + and, at the far end, the minimized switch. Every session tiled (the sidebar's All
//! sessions) has no strip.
//!
//! Every tab is a shape of its own, a soft chip with a gap either side, so a crowded strip still reads tab by tab;
//! a session's leads with its agent's mark (with every session or the orchestrators listed, then its project's icon,
//! as in the sidebar) and ends in what it's doing: a spinner while it works, a mark when it waits on you or
//! finished unseen. The one showing takes the content layer's colour and opens into the sessions below (WinUI's
//! TabView). A tab is never narrower than [`TAB_MIN`] nor its name cut below [`NAME_MIN`]: names shorten first, then
//! what still doesn't fit either scrolls along the one row (the default), All staying put while arrows at either
//! end say how many tabs are past them and flag one that wants you, or (the stack setting) wraps onto more rows, at
//! most [`MAX_ROWS`]. Past that, the rest fold into "+n", which lists every session.
//!
//! Motion: every tab's place springs ([`Ui::flow_rect`]), so a tab opening makes its neighbours slide over and one
//! closing lets them close up; the showing tab's fill is one shape that glides from tab to tab; counts roll.

use super::actions::Action;
use super::chrome::{session_badge, session_mark};
use super::kit::motion::Spring;
use super::kit::{Style, Ui, radius, size, weight, widgets};
use super::layout::{flow_begin, key_of};
use crate::app::App;
use crate::app::SideHit;
use crate::app::project_tabs::{SessionTab, StripHit, StripTab, stack_rows};
use crate::pane::PaneId;
use bro_core::config::TabOverflow;
use bro_gfx::text::Align;
use bro_gfx::{Corners, Point, Rect};

/// A row of tabs, and the room above the first (logical px): one row is [`super::layout::STRIP`] tall.
pub const ROW: f32 = 34.0;
pub const TOP: f32 = 6.0;
/// A session tab's least width, and the least its name gets before the tabs scroll or stack.
const TAB_MIN: f32 = 132.0;
const NAME_MIN: f32 = 64.0;
/// The most a name takes, so one long one can't crowd out the rest.
const MAX_NAME: f32 = 176.0;
/// What names keep in a row that scrolls however short they're cut.
const SCROLL_NAME: f32 = 120.0;
/// Between two tabs.
const GAP: f32 = 4.0;
/// The "+n" past the last stacked row.
const FOLD_W: f32 = 48.0;
/// A scroll arrow ("‹ 3" / "3 ›" and a mark when one of them wants you), and the room between it and the tabs.
const ARROW_W: f32 = 52.0;
const ARROW_GAP: f32 = 6.0;
/// The most rows stacked tabs take (fewer in a short window).
const MAX_ROWS: usize = 3;
/// The + after the tabs.
const PLUS_W: f32 = 36.0;
/// How far a wheel notch scrolls the tabs.
pub const NOTCH: f32 = 72.0;
/// What a session's tab leads with: its agent's mark…
const LEAD: f32 = 16.0;
/// …then, with every session or the orchestrators listed, its project's icon.
const PROJECT_ICON: f32 = 14.0;
/// The heading's icon (the project the tabs list), the most its name takes, and the room after it for its rule.
const HEAD_ICON: f32 = 20.0;
const HEAD_NAME: f32 = 200.0;
const HEAD_GAP: f32 = 12.0;
/// The room kept at a session tab's end for what it's doing, so tabs don't shift as agents start and stop.
const STATUS: f32 = 18.0;

/// One tab of the strip, as it reads.
struct StripEntry {
    tab: SessionTab,
    label: String,
    /// a session's pane
    pane: Option<PaneId>,
    /// the project whose icon it leads with (every session listed, or the orchestrators): key, folder, name
    project: Option<(String, std::path::PathBuf, String)>,
    /// All's: the sessions its grid shows
    count: Option<usize>,
    /// its agent (or its subagents, its background tasks) at work
    working: bool,
    /// it waits on you
    asks: bool,
    /// it finished while you weren't looking
    done: bool,
    /// kept out of the grid
    minimized: bool,
}

impl StripEntry {
    /// A session's tab: it has a × that closes the session.
    fn closable(&self) -> bool {
        matches!(self.tab, SessionTab::Session(_))
    }
    /// Room the tab takes besides its name: padding, what it leads with, All's count or a session's status and ×.
    fn chrome(&self) -> f32 {
        let project = if self.project.is_some() { PROJECT_ICON + 6.0 } else { 0.0 };
        if self.closable() { 12.0 + LEAD + 7.0 + project + 8.0 + STATUS + 4.0 + 26.0 + 4.0 } else { 12.0 + 12.0 + 16.0 + 8.0 + 22.0 }
    }
    fn wants_you(&self) -> bool {
        self.asks || self.done
    }
}

/// A session tab `name` wide (its name, measured) with `chrome` around it, its name cut to `cap`.
fn tab_w(name: f32, chrome: f32, cap: f32) -> f32 {
    (chrome + name.min(cap)).max(TAB_MIN)
}

/// Every session tab at name cap `cap`, with the gaps between: the room they take.
fn row_w(names: &[f32], chrome: &[f32], cap: f32) -> f32 {
    let tabs: f32 = names.iter().zip(chrome).map(|(n, c)| tab_w(*n, *c, cap)).sum();
    tabs + GAP * names.len().saturating_sub(1) as f32
}

/// The widest name cap (from the longest name, at most [`MAX_NAME`]) that lets every tab fit `room`, never below
/// [`NAME_MIN`]: names shorten before the tabs scroll.
fn fit_cap(names: &[f32], chrome: &[f32], room: f32) -> f32 {
    let mut cap = names.iter().copied().fold(0.0, f32::max).clamp(NAME_MIN, MAX_NAME);
    while cap > NAME_MIN && row_w(names, chrome, cap) > room {
        cap = (cap - 4.0).max(NAME_MIN);
    }
    cap
}

/// The name cap for one scrolling row: [`fit_cap`] while shortening names lets every tab fit; once they'll scroll
/// anyway, names keep [`SCROLL_NAME`] (cutting them shorter would only make you scroll past more of them).
fn scroll_cap(names: &[f32], chrome: &[f32], room: f32) -> f32 {
    let cap = fit_cap(names, chrome, room);
    if row_w(names, chrome, cap) <= room + 0.5 { cap } else { names.iter().copied().fold(0.0, f32::max).clamp(NAME_MIN, SCROLL_NAME) }
}

/// Rows the stacked tabs may take in a window `window_h` tall: up to [`MAX_ROWS`], the strip never more than a
/// third of the window.
fn rows_allowed(window_h: f32) -> usize {
    (((window_h / 3.0 - TOP) / ROW).floor() as usize).clamp(1, MAX_ROWS)
}

/// What each tab says, in the order of [`App::strip_items`]: All counts the sessions its grid shows (minimized ones
/// aside, as the terminal UI's).
fn entries(app: &App) -> Vec<StripEntry> {
    let tabs = app.session_tabs();
    let mixed = matches!(app.scope, StripTab::All | StripTab::Orchestrator);
    let mut v = Vec::with_capacity(tabs.len() + 1);
    if app.strip_items(&tabs).first() == Some(&SessionTab::Grid) {
        let count = app.tile_ids(app.scope.tile_key()).len();
        v.push(StripEntry { tab: SessionTab::Grid, label: "All".into(), pane: None, project: None, count: Some(count), working: false, asks: false, done: false, minimized: false });
    }
    v.extend(tabs.into_iter().map(|s| StripEntry {
        tab: SessionTab::Session(s.id),
        pane: Some(s.id),
        project: (mixed && !s.global).then(|| (s.project_key.clone(), s.project_root.clone(), s.project_name.clone())),
        label: s.label,
        count: None,
        working: s.working,
        asks: s.asks,
        done: s.done,
        minimized: s.minimized,
    }));
    v
}

/// Where everything on the strip goes this frame.
struct Plan {
    /// what the tabs list (before All): where, its name, its project's key and folder (none: the orchestrators)
    head: Option<(Rect, String, Option<(String, std::path::PathBuf)>)>,
    entries: Vec<StripEntry>,
    /// each tab on screen: its index in `entries`, where it is, and the most its name takes
    placed: Vec<(usize, Rect, f32)>,
    /// tabs before the sessions' (All: always on the first row, never scrolled)
    fixed: usize,
    /// scroll mode with more tabs than fit: the window the session tabs scroll in, and the arrows either side
    view: Option<(Rect, Rect, Rect)>,
    /// stack mode out of rows: the "+n" and the first tab folded into it
    fold: Option<(Rect, usize)>,
    /// the minimized sessions' switch
    minimized: Option<Rect>,
    plus: Rect,
    rows: usize,
}

/// Lay the strip out in `r` (its first row; stacked rows go below it), `max_rows` at most. In scroll mode this keeps
/// `app.strip_scroll` in step: how far it can go, and the tab showing (or the keys' pick) brought into view when that
/// changes.
fn plan(app: &mut App, ui: &mut Ui, r: Rect, max_rows: usize) -> Plan {
    let t = ui.t;
    let entries = entries(app);
    let stack = app.svc.settings().tab_overflow == TabOverflow::Stack;
    // tabs glide to their places (a resize, a mode change or another scope snaps them instead)
    let snap = flow_begin("strip", ui.now, key_of((r.right().round() as i32, stack, &app.scope, super::layout::resizing())));
    let tab_key = |tab: &SessionTab| key_of(format!("{tab:?}"));
    let top = r.y + TOP;
    let mid = top + ROW / 2.0;
    let mut limit = r.right() - 8.0;
    // far end of the first row: the minimized switch
    let mut minimized = None;
    if app.tile_all && !app.minimized.is_empty() {
        let st = Style::new(size::SMALL, weight::MEDIUM, t.muted);
        let w = ui.measure(&format!("{} minimized", app.minimized.len()), st) + 38.0;
        let mr = Rect::new(limit - w, mid - 14.0, w, 28.0);
        limit = mr.x - 8.0;
        minimized = Some(mr);
    }
    limit -= PLUS_W;
    let label_st = Style::new(size::SMALL, weight::SEMIBOLD, t.text);
    let names: Vec<f32> = entries.iter().map(|e| ui.measure(&e.label, label_st)).collect();
    let chrome: Vec<f32> = entries.iter().map(StripEntry::chrome).collect();
    let x0 = r.x + 8.0;
    let mut x = x0;
    // what the tabs list, first: the project's icon and name (the orchestrators' mark), then a rule
    let head = app.scope_heading().map(|(name, project)| {
        let w = 8.0 + HEAD_ICON + 8.0 + ui.measure(&name, Style::new(size::BODY, weight::SEMIBOLD, t.text)).min(HEAD_NAME) + 12.0;
        let hr = Rect::new(x, top, w, ROW);
        x += w + HEAD_GAP;
        (hr, name, project)
    });
    // then All, at its own width
    let fixed = entries.iter().take_while(|e| !e.closable()).count();
    let mut placed: Vec<(usize, Rect, f32)> = vec![];
    for i in 0..fixed {
        let w = chrome[i] + names[i].min(MAX_NAME);
        let at = Rect::new(x, top, w, ROW);
        let sprung = ui.flow_rect("strip", tab_key(&entries[i].tab), at, Rect { w: 0.0, ..at }, super::layout::FLOW, snap);
        placed.push((i, sprung, MAX_NAME));
        x += w + GAP;
    }
    // then the sessions
    let start = x + if fixed > 0 { 4.0 } else { 0.0 };
    let (pn, pc) = (&names[fixed..], &chrome[fixed..]);
    let n = pn.len();
    let mut view = None;
    let mut fold = None;
    let mut rows = 1;
    let mut end = x;
    let anchor = {
        let picked = app.strip_sel.and_then(|i| entries.get(i)).map(|e| e.tab);
        picked.or_else(|| app.showing_session()).filter(|tab| matches!(tab, SessionTab::Session(_)))
    };
    let at = |tab: &SessionTab| entries[fixed..].iter().position(|e| e.tab == *tab);
    if stack && n > 0 {
        // natural widths, wrapped; names shorten only once the rows run out
        let mut cap = pn.iter().copied().fold(0.0, f32::max).clamp(NAME_MIN, MAX_NAME);
        let laid = loop {
            let widths: Vec<f32> = (0..n).map(|j| tab_w(pn[j], pc[j], cap)).collect();
            let laid = stack_rows(&widths, (limit - start).max(0.0), (limit - x0).max(0.0), GAP, max_rows, FOLD_W);
            if laid.len() == n || cap <= NAME_MIN {
                break laid.into_iter().map(|(row, lx)| (row, lx, cap)).collect::<Vec<_>>();
            }
            cap = (cap - 4.0).max(NAME_MIN);
        };
        for (j, &(row, lx, cap)) in laid.iter().enumerate() {
            let rx = if row == 0 { start } else { x0 };
            let w = tab_w(pn[j], pc[j], cap);
            let at = Rect::new(rx + lx, top + row as f32 * ROW, w, ROW);
            let sprung = ui.flow_rect("strip", tab_key(&entries[fixed + j].tab), at, Rect { w: 0.0, ..at }, super::layout::FLOW, snap);
            placed.push((fixed + j, sprung, cap));
            rows = rows.max(row + 1);
        }
        let last = placed.last().map(|(_, tr, _)| *tr).unwrap_or(Rect::new(x, top, 0.0, ROW));
        end = last.right();
        if laid.len() < n {
            let fr = Rect::new(end + GAP, last.y + ROW / 2.0 - 14.0, FOLD_W - 8.0, 28.0);
            fold = Some((fr, fixed + laid.len()));
            end = fr.right();
        }
        app.strip_scroll.max = 0.0;
    } else if n > 0 {
        let room = (limit - start).max(0.0);
        let cap = scroll_cap(pn, pc, room);
        let widths: Vec<f32> = (0..n).map(|j| tab_w(pn[j], pc[j], cap)).collect();
        let content = widths.iter().sum::<f32>() + GAP * (n - 1) as f32;
        let mut offsets = Vec::with_capacity(n);
        let mut o = 0.0;
        for w in &widths {
            offsets.push(o);
            o += w + GAP;
        }
        let s = &mut app.strip_scroll;
        let scroll = if content <= room + 0.5 {
            s.max = 0.0;
            s.offset = 0.0;
            s.revealed = anchor;
            0.0
        } else {
            // the arrows take the window's ends; the tabs scroll between them
            let vr = Rect::new(start + ARROW_W + ARROW_GAP, top, (room - 2.0 * (ARROW_W + ARROW_GAP)).max(0.0), ROW);
            s.max = (content - vr.w).max(0.0);
            s.view = vr.w;
            s.spans = offsets.iter().zip(&widths).map(|(o, w)| (*o, o + w)).collect();
            if anchor != s.revealed {
                if let Some(j) = anchor.as_ref().and_then(at) {
                    s.reveal(offsets[j], offsets[j] + widths[j], vr.w, 0.0);
                }
                s.revealed = anchor;
            }
            s.offset = s.offset.clamp(0.0, s.max);
            let back = Rect::new(start, mid - 14.0, ARROW_W, 28.0);
            let on = Rect::new(vr.right() + ARROW_GAP, mid - 14.0, ARROW_W, 28.0);
            view = Some((vr, back, on));
            ui.ease("strip-scroll", s.offset, 180.0)
        };
        let x1 = view.map_or(start, |(vr, ..)| vr.x) - scroll;
        for j in 0..n {
            // the tabs' places in the row spring (a tab opening or closing, a name that grew); the scroll's own
            // ease carries them along on top of that
            let at = Rect::new(offsets[j], top, widths[j], ROW);
            let sprung = ui.flow_rect("strip", tab_key(&entries[fixed + j].tab), at, Rect { w: 0.0, ..at }, super::layout::FLOW, snap);
            placed.push((fixed + j, Rect { x: x1 + sprung.x, ..sprung }, cap));
        }
        end = match view {
            Some((_, _, on)) => on.right() + 2.0,
            None => x1 + content,
        };
    }
    let plus = Rect::new(end.min(limit) + 6.0, placed.last().map_or(top, |(_, tr, _)| tr.y) + ROW / 2.0 - 14.0, 28.0, 28.0);
    // the + follows the last tab
    let plus = ui.flow_rect("strip", key_of("plus"), plus, plus, super::layout::FLOW, snap);
    Plan { head, entries, placed, fixed, view, fold, minimized, plus, rows }
}

/// How many rows the tabs take in a strip `r` (its first row) of a window `window_h` tall: more than one only when
/// they stack.
pub fn rows(app: &mut App, ui: &mut Ui, r: Rect, window_h: f32) -> usize {
    if app.svc.settings().tab_overflow != TabOverflow::Stack {
        return 1;
    }
    plan(app, ui, r, rows_allowed(window_h)).rows
}

/// What a sidebar row or a tab says about its sessions: how many run (that place spins while one works), then how
/// many wait on you (●) and finished unseen (✓), each its own shape.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Tally {
    pub count: usize,
    pub working: bool,
    pub asks: usize,
    pub done: usize,
}

impl Tally {
    /// The room it takes.
    pub fn width(&self) -> f32 {
        18.0 + if self.asks > 0 { 26.0 } else { 0.0 } + if self.done > 0 { 26.0 } else { 0.0 }
    }
}

/// Draw `tally` from `x` along the line at `mid`: its numbers roll as they change and a new ask pops in. `id` keys
/// its motion.
pub(super) fn draw_tally(ui: &mut Ui, x: f32, mid: f32, id: &str, tally: Tally) {
    let t = ui.t;
    let cs = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
    let line_h = ui.line("0", cs, None).h;
    let roll = |ui: &mut Ui, what: &str, x: f32, n: usize, st: Style| {
        let w = ui.measure(&n.to_string(), st);
        ui.rolling(("tally-n", id, what), x + w, mid - line_h / 2.0, &n.to_string(), st, Some(n as f64));
    };
    let mut mx = x;
    // while one of its agents works, the count's place spins; counts line up at their right
    if tally.working {
        ui.spinner(Point::new(mx + 8.0, mid), 4.0, 1.4, t.accent);
    } else {
        let w = ui.measure(&tally.count.to_string(), cs);
        roll(ui, "count", mx + 13.0 - w, tally.count, cs);
    }
    mx += 18.0;
    let alerted = ui.since_on(("tally-alert", id), tally.asks > 0);
    if tally.asks > 0 {
        let pop = alerted.map_or(1.0, |s| if s > 1.0 { 1.0 } else { Spring::POP.step(s) });
        if pop != 1.0 {
            ui.anim.moving = true;
        }
        ui.icon(Rect::new(mx, mid - 7.0, 14.0, 14.0), "\u{F0028}", size::SMALL * pop.max(0.01), t.danger);
        roll(ui, "asks", mx + 15.0, tally.asks, cs.color(t.danger));
        mx += 26.0;
    }
    if tally.done > 0 {
        ui.icon(Rect::new(mx, mid - 7.0, 14.0, 14.0), "\u{F012C}", size::SMALL, t.good);
        roll(ui, "done", mx + 15.0, tally.done, cs.color(t.good));
    }
}

/// Draw the strip in `r`, its rows stacked from the top. Draws the line under the tabs too (it opens under the tab
/// showing, when that's on the bottom row and in view).
pub fn draw(app: &mut App, ui: &mut Ui, r: Rect, window_h: f32, icons: &mut super::icons::Icons) {
    let t = ui.t;
    let p = plan(app, ui, Rect { h: TOP + ROW, ..r }, rows_allowed(window_h));
    if let Some(sel) = app.strip_sel {
        app.strip_sel = Some(sel.min(p.entries.len().saturating_sub(1)));
    }
    let picked = app.strip_sel.and_then(|i| p.entries.get(i)).map(|e| e.tab);
    let showing = app.showing_session();
    // the focused session reads bold (in a grid too, where All is the tab showing), and every one on screen brighter
    let focused = app.focused();
    let on_screen = app.visible();
    let bottom_row = r.y + TOP + (p.rows - 1) as f32 * ROW;
    let selected = p.placed.iter().position(|(i, ..)| showing == Some(p.entries[*i].tab));
    // where each tab can be seen: the strip, or the scrolling window for a session's
    let clip_of = |k: usize| match p.view {
        Some((vr, ..)) if p.placed[k].0 >= p.fixed => vr,
        _ => r,
    };
    let hot: Vec<f32> = p.placed.iter().enumerate().map(|(k, (i, tr, _))| {
        let seen = tr.intersect(&clip_of(k));
        ui.hover_amount(("strip", format!("{:?}", p.entries[*i].tab)), seen)
    }).collect();

    // the line under the tabs, open under the one showing where it meets the sessions
    let line_y = r.bottom() - ui.hair();
    // the showing tab's fill is one shape that glides from tab to tab (Rune's matched element), over the chips and
    // under the tabs' contents; while it moves it may cross the arrows' edge, so it's cut by the whole strip
    let sel_open = selected.map(|k| p.placed[k].1).filter(|tr| (tr.y - bottom_row).abs() < 0.5).map(|tr| Rect { y: tr.y + 4.0, h: r.bottom() - (tr.y + 4.0) + 1.0, ..tr });
    let sel_glide = selected.zip(sel_open).map(|(k, open)| {
        let g = ui.glide("strip-sel", format!("{:?}", p.entries[p.placed[k].0].tab), open, super::layout::FLOW);
        let travelling = (g.x - open.x).abs() > 0.5 || (g.w - open.w).abs() > 0.5;
        (g, if travelling { r } else { clip_of(k) })
    });
    let gap = sel_glide.map(|(g, c)| g.intersect(&c)).filter(|g| g.w > 0.0);
    match gap {
        Some(g) => {
            ui.hline(r.x + radius::OVERLAY, line_y, (g.x - r.x - radius::OVERLAY).max(0.0), t.stroke);
            ui.hline(g.right(), line_y, (r.right() - g.right()).max(0.0), t.stroke);
        }
        None => ui.hline(r.x + radius::OVERLAY, line_y, (r.w - radius::OVERLAY).max(0.0), t.stroke),
    }

    // the heading: the project the tabs list, its icon and name (the orchestrators' mark on a gold tile), and a rule
    // before All; a click goes to its sessions as its sidebar row does, a right-click opens its menu
    if let Some((hr, name, project)) = &p.head {
        let mid = hr.y + hr.h / 2.0;
        let hot = ui.hover_amount("strip-head", *hr);
        ui.round(Rect::new(hr.x, hr.y + 4.0, hr.w, hr.h - 8.0), radius::OVERLAY, t.text.with_alpha(0.045 * hot));
        let ir = Rect::new(hr.x + 8.0, mid - HEAD_ICON / 2.0, HEAD_ICON, HEAD_ICON);
        match project {
            Some((key, root)) => {
                let look = icons.look(key, root);
                super::sidebar::project_icon(ui, ir, name, t.project(key, &app.theme), look);
            }
            None => {
                ui.round(ir, (HEAD_ICON * 0.27).round(), t.gold.with_alpha(if t.light { 0.12 } else { 0.14 }));
                ui.icon(ir, "\u{F04CE}", 13.0, t.gold);
            }
        }
        let lb = ui.line(name, Style::new(size::BODY, weight::SEMIBOLD, t.text), Some(HEAD_NAME));
        ui.draw(&lb, Point::new(ir.right() + 8.0, mid - lb.h / 2.0));
        ui.vline(hr.right() + HEAD_GAP / 2.0, hr.y + 11.0, hr.h - 22.0, t.border);
        ui.hit(*hr, Action::Side(SideHit::Scope(app.scope.clone())));
    }

    // backgrounds first: a chip for every tab but the showing one, which the gliding fill covers
    for (k, &(_, tr, _)) in p.placed.iter().enumerate() {
        let clip = clip_of(k);
        if tr.intersect(&clip).w <= 0.0 || selected == Some(k) && sel_glide.is_some() {
            continue;
        }
        let chip = Rect::new(tr.x, tr.y + 4.0, tr.w, tr.h - 8.0);
        ui.clip(clip, 0.0);
        if selected == Some(k) {
            // showing, on a row above the last: the same fill and outline, closed all round
            ui.glass(chip, Corners::all(radius::OVERLAY), t.layer);
            ui.outline(chip, radius::OVERLAY, 1.0, t.border);
        } else {
            // every other tab a chip of its own, brighter under the pointer
            let rest = if t.light { 0.045 } else { 0.04 };
            ui.round(chip, radius::OVERLAY, t.text.with_alpha(rest + 0.045 * hot[k]));
        }
        ui.unclip();
    }
    // the showing tab's fill, filled like the content below, outlined on three sides, open at the bottom
    if let Some((g, cut)) = sel_glide {
        ui.clip(cut, 0.0);
        ui.glass(g, Corners::top(radius::OVERLAY), t.layer);
        ui.clip(Rect { h: g.h - 1.0, ..g }.intersect(&cut), 0.0);
        ui.outline(Rect { h: g.h + radius::OVERLAY, ..g }, radius::OVERLAY, 1.0, t.stroke);
        ui.unclip();
        ui.unclip();
    }

    for (k, &(i, tr, cap)) in p.placed.iter().enumerate() {
        let e = &p.entries[i];
        let clip = clip_of(k);
        if tr.intersect(&clip).w <= 0.0 {
            continue;
        }
        // a tab still opening keeps what it holds inside it
        ui.clip(clip.intersect(&tr), 0.0);
        let on = selected == Some(k);
        let kb = picked == Some(e.tab);
        let chip = Rect::new(tr.x, tr.y + 4.0, tr.w, tr.h - 8.0);
        let tab_id = format!("{:?}", e.tab);
        // the keys' pick: keyboard focus, a full outline
        if kb {
            ui.outline(Rect::new(chip.x + 1.0, chip.y + 1.0, chip.w - 2.0, chip.h - 2.0), radius::CONTROL + 2.0, 1.0, t.accent);
        }
        let hit = match e.tab {
            SessionTab::Grid => StripHit::Grid,
            SessionTab::Session(id) => StripHit::Session(id),
        };
        ui.hit(tr, Action::Side(SideHit::Strip(hit)));
        let mid = tr.y + tr.h / 2.0;
        let mut cx = tr.x + 12.0;
        // a tab's ink warms up as it becomes the one showing, and cools as it stops; a session on screen (a tile in
        // the grid) reads brighter than one that isn't, a minimized one fainter
        let want = if on || kb { 1.0 } else { 0.0 };
        let lit = ui.spring(("strip-lit", &tab_id), want, Spring::GLIDE, want).clamp(0.0, 1.0);
        let seen = e.pane.is_some_and(|id| on_screen.contains(&id));
        let quiet = t.text.mix(t.muted, if seen { 0.12 } else { 0.32 }).mix(t.text, lit);
        let fade = if e.minimized && !on { 0.55 } else { 1.0 };
        // All leads with its glyph; a session with its agent's mark (the logo its pane's header wears), then, with
        // every session or the orchestrators listed, its project's icon
        match e.pane {
            None => {
                ui.icon(Rect::new(cx, mid - 8.0, 16.0, 16.0), "\u{F05AF}", size::BODY, quiet);
                cx += 16.0 + 8.0;
            }
            Some(id) => {
                ui.mark_badged(session_mark(app, id, t), session_badge(app, id), Rect::new(cx - 2.0, mid - 9.0, 18.0, 18.0), 15.0, None);
                cx += LEAD + 7.0;
                if let Some((key, root, name)) = &e.project {
                    let color = t.project(key, &app.theme);
                    let look = icons.look(key, root);
                    super::sidebar::project_icon(ui, Rect::new(cx - 1.0, mid - PROJECT_ICON / 2.0, PROJECT_ICON, PROJECT_ICON), name, color, look);
                    cx += PROJECT_ICON + 6.0;
                }
            }
        }
        let bold = on || e.pane.is_some_and(|id| focused == Some(id));
        let st = Style::new(size::SMALL, if bold { weight::SEMIBOLD } else { weight::MEDIUM }, quiet.fade(fade));
        // a session's status sits at its right, before the ×, and its name has what's left; All's count follows it
        let status_x = tr.right() - 34.0 - STATUS;
        let room = if e.closable() { (status_x - 6.0 - cx).max(0.0) } else { cap };
        let lb = ui.line(&e.label, st, Some(room));
        ui.draw(&lb, Point::new(cx, mid - lb.h / 2.0));
        if let Some(n) = e.count {
            draw_tally(ui, cx + lb.w + 8.0, mid, &tab_id, Tally { count: n, ..Default::default() });
        }
        if e.closable() {
            let sc = Point::new(status_x + STATUS / 2.0, mid);
            // an ask arriving pops in
            let alerted = ui.since_on(("strip-alert", &tab_id), e.asks);
            if e.asks {
                let pop = alerted.map_or(1.0, |s| if s > 1.0 { 1.0 } else { Spring::POP.step(s) });
                if pop != 1.0 {
                    ui.anim.moving = true;
                }
                ui.icon(Rect::new(sc.x - 8.0, mid - 8.0, 16.0, 16.0), "\u{F0028}", 13.0 * pop.max(0.01), t.danger);
            } else if e.working {
                ui.spinner(sc, 4.2, 1.5, t.accent);
            } else if e.done {
                ui.icon(Rect::new(sc.x - 8.0, mid - 8.0, 16.0, 16.0), "\u{F012C}", 13.0, t.good);
            }
            // the whole name, when it was cut
            if hot[k] > 0.95 && ui.measure(&e.label, st) > room + 0.5 {
                widgets::tooltip(ui, chip, &e.label);
            }
        }
        // the × shows under the pointer, on the tab showing and on the keys' pick (delete closes it); its room is
        // always kept, so nothing shifts as you move along the strip
        if let Some(id) = e.pane
            && (hot[k] > 0.3 || on || kb)
        {
            let xr = Rect::new(tr.right() - 30.0, mid - 10.0, 20.0, 20.0);
            widgets::icon_button(ui, xr, "\u{F0156}", t.muted, Action::Side(SideHit::Strip(StripHit::Close(id))));
        }
        ui.unclip();
    }

    // scroll mode, more tabs than fit: an arrow at each end of the window, with how many tabs are past it and a mark
    // when one of those wants you; an arrow with nothing past it steps back
    if let Some((vr, back, on)) = p.view {
        let session = |k: &&(usize, Rect, f32)| k.0 >= p.fixed;
        let before: Vec<usize> = p.placed.iter().filter(session).filter(|(_, tr, _)| tr.x < vr.x - 0.5).map(|(i, ..)| *i).collect();
        let after: Vec<usize> = p.placed.iter().filter(session).filter(|(_, tr, _)| tr.right() > vr.right() + 0.5).map(|(i, ..)| *i).collect();
        for (fr, forward, past) in [(back, false, before), (on, true, after)] {
            let wants = past.iter().any(|i| p.entries[*i].wants_you());
            // the edge they're past: a hairline, so a cut tab reads as going on rather than broken
            if !past.is_empty() {
                let ex = if forward { vr.right() } else { vr.x - ui.hair() };
                ui.vline(ex, vr.y + 8.0, vr.h - 16.0, t.border);
            }
            arrow(ui, fr, forward, past.len(), wants, (!past.is_empty()).then_some(StripHit::Scroll(forward)));
        }
    }
    // stack mode out of rows: "+n", the rest in the palette
    if let Some((fr, from)) = p.fold {
        let wants = p.entries[from..].iter().any(StripEntry::wants_you);
        arrow(ui, fr, true, p.entries.len() - from, wants, Some(StripHit::More));
    }
    // then +, a new session
    if widgets::icon_button(ui, p.plus, "\u{F0415}", t.muted, Action::Side(SideHit::Strip(StripHit::New))) {
        widgets::tooltip(ui, p.plus, "New session");
    }
    // far end: the minimized sessions' switch, "▸ 2 minimized" / "▾ 2 minimized"
    if let Some(mr) = p.minimized {
        let shown = app.show_minimized;
        let st = Style::new(size::SMALL, weight::MEDIUM, if shown { t.accent } else { t.muted });
        let hot = ui.hover_amount("strip-min", mr);
        ui.round(mr, radius::CONTROL, t.text.with_alpha(0.06 * hot));
        ui.icon(Rect::new(mr.x + 8.0, mr.y, 16.0, mr.h), if shown { "\u{F0140}" } else { "\u{F0142}" }, size::SMALL, st.color);
        ui.label_in(Rect::new(mr.x + 28.0, mr.y, mr.w - 32.0, mr.h), &format!("{} minimized", app.minimized.len()), st, Align::Left);
        ui.hit(mr, Action::Side(SideHit::Strip(StripHit::Minimized)));
    }
}

/// An arrow at an end of the strip, or the stacked rows' "+n": "‹ 3" / "3 ›", a mark when one of the `n` tabs past
/// it wants you. With nothing to do (`hit` None) it draws faint and takes no clicks.
fn arrow(ui: &mut Ui, fr: Rect, forward: bool, n: usize, wants: bool, hit: Option<StripHit>) {
    let t = ui.t;
    let live = hit.is_some();
    let mh = if live { ui.hover_amount(("strip-more", forward, fr.y as i32), fr) } else { 0.0 };
    ui.round(fr, radius::CONTROL, t.text.with_alpha(0.06 * mh));
    let ink = if live { t.muted.mix(t.text, 0.5 * mh) } else { t.faint.with_alpha(0.5) };
    let st = Style::new(size::SMALL, weight::SEMIBOLD, ink);
    let label = if n > 0 { n.to_string() } else { String::new() };
    let nw = if n > 0 { ui.measure(&label, st) + 2.0 } else { 0.0 };
    let inner_w = 14.0 + nw + if wants { 14.0 } else { 0.0 };
    let mut fx = fr.x + (fr.w - inner_w) / 2.0;
    let chevron = if forward { "\u{F0142}" } else { "\u{F0141}" };
    if !forward {
        ui.icon(Rect::new(fx, fr.y, 14.0, fr.h), chevron, size::SMALL, ink);
        fx += 14.0;
    }
    if n > 0 {
        ui.label_in(Rect::new(fx + if forward { 0.0 } else { 2.0 }, fr.y, nw, fr.h), &label, st, Align::Left);
        fx += nw;
    }
    if forward {
        ui.icon(Rect::new(fx, fr.y, 14.0, fr.h), chevron, size::SMALL, ink);
        fx += 14.0;
    }
    if wants {
        ui.icon(Rect::new(fx, fr.y, 14.0, fr.h), "\u{F0028}", size::CAPTION, t.danger);
    }
    if let Some(h) = hit {
        ui.hit(fr, Action::Side(SideHit::Strip(h)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_shorten_before_tabs_scroll_and_tabs_keep_their_least_width() {
        // three 140px names with 72px of chrome: 3 × 212 + 2 gaps = 644 fits whole
        let names = [140.0, 140.0, 140.0];
        let chrome = [72.0, 72.0, 72.0];
        assert_eq!(fit_cap(&names, &chrome, 644.0), 140.0);
        // at 524 they shorten to 100 (3 × 172 + 8)
        assert_eq!(fit_cap(&names, &chrome, 524.0), 100.0);
        // never below the name's minimum, however tight: then they scroll
        assert_eq!(fit_cap(&names, &chrome, 10.0), NAME_MIN);
        // no wider than the longest name, nor the maximum
        assert_eq!(fit_cap(&[300.0], &[72.0], 1000.0), MAX_NAME);
        // a short name still makes a tab you can hit
        assert_eq!(tab_w(20.0, 72.0, MAX_NAME), TAB_MIN);
        assert_eq!(tab_w(150.0, 72.0, 100.0), 172.0);
        // a row that has to scroll keeps names readable rather than cutting them all to the least
        assert_eq!(scroll_cap(&names, &chrome, 524.0), 100.0);
        assert_eq!(scroll_cap(&names, &chrome, 10.0), SCROLL_NAME);
    }

    #[test]
    fn a_short_window_stacks_fewer_rows() {
        assert_eq!(rows_allowed(320.0), 2);
        assert_eq!(rows_allowed(900.0), MAX_ROWS);
        assert_eq!(rows_allowed(60.0), 1);
    }

    #[test]
    fn a_tally_keeps_room_for_its_count_and_each_mark() {
        assert_eq!(Tally { count: 3, ..Default::default() }.width(), 18.0);
        assert_eq!(Tally { count: 3, working: true, asks: 1, done: 2 }.width(), 70.0);
    }
}
