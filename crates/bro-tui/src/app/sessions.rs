//! Sessions: turning launch results into PTY panes, the sidebar's view of them, resume/fork, remote (bridge)
//! commands, and agent tracking (done-while-unseen, needs-you).

use super::App;
use super::overlays::{ForkPicker, Overlay};
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
use std::path::PathBuf;
use std::time::Instant;

/// What a remote create's `profile_id` asks for: an agent (harness + bro profile) or None for a shell.
/// Accepts bro profile ids ("claude:work"), bare harness names ("claude", "pi") and "shell".
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
        let Launched { spec, place, reply, name, remember, result, route_id, note } = l;
        let cmd = match result {
            Ok(c) => c,
            Err(e) => {
                self.raise(Kind::Error, format!("couldn't launch {}: {e}", spec.harness.label()));
                if let Some(r) = reply {
                    let _ = r.send(Err(anyhow::anyhow!(e)));
                }
                return;
            }
        };
        if remember {
            crate::recents::push(
                &mut self.recents,
                crate::recents::Recent { harness: spec.harness, profile_id: spec.profile_id.clone(), provider_id: spec.provider_id.clone(), model: spec.model.clone(), cwd: spec.cwd.clone(), permission: spec.permission, browser: spec.browser, at: crate::util::now_secs() },
            );
            if !self.svc.is_demo() {
                crate::recents::save(self.recents.clone());
            }
        }
        let cwd = if cmd.cwd.as_os_str().is_empty() { spec.cwd.clone() } else { cmd.cwd.clone() };
        let label = if cmd.label.is_empty() { crate::services::launch::label_for(&spec) } else { cmd.label.clone() };
        let meta = Meta {
            sid: uuid::Uuid::new_v4().to_string(),
            harness: Some(spec.harness),
            profile: spec.provider_id.clone().or(spec.profile_id.clone()),
            model: spec.model.clone(),
            label,
            name,
            project: self.svc.project_for(&cwd),
            cwd,
            started: Instant::now(),
            route_id,
            cleanup: cmd.cleanup.clone(),
        };
        let sid = meta.sid.clone();
        let term = Term::new(meta, Spawn { program: cmd.program, args: cmd.args, env: cmd.env, env_remove: cmd.env_remove }, self.svc.clone());
        let background = reply.is_some();
        let here = self.cur;
        let id = self.open(Box::new(term), if self.tabs.is_empty() { Place::Tab } else { place });
        if background {
            // a remote client asked: start it now and stay where you are
            self.start_now(id);
            self.cur = here.min(self.tabs.len().saturating_sub(1));
            if let Some(r) = reply {
                let _ = r.send(Ok(sid));
            }
        } else {
            self.side_focus = false;
        }
        if let Some(n) = note {
            self.toast(Kind::Info, n);
        }
    }

    /// Spawn a term pane right away at the body size (it isn't on screen yet).
    pub(super) fn start_now(&mut self, id: PaneId) {
        let (rows, cols) = (self.body.height.saturating_sub(2).max(10), self.body.width.saturating_sub(2).max(40));
        let waker = Waker { id, tx: self.tx.clone() };
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
        let meta = Meta { sid: uuid::Uuid::new_v4().to_string(), harness: None, profile: None, model: None, label: name, name: None, project: self.svc.project_for(&cwd), cwd, started: Instant::now(), route_id: None, cleanup: vec![] };
        let term = Term::new(meta, Spawn { program: prog, args, ..Spawn::default() }, self.svc.clone());
        self.open(Box::new(term), place)
    }

    /// The cwd of the focused session, if it is one.
    pub(crate) fn focused_cwd(&self) -> Option<PathBuf> {
        self.focused().and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()).map(|t| t.meta.cwd.clone())
    }

    fn term_id(&self, sid: &str) -> Option<PaneId> {
        self.panes.iter().find(|(_, p)| p.as_term_ref().is_some_and(|t| t.meta.sid == sid)).map(|(id, _)| *id)
    }

    /// Execute a command from a remote client.
    pub(super) fn bridge_command(&mut self, c: BridgeCommand) {
        match c {
            BridgeCommand::Input { id, data } => {
                if let Some(t) = self.term_id(&id).and_then(|pid| self.panes.get_mut(&pid)).and_then(|p| p.as_term()) {
                    t.send(&data);
                }
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
                }
            }
            BridgeCommand::Focus { id } => {
                if let Some(pid) = self.term_id(&id) {
                    self.focus_pane(pid);
                }
            }
            BridgeCommand::Create { req, reply } => {
                let cwd = req.cwd.clone().filter(|c| c.is_dir()).or_else(dirs::home_dir).unwrap_or_default();
                match create_target(req.profile_id.as_deref()) {
                    Some((harness, profile_id)) => {
                        let spec = LaunchSpec { harness, profile_id, provider_id: None, model: None, cwd, resume: None, permission: Permission::Default, browser: BrowserMode::Off, extra_args: req.args.clone() };
                        let mut lr = LaunchRequest::new(spec, Place::Tab);
                        lr.reply = Some(reply);
                        lr.name = req.title.clone();
                        self.svc.launch(lr);
                    }
                    None => {
                        let here = self.cur;
                        let id = match req.shell.clone() {
                            Some(shell) => {
                                let meta = Meta { sid: uuid::Uuid::new_v4().to_string(), harness: None, profile: None, model: None, label: shell.clone(), name: req.title.clone(), project: self.svc.project_for(&cwd), cwd, started: Instant::now(), route_id: None, cleanup: vec![] };
                                let term = Term::new(meta, Spawn { program: shell, args: req.args.clone(), ..Spawn::default() }, self.svc.clone());
                                self.new_tab(Box::new(term))
                            }
                            None => self.open_shell(Some(cwd), Place::Tab),
                        };
                        self.start_now(id);
                        self.cur = here.min(self.tabs.len().saturating_sub(1));
                        let sid = self.panes.get(&id).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap_or_default();
                        let _ = reply.send(Ok(sid));
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ sidebar data

    /// Live sessions for the sidebar.
    pub(crate) fn live_infos(&self) -> Vec<LiveInfo> {
        self.panes
            .iter()
            .filter_map(|(id, p)| {
                let t = p.as_term_ref()?;
                Some(LiveInfo {
                    pane: *id,
                    harness: t.meta.harness,
                    name: t.meta.name.clone(),
                    profile: t.meta.profile.clone(),
                    model: t.meta.model.clone(),
                    project_key: t.meta.project.key.clone(),
                    project_name: t.meta.project.name.clone(),
                    project_root: t.meta.project.root.clone(),
                    activity: p.activity(),
                    done: self.done.contains(id),
                    age_secs: t.meta.started.elapsed().as_secs(),
                    seq: self.seqs.get(id).copied().unwrap_or(*id),
                })
            })
            .collect()
    }

    /// Past sessions for the sidebar.
    pub(crate) fn past_infos(&self) -> Vec<PastInfo> {
        let st = self.svc.state();
        let Some(past) = st.past.ready() else { return vec![] };
        past.iter()
            .enumerate()
            .filter_map(|(idx, s)| {
                let pk = s.project.clone().or_else(|| s.cwd.as_ref().map(|c| self.svc.project_for(c)))?;
                Some(PastInfo { idx, harness: s.harness, profile: s.profile_id.clone(), title: s.title.clone(), age_secs: crate::util::secs_since(s.modified), project_key: pk.key, project_name: pk.name, project_root: pk.root })
            })
            .collect()
    }

    /// The sidebar rows right now.
    pub(crate) fn rows(&self) -> Vec<Row> {
        sidebar::build(&self.live_infos(), &self.past_infos(), &self.side)
    }

    fn past_session(&self, idx: usize) -> Option<SessionInfo> {
        self.svc.state().past.ready().and_then(|p| p.get(idx).cloned())
    }

    /// Resume a past session in its own profile.
    pub(crate) fn resume(&mut self, idx: usize) {
        let Some(s) = self.past_session(idx) else { return };
        let cwd = s.cwd.clone().or_else(|| s.project.as_ref().map(|p| p.root.clone())).or_else(dirs::home_dir).unwrap_or_default();
        let spec = LaunchSpec { harness: s.harness, profile_id: s.profile_id.clone(), provider_id: None, model: None, cwd, resume: Some(Resume { session_id: s.id.clone(), fork: false }), permission: Permission::Default, browser: BrowserMode::Off, extra_args: vec![] };
        let mut req = LaunchRequest::new(spec, Place::Tab);
        req.name = Some(crate::ui::fit(&s.title, 28));
        self.svc.launch(req);
        self.toast(Kind::Info, format!("resuming “{}”…", crate::ui::fit(&s.title, 40)));
    }

    /// Open the fork picker for a past session.
    pub(crate) fn open_fork(&mut self, idx: usize) {
        let Some(s) = self.past_session(idx) else { return };
        if matches!(s.harness, Harness::Pi | Harness::Omp) {
            self.toast(Kind::Info, format!("{} sessions don't belong to a profile — enter resumes it", s.harness.label()));
            return;
        }
        let st = self.svc.state();
        let targets: Vec<(String, String)> = st
            .profiles
            .ready()
            .map(|ps| {
                ps.iter()
                    .filter(|p| p.authenticated && Some(&p.id) != s.profile_id.as_ref() && if s.harness == Harness::Codex { p.is_codex() } else { p.is_claude() })
                    .map(|p| (p.id.clone(), [p.plan.clone().unwrap_or_default(), p.email.clone().unwrap_or_default()].join("  ")))
                    .collect()
            })
            .unwrap_or_default();
        drop(st);
        self.overlay = Overlay::Fork(Box::new(ForkPicker { session: s, targets, sel: 0 }));
    }

    /// Stage `s` into `target` and resume it there as a fork.
    pub(super) fn fork_into(&mut self, s: SessionInfo, target: String) {
        let cwd = s.cwd.clone().or_else(|| s.project.as_ref().map(|p| p.root.clone())).or_else(dirs::home_dir).unwrap_or_default();
        let spec = LaunchSpec { harness: s.harness, profile_id: Some(target.clone()), provider_id: None, model: None, cwd, resume: Some(Resume { session_id: s.id.clone(), fork: true }), permission: Permission::Default, browser: BrowserMode::Off, extra_args: vec![] };
        let mut req = LaunchRequest::new(spec, Place::Tab);
        req.name = Some(crate::ui::fit(&s.title, 28));
        req.stage = Some(s);
        self.svc.launch(req);
        self.toast(Kind::Info, format!("forking into {target}…"));
    }

    // ------------------------------------------------------------------ agents

    /// Notice agents finishing or getting stuck while you aren't looking at them.
    pub(super) fn track_agents(&mut self) {
        let visible = self.visible();
        let mut notes = vec![];
        for (id, p) in &self.panes {
            let Some(a) = p.activity() else { continue };
            let prev = self.agent_state.insert(*id, a);
            let seen = visible.contains(id) && self.term_focused;
            if prev == Some(Activity::Working) && a == Activity::Idle && !seen {
                self.done.insert(*id);
                notes.push((Kind::AgentDone, format!("{} finished", p.title())));
            }
            if a == Activity::Blocked && prev.is_some_and(|x| x != Activity::Blocked) && !seen {
                notes.push((Kind::NeedsYou, format!("{} needs you", p.title())));
            }
            if visible.contains(id) {
                self.done.remove(id);
            }
        }
        self.agent_state.retain(|id, _| self.panes.contains_key(id));
        self.done.retain(|id| self.panes.contains_key(id));
        for (k, text) in notes {
            self.raise(k, text);
        }
    }

    /// The session that most needs you: blocked first, then finished-unseen.
    pub(crate) fn attention_target(&self) -> Option<PaneId> {
        let order = sidebar::live_order(&self.live_infos(), &self.past_infos());
        let cur = self.focused();
        let pick = |want: &dyn Fn(PaneId) -> bool| order.iter().copied().filter(|id| Some(*id) != cur).find(|id| want(*id));
        pick(&|id| self.panes.get(&id).and_then(|p| p.activity()) == Some(Activity::Blocked)).or_else(|| pick(&|id| self.done.contains(&id)))
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
                model: d.model.map(String::from),
                label: label.clone(),
                name: None,
                project: self.svc.project_for(&cwd),
                cwd: cwd.clone(),
                started: Instant::now() - std::time::Duration::from_secs([420, 1_900, 60, 5_400, 12_000][i % 5]),
                route_id: None,
                cleanup: vec![],
            };
            let mut term = if self.opts.fixed_demo {
                Term::fixed(meta, &crate::services::demo::transcript(d.harness), self.svc.clone())
            } else {
                let c = crate::services::demo::shell_command(d.harness, &label, cwd);
                Term::new(meta, Spawn { program: c.program, args: c.args, ..Spawn::default() }, self.svc.clone())
            };
            term.demo_activity = d.activity;
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
