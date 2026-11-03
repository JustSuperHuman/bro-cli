//! The project tabs (a setting): a strip across the top of the sessions, beside the sidebar rather than in it.
//! All tiles every session the way the tile-all shortcut does; the orchestrator, when one runs, has its own ★ tab;
//! each project with running sessions has a tab in its own color that tiles just that project's sessions.
//!
//! A tab's first glyph is its color and what its agents are doing (◐ working, ○ idle); after the name come its
//! session count and what wants you, each its own shape (● waiting on you, ✓ finished unseen). A tab is never
//! narrower than [`TAB_MIN`] nor its name cut below [`MIN_NAME`]: names shorten first, then the tabs that still
//! don't fit scroll (arrows at either end page through them, the wheel too) or, with the stack setting, wrap onto
//! more rows. The strip's right end holds the minimized sessions' switch and +, a new session. A tab under the
//! mouse shows a × that closes its project (after a confirm).

use super::{App, SideHit};
use crate::alerts::Kind;
use crate::pane::{Activity, PaneId, Place};
use crate::sidebar::ProjectTab;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::config::TabOverflow;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
};
use std::time::{Duration, Instant};

/// A project's name never shrinks below this many columns to make room for the others…
const MIN_NAME: usize = 10;
/// …nor takes more than this many, so one long name can't push the others (or the showing tab) off.
const MAX_NAME: usize = 24;
/// A project tab's least width, so a short name still makes a tab you can hit.
const TAB_MIN: u16 = 16;
/// Room an arrow ("◂2", "3▸") or a "+N" fold takes.
const MORE_W: u16 = 6;
/// A trackpad's burst of wheel events switches one tab.
const WHEEL_GAP: Duration = Duration::from_millis(150);

/// A tab.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StripTab {
    /// every session tiled
    All,
    /// Every project orchestrator and the root Orchestrator.
    Orchestrator,
    /// one project's sessions tiled (its key)
    Project(String),
}

/// Something to click on the strip.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StripHit {
    Tab(StripTab),
    /// a project tab's × (its key): close the project, after a confirm
    Close(String),
    /// a "+N" fold: every tab in the palette
    More,
    /// an arrow at an end of the strip: scroll the tabs on (true) or back
    Scroll(bool),
    /// show / hide the minimized sessions
    Minimized,
    /// a new session in the current project
    New,
}

/// The project tabs' sideways scroll, once they're more than fit (and the setting scrolls them), in the drawing's
/// own units: tabs in the terminal (the first one shown), logical pixels in the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StripScroll {
    /// how far along they are
    pub offset: f32,
    /// the furthest they go, as last drawn: 0 while every tab fits
    pub max: f32,
    /// how wide the window they scroll in is
    pub view: f32,
    /// where each tab starts and ends along them, so an arrow pages to whole tabs
    pub spans: Vec<(f32, f32)>,
    /// the tab last brought into view: when the one showing (or the keys' pick) changes, the new one is
    pub revealed: Option<StripTab>,
}

impl StripScroll {
    /// There are more tabs than fit, and they scroll.
    pub fn scrolls(&self) -> bool {
        self.max > 0.0
    }

    /// Along by `d` (back when negative), no further than the ends.
    pub fn by(&mut self, d: f32) {
        self.offset = (self.offset + d).clamp(0.0, self.max);
    }

    /// A page on (`forward`) or back, to whole tabs: on, the first tab cut off at the right edge starts the window;
    /// back, the window ends about where it started, its first tab whole.
    pub fn page(&mut self, forward: bool) {
        let (offset, view) = (self.offset, self.view.max(1.0));
        let to = if forward {
            self.spans.iter().find(|s| s.1 > offset + view + 0.5).map(|s| s.0).filter(|&s| s > offset + 0.5).unwrap_or(offset + view)
        } else {
            let back = self.spans.iter().map(|s| s.0).filter(|&s| s < offset - 0.5);
            back.clone().filter(|&s| s >= offset - view - 0.5).reduce(f32::min).or_else(|| back.reduce(f32::max)).unwrap_or(offset - view)
        };
        self.offset = to.clamp(0.0, self.max);
    }

    /// Bring `start..end` into a window `view` wide, `margin` clear of its ends where there's room.
    pub fn reveal(&mut self, start: f32, end: f32, view: f32, margin: f32) {
        if start - margin < self.offset {
            self.offset = start - margin;
        } else if end + margin > self.offset + view {
            self.offset = end + margin - view;
        }
        self.offset = self.offset.clamp(0.0, self.max);
    }
}

/// Stack tabs `widths` wide into rows, `gap` apart: the first row `first_room` wide (it follows All and ★), the rest
/// `room`, at most `max_rows` of them. Each tab that fits: its row and its x from that row's start. When some don't,
/// the last row keeps `fold` free at its end for the "+N" they fold into. A tab wider than a whole row still gets
/// one (cut at its end).
pub(crate) fn stack_rows(widths: &[f32], first_room: f32, room: f32, gap: f32, max_rows: usize, fold: f32) -> Vec<(usize, f32)> {
    let mut out = Vec::with_capacity(widths.len());
    let (mut row, mut x) = (0usize, 0.0f32);
    for &w in widths {
        let row_room = if row == 0 { first_room } else { room };
        // a full row, or a first row too short for even this tab: the next row
        if x + w > row_room && (x > 0.0 || row_room < room) {
            if row + 1 >= max_rows.max(1) {
                break;
            }
            row += 1;
            x = 0.0;
        }
        out.push((row, x));
        x += w + gap;
    }
    if out.len() < widths.len() {
        let last_row = out.last().map_or(0, |p| p.0);
        let row_room = if last_row == 0 { first_room } else { room };
        while let Some(&(r, lx)) = out.last() {
            if r != last_row || lx + widths[out.len() - 1] + gap + fold <= row_room {
                break;
            }
            out.pop();
        }
    }
    out
}

impl App {
    /// The setting is on and something is open.
    pub(super) fn strip_on(&self) -> bool {
        !self.tabs.is_empty() && self.svc.settings().project_tabs
    }

    /// Projects with running sessions, in sidebar order, with the order and names the phone / web gave them.
    pub(crate) fn project_tabs(&self) -> Vec<ProjectTab> {
        let hidden: &[PaneId] = if self.show_minimized { &[] } else { &self.minimized };
        let mut tabs = crate::sidebar::project_tabs(&self.live_infos(), &self.open_infos(), hidden);
        let st = self.svc.state();
        let client = |key: &str| st.bridge.projects.iter().find(|c| c.key == key);
        for tab in &mut tabs {
            if let Some(name) = client(&tab.key).and_then(|c| c.name.clone()) {
                tab.name = name;
            }
        }
        // the phone's arrangement first, the rest as the sidebar has them
        tabs.sort_by_key(|tab| client(&tab.key).and_then(|c| c.order).unwrap_or(usize::MAX));
        tabs
    }

    /// Every tab, left to right.
    pub(crate) fn strip_items(&self, tabs: &[ProjectTab]) -> Vec<StripTab> {
        let mut v = vec![StripTab::All];
        if !self.orchestrator_panes().is_empty() {
            v.push(StripTab::Orchestrator);
        }
        v.extend(tabs.iter().map(|t| StripTab::Project(t.key.clone())));
        v
    }

    /// The tab that matches what's on screen: the grid you tiled, the orchestrator alone, or a project whose
    /// every session is on screen and nothing else. None for anything else (a mix you stacked, a view).
    pub(crate) fn showing_tab(&self) -> Option<StripTab> {
        if self.tile_all {
            return Some(match self.tile_project.as_deref() {
                Some(crate::panes::term::ORCHESTRATOR_KEY) => StripTab::Orchestrator,
                Some(key) => StripTab::Project(key.to_string()),
                None => StripTab::All,
            });
        }
        if self.stacked() {
            return None;
        }
        let vis: Vec<PaneId> = self.visible().into_iter().filter(|id| self.panes.get(id).is_some_and(|p| p.is_terminal())).collect();
        let key_of = |id: &PaneId| self.panes.get(id).and_then(|p| p.as_term_ref()).map(|t| (t.meta.orchestrator, t.meta.project.key.clone()));
        let (orchestrator, key) = key_of(vis.first()?)?;
        if orchestrator {
            return (vis.len() == 1 && self.orchestrator_panes().len() == 1).then_some(StripTab::Orchestrator);
        }
        let only_it = vis.iter().all(|id| key_of(id).is_some_and(|(_, k)| k == key));
        let all_of_it = self.tile_ids(Some(&key)).iter().all(|id| vis.contains(id));
        (only_it && all_of_it).then_some(StripTab::Project(key))
    }

    /// Show `tab` (nothing changes when it's showing already).
    pub(crate) fn show_tab(&mut self, tab: StripTab) {
        self.dismiss_session_screen();
        if tab != StripTab::Orchestrator && self.showing_tab().as_ref() == Some(&tab) {
            self.leave_sidebar();
            return;
        }
        match tab {
            StripTab::All => self.toggle_tiles(),
            StripTab::Orchestrator => {
                let ids = self.orchestrator_panes();
                if ids.is_empty() {
                    self.open_orchestrator();
                } else {
                    self.minimized.retain(|id| !ids.contains(id));
                    self.stack_focus = self.focused().filter(|id| ids.contains(id)).or_else(|| ids.first().copied());
                    self.stack = ids;
                    self.stack_zoom = false;
                    self.tile_all = true;
                    self.tile_project = Some(crate::panes::term::ORCHESTRATOR_KEY.into());
                    self.leave_sidebar();
                }
            }
            StripTab::Project(key) => {
                let Some(root) = self.project_tabs().into_iter().find(|t| t.key == key).map(|t| t.root) else {
                    self.toast(Kind::Info, "nothing is running in that project");
                    return;
                };
                // new sessions start where you're looking
                self.pick_project(root);
                self.show_project_tiles(&key, None);
            }
        }
    }

    /// The session counter under usage: every session, tiled (All); a lone one on its own; with none, the launcher.
    pub(crate) fn show_every_session(&mut self) {
        let mut ids = self.panes.iter().filter(|(_, p)| p.is_terminal()).map(|(id, _)| *id);
        match (ids.next(), ids.next()) {
            (None, _) => self.open_launcher(None, Place::Tab),
            (Some(id), None) => self.go_session(id),
            _ => self.show_tab(StripTab::All),
        }
    }

    /// A click (or ⏎) on a tab. All again goes back to the session you're on, like the tile-all shortcut.
    pub(crate) fn click_tab(&mut self, tab: StripTab) {
        let dismissed = self.dismiss_session_screen();
        if !dismissed && tab == StripTab::All && self.showing_tab() == Some(StripTab::All) {
            self.toggle_tiles();
        } else {
            self.show_tab(tab);
        }
    }

    /// The next (`d` = 1) or previous tab, wrapping; the orchestrator's only with `orchestrator`.
    pub(crate) fn step_tab(&mut self, d: i32, orchestrator: bool) {
        let tabs = self.project_tabs();
        let items: Vec<StripTab> = self.strip_items(&tabs).into_iter().filter(|t| *t != StripTab::Orchestrator || (orchestrator && !self.orchestrator_panes().is_empty())).collect();
        let i = self.showing_tab().and_then(|s| items.iter().position(|t| *t == s));
        let next = match i {
            Some(i) => (i as i32 + d).rem_euclid(items.len() as i32) as usize,
            None if d > 0 => 0,
            None => items.len() - 1,
        };
        self.show_tab(items[next].clone());
    }

    /// The sidebar is on but the window is too narrow to draw it.
    pub(crate) fn sidebar_squeezed(&self) -> bool {
        self.sidebar && self.side_area.width == 0 && self.body.width > 0
    }

    /// Keys go to the strip: ← → pick a tab, ⏎ shows it.
    pub(crate) fn focus_strip(&mut self) {
        self.leave_sidebar();
        let items = self.strip_items(&self.project_tabs());
        self.strip_sel = Some(self.showing_tab().and_then(|s| items.iter().position(|t| *t == s)).unwrap_or(0));
    }

    /// Keys while the strip has them.
    pub(crate) fn strip_key(&mut self, k: KeyEvent) {
        let tabs = self.project_tabs();
        let items = self.strip_items(&tabs);
        let n = items.len();
        let sel = self.strip_sel.unwrap_or(0).min(n - 1);
        match k.code {
            KeyCode::Left => self.strip_sel = Some((sel + n - 1) % n),
            KeyCode::Right => self.strip_sel = Some((sel + 1) % n),
            KeyCode::Home => self.strip_sel = Some(0),
            KeyCode::End => self.strip_sel = Some(n - 1),
            KeyCode::Char(c @ '0'..='9') => {
                self.leave_sidebar();
                self.run_act(crate::keymap::Act::ShowTab(c as u8 - b'0'));
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.leave_sidebar();
                self.click_tab(items[sel].clone());
            }
            KeyCode::Delete => {
                if let StripTab::Project(key) = &items[sel] {
                    self.close_project_tab(key);
                }
            }
            KeyCode::Esc | KeyCode::Down | KeyCode::Up | KeyCode::Tab => self.strip_sel = None,
            _ => {}
        }
    }

    /// A click on the strip: a tab's × (or a middle click on it) closes the project.
    pub(crate) fn strip_click(&mut self, hit: StripHit, middle: bool) {
        match hit {
            StripHit::Close(key) => self.close_project_tab(&key),
            StripHit::Tab(StripTab::Project(key)) if middle => self.close_project_tab(&key),
            _ if middle => {}
            StripHit::Tab(tab) => self.click_tab(tab),
            StripHit::More => self.open_palette("tiles "),
            StripHit::Scroll(on) => self.strip_scroll.page(on),
            StripHit::Minimized => self.toggle_minimized_visibility(),
            StripHit::New => self.open_launcher(None, Place::Tab),
        }
    }

    /// Close a project tab's project; its running sessions ask first.
    fn close_project_tab(&mut self, key: &str) {
        if let Some(tab) = self.project_tabs().into_iter().find(|t| t.key == key) {
            self.close_project(tab.root, tab.live + tab.hidden);
        }
    }

    /// The wheel over the strip, `delta` toward the later tabs (back when negative), in the drawing's units: when
    /// they scroll, it scrolls them; otherwise the next or previous tab, once per burst.
    pub(crate) fn strip_wheel(&mut self, delta: f32) {
        if self.strip_scroll.scrolls() {
            self.strip_scroll.by(delta);
            return;
        }
        if delta == 0.0 || self.strip_wheel.is_some_and(|at| at.elapsed() < WHEEL_GAP) {
            return;
        }
        self.strip_wheel = Some(Instant::now());
        self.step_tab(if delta > 0.0 { 1 } else { -1 }, true);
    }

    /// After each batch of events: remember the tab you left (the last-tab key), and give each grid back the
    /// tile edges you held in it.
    pub(super) fn follow_tab(&mut self) {
        let now = self.showing_tab();
        if now != self.tab_now {
            if let Some(old) = self.tab_now.take()
                && now.is_some()
            {
                self.tab_before = Some(old);
            }
            self.tab_now = now;
        }
        let scope = if self.stacked() { self.tab_now.clone() } else { None };
        if scope != self.lock_scope {
            let old = std::mem::replace(&mut self.lock_scope, scope.clone());
            let held = std::mem::take(&mut self.locks);
            if !held.is_empty() {
                self.saved_locks.insert(old, held);
            }
            self.locks = self.saved_locks.remove(&scope).unwrap_or_default();
            let panes = &self.panes;
            self.saved_locks.retain(|_, locks| {
                locks.retain(|l| panes.contains_key(&l.pane));
                !locks.is_empty()
            });
        }
    }

    /// Rows the strip takes at `r`'s width (`r` is its first row): one, or when they stack as many as the project
    /// tabs need, up to `max_rows`.
    pub(super) fn strip_rows(&mut self, r: Rect, max_rows: u16) -> u16 {
        if self.svc.settings().tab_overflow != TabOverflow::Stack {
            return 1;
        }
        let tabs = self.project_tabs();
        let ends = self.strip_ends(r);
        match ends.start {
            Some(start) => self.lay_tabs(&tabs, start, ends.end, r, None, max_rows as usize).rows,
            None => 1,
        }
    }

    /// The strip's fixed parts in `r` (its first row): + and the minimized switch at the right end, All and ★ at
    /// the left.
    fn strip_ends(&self, r: Rect) -> Ends {
        let mut end = r.right();
        let mut plus = None;
        if r.width >= 40 {
            end -= 3;
            plus = Some(Rect { x: end, width: 3, ..r });
        }
        let mut minimized = None;
        if self.tile_all && !self.minimized.is_empty() {
            let text = format!(" {} {} minimized ", if self.show_minimized { "▾" } else { "▸" }, self.minimized.len());
            let w = ui::width(&text) as u16;
            if end >= r.x + w + 30 {
                end -= w + 1;
                minimized = Some((Rect { x: end, width: w, ..r }, text));
            }
        }
        let end = end.saturating_sub(1);
        let all_text = format!(" ▦ All {} ", self.tile_ids(None).len());
        let all = Rect { x: r.x, width: (ui::width(&all_text) as u16).min(end.saturating_sub(r.x)), ..r };
        let ids = self.orchestrator_panes();
        let blocked = ids.iter().filter(|id| self.panes.get(id).and_then(|p| p.activity()) == Some(Activity::Blocked)).count();
        let done = ids.iter().filter(|id| self.done.contains(id) && self.panes.get(id).and_then(|p| p.activity()) != Some(Activity::Blocked)).count();
        let star_text = format!(" ★ Orchestrators {} ", ids.len());
        let sx = all.right() + 1;
        let star_w = ui::width(&star_text) as u16 + 2 + badges_width(blocked, done) as u16;
        let star = (!ids.is_empty()).then(|| (
            Rect { x: sx, width: star_w.min(end.saturating_sub(sx)), ..r },
            star_text,
            blocked,
            done,
            ids.iter().any(|id| self.panes.get(id).and_then(|p| p.activity()) == Some(Activity::Working)),
        ));
        let fixed_end = star.as_ref().map_or(all.right(), |(r, ..)| r.right());
        let start = (end.saturating_sub(fixed_end) >= 3).then_some(fixed_end + 3);
        Ends { plus, minimized, end, all: (all, all_text), star, start }
    }

    /// Where the project tabs go, from column `start` of the strip's first row `r` to `end` (and the full width of
    /// the rows under it, when they stack), `anchor` (the tab the keys picked, else the one showing) kept in view.
    /// Names shorten first; then they scroll (the arrows page, keeping `strip_scroll` in tabs) or stack.
    fn lay_tabs(&mut self, tabs: &[ProjectTab], start: u16, end: u16, r: Rect, anchor: Option<StripTab>, max_rows: usize) -> Laid {
        let n = tabs.len();
        let room = end.saturating_sub(start);
        let widths = |cap: usize| tabs.iter().map(|tab| tab_width(tab, cap)).collect::<Vec<u16>>();
        let longest = tabs.iter().map(|tab| ui::width(&tab.name)).max().unwrap_or(0).clamp(MIN_NAME, MAX_NAME);
        let mut laid = Laid { cap: longest, tabs: vec![], arrows: vec![], rows: 1 };
        if n == 0 {
            return laid;
        }
        if self.svc.settings().tab_overflow == TabOverflow::Stack {
            self.strip_scroll = StripScroll::default();
            // their own widths, wrapped; names shorten only once the rows run out
            let mut cap = longest;
            let rows = loop {
                let w: Vec<f32> = widths(cap).into_iter().map(f32::from).collect();
                let rows = stack_rows(&w, f32::from(room), f32::from(end.saturating_sub(r.x)), 1.0, max_rows, f32::from(MORE_W));
                if rows.len() == n || cap <= MIN_NAME {
                    break rows;
                }
                cap -= 1;
            };
            let w = widths(cap);
            laid.cap = cap;
            for (i, &(row, x)) in rows.iter().enumerate() {
                let x0 = if row == 0 { start } else { r.x };
                laid.tabs.push((i, Rect { x: x0 + x as u16, y: r.y + row as u16, width: w[i], height: 1 }));
                laid.rows = laid.rows.max(row as u16 + 1);
            }
            if rows.len() < n {
                let last = laid.tabs.last().map(|(_, b)| *b).unwrap_or(Rect { x: start, width: 0, ..r });
                let x = if last.width == 0 { last.x } else { last.right() + 1 };
                laid.arrows.push((Rect { x, y: last.y, width: MORE_W, height: 1 }, StripHit::More, rows.len()..n));
            }
            return laid;
        }
        // scroll: the widest names that let every tab fit, else whole tabs from the first one scrolled to
        let total = |cap: usize| widths(cap).iter().map(|w| w + 1).sum::<u16>();
        let mut cap = longest;
        while cap > MIN_NAME && total(cap) > room {
            cap -= 1;
        }
        laid.cap = cap;
        let w = widths(cap);
        let mut s = std::mem::take(&mut self.strip_scroll);
        let (first, hi) = if total(cap) <= room {
            s = StripScroll { revealed: anchor, ..Default::default() };
            (0, n)
        } else {
            // the tabs that fit from `lo`, with an arrow before them when some are past it and one after likewise
            let fits = |lo: usize, hi: usize| w[lo..hi].iter().map(|w| w + 1).sum::<u16>() + if lo > 0 { MORE_W } else { 0 } + if hi < n { MORE_W } else { 0 } <= room;
            let hi_from = |lo: usize| (lo + 1..=n).rev().find(|&hi| fits(lo, hi)).unwrap_or(lo + 1);
            let max_first = (0..n).find(|&lo| hi_from(lo) == n).unwrap_or(n - 1);
            let mut first = (s.offset.max(0.0) as usize).min(max_first);
            if anchor != s.revealed {
                let at = anchor.as_ref().and_then(|a| tabs.iter().position(|t| matches!(a, StripTab::Project(k) if *k == t.key)));
                if let Some(a) = at {
                    if a < first {
                        first = a;
                    } else if a >= hi_from(first) {
                        first = (first..=a).find(|&lo| hi_from(lo) > a).unwrap_or(a);
                    }
                }
                s.revealed = anchor;
            }
            let first = first.min(max_first);
            let hi = hi_from(first);
            s.offset = first as f32;
            s.max = max_first as f32;
            s.view = (hi - first).max(1) as f32;
            s.spans = (0..n).map(|i| (i as f32, i as f32 + 1.0)).collect();
            (first, hi)
        };
        self.strip_scroll = s;
        let mut x = start;
        if first > 0 {
            laid.arrows.push((Rect { x, width: MORE_W, ..r }, StripHit::Scroll(false), 0..first));
            x += MORE_W;
        }
        for (i, &tw) in w.iter().enumerate().take(hi).skip(first) {
            laid.tabs.push((i, Rect { x, width: tw.min(end.saturating_sub(x)), ..r }));
            x += tw + 1;
        }
        if hi < n {
            laid.arrows.push((Rect { x, width: MORE_W.min(end.saturating_sub(x)), ..r }, StripHit::Scroll(true), hi..n));
        }
        laid
    }

    /// The strip at `r` (its height the rows it takes): All, the orchestrators' ★, a rule, then a tab per project;
    /// the new-session + and the minimized switch on the right of the first row. Names shorten before the tabs
    /// scroll (arrows at either end page through them) or, with the stack setting, wrap onto more rows.
    pub(super) fn draw_project_tabs(&mut self, f: &mut Frame, r: Rect) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let tabs = self.project_tabs();
        let items = self.strip_items(&tabs);
        if let Some(sel) = self.strip_sel {
            self.strip_sel = Some(sel.min(items.len() - 1));
        }
        let selected = self.strip_sel.and_then(|i| items.get(i)).cloned();
        let showing = self.drawn_tab.clone();
        let hover = self.hover;
        let focus_key = self.focused().and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()).map(|term| term.meta.project.key.clone());
        let first_row = Rect { height: 1, ..r };
        let ends = self.strip_ends(first_row);

        // the right end: the minimized sessions' switch, then + (a new session)
        if let Some(b) = ends.plus {
            let style = if b.contains(hover) { ui::band_style(t.accent, false, &t).add_modifier(Modifier::BOLD) } else { ui::bold_accent(&t) };
            ui::line(f, b, vec![Span::styled(" + ", style)]);
            self.side_hits.push((b, SideHit::Strip(StripHit::New)));
        }
        if let Some((b, text)) = &ends.minimized {
            let style = if b.contains(hover) { ui::band_style(t.accent, false, &t) } else if self.show_minimized { ui::accent(&t) } else { muted(&t) };
            ui::line(f, *b, vec![Span::styled(text.clone(), style)]);
            self.side_hits.push((*b, SideHit::Strip(StripHit::Minimized)));
        }

        // All: every session the grid would show
        let (b, text) = &ends.all;
        if b.width > 0 {
            let style = tab_style(t.accent, showing == Some(StripTab::All), b.contains(hover), selected == Some(StripTab::All), &t).add_modifier(Modifier::BOLD);
            ui::line(f, *b, vec![Span::styled(text.clone(), style)]);
            self.side_hits.push((*b, SideHit::Strip(StripHit::Tab(StripTab::All))));
        }
        // every coordinator, with what they're doing and what wants you
        if let Some((b, label, blocked, done, working)) = &ends.star && b.width > 0 {
            let gold = crate::theme::gold(&t);
            let on = showing == Some(StripTab::Orchestrator);
            let base = tab_style(gold, on, b.contains(hover), selected == Some(StripTab::Orchestrator), &t);
            let activity = working.then_some(Activity::Working);
            let mut spans = vec![Span::styled(label.clone(), base.add_modifier(Modifier::BOLD)), Span::styled(activity_glyph(activity, time), glyph_style(base, gold, on))];
            spans.extend(badges(*blocked, *done, base, on, &t));
            spans.push(Span::styled(" ", base));
            ui::line(f, *b, spans);
            self.side_hits.push((*b, SideHit::Strip(StripHit::Tab(StripTab::Orchestrator))));
        }
        let Some(start) = ends.start.filter(|_| !tabs.is_empty()) else { return };
        ui::line(f, Rect { x: start - 3, width: 3, ..first_row }, vec![Span::styled(" │ ", fg(t.frame))]);

        let anchor = selected.clone().filter(|s| matches!(s, StripTab::Project(_))).or_else(|| showing.clone().filter(|s| matches!(s, StripTab::Project(_))));
        let laid = self.lay_tabs(&tabs, start, ends.end, first_row, anchor, r.height.max(1) as usize);
        let cap = laid.cap;
        // what each tab looks like: (on, hot, picked)
        let looks: Vec<(bool, bool, bool)> = laid
            .tabs
            .iter()
            .map(|(i, b)| {
                let key = StripTab::Project(tabs[*i].key.clone());
                (showing.as_ref() == Some(&key), b.contains(hover), selected.as_ref() == Some(&key))
            })
            .collect();
        for (k, &(i, b)) in laid.tabs.iter().enumerate() {
            let tab = &tabs[i];
            let key = StripTab::Project(tab.key.clone());
            let color = self.project_color(&tab.key);
            let (on, hot, picked) = looks[k];
            let base = tab_style(color, on, hot, picked, &t);
            let name_style = if on || focus_key.as_deref() == Some(tab.key.as_str()) { base.add_modifier(Modifier::BOLD) } else { base };
            let lead = if tab.working > 0 { Some(Activity::Working) } else { Some(Activity::Idle) };
            let mut spans = vec![
                Span::styled(" ", base),
                Span::styled(activity_glyph(lead, time), glyph_style(base, color, on)),
                Span::styled(" ", base),
                Span::styled(ui::fit(&tab.name, cap), name_style),
                Span::styled(format!(" {}", tab.live), if on { base } else { base.fg(t.muted) }),
            ];
            spans.extend(badges(tab.blocked, tab.done, base, on, &t));
            // a short name's tab is padded to the least width, before the ×
            let used: u16 = spans.iter().map(|s| ui::width(&s.content) as u16).sum::<u16>() + 3;
            let full = tab_width(tab, cap);
            if full > used {
                spans.push(Span::styled(" ".repeat((full - used) as usize), base));
            }
            // the × shows under the mouse (or the keys, where delete closes it); its room is always kept so the
            // strip doesn't shift as you move along it
            let close = Rect { x: b.x + full.saturating_sub(3), width: 2, ..b };
            let close_hot = close.contains(hover);
            let close_style = if close_hot { base.fg(t.danger).add_modifier(Modifier::BOLD) } else if on { base } else { base.fg(t.muted) };
            spans.push(Span::styled(" ", base));
            spans.push(Span::styled(if hot || picked { "×" } else { " " }, close_style));
            spans.push(Span::styled(" ", base));
            if b.width > 0 {
                ui::line(f, b, spans);
                // the first hit that matches wins: the × before its tab
                if close.right() <= b.right() {
                    self.side_hits.push((close, SideHit::Strip(StripHit::Close(tab.key.clone()))));
                }
                self.side_hits.push((b, SideHit::Strip(StripHit::Tab(key))));
            }
            // a hairline between two quiet tabs on a row, so they read apart
            let quiet = |l: (bool, bool, bool)| !(l.0 || l.1 || l.2);
            if let Some(&(_, nb)) = laid.tabs.get(k + 1)
                && nb.y == b.y
                && quiet(looks[k])
                && quiet(looks[k + 1])
                && b.right() < nb.x
            {
                ui::line(f, Rect { x: b.right(), width: 1, ..b }, vec![Span::styled("│", fg(t.frame))]);
            }
        }
        // past either end: "◂2" / "3▸" scroll (stacked out of rows: "+3" lists them all), each marked when a tab
        // past it wants you
        for (b, hit, range) in laid.arrows {
            let folded = &tabs[range];
            let dot = folded.iter().any(ProjectTab::attention);
            let n = folded.len();
            let (lead, tail) = match hit {
                StripHit::Scroll(false) => (format!(" ◂{n}"), " "),
                StripHit::Scroll(true) => (format!(" {n}"), "▸ "),
                _ => (format!(" +{n}"), " "),
            };
            let mut spans = vec![Span::styled(lead, muted(&t))];
            if dot {
                spans.push(Span::styled("●", fg(t.danger)));
            }
            spans.push(Span::styled(tail, muted(&t)));
            if b.width == 0 {
                continue;
            }
            if b.contains(hover) {
                f.buffer_mut().set_style(b, Style::default().bg(crate::theme::project_tint(t.accent, &t)));
            }
            ui::line(f, b, spans);
            self.side_hits.push((b, SideHit::Strip(hit)));
        }
    }
}

/// The strip's fixed parts (see [`App::strip_ends`]).
struct Ends {
    plus: Option<Rect>,
    /// the minimized switch and its text
    minimized: Option<(Rect, String)>,
    /// where the tabs stop on the first row
    end: u16,
    all: (Rect, String),
    /// ★ while an orchestrator is open: where, its label, waiting on you, finished unseen, working
    star: Option<(Rect, String, usize, usize, bool)>,
    /// the first project tab's column (after the rule), when there's room for any
    start: Option<u16>,
}

/// The project tabs, laid out (see [`App::lay_tabs`]).
struct Laid {
    /// the most columns a name takes
    cap: usize,
    /// each tab shown: its index, where
    tabs: Vec<(usize, Rect)>,
    /// the arrows (scroll) or "+N" (stacked out of rows): where, what a click does, the tabs past it
    arrows: Vec<(Rect, StripHit, std::ops::Range<usize>)>,
    rows: u16,
}

/// What a tab's agents are doing, as one glyph: a spinner while any works, else a ring.
fn activity_glyph(a: Option<Activity>, time: f64) -> &'static str {
    match a {
        Some(Activity::Working) => ["◐", "◓", "◑", "◒"][(time * 6.0) as usize % 4],
        _ => "○",
    }
}

/// The lead glyph takes the project's color, except on the showing tab, whose text is all one ink.
fn glyph_style(base: Style, color: Color, on: bool) -> Style {
    if on { base } else { base.fg(color).add_modifier(Modifier::BOLD) }
}

/// " ●2" for sessions waiting on you, " ✓1" for ones that finished unseen: shape says which, so they read
/// without color too.
fn badges(blocked: usize, done: usize, base: Style, on: bool, t: &Theme) -> Vec<Span<'static>> {
    let tone = |c: Color| if on { base } else { base.fg(c).add_modifier(Modifier::BOLD) };
    let mut v = vec![];
    if blocked > 0 {
        v.push(Span::styled(format!(" ●{blocked}"), tone(t.danger)));
    }
    if done > 0 {
        v.push(Span::styled(format!(" ✓{done}"), tone(t.good)));
    }
    v
}

fn badges_width(blocked: usize, done: usize) -> usize {
    let one = |n: usize| if n > 0 { 2 + n.to_string().len() } else { 0 };
    one(blocked) + one(done)
}

/// A project tab's width with its name cut to `cap` columns: " ◐ name 3 ●1 ✓1 × ", never under [`TAB_MIN`].
fn tab_width(tab: &ProjectTab, cap: usize) -> u16 {
    ((3 + ui::width(&tab.name).min(cap) + 1 + tab.live.to_string().len() + badges_width(tab.blocked, tab.done) + 3) as u16).max(TAB_MIN)
}

/// The showing tab is solid in its color; one under the mouse or picked with the keys takes a tint of it (an
/// underline too for the keys); the rest are colored text. A named terminal color can't be tinted, so it
/// reverses instead.
fn tab_style(color: Color, on: bool, hot: bool, picked: bool, t: &Theme) -> Style {
    let style = if on {
        ui::band_style(color, true, t)
    } else if hot || picked {
        match color {
            Color::Rgb(..) => ui::band_style(color, false, t),
            _ => fg(color).add_modifier(Modifier::REVERSED),
        }
    } else {
        fg(color)
    };
    if picked { style.add_modifier(Modifier::UNDERLINED) } else { style }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacked_tabs_wrap_into_rows_and_fold_what_the_rows_cant_hold() {
        // a 50 wide first row after All and ★, 100 wide rows under it, tabs 30 wide a gap of 2 apart
        let w = [30.0; 7];
        let rows = stack_rows(&w, 50.0, 100.0, 2.0, 3, 6.0);
        assert_eq!(rows, vec![(0, 0.0), (1, 0.0), (1, 32.0), (1, 64.0), (2, 0.0), (2, 32.0), (2, 64.0)]);
        // two rows only: the rest fold, the last row keeping room for the "+N"
        let rows = stack_rows(&w, 50.0, 100.0, 2.0, 2, 6.0);
        assert_eq!(rows, vec![(0, 0.0), (1, 0.0), (1, 32.0)], "the third on row 1 gives way to the fold");
        // a first row too short for any tab starts them on the next
        assert_eq!(stack_rows(&[30.0], 10.0, 100.0, 2.0, 3, 6.0), vec![(1, 0.0)]);
        // one row: whatever fits, then the fold
        assert_eq!(stack_rows(&w, 100.0, 100.0, 2.0, 1, 6.0), vec![(0, 0.0), (0, 32.0)]);
        assert!(stack_rows(&[], 100.0, 100.0, 2.0, 3, 6.0).is_empty());
    }

    #[test]
    fn the_strip_scrolls_no_further_than_its_ends_and_brings_a_tab_into_view() {
        let mut s = StripScroll { max: 300.0, ..Default::default() };
        s.by(-40.0);
        assert_eq!(s.offset, 0.0);
        s.by(500.0);
        assert_eq!(s.offset, 300.0);
        // a tab at 100..200 in a 150 wide window scrolled to 300: back to show it, with the margin
        s.reveal(100.0, 200.0, 150.0, 10.0);
        assert_eq!(s.offset, 90.0);
        // one past the right edge: on until its end shows
        s.reveal(260.0, 320.0, 150.0, 10.0);
        assert_eq!(s.offset, 180.0);
        // already in view: stays
        s.reveal(200.0, 250.0, 150.0, 10.0);
        assert_eq!(s.offset, 180.0);
    }

    #[test]
    fn the_arrows_page_to_whole_tabs() {
        // five tabs 100 wide with gaps of 4, a window 250 wide
        let spans: Vec<(f32, f32)> = (0..5).map(|i| (i as f32 * 104.0, i as f32 * 104.0 + 100.0)).collect();
        let mut s = StripScroll { max: 516.0 - 250.0, view: 250.0, spans, ..Default::default() };
        // the third tab is cut at the right edge: on, it starts the window
        s.page(true);
        assert_eq!(s.offset, 208.0);
        // the last page stops at the end
        s.page(true);
        assert_eq!(s.offset, 266.0);
        // back: the first whole tab within a window's width before
        s.page(false);
        assert_eq!(s.offset, 104.0);
        s.page(false);
        assert_eq!(s.offset, 0.0);
        // a tab wider than the window still moves on
        let mut s = StripScroll { max: 900.0, view: 250.0, spans: vec![(0.0, 600.0), (604.0, 1000.0)], ..Default::default() };
        s.page(true);
        assert_eq!(s.offset, 250.0);
    }
}
