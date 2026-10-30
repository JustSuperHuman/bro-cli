//! Sessions: turning launch results into PTY panes, the sidebar's view of them, resume/fork, remote (bridge)
//! commands, and agent tracking (done-while-unseen, needs-you).

use super::App;
use super::overlays::Overlay;
use crate::alerts::Kind;
use crate::layout::PaneId;
use crate::pane::{Activity, Place, Waker};
use crate::panes::term::{Meta, Spawn, Term};
use crate::services::{LaunchRequest, Launched};
use crate::sidebar::{self, LiveInfo, PastInfo, Row};
use bro_bridge::BridgeCommand;
use bro_core::Harness;
use bro_core::browser::BrowserMode;
use bro_core::launch::{LaunchSpec, Permission, Resume};
use bro_core::sessions::SessionInfo;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Quiet time after one queued resume starts before the next one launches.
const RESUME_GAP: Duration = Duration::from_millis(1500);
/// A queued launch that never reports back stops holding up the rest after this long.
const RESUME_STUCK: Duration = Duration::from_secs(20);

/// Earlier sessions resumed together: each waits in a placeholder pane and they launch one at a time,
/// so a dozen agents don't all start (and hit the disk and network) at once.
#[derive(Default)]
pub(crate) struct ResumeQueue {
    pub(super) waiting: VecDeque<(PaneId, LaunchRequest)>,
    /// the launch building now, and since when
    pub(super) starting: Option<(PaneId, Instant)>,
    /// the next one launches no earlier than this
    pub(super) next_at: Option<Instant>,
}

/// What a remote create's `profile_id` asks for: an agent (harness + bro profile) or None for a shell.
/// Accepts bro profile ids ("claude:work"), bare harness names ("claude", "pi") and "shell".
/// The profile id a remote client sends to open bro's orchestrator session.
pub(crate) const ORCHESTRATOR_PROFILE: &str = "orchestrator";

pub(crate) fn create_target(profile_id: Option<&str>) -> Option<(Harness, Option<String>)> {
    let id = profile_id?;
    if let Some(h) = Harness::ALL.iter().find(|h| h.label() == id) {
        return Some((*h, None));
    }
    match id.split_once(':') {
        Some(("claude", _)) => Some((Harness::Claude, Some(id.to_string()))),
        Some(("codex", _)) => Some((Harness::Codex, Some(id.to_string()))),
        _ => None,
    }
}

impl App {
    /// A launch finished building: open its pane (or report why not).
    pub(super) fn launched(&mut self, l: Launched) {
        let Launched { spec, place, reply, name, remember, result, route_id, note, into } = l;
        // a queued resume: its placeholder (unless you closed it meanwhile) becomes the session
        let replacement = into.filter(|id| self.panes.get(id).and_then(|p| p.as_term_ref()).is_some());
        let placeholder = replacement.filter(|id| self.panes.get(id).and_then(|p| p.as_term_ref()).is_some_and(|t| t.queued.is_some()));
        if let Some(t) = replacement.and_then(|id| self.panes.get_mut(&id)).and_then(|p| p.as_term()) {
            t.reloading = false;
        }
        if let Some(id) = into {
            self.resume_started(id);
        }
        if spec.orchestrator {
            self.orchestrator_pending.remove(&bro_core::orchestrator::scope_key(&spec.cwd));
        }
        let cmd = match result {
            Ok(c) => c,
            Err(e) => {
                // it can't start this way: don't try it again on every start
                if spec.orchestrator && self.persist && !self.guest && self.orchestrator_for(&spec.cwd).is_none() {
                    crate::orchestrator_state::forget(&spec.cwd);
                }
                self.raise(Kind::Error, format!("couldn't launch {}: {e}", spec.harness.label()));
                if let Some(r) = reply {
                    let _ = r.send(Err(anyhow::anyhow!(e)));
                }
                if let Some(id) = placeholder {
                    self.close(id);
                }
                return;
            }
        };
        if remember {
            crate::recents::push(
                &mut self.recents,
                crate::recents::Recent { harness: spec.harness, profile_id: spec.profile_id.clone(), provider_id: spec.provider_id.clone(), model: spec.model.clone(), cwd: spec.cwd.clone(), permission: spec.permission, browser: spec.browser, at: crate::util::now_secs(), orchestrator: spec.orchestrator },
            );
            if !self.svc.is_demo() {
                crate::recents::save(self.recents.clone());
            }
        }
        let cwd = if cmd.cwd.as_os_str().is_empty() { spec.cwd.clone() } else { cmd.cwd.clone() };
        let label = if cmd.label.is_empty() { crate::services::launch::label_for(&spec) } else { cmd.label.clone() };
        // the orchestrator's frame already says what it is
        let label = if spec.orchestrator { label.trim_start_matches("orchestrator · ").to_string() } else { label };
        let meta = Meta {
            sid: uuid::Uuid::new_v4().to_string(),
            harness: Some(spec.harness),
            profile: spec.provider_id.clone().or(spec.profile_id.clone()),
            // where the launch put it (proxy mode picks the login a session runs as)
            store: cmd.store.clone().or_else(|| transcript_store(&spec)),
            model: spec.model.clone(),
            label,
            // a name the bridge asked for (the orchestrator's "Toast · justgains") outlives the agent's own title
            renamed: name.is_some() && reply.is_some(),
            name,
            project: if spec.orchestrator { crate::panes::term::orchestrator_project(&cwd) } else { self.svc.project_for(&cwd) },
            cwd,
            started: Instant::now(),
            route_id,
            cleanup: cmd.cleanup.clone(),
            orchestrator: spec.orchestrator,
        };
        let sid = meta.sid.clone();
        let mut term = Term::new(meta, Spawn { program: cmd.program, args: cmd.args, env: cmd.env, env_remove: cmd.env_remove }, self.svc.clone());
        term.launch_spec = Some(spec.clone());
        // A second launch for the same project is discarded before its process starts.
        if spec.orchestrator && let Some(existing) = self.orchestrator_for(&spec.cwd).filter(|id| Some(*id) != replacement) {
            let id = self.add(Box::new(term));
            self.close(id);
            if let Some(reply) = reply {
                let sid = self.panes[&existing].as_term_ref().unwrap().meta.sid.clone();
                let _ = reply.send(Ok(sid));
            } else {
                self.focus_pane(existing);
                self.toast(Kind::Info, "the orchestrator is already running in this project");
            }
            return;
        }
        let place = if spec.orchestrator { Place::Tab } else { place };
        if spec.orchestrator && self.persist && !self.guest {
            crate::orchestrator_state::save(&spec);
        }
        if into.is_some() {
            match replacement {
                Some(id) => {
                    // same pane id: it keeps its tab, grid slot and sidebar place
                    let old_route = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.meta.route_id.clone());
                    if let Some(old) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                        self.svc.watchdog_stop(&old.meta.sid);
                        old.kill();
                        old.on_exit();
                        // Keep staged transcript files until the reloaded session ends.
                        term.meta.cleanup.append(&mut old.meta.cleanup);
                        term.meta.name = old.meta.name.clone();
                        term.meta.renamed = old.meta.renamed;
                    }
                    self.panes.insert(id, Box::new(term));
                    if let Some(route) = old_route {
                        if !self.panes.values().any(|p| p.as_term_ref().is_some_and(|t| t.meta.route_id.as_deref() == Some(&route))) {
                            self.svc.remove_route(&route);
                        }
                    }
                    self.agent_state.remove(&id);
                    self.done.remove(&id);
                    self.flips.remove(&id);
                    self.last_tick.remove(&id);
                    self.start_now(id);
                }
                None => {
                    // closed while it was building: end it the way closing a session does (route, staged files)
                    let id = self.add(Box::new(term));
                    self.close(id);
                }
            }
            if let Some(n) = note {
                self.toast(Kind::Info, n);
            }
            return;
        }
        let background = reply.is_some();
        let here = self.cur;
        let tiles = (self.stack.clone(), self.stack_focus, self.stack_zoom, self.tile_all, self.tile_project.clone());
        let id = self.open(Box::new(term), if self.tabs.is_empty() { Place::Tab } else { place });
        if background {
            // a remote client asked: start it now and stay where you are
            self.start_now(id);
            self.cur = here.min(self.tabs.len().saturating_sub(1));
            self.restore_background_tiles(tiles);
            if let Some(r) = reply {
                let _ = r.send(Ok(sid));
            }
        } else {
            self.leave_sidebar();
        }
        if let Some(n) = note {
            self.toast(Kind::Info, n);
        }
    }

    /// Spawn a term pane right away at the body size (it isn't on screen yet).
    pub(super) fn start_now(&mut self, id: PaneId) {
        let (rows, cols) = (self.body.height.saturating_sub(2).max(10), self.body.width.saturating_sub(2).max(40));
        let waker = Waker::new(id, self.tx.clone());
        if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
            t.ensure_started(rows, cols, waker);
        }
    }

    /// Open a plain shell tab in `cwd`.
    pub(crate) fn open_shell(&mut self, cwd: Option<PathBuf>, place: Place) -> PaneId {
        let settings = self.svc.settings();
        let (prog, args) = crate::util::default_shell(settings.shell.as_deref());
        let cwd = cwd.or_else(|| self.focused_cwd()).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let name = std::path::Path::new(&prog).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| prog.clone());
        let meta = Meta { sid: uuid::Uuid::new_v4().to_string(), harness: None, profile: None, store: None, model: None, label: name, name: None, renamed: false, project: self.svc.project_for(&cwd), cwd, started: Instant::now(), route_id: None, cleanup: vec![], orchestrator: false };
        let term = Term::new(meta, Spawn { program: prog, args, ..Spawn::default() }, self.svc.clone());
        self.open(Box::new(term), place)
    }

    /// Install an agent in a new tab running `bro install <agent>`, so its installer's output and
    /// prompts are visible. The launcher sees it as soon as it lands on PATH (or a known shim dir).
    pub(crate) fn install_agent(&mut self, h: Harness) -> Option<PaneId> {
        if self.svc.is_demo() {
            self.toast(Kind::Info, "the demo doesn't install anything");
            return None;
        }
        let cwd = self.focused_cwd().or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let term = crate::views::quick_install::install_term(&self.svc, cwd, &[h.label()]);
        Some(self.open(Box::new(term), Place::Tab))
    }

    /// Sign in to Gemini with a Google account (`bro gemini login` in a tab: it opens the browser and waits).
    pub(crate) fn sign_in_gemini(&mut self) -> Option<PaneId> {
        if self.svc.is_demo() {
            self.toast(Kind::Info, "the demo doesn't sign in to anything");
            return None;
        }
        let cwd = self.focused_cwd().or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let term = crate::views::quick_install::bro_term(&self.svc, cwd, "sign in · gemini", &["gemini", "login", "--wait"]);
        self.svc.watch_login("gemini-sub:local".into());
        Some(self.open(Box::new(term), Place::Tab))
    }

    /// The cwd of the focused session, if it is one.
    pub(crate) fn focused_cwd(&self) -> Option<PathBuf> {
        self.focused().and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()).map(|t| t.meta.cwd.clone())
    }

    pub(super) fn term_id(&self, sid: &str) -> Option<PaneId> {
        self.panes.iter().find(|(_, p)| p.as_term_ref().is_some_and(|t| t.meta.sid == sid)).map(|(id, _)| *id)
    }

    /// Execute a command from a remote client.
    pub(super) fn bridge_command(&mut self, c: BridgeCommand) {
        match c {
            BridgeCommand::FocusControls { id, change, reply } => {
                let result = self.term_id(&id).ok_or_else(|| "The session is no longer available.".to_string())
                    .and_then(|pane| self.focus_remote_controls(pane, change));
                let _ = reply.send(result);
            }
            BridgeCommand::Seen { id, event_id } => {
                if let Some(pane) = self.term_id(&id) { self.acknowledge_session(pane, &event_id); }
            }
            BridgeCommand::Input { id, data } => {
                if let Some(t) = self.term_id(&id).and_then(|pid| self.panes.get_mut(&pid)).and_then(|p| p.as_term()) {
                    t.send(&data);
                }
            }
            BridgeCommand::Write { id, data, reply } => {
                if reply.is_closed() { return; }
                let result = self.term_id(&id).and_then(|pid| self.panes.get_mut(&pid)).and_then(|p| p.as_term())
                    .ok_or_else(|| "The session is no longer available.".to_string())
                    .and_then(|term| term.send_confirmed(&data));
                let _ = reply.send(result);
            }
            BridgeCommand::Resize { id, cols, rows } => {
                // the local view wins while the pane is on screen
                if let Some(pid) = self.term_id(&id).filter(|pid| !self.visible().contains(pid))
                    && let Some(t) = self.panes.get_mut(&pid).and_then(|p| p.as_term()) {
                        t.resize(rows, cols);
                    }
            }
            BridgeCommand::Kill { id } => {
                if let Some(pid) = self.term_id(&id) {
                    self.close(pid);
                }
            }
            BridgeCommand::Rename { id, title } => {
                if let Some(t) = self.term_id(&id).and_then(|pid| self.panes.get_mut(&pid)).and_then(|p| p.as_term()) {
                    t.meta.name = (!title.trim().is_empty()).then(|| title.trim().to_string());
                    t.meta.renamed = t.meta.name.is_some();
                }
            }
            BridgeCommand::Focus { id } => {
                if let Some(pid) = self.term_id(&id) {
                    self.focus_pane(pid);
                }
            }
            BridgeCommand::OpenFile { id, path, line, column } => self.agent_opened_file(&id, path, line, column),
            BridgeCommand::Orchestrator(c) => self.orchestrator_command(c),
            BridgeCommand::CoordinationWake {task_id, revision, seq, project, action} => self.coordination_changed(&task_id, revision, seq, &project, &action),
            BridgeCommand::Tile { project } => {
                let tab = match project {
                    Some(dir) => super::project_tabs::StripTab::Project(self.svc.project_for(&dir).key),
                    None => super::project_tabs::StripTab::All,
                };
                self.show_tab(tab);
            }
            BridgeCommand::OpenInEditor { id, path, line, column } => {
                let pane = self.term_id(&id);
                self.open_in_editor(pane, path, line, column);
            }
            BridgeCommand::OpenOrchestrator { cwd, source, reply } => self.create_orchestrator(cwd, Some(&source), reply),
            BridgeCommand::Create { req, reply } if req.profile_id.as_deref() == Some(ORCHESTRATOR_PROFILE) => {
                let cwd = req.cwd.unwrap_or_else(bro_core::orchestrator::workspace);
                self.create_orchestrator(cwd, None, reply);
            }
            BridgeCommand::Create { req, reply } => {
                // no folder from the phone → the current project, like alt+n
                let cwd = req.cwd.clone().filter(|c| c.is_dir()).or_else(|| self.current_project()).or_else(dirs::home_dir).unwrap_or_default();
                match create_target(req.profile_id.as_deref()) {
                    Some((harness, profile_id)) => {
                        // a watchdog's second opinion asks for a model ("opus")
                        let spec = LaunchSpec { harness, profile_id, provider_id: None, model: req.model.clone(), cwd, resume: None, permission: Permission::Skip, browser: BrowserMode::Off, extra_args: req.args.clone(), orchestrator: false };
                        let mut lr = LaunchRequest::new(spec, Place::Tab);
                        lr.reply = Some(reply);
                        lr.name = req.title.clone();
                        self.svc.launch(lr);
                    }
                    None => {
                        let here = self.cur;
                        let tiles = (self.stack.clone(), self.stack_focus, self.stack_zoom, self.tile_all, self.tile_project.clone());
                        let id = match req.shell.clone() {
                            Some(shell) => {
                                let meta = Meta { sid: uuid::Uuid::new_v4().to_string(), harness: None, profile: None, store: None, model: None, label: std::path::Path::new(&shell).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| shell.clone()), name: req.title.clone(), renamed: req.title.is_some(), project: self.svc.project_for(&cwd), cwd, started: Instant::now(), route_id: None, cleanup: vec![], orchestrator: false };
                                let term = Term::new(meta, Spawn { program: shell, args: req.args.clone(), ..Spawn::default() }, self.svc.clone());
                                self.new_tab(Box::new(term))
                            }
                            None => self.open_shell(Some(cwd), Place::Tab),
                        };
                        self.start_now(id);
                        self.cur = here.min(self.tabs.len().saturating_sub(1));
                        self.restore_background_tiles(tiles);
                        let sid = self.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap_or_default();
                        let _ = reply.send(Ok(sid));
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ sidebar data

    /// A remote launch joins the grid it belongs in (every session's, or the project's on screen when it's
    /// one of its own) without stealing focus, unzooming it, or moving you to another project's tab.
    pub(super) fn restore_background_tiles(&mut self, before: (Vec<PaneId>, Option<PaneId>, bool, bool, Option<String>)) {
        let (stack, focus, zoom, tile_all, project) = before;
        if tile_all {
            self.tile_all = true;
            self.tile_project = project;
            self.stack = self.tile_ids(self.tile_project.as_deref());
            self.stack_focus = focus.filter(|id| self.stack.contains(id)).or_else(|| self.stack.first().copied());
            self.stack_zoom = zoom;
        } else {
            (self.stack, self.stack_focus, self.stack_zoom, self.tile_all, self.tile_project) = (stack, focus, zoom, false, None);
        }
    }

    /// At startup: start the orchestrator that was running when bro last stopped, with the same harness, login,
    /// model, permissions and browser.
    pub(crate) fn restore_orchestrator(&mut self) {
        if !self.persist || self.guest {
            return;
        }
        for spec in crate::orchestrator_state::load_all() {
            if self.orchestrator_for(&spec.cwd).is_none() {
                self.launch_spec(spec, Place::Tab);
            }
        }
    }

    /// bro's orchestrator session, when one is open.
    #[cfg(test)]
    pub(crate) fn orchestrator_pane(&self) -> Option<PaneId> {
        self.orchestrator_for(&bro_core::orchestrator::workspace())
            .or_else(|| self.current_project().and_then(|cwd| self.orchestrator_for(&cwd)))
            .or_else(|| self.orchestrator_panes().into_iter().next())
    }

    pub(crate) fn orchestrator_for(&self, cwd: &std::path::Path) -> Option<PaneId> {
        let key = bro_core::orchestrator::scope_key(cwd);
        self.panes.iter().find(|(_, p)| p.as_term_ref().is_some_and(|t| t.meta.orchestrator && t.meta.project.key == key)).map(|(id, _)| *id)
    }

    pub(crate) fn orchestrator_panes(&self) -> Vec<PaneId> {
        crate::sidebar::live_order(&self.live_infos(), &[], &self.open_infos()).into_iter()
            .filter(|id| self.panes.get(id).and_then(|p| p.as_term_ref()).is_some_and(|t| t.meta.orchestrator)).collect()
    }

    pub(crate) fn orchestrator_starting(&self, cwd: &std::path::Path) -> bool {
        self.orchestrator_pending.get(&bro_core::orchestrator::scope_key(cwd)).is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(90))
    }

    pub(super) fn inherited_orchestrator_spec(&self, source: &str, cwd: PathBuf) -> Option<LaunchSpec> {
        let t = self.term_id(source).and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()).filter(|t| t.meta.orchestrator)?;
        let mut spec = t.launch_spec.clone()?;
        // A model changed in the running conversation takes precedence over its original launch choice.
        spec.model = t.insights().and_then(|s| s.conversation).and_then(|c| c.model.clone()).or_else(|| t.meta.model.clone()).or(spec.model);
        spec.cwd = cwd;
        spec.resume = None;
        spec.orchestrator = true;
        Some(spec)
    }

    fn create_orchestrator(&mut self, cwd: PathBuf, source: Option<&str>, reply: tokio::sync::oneshot::Sender<anyhow::Result<String>>) {
        if self.guest {
            let _ = reply.send(Err(anyhow::anyhow!("Open orchestrators in the primary Bro window.")));
            return;
        }
        if !bro_core::orchestrator::is_global(&cwd) && !cwd.is_dir() {
            let _ = reply.send(Err(anyhow::anyhow!("The project folder does not exist: {}", cwd.display())));
            return;
        }
        let config = match source {
            Some(sid) => self.inherited_orchestrator_spec(sid, cwd.clone()),
            None => self.voice_orchestrator_spec(),
        };
        let Some(mut spec) = config else {
            let _ = reply.send(Err(anyhow::anyhow!("No launch configuration is available for the source orchestrator.")));
            return;
        };
        if let Some(t) = self.orchestrator_for(&cwd).and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()) {
            let _ = reply.send(Ok(t.meta.sid.clone()));
            return;
        }
        if self.orchestrator_starting(&cwd) {
            let _ = reply.send(Err(anyhow::anyhow!("The orchestrator is already starting in this project. Check list_sessions in a moment.")));
            return;
        }
        spec.cwd = cwd;
        spec.resume = None;
        spec.orchestrator = true;
        self.orchestrator_pending.insert(bro_core::orchestrator::scope_key(&spec.cwd), Instant::now());
        let mut request = LaunchRequest::new(spec, Place::Tab);
        request.reply = Some(reply);
        request.remember = true;
        self.svc.launch(request);
    }

    /// Live sessions for the sidebar.
    pub(crate) fn live_infos(&self) -> Vec<LiveInfo> {
        self.panes
            .iter()
            .filter_map(|(id, p)| {
                let t = p.as_term_ref()?;
                Some(LiveInfo {
                    pane: *id,
                    harness: t.meta.harness,
                    name: t.display_title(),
                    profile: t.meta.profile.clone(),
                    model: t.meta.model.clone(),
                    project_key: t.meta.project.key.clone(),
                    project_name: t.meta.project.name.clone(),
                    project_root: t.meta.project.root.clone(),
                    activity: p.activity(),
                    done: self.done.contains(id),
                    note: self.notes.get(id).cloned(),
                    age_secs: t.meta.started.elapsed().as_secs(),
                    seq: self.seqs.get(id).copied().unwrap_or(*id),
                    orchestrator: t.meta.orchestrator,
                    team: t.team().filter(|team| !team.is_empty()),
                })
            })
            .collect()
    }

    /// Past sessions for the sidebar.
    pub(crate) fn past_infos(&self) -> Vec<PastInfo> {
        let st = self.svc.state();
        let Some(past) = st.past.ready() else { return vec![] };
        past.iter()
            .filter_map(|s| {
                // archived sessions don't count for the sidebar (continue picker shows them on request)
                let archived = self.archive.contains(&s.id);
                if archived {
                    return None;
                }
                let pk = s.project.clone().or_else(|| s.cwd.as_ref().map(|c| self.svc.project_for(c)))?;
                Some(PastInfo { age_secs: crate::util::secs_since(s.modified), project_key: pk.key, project_root: pk.root })
            })
            .collect()
    }

    /// The opened projects, resolved to project keys.
    pub(crate) fn open_infos(&self) -> Vec<sidebar::OpenInfo> {
        self.open_projects
            .roots
            .iter()
            .map(|r| {
                let pk = self.svc.project_for(r);
                sidebar::OpenInfo { key: pk.key, name: pk.name, root: pk.root }
            })
            .collect()
    }

    /// The sidebar's projects for the Skills view while it's open (it lists their own skills): a change looks again.
    pub(crate) fn publish_projects(&self) {
        if !self.panes.values().any(|p| p.view() == Some("skills")) {
            return;
        }
        let projects = sidebar::projects(&self.live_infos(), &self.open_infos()).into_iter().map(|(key, name, root)| crate::services::KnownProject { key, name, root }).collect();
        if self.svc.set_projects(projects) {
            self.svc.rescan_skills();
        }
    }

    /// What the Tasks view reads from the app while it's open: the sessions' names and where a new task starts
    /// (the project New Session opens in, so reopening New follows the latest interaction). The folders the
    /// chooser lists come from past sessions, so they're rebuilt at most every few seconds.
    pub(crate) fn publish_task_context(&mut self) {
        if !self.panes.values().any(|p| p.view() == Some("tasks")) {
            return;
        }
        self.svc.set_coordination_workers(self.panes.values().filter_map(|p| p.as_term_ref().map(|t| (t.meta.sid.clone(), t.display_name()))).collect());
        let default = self.preferred_dir();
        let stale = self.task_choices_at.is_none_or(|at| at.elapsed() > Duration::from_secs(5));
        let known = self.svc.task_projects();
        if !stale && known.default == default {
            return;
        }
        let choices = if stale {
            self.task_choices_at = Some(Instant::now());
            self.launcher_projects()
        } else {
            known.choices
        };
        self.svc.set_task_projects(crate::services::TaskProjects { default, choices });
    }

    /// Demo: open the demo projects (a real run opens its start folder via [`App::start_in`]).
    pub(crate) fn seed_projects(&mut self) {
        if self.svc.is_demo() {
            let roots: Vec<std::path::PathBuf> = self.past_infos().into_iter().map(|p| p.project_root).collect();
            for r in crate::folder::dedup(roots) {
                self.open_projects.add(r);
            }
        }
    }

    /// The folder bro was started in becomes (and stays) an open project, and the current one.
    pub(crate) fn start_in(&mut self, dir: std::path::PathBuf) {
        let root = self.svc.project_for(&dir).root;
        if self.open_projects.add(root.clone()) {
            self.open_projects.save(self.persist);
        }
        self.pick_project(root);
    }

    /// The project a sidebar row belongs to.
    pub(crate) fn row_root(&self, _rows: &[Row], r: &Row) -> Option<std::path::PathBuf> {
        match r {
            Row::Project { root, .. } => Some(root.clone()),
            Row::Live { info, .. } => (info.project_key != crate::panes::term::ORCHESTRATOR_KEY).then(|| info.project_root.clone()),
        }
    }

    /// Where new sessions start: the sidebar selection while you browse it, else the project you last picked
    /// or worked in, else the focused session's, else the first opened project.
    pub(crate) fn current_project(&self) -> Option<std::path::PathBuf> {
        if self.side_focus {
            let rows = self.rows();
            if let Some(root) = rows.get(self.side_sel).and_then(|r| self.row_root(&rows, r)) {
                return Some(root);
            }
        }
        self.cur_project
            .clone()
            .or_else(|| self.focused().and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()).filter(|t| t.meta.project.key != crate::panes::term::ORCHESTRATOR_KEY).map(|t| t.meta.project.root.clone()))
            .or_else(|| self.open_projects.roots.first().cloned())
    }

    /// Open projects first, then folders from earlier sessions for the launcher's project chooser.
    pub(crate) fn launcher_projects(&self) -> Vec<PathBuf> {
        let mut dirs = self.open_projects.roots.clone();
        dirs.extend(self.past_infos().into_iter().map(|p| p.project_root));
        dirs.extend(crate::recents::dirs(&self.recents));
        dirs.extend(std::env::current_dir().ok());
        crate::folder::dedup(dirs)
    }

    /// Use a project in the current launcher without resetting its agent, login, or model choices.
    pub(crate) fn select_launcher_project(&mut self, dir: PathBuf) {
        let project = self.svc.project_for(&dir);
        let root = project.root.clone();
        let added = self.open_projects.add(root.clone());
        if added {
            self.open_projects.save(self.persist);
        }
        self.pick_project(root.clone());
        if let Some(i) = self.rows().iter().position(|r| matches!(r, Row::Project { root: r, .. } if *r == root)) {
            self.side_sel = i;
        }
        if let Overlay::Launcher(l) = &mut self.overlay {
            l.select_project(dir);
            l.continue_picker.set_project(Some((project.key, project.name)));
        }
        if added {
            let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            self.toast(Kind::Info, format!("opened {name}"));
        }
    }

    /// Create a typed folder before it becomes a project or a session's working directory.
    pub(crate) fn create_project_folder(&mut self, dir: PathBuf, request: uuid::Uuid) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::metadata(&dir))
                .map_err(|error| format!("couldn't create folder: {error}"))
                .and_then(|meta| if meta.is_dir() { Ok(dir) } else { Err("that path is a file; choose a folder".into()) });
            let _ = tx.send(crate::pane::Event::FolderCreated(request, result));
        });
    }

    pub(crate) fn project_folder_created(&mut self, request: uuid::Uuid, result: Result<PathBuf, String>) {
        if let Overlay::Launcher(l) = &mut self.overlay {
            let Some(p) = &mut l.project_picker else { return };
            if p.pending != Some(request) {
                return;
            }
            p.pending = None;
            match result {
                Ok(dir) => self.select_launcher_project(dir),
                Err(error) => p.error = Some(error),
            }
            return;
        }
    }

    /// Group the folder under its git root and open the session picker in the selected directory.
    pub(crate) fn add_project(&mut self, dir: std::path::PathBuf) {
        let root = self.svc.project_for(&dir).root;
        let added = self.open_projects.add(root.clone());
        self.open_projects.save(self.persist);
        self.pick_project(root.clone());
        let rows = self.rows();
        if let Some(i) = rows.iter().position(|r| matches!(r, Row::Project { root: r, .. } if *r == root)) {
            self.side_sel = i;
            self.side_focus = true;
        }
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.toast(Kind::Info, if added { format!("opened {name}") } else { format!("{name} is already open") });
        self.open_launcher(Some(dir), Place::Tab);
    }

    /// x on a project row: take it off the sidebar (the folder and its sessions are untouched).
    pub(crate) fn close_project(&mut self, root: std::path::PathBuf, live: usize) {
        if live > 0 {
            let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            self.overlay = Overlay::Confirm(super::overlays::Confirm {
                text: format!("close {name} and its {live} running session{}?", if live == 1 { "" } else { "s" }),
                detail: "the agents stop; their conversations stay resumable (continue session)".into(),
                action: super::overlays::ConfirmAction::CloseProject(root),
            });
            return;
        }
        self.close_project_now(root);
    }

    /// Close a project's running sessions and take it off the sidebar.
    pub(crate) fn close_project_now(&mut self, root: std::path::PathBuf) {
        let key = self.svc.project_for(&root).key;
        let panes: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.as_term_ref().is_some_and(|t| t.meta.project.key == key)).map(|(id, _)| *id).collect();
        for id in panes {
            self.close(id);
        }
        {
            self.open_projects.remove(&root);
            self.open_projects.save(self.persist);
            if self.cur_project.as_ref() == Some(&root) {
                self.cur_project = None;
            }
            let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            self.toast(Kind::Info, format!("closed {name} · choose it from New session"));
        }
    }

    /// The sidebar rows right now.
    pub(crate) fn rows(&self) -> Vec<Row> {
        sidebar::build(&self.live_infos(), &self.past_infos(), &self.open_infos(), &self.side)
    }

    fn past_session(&self, idx: usize) -> Option<SessionInfo> {
        self.svc.state().past.ready().and_then(|p| p.get(idx).cloned())
    }

    /// Resume a past session in its own profile.
    pub(crate) fn resume(&mut self, idx: usize) {
        let Some(s) = self.past_session(idx) else { return };
        self.svc.launch(resume_request(&s));
        self.toast(Kind::Info, format!("resuming “{}”…", crate::ui::fit(&s.title, 40)));
    }

    /// Resume several earlier sessions, each in its own login: their panes open now with a loader and
    /// the sessions start one at a time. Two or more show side by side.
    pub(crate) fn resume_many(&mut self, idx: &[usize]) {
        let sessions: Vec<SessionInfo> = idx.iter().filter_map(|&i| self.past_session(i)).collect();
        if sessions.is_empty() {
            return;
        }
        let mut ahead = self.resume_queue.waiting.len() + usize::from(self.resume_queue.starting.is_some());
        let mut ids = vec![];
        for s in &sessions {
            let mut req = resume_request(s);
            let spec = &req.spec;
            let meta = Meta {
                sid: uuid::Uuid::new_v4().to_string(),
                harness: Some(spec.harness),
                profile: spec.profile_id.clone(),
                store: transcript_store(spec),
                model: None,
                label: crate::services::launch::label_for(spec),
                name: req.name.clone(),
                renamed: false,
                project: self.svc.project_for(&spec.cwd),
                cwd: spec.cwd.clone(),
                started: Instant::now(),
                route_id: None,
                cleanup: vec![],
                orchestrator: false,
            };
            let id = self.open(Box::new(Term::queued(meta, ahead, self.svc.clone())), Place::Tab);
            ahead += 1;
            req.into = Some(id);
            self.resume_queue.waiting.push_back((id, req));
            ids.push(id);
        }
        if ids.len() >= 2 && !self.tile_all {
            self.stack = ids.clone();
            self.stack_focus = ids.first().copied();
            self.stack_zoom = false;
        } else {
            self.focus_pane(ids[0]);
        }
        self.leave_sidebar();
        let n = sessions.len();
        self.toast(Kind::Info, if n == 1 { format!("resuming “{}”…", crate::ui::fit(&sessions[0].title, 40)) } else { format!("resuming {n} sessions, one at a time") });
        self.pump_resumes();
    }

    /// A queued resume reported back (started or failed): the next one may go after a short gap.
    fn resume_started(&mut self, id: PaneId) {
        if self.resume_queue.starting.is_some_and(|(s, _)| s == id) {
            self.resume_queue.starting = None;
            self.resume_queue.next_at = Some(Instant::now() + RESUME_GAP);
        }
        self.pump_resumes();
    }

    /// Launch the next queued resume when its turn comes, and keep each placeholder's place in line current.
    pub(super) fn pump_resumes(&mut self) {
        let q = &mut self.resume_queue;
        if q.waiting.is_empty() && q.starting.is_none() {
            return;
        }
        let panes = &self.panes;
        q.waiting.retain(|(id, _)| panes.contains_key(id));
        if q.starting.is_some_and(|(_, at)| at.elapsed() >= RESUME_STUCK) {
            q.starting = None;
        }
        let due = q.starting.is_none() && q.next_at.is_none_or(|t| Instant::now() >= t);
        if due && let Some((id, req)) = q.waiting.pop_front() {
            q.starting = Some((id, Instant::now()));
            q.next_at = None;
            self.svc.launch(req);
        }
        let q = &self.resume_queue;
        let mut line: Vec<(PaneId, usize, bool)> = q.starting.iter().map(|(id, _)| (*id, 0, true)).collect();
        let offset = line.len();
        line.extend(q.waiting.iter().enumerate().map(|(k, (id, _))| (*id, k + offset, false)));
        for (id, ahead, starting) in line {
            if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term())
                && let Some(queued) = &mut t.queued
            {
                queued.ahead = ahead;
                queued.starting = starting;
            }
        }
    }

    /// The logins a session of `harness` can continue in: the current one first, then the rest by what's left.
    pub(crate) fn resume_targets(&self, harness: Harness, current: Option<&str>) -> Vec<super::overlays::ResumeTarget> {
        let st = self.svc.state();
        let Some(ps) = st.profiles.ready() else { return vec![] };
        let left_of = |id: &str| -> Option<f64> {
            let p = ps.iter().find(|p| p.id == id)?;
            let u = st.usage.get(id)?.usage.as_ref()?;
            let d = bro_core::usage::headroom_detail(p, u);
            [d.h5, d.wk].into_iter().flatten().reduce(f64::min)
        };
        let mut v: Vec<super::overlays::ResumeTarget> = ps
            .iter()
            .filter(|p| p.authenticated && if harness == Harness::Codex { p.is_codex() } else { p.is_claude() })
            .map(|p| super::overlays::ResumeTarget {
                profile_id: p.id.clone(),
                name: p.name.clone(),
                detail: [p.plan.clone().unwrap_or_default(), p.email.clone().unwrap_or_default()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "),
                left: left_of(&p.id),
                current: Some(p.id.as_str()) == current,
            })
            .collect();
        v.sort_by(|a, b| b.current.cmp(&a.current).then(b.left.unwrap_or(-1.0).total_cmp(&a.left.unwrap_or(-1.0))));
        v
    }

    /// Start on the current login unless it's nearly out (under 10% left) — then on the roomiest one.
    pub(crate) fn resume_default(targets: &[super::overlays::ResumeTarget]) -> usize {
        match targets.first() {
            Some(t) if t.current && t.left.is_some_and(|l| l < 10.0) => targets.iter().position(|t| !t.current && t.left.is_none_or(|l| l >= 10.0)).unwrap_or(0),
            _ => 0,
        }
    }

    /// Enter / f on an earlier session: pick the login to continue in (straight in when there's only one).
    pub(crate) fn open_resume(&mut self, idx: usize) {
        let Some(s) = self.past_session(idx) else { return };
        if matches!(s.harness, Harness::Pi | Harness::Omp) {
            self.resume(idx);
            return;
        }
        let targets = self.resume_targets(s.harness, s.profile_id.as_deref());
        if targets.len() <= 1 {
            self.resume(idx);
            return;
        }
        let sel = Self::resume_default(&targets);
        self.overlay = Overlay::Resume(Box::new(super::overlays::ResumePicker { from: super::overlays::ResumeFrom::Past(s), targets, sel, hits: vec![] }));
    }

    /// Move a running Claude / Codex session to another login.
    pub(crate) fn open_switch(&mut self, pane: PaneId) {
        let Some(t) = self.panes.get(&pane).and_then(|p| p.as_term_ref()) else { return };
        let (Some(harness), Some(store)) = (t.meta.harness, t.meta.store.clone()) else {
            self.toast(Kind::Info, "only Claude and Codex sessions can move to another login");
            return;
        };
        let since = std::time::SystemTime::now() - t.meta.started.elapsed();
        let title = t.meta.name.clone().unwrap_or_else(|| t.meta.label.clone());
        let cwd = t.meta.cwd.clone();
        let targets = self.resume_targets(harness, Some(&store));
        if targets.len() <= 1 {
            self.toast(Kind::Info, format!("no other {} login to move to — add one in profiles (alt+o)", harness.label()));
            return;
        }
        let sel = Self::resume_default(&targets).max(usize::from(targets[0].current).min(targets.len() - 1));
        let from = super::overlays::ResumeFrom::Live { pane, harness, store, cwd, since, title };
        self.overlay = Overlay::Resume(Box::new(super::overlays::ResumePicker { from, targets, sel, hits: vec![] }));
    }

    /// Reload an agent in place after choosing its profile, even when there is only one.
    pub(crate) fn open_reload(&mut self, pane: PaneId) {
        use super::overlays::{ResumeFrom, ResumePicker, ResumeTarget};
        let Some(t) = self.panes.get(&pane).and_then(|p| p.as_term_ref()) else {
            self.toast(Kind::Info, "choose an agent session to reload");
            return;
        };
        let Some(harness) = t.meta.harness else {
            self.toast(Kind::Info, "choose an agent session to reload");
            return;
        };
        if t.queued.is_some() || t.reloading {
            self.toast(Kind::Info, "this session is already starting");
            return;
        }
        let spec = t.launch_spec.clone().unwrap_or_else(|| LaunchSpec {
            harness, profile_id: t.meta.profile.clone(), provider_id: None, model: t.meta.model.clone(),
            cwd: t.meta.cwd.clone(), resume: None, permission: self.svc.settings().permission,
            browser: BrowserMode::Off, extra_args: vec![], orchestrator: t.meta.orchestrator,
        });
        let current = spec.provider_id.as_deref().or(spec.profile_id.as_deref());
        let store = t.meta.store.clone();
        let since = std::time::SystemTime::now() - t.meta.started.elapsed();
        let title = t.meta.name.clone().unwrap_or_else(|| t.meta.label.clone());
        let mut targets = if matches!(harness, Harness::Claude | Harness::Codex) {
            self.resume_targets(harness, current)
        } else {
            self.svc.state().profiles.ready().into_iter().flatten().filter(|p| p.authenticated).map(|p| ResumeTarget {
                profile_id: p.id.clone(), name: p.name.clone(), detail: p.email.clone().unwrap_or_default(),
                left: None, current: Some(p.id.as_str()) == current,
            }).collect()
        };
        if !targets.iter().any(|t| t.current) {
            let id = current.unwrap_or("").to_string();
            targets.insert(0, ResumeTarget {
                name: if id.is_empty() { "default".into() } else { id.clone() },
                profile_id: id, detail: "current session profile".into(), left: None, current: true,
            });
        }
        let sel = targets.iter().position(|t| t.current).unwrap_or(0);
        let from = ResumeFrom::Reload { pane, spec, store, since, title };
        self.overlay = Overlay::Resume(Box::new(ResumePicker { from, targets, sel, hits: vec![] }));
    }

    /// Enter in the resume picker.
    pub(crate) fn resume_chosen(&mut self) {
        let Overlay::Resume(p) = std::mem::take(&mut self.overlay) else { return };
        let Some(target) = p.targets.get(p.sel) else { return };
        let (profile, current) = (target.profile_id.clone(), target.current);
        match p.from {
            super::overlays::ResumeFrom::Past(s) => {
                if current {
                    let idx = self.svc.state().past.ready().and_then(|v| v.iter().position(|x| x.id == s.id));
                    if let Some(idx) = idx {
                        self.resume(idx);
                    }
                } else {
                    self.fork_into(s, profile);
                }
            }
            super::overlays::ResumeFrom::Live { pane, harness, store, cwd, since, title } => {
                if current {
                    return;
                }
                self.close(pane);
                let spec = LaunchSpec { harness, profile_id: Some(profile.clone()), provider_id: None, model: None, cwd: cwd.clone(), resume: None, permission: Permission::Skip, browser: BrowserMode::Off, extra_args: vec![], orchestrator: false };
                let mut req = LaunchRequest::new(spec, Place::Tab);
                req.name = Some(crate::ui::fit(&title, 28));
                req.find_live = Some(crate::services::launch::FindLive { harness, store: Some(store), cwd, since, session_id: None });
                self.svc.launch(req);
                self.toast(Kind::Info, format!("moving to {profile}\u{2026}"));
            }
            super::overlays::ResumeFrom::Reload { pane, mut spec, store, since, title } => {
                let Some(t) = self.panes.get_mut(&pane).and_then(|p| p.as_term()) else { return };
                if t.reloading { return; }
                t.reloading = true;
                if !current {
                    spec.profile_id = Some(profile.clone());
                    spec.provider_id = None;
                    spec.model = None;
                }
                let session_id = t.insights().and_then(|s| s.session_id).or_else(|| spec.resume.as_ref().filter(|r| !r.fork).map(|r| r.session_id.clone()));
                let find = crate::services::launch::FindLive { harness: spec.harness, store, cwd: spec.cwd.clone(), since, session_id };
                spec.resume = None;
                let mut req = LaunchRequest::new(spec, Place::Tab);
                req.name = Some(title);
                req.find_live = Some(find);
                req.into = Some(pane);
                self.svc.launch(req);
                self.toast(Kind::Info, "reloading session…");
            }
        }
    }

    /// Stage `s` into `target` and resume it there as a fork.
    pub(super) fn fork_into(&mut self, s: SessionInfo, target: String) {
        let cwd = s.cwd.clone().or_else(|| s.project.as_ref().map(|p| p.root.clone())).or_else(dirs::home_dir).unwrap_or_default();
        let spec = LaunchSpec { harness: s.harness, profile_id: Some(target.clone()), provider_id: None, model: None, cwd, resume: Some(Resume { session_id: s.id.clone(), fork: true }), permission: Permission::Skip, browser: BrowserMode::Off, extra_args: vec![], orchestrator: false };
        let mut req = LaunchRequest::new(spec, Place::Tab);
        req.name = Some(crate::ui::fit(&s.title, 28));
        req.stage = Some(s);
        self.svc.launch(req);
        self.toast(Kind::Info, format!("resuming in {target}\u{2026}"));
    }

    // ------------------------------------------------------------------ agents

    /// Notice agents finishing or getting stuck while you aren't looking at them.
    pub(super) fn track_agents(&mut self) {
        let mut notes = vec![];
        for (id, p) in &self.panes {
            let Some(a) = p.activity() else { continue };
            let prev = self.agent_state.insert(*id, a);
            let seen = self.focused() == Some(*id) && self.term_focused && !self.side_focus && !self.overlay.is_open();
            if prev == Some(Activity::Working) && a == Activity::Idle && !seen {
                self.done.insert(*id);
                notes.push((Kind::AgentDone, *id, format!("{} finished", p.title())));
            }
            if a == Activity::Blocked && prev.is_some_and(|x| x != Activity::Blocked) && !seen {
                notes.push((Kind::NeedsYou, *id, format!("{} needs you", p.title())));
            }
            if seen {
                self.done.remove(id);
            }
        }
        self.agent_state.retain(|id, _| self.panes.contains_key(id));
        self.done.retain(|id| self.panes.contains_key(id));
        for (k, id, text) in notes {
            // say why it waits / what it did, when the transcript tells
            let view = self.pane_view(id);
            let extra = match k {
                Kind::NeedsYou => view.as_ref().and_then(|v| v.reason.clone()),
                _ => view.as_ref().and_then(|v| v.reason.clone().or_else(|| v.recap.as_ref().map(|r| r.text.clone()))),
            };
            let text = match extra {
                Some(x) => format!("{text} · {}", crate::ui::fit(&x.split_whitespace().collect::<Vec<_>>().join(" "), 72)),
                None => text,
            };
            self.raise(k, text);
        }
    }

    /// The next session that needs you, in sidebar order after the focused one (so pressing again moves on):
    /// one waiting on you first, then one that finished unseen.
    pub(crate) fn attention_target(&self) -> Option<PaneId> {
        let mut order = sidebar::live_order(&self.live_infos(), &self.past_infos(), &self.open_infos());
        let cur = self.focused();
        if let Some(i) = cur.and_then(|c| order.iter().position(|id| *id == c)) {
            order.rotate_left(i + 1);
        }
        let pick = |want: &dyn Fn(PaneId) -> bool| order.iter().copied().filter(|id| Some(*id) != cur).find(|id| want(*id));
        pick(&|id| self.panes.get(&id).and_then(|p| p.activity()) == Some(Activity::Blocked) && self.session_notice(id).is_none_or(|n| n.unread))
            .or_else(|| pick(&|id| self.done.contains(&id)))
    }

    /// `bro --demo`: a few sessions across three projects.
    pub(super) fn open_demo_sessions(&mut self) {
        let root = crate::services::demo::root();
        for (i, d) in crate::services::demo::sessions().into_iter().enumerate() {
            let cwd = root.join(d.project);
            let label = match d.harness {
                Some(h) => {
                    let mut parts = vec![h.label().to_string()];
                    if let Some(p) = d.profile {
                        parts.push(p.split(':').next_back().unwrap_or(p).to_string());
                    }
                    if let Some(m) = d.model {
                        parts.push(crate::services::launch::short_model(m.rsplit('/').next().unwrap_or(m)));
                    }
                    parts.join(" · ")
                }
                None => "pwsh".into(),
            };
            let meta = Meta {
                sid: format!("demo-{i}"),
                harness: d.harness,
                profile: d.profile.map(String::from),
                store: d.profile.filter(|p| p.contains(':')).map(String::from),
                model: d.model.map(String::from),
                label: label.clone(),
                name: None,
                renamed: false,
                project: self.svc.project_for(&cwd),
                cwd: cwd.clone(),
                started: Instant::now() - std::time::Duration::from_secs([420, 1_900, 60, 5_400, 12_000][i % 5]),
                route_id: None,
                cleanup: vec![],
                orchestrator: false,
            };
            let mut term = if self.opts.fixed_demo {
                Term::fixed(meta, &crate::services::demo::transcript(d.harness), self.svc.clone())
            } else {
                // the session that waits on you shows its prompt at the bottom, as Claude Code draws it
                let c = if d.activity == Some(crate::pane::Activity::Blocked) {
                    crate::services::demo::shell_showing(d.harness, &label, cwd, &crate::services::demo::blocked_screen(), Some("blocked"))
                } else {
                    crate::services::demo::shell_command(d.harness, &label, cwd)
                };
                Term::new(meta, Spawn { program: c.program, args: c.args, ..Spawn::default() }, self.svc.clone())
            };
            term.demo_activity = d.activity;
            // the first Claude session leads an agent team (not in the fixed demo the tests draw)
            if i == 0 && !self.opts.fixed_demo {
                term.set_demo_team(crate::services::demo::team());
            }
            // what the agents said and did, for focus mode (not in the fixed demo either)
            if !self.opts.fixed_demo
                && let Some(jsonl) = crate::services::demo::conversation(i)
            {
                term.set_demo_transcript(&jsonl);
            }
            // the shell shares the first tab as a split; the rest get tabs
            let id = if i == 2 {
                self.cur = 0;
                if let Some(t) = self.tabs.first_mut() {
                    t.focus = t.root.leaf_ids()[0];
                }
                self.open(Box::new(term), Place::SplitRight)
            } else {
                self.new_tab(Box::new(term))
            };
            if d.done {
                self.done.insert(id);
            }
        }
        self.cur = 0;
        if let Some(t) = self.tabs.first_mut() {
            t.focus = t.root.leaf_ids()[0];
        }
    }
}

/// Resume `s` in its own login, named after it.
fn resume_request(s: &SessionInfo) -> LaunchRequest {
    let cwd = s.cwd.clone().or_else(|| s.project.as_ref().map(|p| p.root.clone())).or_else(dirs::home_dir).unwrap_or_default();
    let spec = LaunchSpec { harness: s.harness, profile_id: s.profile_id.clone(), provider_id: None, model: None, cwd, resume: Some(Resume { session_id: s.id.clone(), fork: false }), permission: Permission::Skip, browser: BrowserMode::Off, extra_args: vec![], orchestrator: false };
    let mut req = LaunchRequest::new(spec, Place::Tab);
    req.name = Some(crate::ui::fit(&s.title, 28));
    req
}

/// Where a launch keeps its transcript: its own login when it's the harness's kind, else the machine's own.
pub(crate) fn transcript_store(spec: &LaunchSpec) -> Option<String> {
    crate::services::launch::transcript_store(spec)
}
