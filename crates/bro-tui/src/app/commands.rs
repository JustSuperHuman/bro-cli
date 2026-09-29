//! What each keymap action does, plus the launcher/palette/view openers and sidebar activation.

use super::App;
use super::overlays::{Confirm, ConfirmAction, Overlay, launcher_data};
use crate::alerts::Kind;
use crate::help::Help;
use crate::keymap::Act;
use crate::launcher::Launcher;
use crate::layout::{Dir, PaneId, neighbor};
use crate::palette::{Cmd, Palette};
use crate::pane::Place;
use crate::services::LaunchRequest;
use crate::sidebar::{self, Row};
use std::path::PathBuf;

impl App {
    /// Run a keymap action.
    pub(crate) fn run_act(&mut self, a: Act) {
        self.sel = None;
        match a {
            Act::NewSession => self.open_launcher(None, Place::Tab),
            Act::Split => self.open_launcher(None, Place::Split),
            Act::NewShell => {
                self.open_shell(None, Place::Tab);
            }
            Act::SplitRight => {
                self.open_shell(None, Place::SplitRight);
            }
            Act::SplitDown => {
                self.open_shell(None, Place::SplitDown);
            }
            Act::Close => {
                if let Some(id) = self.focused() {
                    self.ask_close(id);
                }
            }
            Act::Zoom => {
                if let Some(t) = self.tabs.get_mut(self.cur) {
                    t.zoom = !t.zoom;
                }
            }
            Act::FocusLeft => self.move_focus(-1, 0),
            Act::FocusRight => self.move_focus(1, 0),
            Act::FocusUp => self.move_focus(0, -1),
            Act::FocusDown => self.move_focus(0, 1),
            Act::ResizeLeft => self.resize(Dir::Right, -0.05),
            Act::ResizeRight => self.resize(Dir::Right, 0.05),
            Act::ResizeUp => self.resize(Dir::Down, -0.05),
            Act::ResizeDown => self.resize(Dir::Down, 0.05),
            Act::Jump(n) => {
                let order = sidebar::live_order(&self.live_infos(), &self.past_infos(), &self.open_infos());
                match order.get(n as usize - 1) {
                    Some(&id) => self.go_session(id),
                    None => self.toast(Kind::Info, format!("no live session {n}")),
                }
            }
            Act::NextSession => self.cycle_session(1),
            Act::PrevSession => self.cycle_session(-1),
            Act::NextProject => self.cycle_project(1),
            Act::PrevProject => self.cycle_project(-1),
            Act::NextTab if !self.tabs.is_empty() => self.cur = (self.cur + 1) % self.tabs.len(),
            Act::PrevTab if !self.tabs.is_empty() => self.cur = (self.cur + self.tabs.len() - 1) % self.tabs.len(),
            Act::NextTab | Act::PrevTab => {}
            Act::Attention => match self.attention_target() {
                Some(id) => self.go_session(id),
                None => self.toast(Kind::Info, "nothing needs you right now"),
            },
            Act::ToggleSidebar => {
                self.sidebar = !self.sidebar;
                if !self.sidebar {
                    self.side_focus = false;
                }
            }
            Act::FocusSidebar => {
                self.sidebar = true;
                self.side_focus = !self.side_focus;
                if self.side_focus {
                    self.select_focused_row();
                }
            }
            Act::Palette => self.open_palette(""),
            Act::Usage => self.open_view("usage"),
            Act::Profiles => self.open_view("profiles"),
            Act::Proxy => self.open_view("proxy"),
            Act::Bridge => self.open_view("bridge"),
            Act::Help => self.overlay = Overlay::Help(Help::default()),
            Act::Rename => {
                if let Some(id) = self.focused() {
                    self.start_rename(id);
                }
            }
            Act::Themes => self.open_palette("theme "),
            Act::ToggleIcons => {
                let n = !crate::ui::NERD.load(std::sync::atomic::Ordering::Relaxed);
                crate::ui::NERD.store(n, std::sync::atomic::Ordering::Relaxed);
                let mut s = self.svc.settings();
                s.nerd_font = n;
                self.svc.save_settings(s);
                self.toast(Kind::Info, if n { "nerd font icons" } else { "plain icons (no nerd font)" });
            }
            Act::UsageDetails => self.toggle_usage_details(),
            Act::ShowArchived => self.toggle_show_archived(),
            Act::OpenProject => self.open_folder(),
            Act::SwitchLogin => {
                if let Some(id) = self.focused() {
                    self.open_switch(id);
                }
            }
            Act::RefreshUsage => {
                self.svc.refresh_usage();
                self.toast(Kind::Usage, "refreshing usage…");
            }
            Act::Quit => {
                let live = self.panes.values().filter(|p| p.is_terminal()).count();
                if live == 0 {
                    self.quit = true;
                } else {
                    self.overlay = Overlay::Confirm(Confirm { text: format!("quit bro and end {live} session{}?", if live == 1 { "" } else { "s" }), detail: "running agents are stopped; past sessions stay resumable".into(), action: ConfirmAction::Quit });
                }
            }
        }
    }

    /// Close a pane, asking first if an agent is working in it.
    pub(crate) fn ask_close(&mut self, id: PaneId) {
        let working = self.panes.get(&id).and_then(|p| p.activity()).is_some_and(|a| a != crate::pane::Activity::Idle);
        if working {
            let title = self.panes.get(&id).map(|p| p.title()).unwrap_or_default();
            self.overlay = Overlay::Confirm(Confirm { text: format!("close {title}?"), detail: "the agent is still busy — it will be stopped".into(), action: ConfirmAction::Close(id) });
        } else {
            self.close(id);
        }
    }

    fn move_focus(&mut self, dx: i32, dy: i32) {
        let Some(from) = self.focused() else { return };
        match neighbor(&self.outer, from, dx, dy) {
            Some(to) if self.stacked() => self.stack_focus = Some(to),
            Some(to) => self.tabs[self.cur].focus = to,
            // off the left edge: into the sidebar
            None if dx < 0 && self.sidebar => {
                self.side_focus = true;
                self.select_focused_row();
            }
            None => {}
        }
    }

    fn resize(&mut self, dir: Dir, delta: f32) {
        if let Some(id) = self.focused() {
            self.tabs[self.cur].root.resize(id, dir, delta);
        }
    }

    /// Focus a live session (switching tabs) and leave the sidebar.
    pub(crate) fn go_session(&mut self, id: PaneId) {
        // picking one session shows just that one
        self.stack.clear();
        self.stack_focus = None;
        if let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) {
            self.cur_project = Some(t.meta.project.root.clone());
        }
        self.focus_pane(id);
        self.side_focus = false;
    }

    fn cycle_session(&mut self, d: i32) {
        let order = sidebar::live_order(&self.live_infos(), &self.past_infos(), &self.open_infos());
        if order.is_empty() {
            return;
        }
        let i = self.focused().and_then(|f| order.iter().position(|x| *x == f));
        let next = match i {
            Some(i) => (i as i32 + d).rem_euclid(order.len() as i32) as usize,
            None => 0,
        };
        self.go_session(order[next]);
    }

    fn cycle_project(&mut self, d: i32) {
        let live = self.live_infos();
        let order = sidebar::live_order(&live, &self.past_infos(), &self.open_infos());
        let mut projects: Vec<(String, PaneId)> = vec![];
        for id in &order {
            let key = live.iter().find(|l| l.pane == *id).map(|l| l.project_key.clone()).unwrap_or_default();
            if !projects.iter().any(|(k, _)| *k == key) {
                projects.push((key, *id));
            }
        }
        if projects.is_empty() {
            return;
        }
        let cur_key = self.focused().and_then(|f| live.iter().find(|l| l.pane == f)).map(|l| l.project_key.clone());
        let i = cur_key.and_then(|k| projects.iter().position(|(p, _)| *p == k));
        let next = match i {
            Some(i) => (i as i32 + d).rem_euclid(projects.len() as i32) as usize,
            None => 0,
        };
        self.go_session(projects[next].1);
    }

    /// The directory new sessions start in: the current project.
    pub(crate) fn preferred_dir(&self) -> Option<PathBuf> {
        self.current_project()
    }

    pub(crate) fn open_launcher(&mut self, cwd: Option<PathBuf>, place: Place) {
        let cwd = cwd.or_else(|| self.preferred_dir());
        let data = launcher_data(self);
        self.overlay = Overlay::Launcher(Box::new(Launcher::new(data, cwd, place)));
    }

    /// Launch from the launcher.
    pub(crate) fn launch_spec(&mut self, spec: bro_core::launch::LaunchSpec, place: Place) {
        let mut req = LaunchRequest::new(spec, place);
        req.remember = true;
        self.toast(Kind::Info, format!("launching {}…", crate::services::launch::label_for(&req.spec)));
        self.svc.launch(req);
    }

    /// Focus a view if it's open anywhere, else open it in a new tab.
    pub(crate) fn open_view(&mut self, name: &str) {
        if let Some(id) = self.panes.iter().find(|(_, p)| p.view() == Some(name)).map(|(id, _)| *id) {
            if self.focused() == Some(id) && self.tabs.len() > 1 {
                // pressing the key again goes back where you were
                self.cur = (self.cur + self.tabs.len() - 1) % self.tabs.len();
                return;
            }
            self.focus_pane(id);
            self.side_focus = false;
            return;
        }
        if let Some(p) = crate::views::open(name) {
            self.new_tab(p);
            self.side_focus = false;
        }
    }

    pub(crate) fn open_palette(&mut self, query: &str) {
        let themes = crate::theme::names();
        self.overlay = Overlay::Palette(Box::new(Palette::new(&self.keymap, &self.recents, &themes, &self.theme.name, query)));
    }

    pub(crate) fn run_palette(&mut self, c: Cmd) {
        match c {
            Cmd::Act(a) => self.run_act(a),
            Cmd::Theme(t) => {
                self.set_theme(&t, true);
                self.toast(Kind::Info, format!("theme: {t}"));
            }
            Cmd::Recent(i) => {
                if let Some(r) = self.recents.get(i).cloned() {
                    let spec = bro_core::launch::LaunchSpec { harness: r.harness, profile_id: r.profile_id, provider_id: r.provider_id, model: r.model, cwd: r.cwd, resume: None, permission: r.permission, browser: r.browser, extra_args: vec![] };
                    self.launch_spec(spec, Place::Tab);
                }
            }
        }
    }

    pub(crate) fn start_rename(&mut self, id: PaneId) {
        let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) else {
            self.toast(Kind::Info, "only sessions can be renamed");
            return;
        };
        self.renaming = Some((id, t.meta.name.clone().unwrap_or_default()));
        self.sidebar = true;
    }

    /// Point the sidebar selection at the focused session's row.
    pub(crate) fn select_focused_row(&mut self) {
        let Some(f) = self.focused() else { return };
        if let Some(i) = self.rows().iter().position(|r| matches!(r, Row::Live { info, .. } if info.pane == f)) {
            self.side_sel = i;
        }
    }

    /// Sidebar usage: Claude / Codex totals ⇄ every profile (remembered in v2.toml).
    pub(crate) fn toggle_usage_details(&mut self) {
        self.usage_expanded = !self.usage_expanded;
        let mut s = self.svc.settings();
        s.usage_expanded = self.usage_expanded;
        self.svc.save_settings(s);
    }

    /// "a" on a sidebar row: archive an earlier session (or restore an archived one), or every earlier
    /// session of a project.
    pub(crate) fn archive_row(&mut self, i: usize) {
        let rows = self.rows();
        let Some(r) = rows.get(i) else { return };
        let past = self.svc.state().past.ready().cloned().unwrap_or_default();
        let id_of = |idx: usize| past.get(idx).map(|s| s.id.clone());
        let project_name = |key: &str| rows.iter().find_map(|r| match r {
            Row::Project { key: k, name, .. } if k == key => Some(name.clone()),
            _ => None,
        });
        match r {
            Row::Past { info } if info.archived => {
                if let Some(id) = id_of(info.idx) {
                    self.archive.remove(&[id]);
                    self.archive.save(self.persist);
                    self.toast(Kind::Info, format!("restored \u{201c}{}\u{201d}", crate::ui::fit(&info.title, 40)));
                }
            }
            Row::Past { info } => {
                if let Some(id) = id_of(info.idx) {
                    self.archive.add([id]);
                    self.archive.save(self.persist);
                    self.toast(Kind::Info, format!("archived \u{201c}{}\u{201d} \u{b7} u to undo", crate::ui::fit(&info.title, 40)));
                }
            }
            Row::Project { key, .. } | Row::More { key, .. } => {
                let name = project_name(key).unwrap_or_default();
                let ids: Vec<String> = self.past_infos().into_iter().filter(|p| &p.project_key == key && !p.archived).filter_map(|p| id_of(p.idx)).collect();
                let n = self.archive.add(ids);
                self.archive.save(self.persist);
                let msg = match n {
                    0 => format!("nothing to archive in {name}"),
                    n => format!("archived {n} earlier session{} in {name} \u{b7} u to undo", if n == 1 { "" } else { "s" }),
                };
                self.toast(Kind::Info, msg);
            }
            Row::Live { .. } => self.toast(Kind::Info, "a running session can't be archived \u{2014} close it first (x)"),
            Row::New | Row::OpenFolder => {}
        }
        let n = self.rows().len();
        self.side_sel = self.side_sel.min(n.saturating_sub(1));
    }

    /// "u": undo the last archive.
    pub(crate) fn undo_archive(&mut self) {
        match self.archive.undo() {
            0 => self.toast(Kind::Info, "nothing to undo"),
            n => {
                self.archive.save(self.persist);
                self.toast(Kind::Info, format!("restored {n} session{}", if n == 1 { "" } else { "s" }));
            }
        }
    }

    /// Show / hide archived sessions in the sidebar.
    pub(crate) fn toggle_show_archived(&mut self) {
        self.side.show_archived = !self.side.show_archived;
        let n = self.archive.ids.len();
        let msg = if self.side.show_archived { format!("showing {n} archived \u{b7} a restores one") } else { "archived sessions hidden".to_string() };
        self.toast(Kind::Info, msg);
    }

    /// Enter on a sidebar row.
    pub(crate) fn activate_row(&mut self, i: usize) {
        let rows = self.rows();
        let Some(r) = rows.get(i) else { return };
        self.side_sel = i;
        match r {
            Row::New => self.open_launcher(None, Place::Tab),
            Row::Project { key, root, collapsed, .. } => {
                self.cur_project = Some(root.clone());
                self.side.set_collapsed(key, !*collapsed);
            }
            Row::OpenFolder => self.open_folder(),
            Row::Live { info, .. } => self.go_session(info.pane),
            Row::Past { info } => self.open_resume(info.idx),
            Row::More { key, .. } => {
                self.side.past_open.insert(key.clone());
            }
        }
    }

    /// The project header row for `key`, as (index, has live sessions).
    fn project_row(rows: &[Row], key: &str) -> Option<(usize, bool)> {
        rows.iter().enumerate().find_map(|(i, r)| match r {
            Row::Project { key: k, live, .. } if k == key => Some((i, *live > 0)),
            _ => None,
        })
    }

    /// h / ←: fold the project (from any of its rows, landing on its header).
    pub(crate) fn side_collapse(&mut self) {
        let rows = self.rows();
        let Some(r) = rows.get(self.side_sel) else { return };
        let key = r.project_key().to_string();
        if key.is_empty() {
            return;
        }
        // an opened "more" list shrinks back first
        if matches!(r, Row::Past { .. }) && self.side.past_open.remove(&key) {
            return;
        }
        if let Some((i, live)) = Self::project_row(&rows, &key) {
            let _ = live;
            self.side.set_collapsed(&key, true);
            self.side_sel = i;
        }
    }

    /// l / →: unfold a project, open "more", or step into the project.
    pub(crate) fn side_expand(&mut self) {
        let rows = self.rows();
        match rows.get(self.side_sel) {
            Some(Row::Project { key, collapsed: true, .. }) => self.side.set_collapsed(key, false),
            Some(Row::More { key, .. }) => {
                self.side.past_open.insert(key.clone());
            }
            Some(Row::Project { .. } | Row::New | Row::OpenFolder) => self.side_sel = (self.side_sel + 1).min(rows.len().saturating_sub(1)),
            _ => {}
        }
    }
}
