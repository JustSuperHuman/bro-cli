//! The session tabs: a strip across the top of the sessions, one tab per session in what the sidebar picked (its
//! scope: one project, or the orchestrators). It opens with that project's name (and in the window its icon),
//! then its All tab, which tiles every session in it the way the tile-all shortcut does; a session's tab shows
//! that session on its own. Every session (the sidebar's All sessions) is their grid, with no strip: going to one
//! of them on its own lists its project's. The scope is the one you picked until you go to a session outside it
//! (a key, an ask in the sidebar), which brings its own project.
//!
//! A tab's first glyph is what its agent is doing (◐ working, ○ idle) in its project's colour; after the name, what
//! wants you, each its own shape (● waiting on you, ✓ finished unseen). A tab is never narrower than [`TAB_MIN`]
//! nor its name cut below [`MIN_NAME`]: names shorten first, then the tabs that still don't fit scroll (arrows at
//! either end page through them, the wheel too) or, with the stack setting, wrap onto more rows. The strip's right
//! end holds the minimized sessions' switch and +, a new session. A tab under the mouse shows a × that closes its
//! session (asking first while its agent works).

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
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A session's name never shrinks below this many columns to make room for the others…
const MIN_NAME: usize = 10;
/// …nor takes more than this many, so one long name can't push the others (or the showing tab) off.
const MAX_NAME: usize = 24;
/// A session tab's least width, so a short name still makes a tab you can hit.
const TAB_MIN: u16 = 14;
/// Room an arrow ("◂2", "3▸") or a "+N" fold takes.
const MORE_W: u16 = 6;
/// The most of the strip the project's name, its heading, takes.
const HEAD_NAME: usize = 20;
/// A trackpad's burst of wheel events switches one tab.
const WHEEL_GAP: Duration = Duration::from_millis(150);

/// What the sidebar picks and the strip lists the sessions of: every session, the orchestrators, or one project.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum StripTab {
    /// every session
    #[default]
    All,
    /// Every project orchestrator and the root Orchestrator.
    Orchestrator,
    /// one project's sessions (its key)
    Project(String),
}

impl StripTab {
    /// The key the grid of this scope tiles by (see [`App::tile_ids`]).
    pub(crate) fn tile_key(&self) -> Option<&str> {
        match self {
            StripTab::All => None,
            StripTab::Orchestrator => Some(crate::panes::term::ORCHESTRATOR_KEY),
            StripTab::Project(key) => Some(key),
        }
    }
}

/// A tab on the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SessionTab {
    /// the scope's All: every session in it tiled
    Grid,
    /// one session, on its own
    Session(PaneId),
}

/// Something to click on the strip.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StripHit {
    /// the scope's All tab
    Grid,
    /// a session's tab
    Session(PaneId),
    /// a session tab's × (or a middle click on it): close the session, asking first while its agent works
    Close(PaneId),
    /// a "+N" fold: every session in the palette
    More,
    /// an arrow at an end of the strip: scroll the tabs on (true) or back
    Scroll(bool),
    /// show / hide the minimized sessions
    Minimized,
    /// a new session in the current project
    New,
}

/// One session's tab, as both drawings read it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionTabInfo {
    pub id: PaneId,
    /// what to call it: your name, the agent's title, else its login · model
    pub label: String,
    pub project_key: String,
    pub project_name: String,
    pub project_root: PathBuf,
    /// bro's orchestrator over every project (it has no project of its own)
    pub global: bool,
    /// its agent (or its subagents, its background tasks) at work
    pub working: bool,
    /// it waits on you: a permission to give, a question its turn ended on
    pub asks: bool,
    /// it finished while you weren't looking
    pub done: bool,
    /// kept out of the grid
    pub minimized: bool,
}

/// The session tabs' sideways scroll, once they're more than fit (and the setting scrolls them), in the drawing's
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
    pub revealed: Option<SessionTab>,
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

/// Stack tabs `widths` wide into rows, `gap` apart: the first row `first_room` wide (it follows All), the rest
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
    /// Something is open and the tabs list a project's sessions or the orchestrators: the strip shows. Every
    /// session (their grid) has none: the sidebar's All sessions is all the heading it needs.
    pub(super) fn strip_on(&self) -> bool {
        !self.tabs.is_empty() && self.scope != StripTab::All
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

    /// Every scope the keys step through, in order: All, the orchestrators while one runs, then each project.
    pub(crate) fn scope_items(&self, tabs: &[ProjectTab]) -> Vec<StripTab> {
        let mut v = vec![StripTab::All];
        if !self.orchestrator_panes().is_empty() {
            v.push(StripTab::Orchestrator);
        }
        v.extend(tabs.iter().map(|t| StripTab::Project(t.key.clone())));
        v
    }

    /// The sessions in the scope, in sidebar order (minimized ones too: they're still yours to go to).
    pub(crate) fn session_tabs(&self) -> Vec<SessionTabInfo> {
        let live = self.live_infos();
        let order = crate::sidebar::live_order(&live, &[], &self.open_infos());
        order
            .into_iter()
            .filter_map(|id| {
                let l = live.iter().find(|l| l.pane == id)?;
                let inside = match &self.scope {
                    StripTab::All => true,
                    StripTab::Orchestrator => l.orchestrator,
                    StripTab::Project(key) => l.project_key == *key,
                };
                let asks = crate::sidebar::needs_you(l);
                inside.then(|| SessionTabInfo {
                    id,
                    label: crate::sidebar::session_label(l),
                    project_key: l.project_key.clone(),
                    project_name: l.project_name.clone(),
                    project_root: l.project_root.clone(),
                    global: l.orchestrator && l.project_key == crate::panes::term::ORCHESTRATOR_KEY,
                    working: !asks && (l.activity == Some(Activity::Working) || l.background.0 + l.background.1 > 0),
                    asks,
                    done: l.done && !asks,
                    minimized: self.minimized.contains(&id),
                })
            })
            .collect()
    }

    /// Every tab on the strip, left to right: All (once there are two sessions to tile), then each session.
    pub(crate) fn strip_items(&self, tabs: &[SessionTabInfo]) -> Vec<SessionTab> {
        let mut v = Vec::with_capacity(tabs.len() + 1);
        if tabs.len() >= 2 {
            v.push(SessionTab::Grid);
        }
        v.extend(tabs.iter().map(|t| SessionTab::Session(t.id)));
        v
    }

    /// The scope that matches what's on screen: the grid you tiled, the orchestrator alone, or a project whose
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

    /// The strip's tab that's on screen: All while the scope's grid shows, else the focused session's.
    pub(crate) fn showing_session(&self) -> Option<SessionTab> {
        if self.tile_all && !self.stack_zoom && self.stack.len() >= 2 {
            return Some(SessionTab::Grid);
        }
        let id = self.focused()?;
        self.panes.get(&id).is_some_and(|p| p.is_terminal()).then_some(SessionTab::Session(id))
    }

    /// The scope a session belongs to on its own: its project, or the orchestrators for the one over them all.
    fn own_scope(&self, id: PaneId) -> Option<StripTab> {
        let t = self.panes.get(&id)?.as_term_ref()?;
        Some(if t.meta.project.key == crate::panes::term::ORCHESTRATOR_KEY { StripTab::Orchestrator } else { StripTab::Project(t.meta.project.key.clone()) })
    }

    /// The session is one the scope lists.
    fn in_scope(&self, id: PaneId, scope: &StripTab) -> bool {
        let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) else { return false };
        match scope {
            StripTab::All => true,
            StripTab::Orchestrator => t.meta.orchestrator,
            StripTab::Project(key) => t.meta.project.key == *key,
        }
    }

    /// Keep the strip's scope with what you look at: a grid is its own scope (every session's only while their grid
    /// shows); a session on its own, or one outside the scope (you went to it with a key, from an ask, from the
    /// phone), brings its project; on a view, a scope with nothing left gives way to the project you're in.
    pub(crate) fn follow_scope(&mut self) {
        if self.tile_all {
            self.scope = match self.tile_project.as_deref() {
                Some(crate::panes::term::ORCHESTRATOR_KEY) => StripTab::Orchestrator,
                Some(key) => StripTab::Project(key.to_string()),
                None => StripTab::All,
            };
            return;
        }
        if let Some(id) = self.focused().filter(|id| self.panes.get(id).is_some_and(|p| p.is_terminal())) {
            if (self.scope == StripTab::All || !self.in_scope(id, &self.scope))
                && let Some(own) = self.own_scope(id)
            {
                self.scope = own;
            }
            return;
        }
        let scope = self.scope.clone();
        if scope == StripTab::All || !self.panes.keys().any(|id| self.in_scope(*id, &scope)) {
            let here = self.cur_project.as_ref().map(|root| StripTab::Project(self.svc.project_for(root).key));
            self.scope = here.filter(|s| self.panes.keys().any(|id| self.in_scope(*id, s))).unwrap_or(StripTab::All);
        }
    }

    /// The strip's heading, before All: the project the tabs list (its name as the phone gave it, and its key and
    /// folder for its icon), or the orchestrators. None for every session, which has no strip.
    pub(crate) fn scope_heading(&self) -> Option<(String, Option<(String, PathBuf)>)> {
        match &self.scope {
            StripTab::All => None,
            StripTab::Orchestrator => Some(("Orchestrators".into(), None)),
            StripTab::Project(key) => {
                let (name, root) = match self.project_tabs().into_iter().find(|t| t.key == *key) {
                    Some(t) => (t.name, t.root),
                    None => self.open_infos().into_iter().find(|o| o.key == *key).map(|o| (o.name, o.root))?,
                };
                Some((name, Some((key.clone(), root))))
            }
        }
    }

    /// Show `tab`'s sessions: its grid, or its one session (nothing changes when it's showing already). It
    /// becomes the scope the strip lists.
    pub(crate) fn show_tab(&mut self, tab: StripTab) {
        self.dismiss_session_screen();
        if tab != StripTab::Orchestrator && self.showing_tab().as_ref() == Some(&tab) {
            self.scope = tab;
            self.leave_sidebar();
            return;
        }
        match tab {
            StripTab::All => {
                self.scope = StripTab::All;
                self.toggle_tiles();
            }
            StripTab::Orchestrator => {
                let ids = self.orchestrator_panes();
                if ids.is_empty() {
                    self.open_orchestrator();
                } else {
                    self.scope = StripTab::Orchestrator;
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
                self.scope = StripTab::Project(key.clone());
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

    /// A click (or ⏎) on a tab of the strip: a session's shows it on its own; All tiles the scope, and again goes
    /// back to the session you're on, like the tile-all shortcut.
    pub(crate) fn click_session_tab(&mut self, tab: SessionTab) {
        let dismissed = self.dismiss_session_screen();
        let showing = self.showing_session();
        match tab {
            SessionTab::Grid if !dismissed && showing == Some(SessionTab::Grid) => {
                let focus = self.focused();
                self.clear_stack();
                if let Some(id) = focus {
                    self.focus_pane(id);
                }
            }
            SessionTab::Grid => self.show_tab(self.scope.clone()),
            SessionTab::Session(id) if !self.panes.contains_key(&id) => {}
            SessionTab::Session(id) => {
                if showing != Some(tab) || self.stacked() {
                    self.clear_stack();
                    self.focus_pane(id);
                }
            }
        }
        self.leave_sidebar();
    }

    /// The next (`d` = 1) or previous scope, wrapping; the orchestrators' only with `orchestrator`.
    pub(crate) fn step_tab(&mut self, d: i32, orchestrator: bool) {
        let tabs = self.project_tabs();
        let items: Vec<StripTab> = self.scope_items(&tabs).into_iter().filter(|t| *t != StripTab::Orchestrator || (orchestrator && !self.orchestrator_panes().is_empty())).collect();
        let i = self.showing_tab().and_then(|s| items.iter().position(|t| *t == s));
        let next = match i {
            Some(i) => (i as i32 + d).rem_euclid(items.len() as i32) as usize,
            None if d > 0 => 0,
            None => items.len() - 1,
        };
        self.show_tab(items[next].clone());
    }

    /// The next (`d` = 1) or previous tab of the strip, wrapping.
    fn step_session_tab(&mut self, d: i32) {
        let items = self.strip_items(&self.session_tabs());
        if items.is_empty() {
            return;
        }
        let i = self.showing_session().and_then(|s| items.iter().position(|t| *t == s));
        let next = match i {
            Some(i) => (i as i32 + d).rem_euclid(items.len() as i32) as usize,
            None if d > 0 => 0,
            None => items.len() - 1,
        };
        self.click_session_tab(items[next]);
    }

    /// The sidebar is on but the window is too narrow to draw it.
    pub(crate) fn sidebar_squeezed(&self) -> bool {
        self.sidebar && self.side_area.width == 0 && self.body.width > 0
    }

    /// Keys go to the strip: ← → pick a tab, ⏎ shows it.
    pub(crate) fn focus_strip(&mut self) {
        self.leave_sidebar();
        let items = self.strip_items(&self.session_tabs());
        self.strip_sel = Some(self.showing_session().and_then(|s| items.iter().position(|t| *t == s)).unwrap_or(0));
    }

    /// Keys while the strip has them.
    pub(crate) fn strip_key(&mut self, k: KeyEvent) {
        let items = self.strip_items(&self.session_tabs());
        let n = items.len();
        if n == 0 {
            self.strip_sel = None;
            return;
        }
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
                self.click_session_tab(items[sel]);
            }
            KeyCode::Delete => {
                if let SessionTab::Session(id) = items[sel] {
                    self.ask_close(id);
                }
            }
            KeyCode::Esc | KeyCode::Down | KeyCode::Up | KeyCode::Tab => self.strip_sel = None,
            _ => {}
        }
    }

    /// A click on the strip: a tab's × (or a middle click on it) closes its session.
    pub(crate) fn strip_click(&mut self, hit: StripHit, middle: bool) {
        match hit {
            StripHit::Close(id) => self.ask_close(id),
            StripHit::Session(id) if middle => self.ask_close(id),
            _ if middle => {}
            StripHit::Grid => self.click_session_tab(SessionTab::Grid),
            StripHit::Session(id) => self.click_session_tab(SessionTab::Session(id)),
            StripHit::More => self.open_palette("sessions "),
            StripHit::Scroll(on) => self.strip_scroll.page(on),
            StripHit::Minimized => self.toggle_minimized_visibility(),
            StripHit::New => self.open_launcher(None, Place::Tab),
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
        self.step_session_tab(if delta > 0.0 { 1 } else { -1 });
    }

    /// After each batch of events: keep the scope with what you look at, remember the scope you left (the last-tab
    /// key), and give each grid back the tile edges you held in it.
    pub(super) fn follow_tab(&mut self) {
        self.follow_scope();
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

    /// Rows the strip takes at `r`'s width (`r` is its first row): one, or when they stack as many as the session
    /// tabs need, up to `max_rows`.
    pub(super) fn strip_rows(&mut self, r: Rect, max_rows: u16) -> u16 {
        if self.svc.settings().tab_overflow != TabOverflow::Stack {
            return 1;
        }
        let tabs = self.session_tabs();
        let ends = self.strip_ends(r, &tabs);
        let (names, extra) = dims(&tabs);
        match ends.start {
            Some(start) => self.lay_tabs(&names, &extra, start, ends.end, r, None, max_rows as usize).rows,
            None => 1,
        }
    }

    /// The strip's fixed parts in `r` (its first row): + and the minimized switch at the right end, All at the left.
    fn strip_ends(&self, r: Rect, tabs: &[SessionTabInfo]) -> Ends {
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
        // what the tabs list first, then a rule
        let heading = self.scope_heading().map(|(name, project)| {
            let text = match &project {
                Some(_) => format!(" {} ", ui::fit(&name, HEAD_NAME)),
                None => format!(" ★ {name} "),
            };
            let color = match &project {
                Some((key, _)) => self.project_color(key),
                None => crate::theme::gold(&self.theme),
            };
            (Rect { x: r.x, width: (ui::width(&text) as u16).min(end.saturating_sub(r.x)), ..r }, text, color)
        });
        let x0 = heading.as_ref().map_or(r.x, |(b, ..)| (b.right() + 3).min(end));
        let grid = (tabs.len() >= 2).then(|| {
            let text = format!(" ▦ All {} ", self.tile_ids(self.scope.tile_key()).len());
            (Rect { x: x0, width: (ui::width(&text) as u16).min(end.saturating_sub(x0)), ..r }, text)
        });
        let start = match &grid {
            Some((b, _)) => (end.saturating_sub(b.right()) >= 3).then_some(b.right() + 3),
            None => (end > x0).then_some(x0),
        };
        Ends { plus, minimized, end, heading, grid, start }
    }

    /// Where the session tabs go, from column `start` of the strip's first row `r` to `end` (and the full width of
    /// the rows under it, when they stack), `anchor` (the tab the keys picked, else the one showing: its index and
    /// tab) kept in view. Each tab is its name (`names` columns, cut to a shared cap) and `extra` around it. Names
    /// shorten first; then they scroll (the arrows page, keeping `strip_scroll` in tabs) or stack.
    #[allow(clippy::too_many_arguments)]
    fn lay_tabs(&mut self, names: &[usize], extra: &[usize], start: u16, end: u16, r: Rect, anchor: Option<(usize, SessionTab)>, max_rows: usize) -> Laid {
        let n = names.len();
        let room = end.saturating_sub(start);
        let widths = |cap: usize| (0..n).map(|i| tab_width(names[i], extra[i], cap)).collect::<Vec<u16>>();
        let longest = names.iter().copied().max().unwrap_or(0).clamp(MIN_NAME, MAX_NAME);
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
        let anchor_tab = anchor.map(|(_, tab)| tab);
        let (first, hi) = if total(cap) <= room {
            s = StripScroll { revealed: anchor_tab, ..Default::default() };
            (0, n)
        } else {
            // the tabs that fit from `lo`, with an arrow before them when some are past it and one after likewise
            let fits = |lo: usize, hi: usize| w[lo..hi].iter().map(|w| w + 1).sum::<u16>() + if lo > 0 { MORE_W } else { 0 } + if hi < n { MORE_W } else { 0 } <= room;
            let hi_from = |lo: usize| (lo + 1..=n).rev().find(|&hi| fits(lo, hi)).unwrap_or(lo + 1);
            let max_first = (0..n).find(|&lo| hi_from(lo) == n).unwrap_or(n - 1);
            let mut first = (s.offset.max(0.0) as usize).min(max_first);
            if anchor_tab != s.revealed {
                if let Some((a, _)) = anchor {
                    if a < first {
                        first = a;
                    } else if a >= hi_from(first) {
                        first = (first..=a).find(|&lo| hi_from(lo) > a).unwrap_or(a);
                    }
                }
                s.revealed = anchor_tab;
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

    /// The strip at `r` (its height the rows it takes): the scope's All, a rule, then a tab per session; the
    /// new-session + and the minimized switch on the right of the first row. Names shorten before the tabs scroll
    /// (arrows at either end page through them) or, with the stack setting, wrap onto more rows.
    pub(super) fn draw_session_tabs(&mut self, f: &mut Frame, r: Rect) {
        let t = self.theme.clone();
        let time = self.start.elapsed().as_secs_f64();
        let tabs = self.session_tabs();
        let items = self.strip_items(&tabs);
        if let Some(sel) = self.strip_sel {
            self.strip_sel = Some(sel.min(items.len().saturating_sub(1)));
        }
        let selected = self.strip_sel.and_then(|i| items.get(i)).copied();
        let showing = self.showing_session();
        let hover = self.hover;
        let focused = self.focused();
        let first_row = Rect { height: 1, ..r };
        let ends = self.strip_ends(first_row, &tabs);

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

        // what the tabs list: the project's name in its colour (the orchestrators' ★ in gold), then a rule
        if let Some((b, text, color)) = &ends.heading
            && b.width > 0
        {
            let style = Style::default().fg(*color).add_modifier(Modifier::BOLD);
            ui::line(f, *b, vec![Span::styled(text.clone(), if b.contains(hover) { style.add_modifier(Modifier::UNDERLINED) } else { style })]);
            ui::line(f, Rect { x: b.right(), width: 3.min(ends.end.saturating_sub(b.right())), ..first_row }, vec![Span::styled(" │ ", fg(t.frame))]);
            self.side_hits.push((*b, SideHit::Scope(self.scope.clone())));
        }
        // All: every session in the scope, tiled
        if let Some((b, text)) = &ends.grid
            && b.width > 0
        {
            let color = match &self.scope {
                StripTab::Project(key) => self.project_color(key),
                StripTab::Orchestrator => crate::theme::gold(&t),
                StripTab::All => t.accent,
            };
            let style = tab_style(color, showing == Some(SessionTab::Grid), b.contains(hover), selected == Some(SessionTab::Grid), &t).add_modifier(Modifier::BOLD);
            ui::line(f, *b, vec![Span::styled(text.clone(), style)]);
            self.side_hits.push((*b, SideHit::Strip(StripHit::Grid)));
        }
        let Some(start) = ends.start.filter(|_| !tabs.is_empty()) else { return };
        if ends.grid.is_some() {
            ui::line(f, Rect { x: start - 3, width: 3, ..first_row }, vec![Span::styled(" │ ", fg(t.frame))]);
        }

        let anchor = selected.filter(|s| matches!(s, SessionTab::Session(_))).or_else(|| showing.filter(|s| matches!(s, SessionTab::Session(_))));
        let anchor = anchor.and_then(|a| tabs.iter().position(|tab| SessionTab::Session(tab.id) == a).map(|i| (i, a)));
        let (names, extra) = dims(&tabs);
        let laid = self.lay_tabs(&names, &extra, start, ends.end, first_row, anchor, r.height.max(1) as usize);
        let cap = laid.cap;
        // what each tab looks like: (on, hot, picked)
        let looks: Vec<(bool, bool, bool)> = laid
            .tabs
            .iter()
            .map(|(i, b)| {
                let tab = Some(SessionTab::Session(tabs[*i].id));
                (showing == tab, b.contains(hover), selected == tab)
            })
            .collect();
        for (k, &(i, b)) in laid.tabs.iter().enumerate() {
            let tab = &tabs[i];
            let color = if tab.global { crate::theme::gold(&t) } else { self.project_color(&tab.project_key) };
            let (on, hot, picked) = looks[k];
            let mut base = tab_style(color, on, hot, picked, &t);
            if tab.minimized && !on {
                base = base.add_modifier(Modifier::DIM);
            }
            let name_style = if on || focused == Some(tab.id) { base.add_modifier(Modifier::BOLD) } else { base };
            let lead = if tab.working { Some(Activity::Working) } else { Some(Activity::Idle) };
            let mut spans = vec![
                Span::styled(" ", base),
                Span::styled(activity_glyph(lead, time), glyph_style(base, color, on)),
                Span::styled(" ", base),
                Span::styled(ui::fit(&tab.label, cap), name_style),
            ];
            spans.extend(badge(tab, base, on, &t));
            // a short name's tab is padded to the least width, before the ×
            let used: u16 = spans.iter().map(|s| ui::width(&s.content) as u16).sum::<u16>() + 3;
            let full = tab_width(names[i], extra[i], cap);
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
                    self.side_hits.push((close, SideHit::Strip(StripHit::Close(tab.id))));
                }
                self.side_hits.push((b, SideHit::Strip(StripHit::Session(tab.id))));
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
            let dot = folded.iter().any(|tab| tab.asks || tab.done);
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
    /// what the tabs list (the project, or the orchestrators): where, its label, its colour
    heading: Option<(Rect, String, Color)>,
    /// the scope's All, once it has two sessions: where, and its label
    grid: Option<(Rect, String)>,
    /// the first session tab's column (after the rule), when there's room for any
    start: Option<u16>,
}

/// The session tabs, laid out (see [`App::lay_tabs`]).
struct Laid {
    /// the most columns a name takes
    cap: usize,
    /// each tab shown: its index, where
    tabs: Vec<(usize, Rect)>,
    /// the arrows (scroll) or "+N" (stacked out of rows): where, what a click does, the tabs past it
    arrows: Vec<(Rect, StripHit, std::ops::Range<usize>)>,
    rows: u16,
}

/// Each tab's name width and the room around it: " ◐ name ● × ".
fn dims(tabs: &[SessionTabInfo]) -> (Vec<usize>, Vec<usize>) {
    tabs.iter().map(|tab| (ui::width(&tab.label), 3 + if tab.asks || tab.done { 2 } else { 0 } + 3)).unzip()
}

/// What a tab's agent is doing, as one glyph: a spinner while it works, else a ring.
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

/// " ●" for a session waiting on you, " ✓" for one that finished unseen: shape says which, so they read without
/// color too.
fn badge(tab: &SessionTabInfo, base: Style, on: bool, t: &Theme) -> Option<Span<'static>> {
    let tone = |c: Color| if on { base } else { base.fg(c).add_modifier(Modifier::BOLD) };
    if tab.asks {
        Some(Span::styled(" ●", tone(t.danger)))
    } else if tab.done {
        Some(Span::styled(" ✓", tone(t.good)))
    } else {
        None
    }
}

/// A tab's width with its name (`name` columns) cut to `cap` and `extra` around it, never under [`TAB_MIN`].
fn tab_width(name: usize, extra: usize, cap: usize) -> u16 {
    ((name.min(cap) + extra) as u16).max(TAB_MIN)
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
        // a 50 wide first row after All, 100 wide rows under it, tabs 30 wide a gap of 2 apart
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

    #[test]
    fn a_tab_keeps_its_least_width_and_names_cut_to_the_cap() {
        assert_eq!(tab_width(3, 6, MAX_NAME), TAB_MIN);
        assert_eq!(tab_width(40, 8, 20), 28);
    }
}
