//! What each keymap action does, plus the launcher/palette/view openers and sidebar activation.

use super::App;
use super::overlays::{Confirm, ConfirmAction, Overlay, launcher_data};
use crate::alerts::Kind;
use crate::help::Help;
use crate::keymap::Act;
use crate::launcher::Launcher;
use crate::layout::{Dir, PaneId, neighbor};
use crate::palette::{Cmd, Palette};
use crate::pane::{Event, Place};
use crate::services::LaunchRequest;
use crate::sidebar::{self, Row};
use std::path::PathBuf;

impl App {
    /// Run a keymap action.
    pub(crate) fn run_act(&mut self, a: Act) {
        self.sel = None;
        match a {
            Act::NewSession => self.open_launcher(None, Place::Tab),
            Act::QuickAgent => self.quick_agent(),
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
                if self.stacked() {
                    self.stack_zoom = !self.stack_zoom;
                } else if let Some(t) = self.tabs.get_mut(self.cur) {
                    t.zoom = !t.zoom;
                }
            }
            Act::Grow => {
                if let Some(id) = self.focused() {
                    self.toggle_priority(id);
                }
            }
            Act::Shrink => self.priority = None,
            Act::TileAll => self.toggle_tiles(),
            Act::Flip => {
                if let Some(id) = self.focused() {
                    self.flip_pane(id);
                }
            }
            Act::FlipAll => self.flip_all(),
            Act::Focus => self.toggle_focus_mode(),
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
            Act::NextTab if !self.tabs.is_empty() => {
                self.cur = (self.cur + 1) % self.tabs.len();
                self.leave_sidebar();
            }
            Act::PrevTab if !self.tabs.is_empty() => {
                self.cur = (self.cur + self.tabs.len() - 1) % self.tabs.len();
                self.leave_sidebar();
            }
            Act::NextTab | Act::PrevTab => {}
            Act::ShowTab(0) => self.show_tab(super::project_tabs::StripTab::All),
            Act::ShowTab(n) => match self.project_tabs().get(n as usize - 1) {
                Some(tab) => self.show_tab(super::project_tabs::StripTab::Project(tab.key.clone())),
                None => self.toast(Kind::Info, format!("no project tab {n}")),
            },
            Act::LastTab => match self.tab_before.clone() {
                Some(tab) => self.show_tab(tab),
                None => self.toast(Kind::Info, "no earlier tab yet"),
            },
            Act::ToggleProjectTabs => {
                let mut s = self.svc.settings();
                s.project_tabs = !s.project_tabs;
                let on = s.project_tabs;
                self.svc.save_settings(s);
                self.toast(Kind::Info, if on { "project tabs on: All and each project, over the sessions" } else { "project tabs off" });
            }
            Act::Attention => match self.attention_target() {
                Some(id) => self.go_session(id),
                None => self.toast(Kind::Info, "nothing needs you right now"),
            },
            Act::ToggleSidebar => {
                self.sidebar = !self.sidebar;
                if !self.sidebar {
                    self.leave_sidebar();
                }
            }
            Act::FocusSidebar if self.strip_sel.is_some() => self.strip_sel = None,
            // no sidebar on screen (hidden, or the window's too narrow): the project tabs take the keys
            Act::FocusSidebar if self.strip.height > 0 && (!self.sidebar || self.sidebar_squeezed()) => self.focus_strip(),
            Act::FocusSidebar if self.sidebar_squeezed() => self.toast(Kind::Info, "the window is too narrow for the sidebar · widen it, or turn on project tabs"),
            Act::FocusSidebar => {
                self.sidebar = true;
                if self.side_focus {
                    self.leave_sidebar();
                } else {
                    self.side_focus = true;
                    self.side_manual_scroll = false;
                    self.select_focused_row();
                }
            }
            Act::Palette => self.open_palette(""),
            Act::Usage => self.open_view("usage"),
            Act::Profiles => self.open_view("profiles"),
            Act::Stats => self.open_view("stats"),
            Act::Settings => self.open_view("settings"),
            Act::QuickInstall => self.open_view("quick-install"),
            Act::Sounds => self.open_view("sounds"),
            Act::Skills => self.open_view("skills"),
            Act::Quips => self.open_quips(),
            Act::Setup => self.open_view("setup"),
            Act::Proxy => self.open_view("proxy"),
            Act::Bridge => self.open_view("bridge"),
            Act::Tasks => self.open_view("tasks"),
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
            Act::ToggleTrim => {
                let on = !self.svc.settings().proxy.trim;
                self.svc.set_trim(on);
                self.toast(Kind::Info, if on { "request compression on (new sessions go through bro-proxy)" } else { "request compression off" });
            }
            Act::ToggleProxyMode => {
                let on = !self.svc.settings().proxy.shared;
                self.svc.set_shared(on);
                self.toast(Kind::Info, if on { "proxy mode on: new sessions share all your signed-in logins, and a limit moves the work to the next one" } else { "proxy mode off: new sessions run on the login you pick" });
            }
            Act::ToggleAutoResume => {
                let mut s = self.svc.settings();
                s.auto_resume.enabled = !s.auto_resume.enabled;
                let on = s.auto_resume.enabled;
                self.svc.save_settings(s);
                self.toast(Kind::Info, if on { "auto-resume on: agents that stop on an API error get `continue`" } else { "auto-resume off" });
            }
            Act::ToggleCaveman => {
                let s = self.svc.settings().caveman;
                self.set_caveman((!s.on).then_some(s.level));
            }
            Act::Continue => self.open_continue(),
            Act::Reload => {
                if let Some(id) = self.focused() {
                    self.open_reload(id);
                } else {
                    self.toast(Kind::Info, "choose an agent session to reload");
                }
            }
            Act::Orchestrator => self.open_orchestrator(),
            Act::CloseProject => {
                if let Some(root) = self.current_project() {
                    let key = self.svc.project_for(&root).key;
                    let live = self.live_infos().iter().filter(|l| l.project_key == key).count();
                    self.close_project(root, live);
                }
            }
            Act::OpenClipboardFolder => {
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let text = crate::clip::grab_text();
                    let _ = tx.send(Event::ClipboardFolder(crate::folder::clipboard_folder(text.as_deref())));
                });
            }
            Act::OpenedFile => self.show_last_opened(),
            Act::AgentFiles => self.show_agent_files(),
            Act::Team => self.show_team(None),
            Act::Watchdog => self.open_watchdog(),
            Act::WatchdogPrompt => self.open_watchdog_prompt(),
            Act::Media => {
                if crate::panes::mirror::active() {
                    crate::panes::mirror::request(crate::panes::mirror::Request::MediaPanel);
                    self.svc.wake();
                } else {
                    self.toast(Kind::Info, "pictures and videos play beside your sessions in the desktop app: bro gui");
                }
            }
            Act::SwitchLogin => {
                if let Some(id) = self.focused() {
                    self.open_switch(id);
                }
            }
            Act::RefreshUsage => {
                self.svc.refresh_usage();
                self.toast(Kind::Usage, "refreshing usage…");
            }
            Act::RestartServer => self.ask_restart_server(),
            Act::Quit => {
                // sessions in the session server keep running: closing bro doesn't end them
                let keep = self.svc.host().is_some() && self.svc.settings().server.keep_sessions;
                let live = self.panes.values().filter(|p| p.is_terminal() && !(keep && p.as_term_ref().is_some_and(|t| t.hosted()))).count();
                if live == 0 {
                    self.quit = true;
                } else {
                    self.overlay = Overlay::Confirm(Confirm { text: format!("quit bro and end {live} session{}?", if live == 1 { "" } else { "s" }), detail: "running agents are stopped; past sessions stay resumable".into(), action: ConfirmAction::Quit });
                }
            }
        }
        if self.overlay.is_open() {
            self.leave_sidebar();
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
        if self.side_focus && dx > 0 {
            self.leave_sidebar();
            return;
        }
        let Some(from) = self.focused() else { return };
        match neighbor(&self.outer, from, dx, dy) {
            Some(to) => {
                if self.stacked() {
                    self.stack_focus = Some(to);
                } else {
                    self.tabs[self.cur].focus = to;
                }
                self.leave_sidebar();
            }
            // off the left edge: into the sidebar
            None if dx < 0 && self.sidebar => {
                self.side_focus = true;
                self.select_focused_row();
            }
            None => {}
        }
    }

    fn resize(&mut self, dir: Dir, delta: f32) {
        if self.stacked() {
            return;
        }
        if let Some(id) = self.focused() {
            self.tabs[self.cur].root.resize(id, dir, delta);
        }
    }

    /// Focus a live session (switching tabs) and leave the sidebar.
    pub(crate) fn go_session(&mut self, id: PaneId) {
        // In all-sessions mode, navigation moves focus without dismantling the grid.
        if !self.tile_all {
            self.clear_stack();
        }
        // a project's grid moves to the session's own project
        if self.tile_all
            && !self.stack.contains(&id)
            && let Some(key) = self.tile_project.clone()
            && let Some(own) = self.panes.get(&id).and_then(|p| p.as_term_ref()).filter(|t| !t.meta.orchestrator).map(|t| t.meta.project.key.clone())
            && own != key
        {
            self.tile_project = Some(own);
            self.retile(Some(id));
        }
        if let Some(t) = self.panes.get(&id).and_then(|p| p.as_term_ref()) {
            self.cur_project = Some(t.meta.project.root.clone());
        }
        self.focus_pane(id);
        self.leave_sidebar();
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
        // in a grid: the next project tab (All, then each project)
        if self.tile_all {
            self.step_tab(d, false);
            return;
        }
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
        self.svc.refresh_usage();
        self.svc.refresh_sessions();
        let cwd = cwd.or_else(|| self.preferred_dir());
        self.leave_sidebar();
        self.overlay = Overlay::Launcher(Box::new(self.session_launcher(cwd, place)));
    }

    fn session_launcher(&self, cwd: Option<PathBuf>, place: Place) -> Launcher {
        let permission = self.svc.state().settings.permission;
        let launcher = Launcher::new(launcher_data(self), cwd, place).with_projects(self.launcher_projects()).with_permission(permission);
        let project = self.svc.project_for(&launcher.dir);
        launcher.with_continue(self.continue_entries(), Some((project.key, project.name)))
    }

    /// Go to the orchestrator, or open the launcher on its tab to start one.
    pub(crate) fn open_orchestrator(&mut self) {
        let cwd = self.current_project().unwrap_or_else(bro_core::orchestrator::workspace);
        if let Some(id) = self.orchestrator_for(&cwd) {
            self.overlay = Overlay::None;
            self.focus_pane(id);
            self.leave_sidebar();
            return;
        }
        self.svc.refresh_usage();
        let l = self.session_launcher(self.preferred_dir(), Place::Tab).with_orchestrator();
        self.overlay = Overlay::Launcher(Box::new(l));
    }

    /// Keep the permission mode picked in the launcher for the next new session.
    pub(crate) fn remember_launcher_permission(&mut self) {
        let Overlay::Launcher(l) = &self.overlay else { return };
        let permission = l.permission;
        if self.svc.state().settings.permission == permission {
            return;
        }
        let mut s = self.svc.settings();
        s.permission = permission;
        self.svc.save_settings(s);
    }

    /// The sidebar's new-session strip: claude / codex straight away on your usual login (the launcher, on that
    /// harness, when there's still something to pick), or a terminal in the current project.
    pub(crate) fn quick_launch(&mut self, h: Option<bro_core::Harness>) {
        self.follow_focus();
        let Some(h) = h else {
            self.open_shell(self.preferred_dir(), Place::Tab);
            self.leave_sidebar();
            return;
        };
        self.svc.refresh_usage();
        let l = self.session_launcher(self.preferred_dir(), Place::Tab).with_harness(h);
        self.launch_or_choose(l);
    }

    /// Run what the launcher has selected straight away, or open it when that still needs a decision.
    fn launch_or_choose(&mut self, l: Launcher) {
        match l.quick_spec() {
            Some(spec) => {
                self.launch_spec(spec, Place::Tab);
                self.leave_sidebar();
            }
            None => self.overlay = Overlay::Launcher(Box::new(l)),
        }
    }

    /// A new agent in your home folder on the harness and login you launched last (the launcher, there, when
    /// that still needs a decision).
    pub(crate) fn quick_agent(&mut self) {
        self.follow_focus();
        self.svc.refresh_usage();
        let l = self.quick_agent_launcher();
        match l.quick_spec() {
            Some(spec) => {
                self.launch_spec(spec, Place::Tab);
                self.leave_sidebar();
            }
            None => {
                self.leave_sidebar();
                self.overlay = Overlay::Launcher(Box::new(l));
            }
        }
    }

    /// The launcher in your home folder, on the harness of your last (non-orchestrator) launch, its latest login on top.
    pub(crate) fn quick_agent_launcher(&self) -> Launcher {
        let l = self.session_launcher(Some(bro_core::util::home_dir()), Place::Tab);
        // the orchestrator's own launches don't count as your last agent
        match l.data.recents.iter().find(|r| !r.orchestrator).map(|r| r.harness) {
            Some(h) => l.with_harness(h),
            None => l,
        }
    }

    /// Usage totals use quick-launch defaults; account rows always use that exact login. Proxy mode has no login
    /// to pick: a family's total, or one of its logins, starts on all of that family's logins (the launcher's row).
    pub(crate) fn launch_usage(&mut self, h: bro_core::Harness, profile: Option<String>) {
        self.follow_focus();
        if let Some(claude) = self.pooled_family(h, profile.as_deref())
            && let Some(l) = self.session_launcher(self.preferred_dir(), Place::Tab).with_harness(h).with_shared(claude)
        {
            self.svc.refresh_usage();
            self.launch_or_choose(l);
            return;
        }
        let Some(profile) = profile else {
            self.quick_launch(Some(h));
            return;
        };
        self.svc.refresh_usage();
        let is_gemini = bro_core::profiles::is_gemini_id(&profile);
        let h = if is_gemini {
            self.recents.iter().find(|r| r.profile_id.as_deref() == Some(&profile)).map(|r| r.harness).unwrap_or(h)
        } else { h };
        let launcher = self.session_launcher(self.preferred_dir(), Place::Tab).with_harness(h);
        let Some(launcher) = launcher.with_profile(&profile) else {
            self.toast(Kind::Error, "that account is no longer available");
            return;
        };
        if is_gemini {
            self.overlay = Overlay::Launcher(Box::new(launcher));
            return;
        }
        self.launch_or_choose(launcher);
    }

    /// In proxy mode, the family a usage row's launch shares: its login's, else its harness's own (Claude
    /// `true`, Codex `false`). None outside proxy mode and for rows that aren't a Claude or Codex login.
    fn pooled_family(&self, h: bro_core::Harness, profile: Option<&str>) -> Option<bool> {
        if !self.svc.state().settings.proxy.shared {
            return None;
        }
        match profile {
            Some(p) if p.starts_with("claude:") => Some(true),
            Some(p) if p.starts_with("codex:") => Some(false),
            Some(_) => None,
            None => match h {
                bro_core::Harness::Claude => Some(true),
                bro_core::Harness::Codex => Some(false),
                bro_core::Harness::Pi | bro_core::Harness::Omp => None,
            },
        }
    }

    /// Launch from the launcher.
    pub(crate) fn launch_spec(&mut self, spec: bro_core::launch::LaunchSpec, place: Place) {
        if spec.orchestrator && self.guest {
            self.toast(Kind::Info, "the orchestrator runs in your other bro");
            return;
        }
        if spec.orchestrator {
            // Reuse the coordinator that owns this project.
            if let Some(id) = self.orchestrator_for(&spec.cwd) {
                self.clear_stack();
                self.focus_pane(id);
                self.leave_sidebar();
                self.toast(Kind::Info, "the orchestrator is already running · here it is");
                return;
            }
            if self.orchestrator_starting(&spec.cwd) {
                self.toast(Kind::Info, "the orchestrator is already starting…");
                return;
            }
            self.orchestrator_pending.insert(bro_core::orchestrator::scope_key(&spec.cwd), std::time::Instant::now());
        }
        let mut req = LaunchRequest::new(spec, place);
        req.remember = true;
        self.toast(Kind::Info, format!("launching {}…", crate::services::launch::label_for(&req.spec)));
        self.svc.launch(req);
    }

    /// Focus a view if it's open anywhere, else open it in a new tab.
    pub(crate) fn open_view(&mut self, name: &str) {
        if name == "profiles" {
            self.svc.refresh_data();
        }
        if name == "usage" {
            self.svc.refresh_usage();
        }
        if let Some(id) = self.panes.iter().find(|(_, p)| p.view() == Some(name)).map(|(id, _)| *id) {
            if self.focused() == Some(id) && self.tabs.len() > 1 {
                // pressing the key again goes back where you were
                self.cur = (self.cur + self.tabs.len() - 1) % self.tabs.len();
                self.leave_sidebar();
                return;
            }
            self.focus_pane(id);
            self.leave_sidebar();
            return;
        }
        if let Some(p) = crate::views::open(name) {
            self.new_tab(p);
            self.leave_sidebar();
        }
    }

    pub(crate) fn open_palette(&mut self, query: &str) {
        self.leave_sidebar();
        let themes = crate::theme::names();
        let tiles: Vec<(String, String, usize)> = self.project_tabs().into_iter().map(|t| (t.key, t.name, t.live)).collect();
        let all = self.tile_ids(None).len();
        let mut p = Palette::new(&self.keymap, &self.recents, &themes, &self.theme.name, query)
            .with_presets(&self.svc.settings().claude_env.preset)
            .with_caveman(self.svc.settings().caveman.active())
            .with_tiles(&tiles, all);
        // the desktop app has a window of its own to dress
        if crate::fonts::GUI.load(std::sync::atomic::Ordering::Relaxed) {
            p = p.with_materials(self.svc.settings().material);
        }
        self.overlay = Overlay::Palette(Box::new(p));
    }

    /// Caveman mode for new sessions: a level, or None for off.
    pub(crate) fn set_caveman(&mut self, level: Option<bro_core::caveman::Level>) {
        let mut s = self.svc.settings();
        if let Some(l) = level {
            s.caveman.level = l;
        }
        s.caveman.set_on(level.is_some());
        self.svc.save_settings(s);
        self.toast(Kind::Info, match level {
            Some(l) => format!("caveman mode: {} (new sessions)", l.id()),
            None => "caveman mode off (new sessions)".to_string(),
        });
    }

    pub(crate) fn run_palette(&mut self, c: Cmd) {
        match c {
            Cmd::Act(a) => self.run_act(a),
            Cmd::Theme(t) => {
                self.set_theme(&t, true);
                self.toast(Kind::Info, format!("theme: {t}"));
            }
            Cmd::ClaudePreset(id) => {
                let mut s = self.svc.settings();
                s.claude_env.use_preset(&id);
                let label = s.claude_env.summary();
                self.svc.save_settings(s);
                self.toast(Kind::Info, format!("Claude preset: {label} (new launches)"));
            }
            Cmd::Caveman(level) => self.set_caveman(level),
            Cmd::Material(m) => {
                let mut s = self.svc.settings();
                s.material = m;
                self.svc.save_settings(s);
                self.toast(Kind::Info, format!("window: {}", m.id()));
            }
            Cmd::Tiles(None) => self.show_tab(super::project_tabs::StripTab::All),
            Cmd::Tiles(Some(key)) => self.show_tab(super::project_tabs::StripTab::Project(key)),
            Cmd::Recent(i) => {
                if let Some(r) = self.recents.get(i).cloned() {
                    let spec = bro_core::launch::LaunchSpec { harness: r.harness, profile_id: r.profile_id, provider_id: r.provider_id, model: r.model, cwd: r.cwd, resume: None, permission: self.svc.state().settings.permission, browser: r.browser, extra_args: vec![], orchestrator: false };
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

    /// Open the shared session dialog on its Continue tab.
    pub(crate) fn open_continue(&mut self) {
        self.open_launcher(None, Place::Tab);
        if let Overlay::Launcher(l) = &mut self.overlay {
            l.show_continue();
        }
    }

    /// Every earlier session (archived ones flagged), newest first.
    pub(crate) fn continue_entries(&self) -> Vec<crate::continue_picker::Entry> {
        let st = self.svc.state();
        let Some(past) = st.past.ready() else { return vec![] };
        let mut v: Vec<crate::continue_picker::Entry> = past
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| {
                let pk = s.project.clone().or_else(|| s.cwd.as_ref().map(|c| self.svc.project_for(c)))?;
                Some(crate::continue_picker::Entry {
                    idx,
                    id: s.id.clone(),
                    harness: s.harness,
                    title: s.title.clone(),
                    login: s.profile_id.as_deref().map(|p| p.split(':').next_back().unwrap_or(p).to_string()),
                    age_secs: crate::util::secs_since(s.modified),
                    project_key: pk.key,
                    project_name: pk.name,
                    archived: self.archive.contains(&s.id),
                })
            })
            .collect();
        v.sort_by_key(|e| e.age_secs);
        v
    }

    /// Archive / restore from the continue picker, then refresh its list.
    pub(crate) fn continue_archive(&mut self, id: String, archive: bool) {
        if archive {
            self.archive.add([id]);
            self.toast(Kind::Info, "archived \u{b7} ctrl+z undoes");
        } else {
            self.archive.remove(&[id]);
            self.toast(Kind::Info, "restored");
        }
        self.archive.save(self.persist);
        self.refresh_continue();
    }

    /// ctrl+z in the continue picker.
    pub(crate) fn undo_archive(&mut self) {
        match self.archive.undo() {
            0 => self.toast(Kind::Info, "nothing to undo"),
            n => {
                self.archive.save(self.persist);
                self.toast(Kind::Info, format!("restored {n} session{}", if n == 1 { "" } else { "s" }));
            }
        }
        self.refresh_continue();
    }

    pub(crate) fn refresh_continue(&mut self) {
        let entries = self.continue_entries();
        if let Overlay::Launcher(l) = &mut self.overlay {
            l.set_continue_entries(entries);
        }
    }

    /// Enter on a sidebar row.
    pub(crate) fn activate_row(&mut self, i: usize) {
        let rows = self.rows();
        let Some(r) = rows.get(i) else { return };
        self.side_sel = i;
        match r {
            Row::Project { key, root, .. } => {
                self.pick_project(root.clone());
                self.toggle_project_tiles(key);
            }
            Row::Live { info, .. } => self.go_session(info.pane),
        }
    }

    /// The project header row for `key`.
    fn project_row(rows: &[Row], key: &str) -> Option<usize> {
        rows.iter().position(|r| matches!(r, Row::Project { key: k, .. } if k == key))
    }

    /// ←: up to the project's header (from any of its rows).
    pub(crate) fn side_parent(&mut self) {
        let rows = self.rows();
        let Some(r) = rows.get(self.side_sel) else { return };
        if let Some(i) = Self::project_row(&rows, r.project_key()) {
            self.side_sel = i;
        }
    }

    /// →: step into a project.
    pub(crate) fn side_step_in(&mut self) {
        let rows = self.rows();
        if matches!(rows.get(self.side_sel), Some(Row::Project { .. })) {
            self.side_sel = (self.side_sel + 1).min(rows.len().saturating_sub(1));
        }
    }
}
