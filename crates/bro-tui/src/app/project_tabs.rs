//! The project tabs (a setting): a strip across the top of the sessions, beside the sidebar rather than in it.
//! All tiles every session the way the tile-all shortcut does; the orchestrator, when one runs, has its own ★ tab;
//! each project with running sessions has a tab in its own color that tiles just that project's sessions.
//!
//! A tab's first glyph is its color and what its agents are doing (◐ working, ○ idle); after the name come its
//! session count and what wants you, each its own shape (● waiting on you, ✓ finished unseen). Tabs that don't
//! fit fold into "+N" on either side of the one showing; the strip's right end holds the minimized sessions'
//! switch and +, a new session. A tab under the mouse shows a × that closes its project (after a confirm).

use super::{App, SideHit};
use crate::alerts::Kind;
use crate::pane::{Activity, PaneId, Place};
use crate::sidebar::ProjectTab;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
};
use std::time::{Duration, Instant};

/// A project's name never shrinks below this many columns to make room for the others…
const MIN_NAME: usize = 6;
/// …nor takes more than this many, so one long name can't push the others (or the showing tab) off.
const MAX_NAME: usize = 24;
/// Room a "+N" fold takes.
const MORE_W: u16 = 6;
/// A trackpad's burst of wheel events switches one tab.
const WHEEL_GAP: Duration = Duration::from_millis(150);

/// A tab.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StripTab {
    /// every session tiled
    All,
    /// bro's orchestrator, on its own
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
    /// show / hide the minimized sessions
    Minimized,
    /// a new session in the current project
    New,
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
        if self.orchestrator_pane().is_some() {
            v.push(StripTab::Orchestrator);
        }
        v.extend(tabs.iter().map(|t| StripTab::Project(t.key.clone())));
        v
    }

    /// The tab that matches what's on screen: the grid you tiled, the orchestrator alone, or a project whose
    /// every session is on screen and nothing else. None for anything else (a mix you stacked, a view).
    pub(crate) fn showing_tab(&self) -> Option<StripTab> {
        if self.tile_all {
            return Some(self.tile_project.clone().map_or(StripTab::All, StripTab::Project));
        }
        if self.stacked() {
            return None;
        }
        let vis: Vec<PaneId> = self.visible().into_iter().filter(|id| self.panes.get(id).is_some_and(|p| p.is_terminal())).collect();
        let key_of = |id: &PaneId| self.panes.get(id).and_then(|p| p.as_term_ref()).map(|t| (t.meta.orchestrator, t.meta.project.key.clone()));
        let (orchestrator, key) = key_of(vis.first()?)?;
        if orchestrator {
            return (vis.len() == 1).then_some(StripTab::Orchestrator);
        }
        let only_it = vis.iter().all(|id| key_of(id).is_some_and(|(o, k)| !o && k == key));
        let all_of_it = self.tile_ids(Some(&key)).iter().all(|id| vis.contains(id));
        (only_it && all_of_it).then_some(StripTab::Project(key))
    }

    /// Show `tab` (nothing changes when it's showing already).
    pub(crate) fn show_tab(&mut self, tab: StripTab) {
        if self.showing_tab().as_ref() == Some(&tab) {
            self.leave_sidebar();
            return;
        }
        match tab {
            StripTab::All => self.toggle_tiles(),
            StripTab::Orchestrator => match self.orchestrator_pane() {
                Some(id) => {
                    self.clear_stack();
                    self.focus_pane(id);
                    self.leave_sidebar();
                }
                None => {
                    let key = self.keymap.primary(crate::keymap::Act::Orchestrator);
                    self.toast(Kind::Info, format!("the orchestrator isn't running · {key} starts it"));
                }
            },
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
        if tab == StripTab::All && self.showing_tab() == Some(StripTab::All) {
            self.toggle_tiles();
        } else {
            self.show_tab(tab);
        }
    }

    /// The next (`d` = 1) or previous tab, wrapping; the orchestrator's only with `orchestrator`.
    pub(crate) fn step_tab(&mut self, d: i32, orchestrator: bool) {
        let tabs = self.project_tabs();
        let items: Vec<StripTab> = self.strip_items(&tabs).into_iter().filter(|t| orchestrator || *t != StripTab::Orchestrator).collect();
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

    /// The wheel over the strip: the next or previous tab, once per burst.
    pub(crate) fn strip_wheel(&mut self, down: bool) {
        if self.strip_wheel.is_some_and(|at| at.elapsed() < WHEEL_GAP) {
            return;
        }
        self.strip_wheel = Some(Instant::now());
        self.step_tab(if down { 1 } else { -1 }, true);
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

    /// The strip at `r`: All, the orchestrator, a rule, then a tab per project; the minimized switch and + on
    /// the right. Names shorten before tabs fold into "+N" around the one showing.
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

        // the right end: the minimized sessions' switch, then + (a new session)
        let mut end = r.right();
        if r.width >= 40 {
            end -= 3;
            let b = Rect { x: end, width: 3, ..r };
            let style = if b.contains(hover) { ui::band_style(t.accent, false, &t).add_modifier(Modifier::BOLD) } else { ui::bold_accent(&t) };
            ui::line(f, b, vec![Span::styled(" + ", style)]);
            self.side_hits.push((b, SideHit::Strip(StripHit::New)));
        }
        if self.tile_all && !self.minimized.is_empty() {
            let text = format!(" {} {} minimized ", if self.show_minimized { "▾" } else { "▸" }, self.minimized.len());
            let w = ui::width(&text) as u16;
            if end >= r.x + w + 30 {
                end -= w + 1;
                let b = Rect { x: end, width: w, ..r };
                let style = if b.contains(hover) { ui::band_style(t.accent, false, &t) } else if self.show_minimized { ui::accent(&t) } else { muted(&t) };
                ui::line(f, b, vec![Span::styled(text, style)]);
                self.side_hits.push((b, SideHit::Strip(StripHit::Minimized)));
            }
        }
        let end = end.saturating_sub(1);

        let mut x = r.x;
        let put = |app: &mut App, f: &mut Frame, x: &mut u16, tab: StripTab, spans: Vec<Span<'static>>| {
            let w = spans.iter().map(|s| ui::width(&s.content) as u16).sum::<u16>().min(end.saturating_sub(*x));
            if w == 0 {
                return;
            }
            let b = Rect { x: *x, width: w, ..r };
            ui::line(f, b, spans);
            app.side_hits.push((b, SideHit::Strip(StripHit::Tab(tab))));
            *x += w;
        };

        // All: every session the grid would show
        let on = showing == Some(StripTab::All);
        let all_w = ui::width(&format!(" ▦ All {} ", self.tile_ids(None).len())) as u16;
        let style = tab_style(t.accent, on, Rect { x, width: all_w, ..r }.contains(hover), selected == Some(StripTab::All), &t).add_modifier(Modifier::BOLD);
        put(self, f, &mut x, StripTab::All, vec![Span::styled(format!(" ▦ All {} ", self.tile_ids(None).len()), style)]);

        // the orchestrator: gold, with what it's doing and whether it wants you
        if let Some(id) = self.orchestrator_pane() {
            let gold = crate::theme::gold(&t);
            let activity = self.panes.get(&id).and_then(|p| p.activity());
            let (blocked, done) = (usize::from(activity == Some(Activity::Blocked)), usize::from(self.done.contains(&id) && activity != Some(Activity::Blocked)));
            let on = showing == Some(StripTab::Orchestrator);
            let w = 5 + badges_width(blocked, done) as u16;
            let base = tab_style(gold, on, Rect { x: x + 1, width: w, ..r }.contains(hover), selected == Some(StripTab::Orchestrator), &t);
            let mut spans = vec![Span::styled(" ★ ", base.add_modifier(Modifier::BOLD)), Span::styled(activity_glyph(activity, time), glyph_style(base, gold, on))];
            spans.extend(badges(blocked, done, base, on, &t));
            spans.push(Span::styled(" ", base));
            x += 1;
            put(self, f, &mut x, StripTab::Orchestrator, spans);
        }
        if tabs.is_empty() || end.saturating_sub(x) < 3 {
            return;
        }
        ui::line(f, Rect { x, width: 3, ..r }, vec![Span::styled(" │ ", fg(t.frame))]);
        x += 3;

        // the widest names that let every tab fit
        let room = end.saturating_sub(x);
        let total = |cap: usize| tabs.iter().map(|tab| tab_width(tab, cap) + 1).sum::<u16>();
        let mut cap = tabs.iter().map(|tab| ui::width(&tab.name)).max().unwrap_or(0).min(MAX_NAME);
        while cap > MIN_NAME && total(cap) > room {
            cap -= 1;
        }
        // the tabs that fit around the selected (else the showing) one; the rest fold into "+N"
        let widths: Vec<u16> = tabs.iter().map(|tab| tab_width(tab, cap)).collect();
        let fits = |lo: usize, hi: usize| {
            let tabs_w: u16 = widths[lo..hi].iter().map(|w| w + 1).sum();
            tabs_w + if lo > 0 { MORE_W } else { 0 } + if hi < widths.len() { MORE_W } else { 0 } <= room
        };
        let key_at = |tab: &Option<StripTab>| match tab {
            Some(StripTab::Project(k)) => tabs.iter().position(|t| t.key == *k),
            _ => None,
        };
        let anchor = key_at(&selected).or_else(|| key_at(&showing)).unwrap_or(0);
        let (mut lo, mut hi) = (anchor, anchor + 1);
        loop {
            let mut grew = false;
            if hi < tabs.len() && fits(lo, hi + 1) {
                hi += 1;
                grew = true;
            }
            if lo > 0 && fits(lo - 1, hi) {
                lo -= 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        let fold = |app: &mut App, f: &mut Frame, x: &mut u16, folded: &[ProjectTab], left: bool| {
            let dot = folded.iter().any(ProjectTab::attention);
            let n = folded.len();
            let mut spans = vec![Span::styled(if left { format!(" ◂{n}") } else { format!(" +{n}") }, muted(&t))];
            if dot {
                spans.push(Span::styled("●", fg(t.danger)));
            }
            spans.push(Span::styled(if left { " " } else { "▸ " }, muted(&t)));
            let w = spans.iter().map(|s| ui::width(&s.content) as u16).sum::<u16>().min(end.saturating_sub(*x));
            let b = Rect { x: *x, width: w, ..r };
            if b.contains(hover) {
                f.buffer_mut().set_style(b, Style::default().bg(crate::theme::project_tint(t.accent, &t)));
            }
            ui::line(f, b, spans);
            app.side_hits.push((b, SideHit::Strip(StripHit::More)));
            *x += w;
        };
        if lo > 0 {
            fold(self, f, &mut x, &tabs[..lo], true);
        }
        for tab in &tabs[lo..hi] {
            let key = StripTab::Project(tab.key.clone());
            let color = self.project_color(&tab.key);
            let on = showing.as_ref() == Some(&key);
            let w = tab_width(tab, cap);
            let hot = Rect { x, width: w, ..r }.contains(hover);
            let picked = selected.as_ref() == Some(&key);
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
            // the × shows under the mouse (or the keys, where delete closes it); its room is always kept so the
            // strip doesn't shift as you move along it
            let close = Rect { x: x + w - 3, width: 2, ..r };
            let close_hot = close.contains(hover);
            let close_style = if close_hot { base.fg(t.danger).add_modifier(Modifier::BOLD) } else if on { base } else { base.fg(t.muted) };
            spans.push(Span::styled(" ", base));
            spans.push(Span::styled(if hot || picked { "×" } else { " " }, close_style));
            spans.push(Span::styled(" ", base));
            if close.right() <= end {
                self.side_hits.push((close, SideHit::Strip(StripHit::Close(tab.key.clone()))));
            }
            put(self, f, &mut x, key, spans);
            x += 1;
        }
        if hi < tabs.len() {
            fold(self, f, &mut x, &tabs[hi..], false);
        }
    }
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

/// A project tab's width with its name cut to `cap` columns: " ◐ name 3 ●1 ✓1 × ".
fn tab_width(tab: &ProjectTab, cap: usize) -> u16 {
    (3 + ui::width(&tab.name).min(cap) + 1 + tab.live.to_string().len() + badges_width(tab.blocked, tab.done) + 3) as u16
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
