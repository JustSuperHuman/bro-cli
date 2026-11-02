//! The project tabs in the window (a setting; the terminal UI draws its own in `app/project_tabs.rs`): All, the
//! orchestrators' ★, one per project with running sessions, then + and, at the far end, the minimized switch.
//!
//! Every tab is a shape of its own, a soft chip with a gap either side, so a crowded strip still reads tab by tab.
//! The one showing takes the content layer's colour and opens into the sessions below (WinUI's TabView). A
//! project's tab is never narrower than [`TAB_MIN`] nor its name cut below [`NAME_MIN`]: names shorten first, then
//! what still doesn't fit either scrolls along the one row (the default), All and ★ staying put while arrows at
//! either end say how many tabs are past them and flag one that wants you, or (the stack setting) wraps onto more
//! rows, at most [`MAX_ROWS`]. Past that, the rest fold into "+n", which lists every tab.

use super::actions::Action;
use super::kit::{Style, Ui, radius, size, weight, widgets};
use crate::app::App;
use crate::app::SideHit;
use crate::app::project_tabs::{StripHit, StripTab, stack_rows};
use crate::pane::Activity;
use bro_core::config::TabOverflow;
use bro_gfx::text::Align;
use bro_gfx::{Corners, Point, Rect};

/// A row of tabs, and the room above the first (logical px): one row is [`super::layout::STRIP`] tall.
pub const ROW: f32 = 34.0;
pub const TOP: f32 = 6.0;
/// A project tab's least width, and the least its name gets before the tabs scroll or stack.
const TAB_MIN: f32 = 132.0;
const NAME_MIN: f32 = 64.0;
/// The most a name takes, so one long one can't crowd out the rest.
const MAX_NAME: f32 = 176.0;
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
    /// The count (or spinner) and badges, laid right to left before the ×.
    fn meta_w(&self) -> f32 {
        let mut w = 0.0;
        if self.count.is_some() || self.spins_after() {
            w += 18.0;
        }
        if self.blocked > 0 {
            w += 26.0;
        }
        if self.done > 0 {
            w += 26.0;
        }
        w
    }
    fn wants_you(&self) -> bool {
        self.blocked + self.done > 0
    }
}

/// A project tab `name` wide (its name, measured) with `chrome` around it, its name cut to `cap`.
fn tab_w(name: f32, chrome: f32, cap: f32) -> f32 {
    (chrome + name.min(cap)).max(TAB_MIN)
}

/// Every project tab at name cap `cap`, with the gaps between: the room they take.
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

/// Rows the stacked tabs may take in a window `window_h` tall: up to [`MAX_ROWS`], the strip never more than a
/// third of the window.
fn rows_allowed(window_h: f32) -> usize {
    (((window_h / 3.0 - TOP) / ROW).floor() as usize).clamp(1, MAX_ROWS)
}

/// What each tab says: All counts the sessions its grid shows (minimized ones aside, as the terminal UI's).
fn entries(app: &App) -> Vec<StripEntry> {
    let tabs = app.project_tabs();
    app.strip_items(&tabs)
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
        .collect()
}

/// Where everything on the strip goes this frame.
struct Plan {
    entries: Vec<StripEntry>,
    /// each tab on screen: its index in `entries`, where it is, and the most its name takes
    placed: Vec<(usize, Rect, f32)>,
    /// tabs All and ★ (always on the first row, never scrolled)
    fixed: usize,
    /// scroll mode with more tabs than fit: the window the project tabs scroll in, and the arrows either side
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
    // All and ★ first, at their own width
    let fixed = entries.iter().take_while(|e| !e.closable()).count();
    let x0 = r.x + 8.0;
    let mut x = x0;
    let mut placed: Vec<(usize, Rect, f32)> = vec![];
    for i in 0..fixed {
        let w = chrome[i] + names[i].min(MAX_NAME);
        placed.push((i, Rect::new(x, top, w, ROW), MAX_NAME));
        x += w + GAP;
    }
    // then the projects
    let start = x + 4.0;
    let (pn, pc) = (&names[fixed..], &chrome[fixed..]);
    let n = pn.len();
    let mut view = None;
    let mut fold = None;
    let mut rows = 1;
    let mut end = x;
    let anchor = {
        let picked = app.strip_sel.and_then(|i| app.strip_items(&app.project_tabs()).get(i).cloned());
        picked.or_else(|| app.drawn_tab.clone()).filter(|tab| matches!(tab, StripTab::Project(_)))
    };
    let at = |tab: &StripTab| entries[fixed..].iter().position(|e| e.tab == *tab);
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
            placed.push((fixed + j, Rect::new(rx + lx, top + row as f32 * ROW, w, ROW), cap));
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
        let cap = fit_cap(pn, pc, room);
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
            s.revealed = anchor.clone();
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
                s.revealed = anchor.clone();
            }
            s.offset = s.offset.clamp(0.0, s.max);
            let back = Rect::new(start, mid - 14.0, ARROW_W, 28.0);
            let on = Rect::new(vr.right() + ARROW_GAP, mid - 14.0, ARROW_W, 28.0);
            view = Some((vr, back, on));
            ui.ease("strip-scroll", s.offset, 180.0)
        };
        let x1 = view.map_or(start, |(vr, ..)| vr.x) - scroll;
        for j in 0..n {
            placed.push((fixed + j, Rect::new(x1 + offsets[j], top, widths[j], ROW), cap));
        }
        end = match view {
            Some((_, _, on)) => on.right() + 2.0,
            None => x1 + content,
        };
    }
    let plus = Rect::new(end.min(limit) + 6.0, placed.last().map_or(top, |(_, tr, _)| tr.y) + ROW / 2.0 - 14.0, 28.0, 28.0);
    Plan { entries, placed, fixed, view, fold, minimized, plus, rows }
}

/// How many rows the tabs take in a strip `r` (its first row) of a window `window_h` tall: more than one only when
/// they stack.
pub fn rows(app: &mut App, ui: &mut Ui, r: Rect, window_h: f32) -> usize {
    if app.svc.settings().tab_overflow != TabOverflow::Stack {
        return 1;
    }
    plan(app, ui, r, rows_allowed(window_h)).rows
}

/// Draw the strip in `r`, its rows stacked from the top. Draws the line under the tabs too (it opens under the tab
/// showing, when that's on the bottom row and in view).
pub fn draw(app: &mut App, ui: &mut Ui, r: Rect, window_h: f32, stage: &mut super::orbit_stage::Drawn) {
    let t = ui.t;
    let items = app.strip_items(&app.project_tabs());
    if let Some(sel) = app.strip_sel {
        app.strip_sel = Some(sel.min(items.len().saturating_sub(1)));
    }
    let picked = app.strip_sel.and_then(|i| items.get(i)).cloned();
    let showing = app.drawn_tab.clone();
    let p = plan(app, ui, Rect { h: TOP + ROW, ..r }, rows_allowed(window_h));
    // the focused session's project reads bold
    let focus_key = app.focused().and_then(|id| app.panes.get(&id)).and_then(|p| p.as_term_ref()).map(|term| term.meta.project.key.clone());
    let bottom_row = r.y + TOP + (p.rows - 1) as f32 * ROW;
    let selected = p.placed.iter().position(|(i, ..)| showing.as_ref() == Some(&p.entries[*i].tab));
    // where each tab can be seen: the strip, or the scrolling window for a project's
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
    let gap = selected.map(|k| (p.placed[k].1, clip_of(k))).filter(|(tr, _)| (tr.y - bottom_row).abs() < 0.5).map(|(tr, c)| tr.intersect(&c)).filter(|g| g.w > 0.0);
    match gap {
        Some(g) => {
            ui.hline(r.x + radius::OVERLAY, line_y, (g.x - r.x - radius::OVERLAY).max(0.0), t.stroke);
            ui.hline(g.right(), line_y, (r.right() - g.right()).max(0.0), t.stroke);
        }
        None => ui.hline(r.x + radius::OVERLAY, line_y, (r.w - radius::OVERLAY).max(0.0), t.stroke),
    }

    for (k, &(i, tr, cap)) in p.placed.iter().enumerate() {
        let e = &p.entries[i];
        let clip = clip_of(k);
        if tr.intersect(&clip).w <= 0.0 {
            continue;
        }
        ui.clip(clip, 0.0);
        let stage_key = match &e.tab {
            StripTab::All => "all",
            StripTab::Orchestrator => "orchestrators",
            StripTab::Project(key) => key.as_str(),
        };
        stage.tab(stage_key, tr, clip);
        let on = selected == Some(k);
        let kb = picked.as_ref() == Some(&e.tab);
        let chip = Rect::new(tr.x, tr.y + 4.0, tr.w, tr.h - 8.0);
        if on && (tr.y - bottom_row).abs() < 0.5 {
            // filled like the content below, outlined on three sides, open at the bottom
            let open = Rect { y: chip.y, h: r.bottom() - chip.y + 1.0, ..tr };
            ui.glass(open, Corners::top(radius::OVERLAY), t.layer);
            ui.clip(Rect { h: open.h - 1.0, ..open }.intersect(&clip), 0.0);
            ui.outline(Rect { h: open.h + radius::OVERLAY, ..open }, radius::OVERLAY, 1.0, t.stroke);
            ui.unclip();
        } else if on {
            // showing, on a row above the last: the same fill and outline, closed all round
            ui.glass(chip, Corners::all(radius::OVERLAY), t.layer);
            ui.outline(chip, radius::OVERLAY, 1.0, t.border);
        } else {
            // every other tab a chip of its own, brighter under the pointer
            let rest = if t.light { 0.045 } else { 0.04 };
            ui.round(chip, radius::OVERLAY, t.text.with_alpha(rest + 0.045 * hot[k]));
        }
        // the keys' pick: keyboard focus, a full outline
        if kb {
            ui.outline(Rect::new(chip.x + 1.0, chip.y + 1.0, chip.w - 2.0, chip.h - 2.0), radius::CONTROL + 2.0, 1.0, t.accent);
        }
        ui.hit(tr, Action::Side(SideHit::Strip(StripHit::Tab(e.tab.clone()))));
        let mid = tr.y + tr.h / 2.0;
        let mut cx = tr.x + 12.0;
        let quiet = if on || kb { t.text } else { t.text.mix(t.muted, 0.3) };
        // All and the orchestrator lead with an icon; a project's name is in its colour, as in the sidebar
        let ink = match &e.tab {
            StripTab::All => {
                ui.icon(Rect::new(cx, mid - 8.0, 16.0, 16.0), "\u{F05AF}", size::BODY, quiet);
                cx += 16.0 + 8.0;
                quiet
            }
            StripTab::Orchestrator => {
                ui.icon(Rect::new(cx, mid - 8.0, 16.0, 16.0), "\u{F04CE}", size::BODY, t.gold);
                cx += 16.0 + 8.0;
                quiet
            }
            StripTab::Project(key) => t.project(key, &app.theme),
        };
        // a project's count and badges sit at its right, before the ×, and its name has what's left; All's and ★'s
        // follow their names
        let bold = on || matches!(&e.tab, StripTab::Project(key) if focus_key.as_deref() == Some(key.as_str()));
        let st = Style::new(size::SMALL, if bold { weight::SEMIBOLD } else { weight::MEDIUM }, ink);
        let meta_x = tr.right() - 34.0 - e.meta_w();
        let room = if e.closable() { (meta_x - 8.0 - cx).max(0.0) } else { cap };
        let lb = ui.line(&e.label, st, Some(room));
        ui.draw(&lb, Point::new(cx, mid - lb.h / 2.0));
        let mut mx = if e.closable() { meta_x } else { cx + lb.w + 8.0 };
        let cs = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
        if let Some(n) = e.count {
            // while one of its agents works, the count's place spins
            if e.working && (e.closable() || e.spins_after()) {
                ui.spinner(Point::new(mx + 5.0, mid), 4.0, 1.4, if e.spins_after() { t.gold } else { t.accent });
            } else {
                ui.label_in(Rect::new(mx, tr.y, 14.0, tr.h), &n.to_string(), cs, Align::Left);
            }
            mx += 18.0;
        } else if e.spins_after() {
            ui.spinner(Point::new(mx + 6.0, mid), 4.0, 1.4, t.gold);
            mx += 18.0;
        }
        if e.blocked > 0 {
            ui.icon(Rect::new(mx, mid - 7.0, 14.0, 14.0), "\u{F0028}", size::SMALL, t.danger);
            ui.label_in(Rect::new(mx + 15.0, tr.y, 12.0, tr.h), &e.blocked.to_string(), cs.color(t.danger), Align::Left);
            mx += 26.0;
        }
        if e.done > 0 {
            ui.icon(Rect::new(mx, mid - 7.0, 14.0, 14.0), "\u{F012C}", size::SMALL, t.good);
            ui.label_in(Rect::new(mx + 15.0, tr.y, 12.0, tr.h), &e.done.to_string(), cs.color(t.good), Align::Left);
        }
        // the × shows under the pointer, on the tab showing and on the keys' pick (delete closes it); its room is
        // always kept, so nothing shifts as you move along the strip
        if let StripTab::Project(key) = &e.tab
            && (hot[k] > 0.3 || on || kb)
        {
            let xr = Rect::new(tr.right() - 30.0, mid - 10.0, 20.0, 20.0);
            widgets::icon_button(ui, xr, "\u{F0156}", t.muted, Action::Side(SideHit::Strip(StripHit::Close(key.clone()))));
        }
        ui.unclip();
    }

    // scroll mode, more tabs than fit: an arrow at each end of the window, with how many tabs are past it and a mark
    // when one of those wants you; an arrow with nothing past it steps back
    if let Some((vr, back, on)) = p.view {
        let project = |k: &&(usize, Rect, f32)| k.0 >= p.fixed;
        let before: Vec<usize> = p.placed.iter().filter(project).filter(|(_, tr, _)| tr.x < vr.x - 0.5).map(|(i, ..)| *i).collect();
        let after: Vec<usize> = p.placed.iter().filter(project).filter(|(_, tr, _)| tr.right() > vr.right() + 0.5).map(|(i, ..)| *i).collect();
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
    }

    #[test]
    fn a_short_window_stacks_fewer_rows() {
        assert_eq!(rows_allowed(320.0), 2);
        assert_eq!(rows_allowed(900.0), MAX_ROWS);
        assert_eq!(rows_allowed(60.0), 1);
    }
}
