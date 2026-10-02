//! The workspace: tabs of split panes, the project sidebar, overlays, and the event loop.
//!
//! Event-driven (oriel's loop): sleep until input, pane output, a service update or the next deadline a
//! visible pane / animation asked for; coalesce bursts for ~12 ms; redraw once.
//!
//! Split by concern: `input` (keys), `commands` (what actions do), `mouse`, `draw` (+ `side_view`,
//! `welcome`), `sessions` (launch results, bridge commands, agent tracking), `overlays`.

mod commands;
mod draw;
mod input;
mod mouse;
mod overlays;
mod sessions;
mod side_view;
mod welcome;

use crate::alerts::{Kind, Toasts};
use crate::keymap::Keymap;
use crate::layout::{Dir, Node, PaneId};
use crate::pane::{Action, Activity, Cx, Event, Pane, Place};
use crate::recents::Recent;
use crate::services::Services;
use crate::sidebar::SideState;
use crate::theme::{self, Theme};
use crate::ui;
use ratatui::{
    DefaultTerminal,
    layout::{Position, Rect},
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub use overlays::Overlay;

/// One tab: a split tree of panes.
pub(crate) struct Tab {
    pub root: Node,
    pub focus: PaneId,
    pub zoom: bool,
}

/// A mouse selection inside one pane.
#[derive(Clone, Copy)]
pub(crate) struct Sel {
    pub pane: PaneId,
    pub area: Rect,
    pub a: Position,
    pub b: Position,
    pub active: bool,
}

impl Sel {
    /// start and end in reading order
    fn ordered(&self) -> (Position, Position) {
        if (self.a.y, self.a.x) <= (self.b.y, self.b.x) { (self.a, self.b) } else { (self.b, self.a) }
    }
    fn contains(&self, x: u16, y: u16) -> bool {
        let (s, e) = self.ordered();
        if y < s.y || y > e.y || x < self.area.x || x >= self.area.right() {
            return false;
        }
        !(y == s.y && x < s.x) && !(y == e.y && x > e.x)
    }
}

/// Clickable things in the sidebar.
#[derive(Clone, Debug)]
pub(crate) enum SideHit {
    /// index into the current sidebar rows
    Row(usize),
    Usage,
    /// Start a session from a harness total or a specific usage account.
    LaunchUsage(bro_core::Harness, Option<String>),
    UsageToggle,
    /// the × on a running session's row
    Close(PaneId),
    /// the × on a project row: (root, running sessions)
    CloseProject(std::path::PathBuf, usize),
    /// the disclosure arrow beside the Claude usage total
    Fable,
    Proxy,
    Bridge,
    /// the new-session strip's icons: claude / codex, or a terminal (None)
    Quick(Option<bro_core::Harness>),
    /// the new-session strip's "+" and key hint: the launcher
    Launcher,
    TileAll,
}

/// Startup options.
#[derive(Clone, Copy, Default)]
pub struct Opts {
    /// `bro --demo`: open the demo sessions
    pub demo: bool,
    /// demo sessions are fixed transcripts instead of shells (snapshots)
    pub fixed_demo: bool,
    /// read ~/.bro/v2-recents.json
    pub load_recents: bool,
}

/// The app.
pub struct App {
    pub(crate) panes: HashMap<PaneId, Box<dyn Pane>>,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) cur: usize,
    next_id: PaneId,
    /// launch order per pane (sidebar grouping)
    seqs: HashMap<PaneId, u64>,
    pub(crate) theme: Theme,
    project_colors: HashMap<String, usize>,
    pub(crate) keymap: Keymap,
    pub(crate) svc: Services,
    tx: Sender<Event>,
    pub(crate) toasts: Toasts,
    pub(crate) prefix_armed: bool,
    pub(crate) overlay: Overlay,
    quit: bool,
    start: Instant,
    last_tick: HashMap<PaneId, Instant>,
    // geometry from the last draw, for the mouse
    inner: Vec<(PaneId, Rect)>,
    outer: Vec<(PaneId, Rect)>,
    body: Rect,
    side_hits: Vec<(Rect, SideHit)>,
    pane_close: Vec<(Rect, PaneId)>,
    drag: Option<(Vec<bool>, Dir, Rect)>,
    side_area: Rect,
    side_drag: bool,
    sidebar_width: Option<u16>,
    side_edge_click: Option<Instant>,
    sel: Option<Sel>,
    copy_pending: bool,
    /// Cancel a delayed clipboard read when the user changes the input target.
    clipboard_request: Option<Instant>,
    hover: Position,
    last_click: Option<(Instant, u16, u16)>,
    /// The confirmation dialog's buttons as last drawn: (area, is_yes).
    confirm_hits: Vec<(Rect, bool)>,
    /// The last left click bro kept for itself on a pane, for double-click to full screen.
    last_pane_click: Option<(Instant, PaneId, Position)>,
    /// Open only on release at the same cell, so dragging a link still selects text.
    link_press: Option<(PaneId, Position, String)>,
    // sidebar
    pub(crate) sidebar: bool,
    pub(crate) side_focus: bool,
    /// sidebar usage block: every profile (true) or the Claude / Codex totals
    pub(crate) usage_expanded: bool,
    /// the Fable line under the Claude usage total
    pub(crate) show_fable: bool,
    pub(crate) side_sel: usize,
    pub(crate) side: SideState,
    pub(crate) side_filtering: bool,
    pub(crate) renaming: Option<(PaneId, String)>,
    side_scroll: usize,
    // agents
    agent_state: HashMap<PaneId, Activity>,
    pub(crate) done: HashSet<PaneId>,
    pub(crate) recents: Vec<Recent>,
    /// archived earlier sessions (hidden from the sidebar)
    pub(crate) archive: crate::archive::Archive,
    /// write ~/.bro files (off in demo and tests)
    pub(crate) persist: bool,
    /// the projects in the sidebar (you open them; ~/.bro/v2-projects.json)
    pub(crate) open_projects: crate::projects::OpenProjects,
    /// the project new sessions start in (last one you picked in the sidebar or worked in)
    pub(crate) cur_project: Option<std::path::PathBuf>,
    /// the pane focus was on when `cur_project` last followed it
    last_focus: Option<PaneId>,
    /// sessions shown together (shift+click in the sidebar); 2+ = stacked view
    pub(crate) stack: Vec<PaneId>,
    pub(crate) stack_focus: Option<PaneId>,
    /// Keep newly opened sessions in the all-sessions grid.
    tile_all: bool,
    /// The grid shows only this project's sessions (click a project in the sidebar).
    pub(crate) tile_project: Option<String>,
    stack_zoom: bool,
    term_focused: bool,
    opts: Opts,
    _theme_watcher: Option<notify::RecommendedWatcher>,
}

impl App {
    pub fn new(svc: Services, tx: Sender<Event>, opts: Opts) -> App {
        let settings = svc.settings();
        let theme = theme::get(&settings.theme);
        ui::NERD.store(settings.nerd_font && std::env::var_os("BRO_PLAIN").is_none(), std::sync::atomic::Ordering::Relaxed);
        let keymap = Keymap::new(&settings.prefix, &settings.keys);
        let mut app = App {
            panes: HashMap::new(),
            tabs: vec![],
            cur: 0,
            next_id: 1,
            seqs: HashMap::new(),
            theme,
            project_colors: HashMap::new(),
            keymap,
            svc,
            tx: tx.clone(),
            toasts: Toasts::default(),
            prefix_armed: false,
            overlay: Overlay::None,
            quit: false,
            start: Instant::now(),
            last_tick: HashMap::new(),
            inner: vec![],
            outer: vec![],
            body: Rect::default(),
            side_hits: vec![],
            pane_close: vec![],
            drag: None,
            side_area: Rect::default(),
            side_drag: false,
            sidebar_width: settings.sidebar_width,
            side_edge_click: None,
            sel: None,
            copy_pending: false,
            clipboard_request: None,
            hover: Position { x: u16::MAX, y: u16::MAX },
            last_click: None,
            last_pane_click: None,
            confirm_hits: vec![],
            link_press: None,
            sidebar: true,
            side_focus: false,
            usage_expanded: settings.usage_expanded,
            show_fable: false,
            side_sel: 0,
            side: SideState::default(),
            side_filtering: false,
            renaming: None,
            side_scroll: 0,
            agent_state: HashMap::new(),
            done: HashSet::new(),
            recents: if opts.load_recents { crate::recents::load() } else { vec![] },
            archive: if opts.load_recents { crate::archive::Archive::load() } else { Default::default() },
            persist: opts.load_recents,
            open_projects: if opts.load_recents { crate::projects::OpenProjects::load() } else { Default::default() },
            cur_project: None,
            last_focus: None,
            stack: vec![],
            stack_focus: None,
            tile_all: false,
            tile_project: None,
            stack_zoom: false,
            term_focused: true,
            opts,
            _theme_watcher: None,
        };
        app.seed_projects();
        let t2 = tx.clone();
        app._theme_watcher = theme::watch(move || {
            let _ = t2.send(Event::ThemeFilesChanged);
        });
        for e in app.keymap.errors.clone() {
            app.toast(Kind::Error, e);
        }
        if opts.demo {
            app.open_demo_sessions();
        }
        app
    }

    // ------------------------------------------------------------------ panes & tabs

    fn add(&mut self, p: Box<dyn Pane>) -> PaneId {
        let id = self.next_id;
        self.next_id += 1;
        self.seqs.insert(id, id);
        self.panes.insert(id, p);
        id
    }

    pub(crate) fn new_tab(&mut self, p: Box<dyn Pane>) -> PaneId {
        let id = self.add(p);
        self.tabs.push(Tab { root: Node::Leaf(id), focus: id, zoom: false });
        self.cur = self.tabs.len() - 1;
        self.include_in_tiles(id);
        id
    }

    /// The focused pane, if any tab is open.
    pub(crate) fn focused(&self) -> Option<PaneId> {
        if self.stacked() {
            return self.stack_focus.filter(|f| self.stack.contains(f)).or_else(|| self.stack.first().copied());
        }
        self.tabs.get(self.cur).map(|t| t.focus)
    }

    /// Showing several sessions at once (shift+click in the sidebar).
    pub(crate) fn stacked(&self) -> bool {
        self.stack.len() >= 2
    }

    fn clear_stack(&mut self) {
        self.stack.clear();
        self.stack_focus = None;
        self.stack_zoom = false;
        self.tile_all = false;
        self.tile_project = None;
    }

    /// Live sessions in sidebar order: all of them, or one project's.
    fn tile_ids(&self, project: Option<&str>) -> Vec<PaneId> {
        let live = self.live_infos();
        let ids = crate::sidebar::live_order(&live, &self.past_infos(), &self.open_infos());
        match project {
            Some(key) => ids.into_iter().filter(|id| live.iter().any(|l| l.pane == *id && l.project_key == key)).collect(),
            None => ids,
        }
    }

    fn include_in_tiles(&mut self, id: PaneId) {
        if self.tile_all && self.panes.get(&id).is_some_and(|p| p.is_terminal()) {
            let mut ids = self.tile_ids(self.tile_project.as_deref());
            if !ids.contains(&id) {
                // a session in another project: widen the grid so it shows
                self.tile_project = None;
                ids = self.tile_ids(None);
            }
            self.stack = ids;
            self.stack_focus = Some(id);
            self.stack_zoom = false;
        } else {
            self.clear_stack();
        }
    }

    fn toggle_tiles(&mut self) {
        let focus = self.focused();
        if self.tile_all && self.tile_project.is_none() {
            self.clear_stack();
            if let Some(id) = focus {
                self.focus_pane(id);
            }
        } else {
            // from one session or one project's grid: every session
            let ids = self.tile_ids(None);
            if ids.len() < 2 {
                self.toast(Kind::Info, "open two sessions to tile them");
                return;
            }
            self.stack_focus = focus.filter(|id| ids.contains(id)).or_else(|| ids.first().copied());
            self.stack = ids;
            self.tile_all = true;
            self.tile_project = None;
            self.stack_zoom = false;
        }
        self.side_focus = false;
    }

    /// Click / ⏎ on a project: tile only its sessions; again: back to every session.
    pub(crate) fn toggle_project_tiles(&mut self, key: &str) {
        if self.tile_project.as_deref() == Some(key) {
            self.toggle_tiles();
            return;
        }
        let ids = self.tile_ids(Some(key));
        let Some(&first) = ids.first() else { return };
        let focus = self.focused().filter(|id| ids.contains(id)).unwrap_or(first);
        self.clear_stack();
        self.focus_pane(focus);
        if ids.len() >= 2 {
            self.stack = ids;
            self.stack_focus = Some(focus);
            self.tile_all = true;
        }
        self.tile_project = Some(key.to_string());
        self.side_focus = false;
    }

    /// shift+click / shift+⏎ on a session: add it to the stacked view, or take it out again.
    pub(crate) fn toggle_stack(&mut self, id: PaneId) {
        if !self.panes.contains_key(&id) {
            return;
        }
        self.tile_all = false;
        self.tile_project = None;
        self.stack_zoom = false;
        if self.stack.is_empty()
            && let Some(cur) = self.focused()
            && cur != id
            && self.panes.get(&cur).is_some_and(|p| p.is_terminal())
        {
            self.stack.push(cur);
        }
        if let Some(i) = self.stack.iter().position(|x| *x == id) {
            self.stack.remove(i);
            if self.stack_focus == Some(id) {
                self.stack_focus = self.stack.last().copied();
            }
        } else {
            self.stack.push(id);
            self.stack_focus = Some(id);
        }
        if self.stack.len() < 2 {
            let rest = self.stack.pop();
            self.stack_focus = None;
            if let Some(r) = rest.or(Some(id)).filter(|r| self.panes.contains_key(r)) {
                self.focus_pane(r);
            }
        }
        self.side_focus = false;
    }

    /// Open a pane at `place` relative to the focused pane; returns its id.
    pub(crate) fn open(&mut self, p: Box<dyn Pane>, place: Place) -> PaneId {
        let Some(from) = self.focused() else { return self.new_tab(p) };
        let dir = match place {
            Place::Tab => return self.new_tab(p),
            Place::SplitRight => Dir::Right,
            Place::SplitDown => Dir::Down,
            Place::Split => {
                // along the longer side (cells are ~2x taller than wide)
                let r = self.outer.iter().find(|(i, _)| *i == from).map(|x| x.1).unwrap_or(self.body);
                if r.width as f32 > r.height as f32 * 2.2 { Dir::Right } else { Dir::Down }
            }
        };
        let id = self.add(p);
        // The focused tile may belong to a different tab from the last single-session view.
        if let Some(i) = self.tabs.iter().position(|t| t.root.contains(from)) {
            self.cur = i;
        }
        let tab = &mut self.tabs[self.cur];
        if !tab.root.split(from, id, dir) {
            tab.root = Node::Split { dir, ratio: 0.5, a: Box::new(tab.root.clone()), b: Box::new(Node::Leaf(id)) };
        }
        tab.focus = id;
        tab.zoom = false;
        self.include_in_tiles(id);
        id
    }

    /// Close a pane: end its session (bridge, proxy route, staged files) and drop it from its tab.
    pub(crate) fn close(&mut self, id: PaneId) {
        let mut remaining = None;
        if self.stack.contains(&id) {
            self.stack.retain(|x| *x != id);
            if self.stack_focus == Some(id) {
                self.stack_focus = self.stack.last().copied();
            }
            if self.stack.len() < 2 {
                remaining = self.stack.first().copied();
                self.clear_stack();
            }
        }
        if let Some(mut p) = self.panes.remove(&id) {
            if let Some(t) = p.as_term() {
                t.kill();
                t.on_exit();
                // route ids are stable per (upstream, model): keep the route while another session uses it
                if let Some(r) = t.meta.route_id.take() {
                    let shared = self.panes.values().any(|o| o.as_term_ref().is_some_and(|o| o.meta.route_id.as_deref() == Some(r.as_str())));
                    if !shared {
                        self.svc.remove_route(&r);
                    }
                }
                let files = std::mem::take(&mut t.meta.cleanup);
                if !files.is_empty() {
                    std::thread::spawn(move || {
                        if crate::services::guard("sessions::cleanup_staged", || bro_core::sessions::cleanup_staged(&files)).is_err() {
                            for f in files.iter().rev() {
                                let _ = if f.is_dir() { std::fs::remove_dir_all(f) } else { std::fs::remove_file(f) };
                            }
                        }
                    });
                }
            }
            drop(p);
        }
        self.last_tick.remove(&id);
        self.seqs.remove(&id);
        self.done.remove(&id);
        if let Some(ti) = self.tabs.iter().position(|t| t.root.contains(id)) {
            let t = &mut self.tabs[ti];
            if t.root.remove(id) {
                if t.focus == id {
                    t.focus = t.root.leaf_ids()[0];
                }
                t.zoom = false;
            } else {
                self.tabs.remove(ti);
                if self.cur >= ti && self.cur > 0 {
                    self.cur -= 1;
                }
                self.cur = self.cur.min(self.tabs.len().saturating_sub(1));
            }
        }
        if let Some(id) = remaining {
            self.focus_pane(id);
        }
    }

    /// Switch to the tab holding `id` and focus it (inside a stack, just move the focus there).
    pub(crate) fn focus_pane(&mut self, id: PaneId) {
        if self.stacked() && self.stack.contains(&id) {
            self.stack_focus = Some(id);
            self.done.remove(&id);
            return;
        }
        self.clear_stack();
        if let Some(i) = self.tabs.iter().position(|t| t.root.contains(id)) {
            self.cur = i;
            let t = &mut self.tabs[i];
            if t.zoom && t.focus != id {
                t.zoom = false;
            }
            t.focus = id;
            self.done.remove(&id);
        }
    }

    /// Panes on screen now.
    pub(crate) fn visible(&self) -> Vec<PaneId> {
        if self.stacked() {
            return if self.stack_zoom { self.focused().into_iter().collect() } else { self.stack.clone() };
        }
        match self.tabs.get(self.cur) {
            Some(t) if t.zoom => vec![t.focus],
            Some(t) => t.root.leaf_ids(),
            None => vec![],
        }
    }

    pub(crate) fn toast(&mut self, kind: Kind, s: impl Into<String>) {
        self.toasts.push(kind, s);
    }

    /// Keep notifications inside bro, without an additional OS popup.
    pub(crate) fn raise(&mut self, kind: Kind, text: String) {
        self.toast(kind, text);
    }

    pub(crate) fn set_theme(&mut self, name: &str, save: bool) {
        self.theme = theme::get(name);
        crate::icons::retint(self.theme.is_light());
        if save {
            let mut s = self.svc.settings();
            s.theme = name.to_string();
            self.svc.save_settings(s);
        }
    }

    /// Run `f` on pane `id` with a Cx, then apply the actions it asked for.
    pub(crate) fn with_pane<R>(&mut self, id: PaneId, f: impl FnOnce(&mut dyn Pane, &mut Cx) -> R) -> Option<R> {
        let mut actions = vec![];
        let focused = self.focused() == Some(id);
        let r = {
            let p = self.panes.get_mut(&id)?;
            let mut cx = Cx { id, theme: &self.theme, svc: &self.svc, tx: &self.tx, actions: &mut actions, focused };
            f(p.as_mut(), &mut cx)
        };
        self.apply(id, actions);
        Some(r)
    }

    pub(crate) fn apply(&mut self, from: PaneId, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Open(p, place) => {
                    self.open(p, place);
                }
                Action::Close => self.close(from),
                Action::Toast(k, s) => self.toast(k, s),
                Action::LaunchUsage(h, profile) => self.launch_usage(h, profile),
            }
        }
    }

    // ------------------------------------------------------------------ loop

    pub fn run(&mut self, term: &mut DefaultTerminal, rx: Receiver<Event>) -> anyhow::Result<()> {
        crate::icons::init(&self.svc.settings().icons, self.theme.is_light());
        loop {
            term.draw(|f| {
                self.draw(f);
                crate::icons::place(f.buffer_mut());
            })?;
            crate::icons::flush(term)?;
            let timeout = self.next_deadline();
            let ev = match rx.recv_timeout(timeout) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => Event::Tick,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            self.handle(ev);
            // coalesce a burst (a terminal spewing output) into one redraw
            let burst = Instant::now();
            while let Ok(e) = rx.try_recv() {
                self.handle(e);
                if burst.elapsed() > Duration::from_millis(12) {
                    break;
                }
            }
            self.after_events();
            if self.quit {
                break;
            }
        }
        self.shutdown();
        Ok(())
    }

    /// Housekeeping after each batch of events.
    pub(crate) fn after_events(&mut self) {
        self.follow_focus();
        let focused = self.focused().and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone());
        self.svc.set_focused_session(focused.as_deref());
        self.reap();
        self.track_agents();
        self.toasts.expire();
    }

    /// Moving focus onto a session (click, keys, tab switch) makes its project the current one, so new sessions
    /// (the usage panel's, the strip's) start where you are; a view pane keeps the last session's project.
    pub(crate) fn follow_focus(&mut self) {
        let focused = self.focused();
        if focused == self.last_focus {
            return;
        }
        self.last_focus = focused;
        if let Some(t) = focused.and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()) {
            self.cur_project = Some(t.meta.project.root.clone());
        }
    }

    /// Make `root` current by choice (sidebar, folder picker): it holds until focus moves to another session.
    pub(crate) fn pick_project(&mut self, root: std::path::PathBuf) {
        self.cur_project = Some(root);
        self.last_focus = self.focused();
    }

    /// Kill every session and stop the services.
    pub fn shutdown(&mut self) {
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            self.close(id);
        }
        self.svc.shutdown();
    }

    /// Sleep until the soonest thing that needs a redraw without an event.
    fn next_deadline(&self) -> Duration {
        let mut d = Duration::from_secs(30);
        let spinning = self.panes.values().any(|p| p.activity() == Some(Activity::Working));
        // the rainbow only plays on the welcome logo
        if (self.theme.animated && self.tabs.is_empty()) || spinning {
            d = d.min(Duration::from_millis(125));
        }
        if let Some(t) = self.toasts.next_expiry() {
            d = d.min(t + Duration::from_millis(10));
        }
        // terminals tick even when hidden, so agents in other tabs still turn "done"
        let ids: Vec<PaneId> = self.panes.iter().filter(|(id, p)| p.is_terminal() || self.visible().contains(id)).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(every) = self.panes.get(&id).and_then(|p| p.tick_every()) {
                let last = self.last_tick.get(&id).copied().unwrap_or(self.start);
                d = d.min(every.saturating_sub(last.elapsed()));
            }
        }
        d.max(Duration::from_millis(4))
    }

    pub(crate) fn handle(&mut self, ev: Event) {
        use crossterm::event::{Event as CEvent, KeyEventKind};
        match ev {
            Event::Input(CEvent::Key(k)) if k.kind != KeyEventKind::Release => self.key(k),
            Event::Input(CEvent::Mouse(m)) => self.mouse(m),
            Event::Input(CEvent::Paste(s)) => self.paste(&s),
            Event::Input(CEvent::FocusGained) => self.term_focused = true,
            Event::Input(CEvent::FocusLost) => {
                self.term_focused = false;
                if self.side_drag {
                    self.side_drag = false;
                    self.save_sidebar_width();
                }
                self.drag = None;
            }
            Event::Input(_) => {}
            Event::Wake(id) => {
                self.with_pane(id, |p, cx| p.poll(cx));
            }
            Event::Tick => {
                let ids: Vec<PaneId> = self.panes.keys().copied().collect();
                for id in ids {
                    let due = match self.panes.get(&id).and_then(|p| p.tick_every()) {
                        Some(every) => self.last_tick.get(&id).map(|t| t.elapsed() >= every).unwrap_or(true),
                        None => false,
                    };
                    if due {
                        self.last_tick.insert(id, Instant::now());
                        self.with_pane(id, |p, cx| p.poll(cx));
                    }
                }
            }
            Event::Services => {
                if let Overlay::Launcher(l) = &self.overlay {
                    let data = overlays::launcher_data_with_installed(self, l.data.installed.clone());
                    if let Overlay::Launcher(l) = &mut self.overlay {
                        l.set_data(data);
                    }
                }
                // new data (e.g. the re-scan the continue picker asked for): refresh its list in place
                if matches!(self.overlay, Overlay::Continue(_)) {
                    self.refresh_continue();
                }
            }
            Event::Launched(l) => self.launched(*l),
            Event::Bridge(c) => self.bridge_command(c),
            Event::BridgeStarted => {
                for p in self.panes.values() {
                    if let Some(t) = p.as_term_ref() {
                        t.reregister();
                    }
                }
            }
            Event::Toast(k, s) => self.raise(k, s),
            Event::OpenLink(uri) => {
                if !cfg!(test) {
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        if let Err(error) = crate::panes::links::open(&uri) {
                            let _ = tx.send(Event::Toast(Kind::Error, error));
                        }
                    });
                }
            }
            Event::ClipboardText(request, text) => {
                if self.clipboard_request == Some(request) {
                    self.clipboard_request = None;
                    if let Some(text) = text {
                        self.paste(&text);
                    }
                }
            }
            Event::Clipboard(id, got, key) => match got {
                Some(paths) => {
                    let text = crate::clip::paste_form(&paths);
                    self.with_pane(id, |p, cx| p.paste(&text, cx));
                    let what = if paths.len() == 1 && paths[0].contains("paste-") { "image".to_string() } else { format!("{} file{}", paths.len(), if paths.len() == 1 { "" } else { "s" }) };
                    self.toast(Kind::Info, format!("pasted {what} as a path · Claude Code and Codex attach it"));
                }
                // nothing image-like: the program gets its own key back
                None => {
                    self.with_pane(id, |p, cx| p.key(key, cx));
                }
            },
            Event::OpenProject(dir) => {
                // another `bro` was started in this folder: open it, and ring the bell so this tab shows activity
                if !cfg!(test) {
                    use std::io::Write;
                    let mut out = std::io::stdout();
                    let _ = out.write_all(b"\x07");
                    let _ = out.flush();
                }
                self.add_project(dir);
                self.side_focus = false;
            }
            Event::ThemeFilesChanged => {
                if theme::is_custom(&self.theme.name) {
                    let name = self.theme.name.clone();
                    self.set_theme(&name, false);
                    if let Some(p) = theme::problems(&name).into_iter().next() {
                        self.toast(Kind::Error, p);
                    }
                }
            }
        }
    }

    fn paste(&mut self, s: &str) {
        self.clipboard_request = None;
        self.sel = None;
        match &mut self.overlay {
            Overlay::Launcher(l) => return l.paste(s),
            Overlay::Folder(p) => return p.paste(s),
            Overlay::Continue(p) => return p.paste(s),
            Overlay::Palette(p) => {
                p.query.extend(s.chars().filter(|c| !c.is_control()));
                p.sel = 0;
                let theme = p.preview().unwrap_or_else(|| p.theme_before.clone());
                if theme != self.theme.name {
                    self.set_theme(&theme, false);
                }
                return;
            }
            Overlay::None => {}
            _ => return,
        }
        if let Some((_, text)) = &mut self.renaming {
            text.extend(s.chars().filter(|c| !c.is_control()).take(40usize.saturating_sub(text.chars().count())));
            return;
        }
        if self.side_filtering {
            self.side.filter.extend(s.chars().filter(|c| !c.is_control()));
            self.side_sel = 0;
            return;
        }
        if self.side_focus || self.prefix_armed {
            return;
        }
        if let Some(id) = self.focused() {
            self.with_pane(id, |p, cx| p.paste(s, cx));
        }
    }

    /// Close panes whose program exited.
    fn reap(&mut self) {
        let dead: Vec<PaneId> = self.panes.iter().filter(|(_, p)| !p.alive()).map(|(id, _)| *id).collect();
        for id in dead {
            let name = self.panes.get(&id).map(|p| p.title()).unwrap_or_default();
            self.close(id);
            self.toast(Kind::Info, format!("{name} exited"));
        }
    }
}

#[cfg(test)]
mod e2e;
#[cfg(test)]
pub(crate) mod tests;
