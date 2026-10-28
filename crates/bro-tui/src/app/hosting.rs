//! Sessions in bro's session server (see [`bro_host`]): picking up the ones a past window left running, bringing
//! back the ones a server that stopped unasked left behind, leaving them running when bro closes (the background bro
//! carries on with them), restarting the server, picking the connection up again when it drops, and a viewer's view
//! of the sessions another bro drives.

use super::App;
use super::overlays::{Confirm, ConfirmAction, Overlay};
use crate::alerts::Kind;
use crate::pane::{PaneId, Place};
use crate::panes::term::{Meta, Spawn, Term};
use crate::services::LaunchRequest;
use crate::services::host::{HostEvent, role};
use bro_core::Harness;
use bro_core::browser::BrowserMode;
use bro_core::launch::LaunchSpec;
use bro_host::Role;

/// How a pane comes back after its session ended with the server (a restart, a crash, the computer shutting down).
pub(crate) enum Comeback {
    /// a Claude / Codex session: reloaded into its pane, resuming its conversation
    Resume(Box<LaunchRequest>),
    /// anything else: started again in its pane
    Fresh(Box<Meta>, Spawn),
}

/// What a viewer asks the primary for (see [`App::relayed`]).
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Relayed {
    pub launch: Option<RelayedLaunch>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct RelayedLaunch {
    pub spec: LaunchSpec,
    pub name: Option<String>,
    pub remember: bool,
}

impl App {
    /// Open the sessions the server runs (at startup, before the orchestrator is restored: a running one is picked
    /// up rather than started twice), and offer back the ones a server that stopped unasked left behind.
    pub(crate) fn reattach_sessions(&mut self) {
        let Some(host) = self.svc.host() else { return };
        let viewer = role() == Role::Viewer;
        let (mut back, mut ended, mut routes) = (0, 0, vec![]);
        for info in host.info().sessions.clone() {
            if viewer && info.exited {
                // the bro driving the server deals with those
                continue;
            }
            let Some(term) = Term::reattach(&info, self.svc.clone()) else {
                // something this bro can't show (another version's metadata): don't leave it running unseen
                if !viewer {
                    host.close(&info.id);
                }
                continue;
            };
            if info.exited {
                // ended while no bro watched: close it the way closing a session does (route, staged files)
                let id = self.add(Box::new(term));
                self.close(id);
                ended += 1;
                continue;
            }
            routes.extend(term.saved_route().cloned());
            let id = self.open(Box::new(term), Place::Tab);
            // hidden ones too: their status, prompts and auto-resume need their output
            self.start_now(id);
            back += 1;
        }
        if !routes.is_empty() && !viewer {
            self.restore_routes(routes);
        }
        let s = |n: usize| if n == 1 { "" } else { "s" };
        match role() {
            Role::Viewer => self.toast(Kind::Info, format!("watching the {back} session{} of the bro running in another window · typing into one takes it over", s(back))),
            Role::Background => {}
            _ => {
                if back > 0 {
                    self.toast(Kind::Info, format!("{back} session{} kept running while bro was closed", s(back)));
                }
                if ended > 0 {
                    self.toast(Kind::Info, format!("{ended} session{} ended while bro was closed", s(ended)));
                }
                let lost = host.info().lost.clone();
                if !lost.is_empty() {
                    self.offer_lost(lost);
                }
            }
        }
    }

    /// Proxied sessions reach their model through this bro's proxy now.
    fn restore_routes(&self, routes: Vec<bro_proxy::Route>) {
        let svc = self.svc.clone();
        std::thread::spawn(move || {
            if svc.ensure_proxy().is_err() {
                return;
            }
            for r in routes {
                svc.with_proxy("proxy::upsert_route", |p| p.upsert_route(r));
            }
            let all = svc.with_proxy("proxy::routes", |p| p.routes()).unwrap_or_default();
            svc.update(|st| st.proxy.routes = all);
        });
    }

    /// Ask whether to bring back what a server that stopped unasked was running.
    fn offer_lost(&mut self, lost: Vec<bro_host::SessionInfo>) {
        let n = lost.len();
        self.lost = lost;
        let (s, were) = if n == 1 { ("", "was") } else { ("s", "were") };
        self.overlay = Overlay::Confirm(Confirm {
            text: format!("bring back the {n} session{s} that {were} running when bro's session server stopped?"),
            detail: "it stopped without being asked to (a crash, or the computer shutting down) · Claude and Codex resume their conversations; shells start fresh".into(),
            action: ConfirmAction::BringBack,
        });
    }

    /// Yes: each lost session gets its pane back and starts again in it.
    pub(crate) fn bring_back_lost(&mut self) {
        let lost = std::mem::take(&mut self.lost);
        let ids: Vec<String> = lost.iter().map(|l| l.id.clone()).collect();
        let mut n = 0;
        for info in &lost {
            let Some(mut t) = Term::from_saved(info, self.svc.clone()) else { continue };
            let Some(back) = self.comeback(&t) else { continue };
            t.reloading = true;
            let id = self.open(Box::new(t), Place::Tab);
            self.run_comeback(id, back);
            n += 1;
        }
        if let Some(h) = self.svc.host() {
            h.forget_lost(ids);
        }
        let s = if n == 1 { "" } else { "s" };
        self.toast(Kind::Info, format!("{n} session{s} coming back"));
    }

    /// No: let them go (the next bro won't ask again).
    pub(crate) fn let_lost_go(&mut self) {
        let ids: Vec<String> = std::mem::take(&mut self.lost).into_iter().map(|l| l.id).collect();
        if let Some(h) = self.svc.host() {
            h.forget_lost(ids);
        }
    }

    /// Quitting: hosted sessions stay with the server for the next bro (a viewer leaves all of them to the bro it
    /// watched). Returns how many were left running.
    pub(super) fn detach_hosted(&mut self) -> usize {
        let keep = role() == Role::Viewer || self.taken_over || self.svc.host().is_some_and(|_| self.svc.settings().server.keep_sessions);
        if !keep {
            return 0;
        }
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.as_term_ref().is_some_and(|t| t.hosted() && p.alive())).map(|(id, _)| *id).collect();
        for id in &ids {
            if let Some(mut p) = self.panes.remove(id)
                && let Some(t) = p.as_term()
            {
                t.detach();
            }
        }
        ids.len()
    }

    /// Bro is quitting: hand the sessions to the background bro (phone bridge, proxy, auto-resume, watchdogs carry on
    /// until a window opens again), when it should keep working.
    pub(super) fn leave_to_background(&mut self) {
        let s = self.svc.settings();
        let wanted = role() == Role::Window && !self.taken_over && !self.guest && !self.svc.is_demo() && s.server.keep_sessions && s.server.keep_working;
        if wanted && (self.left_running > 0 || self.svc.host_resident()) && self.svc.start_background() {
            self.left_to_background = true;
        }
    }

    pub(super) fn host_event(&mut self, e: HostEvent) {
        match e {
            HostEvent::Lost => self.host_lost(),
            HostEvent::Stopped => self.host_stopped(),
            HostEvent::TakenOver => {
                // a window drives the sessions now: this bro steps aside, leaving them running
                self.taken_over = true;
                self.quit = true;
            }
            HostEvent::Restarted(result) => self.restarted(result),
            HostEvent::Reconnected(result) => self.reconnected(result),
            HostEvent::Added(info) => self.session_added(*info),
            HostEvent::Blob { id, blob } => {
                if let Some(t) = self.host_pane(&id).and_then(|pid| self.panes.get_mut(&pid)).and_then(|p| p.as_term()) {
                    t.apply_blob(&blob);
                }
            }
            HostEvent::Relayed(body) => self.relayed(body),
            HostEvent::Unrelayed(body) => {
                // nobody drives the server: this viewer launches it itself
                if let Ok(Relayed { launch: Some(l) }) = serde_json::from_value::<Relayed>(body) {
                    self.svc.launch_here(Self::relayed_request(l));
                }
            }
            HostEvent::Peers(p) => self.svc.update(|st| st.host.peers = p),
        }
    }

    /// The pane showing hosted session `id`.
    fn host_pane(&self, id: &str) -> Option<PaneId> {
        self.panes.iter().find(|(_, p)| p.as_term_ref().is_some_and(|t| t.host_id() == Some(id))).map(|(pid, _)| *pid)
    }

    /// Another client started a session (a viewer, or the bro a viewer watches): show it, without taking the focus.
    fn session_added(&mut self, info: bro_host::SessionInfo) {
        if info.exited || self.host_pane(&info.id).is_some() {
            return;
        }
        let Some(term) = Term::reattach(&info, self.svc.clone()) else { return };
        let route = term.saved_route().cloned();
        let id = self.open_quietly(Box::new(term));
        self.start_now(id);
        if let Some(r) = route.filter(|_| role() != Role::Viewer) {
            self.restore_routes(vec![r]);
        }
    }

    /// A tab for `p` behind the one you're in.
    pub(crate) fn open_quietly(&mut self, p: Box<dyn crate::pane::Pane>) -> PaneId {
        let had = self.focused().is_some();
        let here = self.cur;
        let tiles = (self.stack.clone(), self.stack_focus, self.stack_zoom, self.tile_all, self.tile_project.clone());
        let id = self.new_tab(p);
        if had {
            self.cur = here.min(self.tabs.len().saturating_sub(1));
            self.restore_background_tiles(tiles);
        }
        id
    }

    /// A viewer asks this bro (the primary) to launch something: it runs here, with this bro's proxy and phone bridge,
    /// and the viewer sees it arrive.
    fn relayed(&mut self, body: serde_json::Value) {
        if let Ok(Relayed { launch: Some(l) }) = serde_json::from_value::<Relayed>(body) {
            self.svc.launch(Self::relayed_request(l));
        }
    }

    fn relayed_request(l: RelayedLaunch) -> LaunchRequest {
        let mut req = LaunchRequest::new(l.spec, Place::Behind);
        req.name = l.name;
        req.remember = l.remember;
        req
    }

    /// The server was stopped on purpose (`bro host stop`, or a restart under way): its sessions ended with it.
    fn host_stopped(&mut self) {
        let restarting = self.svc.state().host.restarting.is_some() || !self.comebacks.is_empty();
        self.svc.host_lost();
        if restarting || self.quit || self.quitting {
            return;
        }
        if role() == Role::Background {
            // nothing left to stand in for
            self.quit = true;
            return;
        }
        self.raise(Kind::Error, "bro's session server was stopped: the sessions it ran have ended".into());
    }

    /// The connection dropped: hold the hosted panes (they keep their screens) and connect again.
    fn host_lost(&mut self) {
        let restarting = self.svc.state().host.restarting.is_some() || !self.comebacks.is_empty();
        self.svc.host_lost();
        if restarting || self.quit || self.quitting {
            return;
        }
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.as_term_ref().is_some_and(|t| t.hosted() && !t.reloading)).map(|(id, _)| *id).collect();
        for id in &ids {
            if let Some(t) = self.panes.get_mut(id).and_then(|p| p.as_term()) {
                t.reloading = true;
            }
        }
        self.held = ids;
        self.svc.reconnect_host();
        if role() != Role::Background {
            self.toast(Kind::Info, "lost bro's session server · connecting again…");
        }
    }

    /// Connected again (or not): sessions still running come back in their panes as they were; ones that went with
    /// a server that stopped start again in their panes; the rest end.
    fn reconnected(&mut self, result: Result<(), String>) {
        let held = std::mem::take(&mut self.held);
        let host = match (result, self.svc.host()) {
            (Ok(()), Some(h)) => h,
            (outcome, _) => {
                for id in held {
                    if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                        t.reloading = false;
                    }
                }
                if role() == Role::Background {
                    // a window has the sessions now (or there's nothing left to stand in for)
                    self.taken_over = true;
                    self.quit = true;
                    return;
                }
                let why = outcome.err().unwrap_or_else(|| "it isn't running".into());
                self.raise(Kind::Error, format!("couldn't reach bro's session server again ({why}): the sessions it ran have ended"));
                return;
            }
        };
        let info = host.info().clone();
        let (mut kept, mut back, mut lost_ids) = (0, 0, vec![]);
        for id in held {
            let Some(sid) = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.host_id().map(str::to_string)) else { continue };
            if let Some(s) = info.sessions.iter().find(|s| s.id == sid && !s.exited)
                && let Some(term) = Term::reattach(s, self.svc.clone())
            {
                // still running: the same session, picked up again from its snapshot
                self.swap_pane(id, term);
                kept += 1;
            } else if info.lost.iter().any(|l| l.id == sid)
                && let Some(come) = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| self.comeback_held(t))
            {
                lost_ids.push(sid);
                self.run_comeback(id, come);
                back += 1;
            } else if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                t.reloading = false;
            }
        }
        // what started meanwhile (a viewer's sessions), and what a crashed server left that wasn't ours
        for s in info.sessions.iter().filter(|s| !s.exited) {
            if self.host_pane(&s.id).is_none() {
                self.session_added(s.clone());
            }
        }
        host.forget_lost(lost_ids.clone());
        let rest: Vec<bro_host::SessionInfo> = info.lost.into_iter().filter(|l| !lost_ids.contains(&l.id)).collect();
        if !rest.is_empty() && role() == Role::Window {
            self.offer_lost(rest);
        }
        if role() == Role::Background {
            return;
        }
        let s = |n: usize| if n == 1 { "" } else { "s" };
        if back > 0 {
            self.raise(Kind::Error, format!("bro's session server stopped unexpectedly · {back} session{} coming back", s(back)));
        } else if kept > 0 {
            self.toast(Kind::Info, format!("connected to bro's session server again · {kept} session{} carried on", s(kept)));
        }
    }

    /// Put `term` in pane `id` in place of what was there, and start it.
    fn swap_pane(&mut self, id: PaneId, term: Term) {
        if let Some(old) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
            // the old one's remote is gone with the connection: nothing to end
            old.detach();
        }
        self.panes.insert(id, Box::new(term));
        self.last_tick.remove(&id);
        self.start_now(id);
    }

    /// Ask before restarting the server (it ends every session it runs).
    pub(crate) fn ask_restart_server(&mut self) {
        if self.svc.is_demo() || self.guest || role() != Role::Window {
            self.toast(Kind::Info, "the session server belongs to the bro that's already running");
            return;
        }
        let hosted = self.panes.values().filter(|p| p.as_term_ref().is_some_and(|t| t.hosted())).count();
        if hosted == 0 {
            self.restart_server();
            return;
        }
        let s = if hosted == 1 { "" } else { "s" };
        self.overlay = Overlay::Confirm(Confirm {
            text: format!("restart the session server and end its {hosted} session{s}?"),
            detail: "Claude and Codex sessions come back in their panes, resuming their conversations; shells start fresh".into(),
            action: ConfirmAction::RestartServer,
        });
    }

    /// Restart the server: every hosted pane is held open (reloading) until the new server can take it.
    pub(crate) fn restart_server(&mut self) {
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.as_term_ref().is_some_and(|t| t.hosted())).map(|(id, _)| *id).collect();
        for id in ids {
            let Some(back) = self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| self.comeback(t)) else { continue };
            if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                t.reloading = true;
            }
            self.comebacks.push((id, back));
        }
        self.svc.restart_host();
        self.toast(Kind::Info, "restarting the session server…");
    }

    /// How `t` comes back once its session ended with the server.
    fn comeback(&self, t: &Term) -> Option<Comeback> {
        if t.queued.is_some() || t.reloading {
            return None;
        }
        self.comeback_held(t)
    }

    /// [`App::comeback`] for a pane already held for it.
    fn comeback_held(&self, t: &Term) -> Option<Comeback> {
        if t.queued.is_some() {
            return None;
        }
        match t.meta.harness {
            Some(harness @ (Harness::Claude | Harness::Codex)) => {
                let spec = t.launch_spec.clone().unwrap_or_else(|| LaunchSpec {
                    harness,
                    profile_id: t.meta.profile.clone(),
                    provider_id: None,
                    model: t.meta.model.clone(),
                    cwd: t.meta.cwd.clone(),
                    resume: None,
                    permission: self.svc.settings().permission,
                    browser: BrowserMode::Off,
                    extra_args: vec![],
                    orchestrator: t.meta.orchestrator,
                });
                let session_id = t.agent_session().or_else(|| spec.resume.as_ref().filter(|r| !r.fork).map(|r| r.session_id.clone()));
                let since = std::time::SystemTime::now() - t.meta.started.elapsed();
                let find = crate::services::launch::FindLive { harness, store: t.meta.store.clone(), cwd: spec.cwd.clone(), since, session_id };
                let mut spec = spec;
                spec.resume = None;
                let mut req = LaunchRequest::new(spec, Place::Tab);
                req.name = Some(t.meta.name.clone().unwrap_or_else(|| t.meta.label.clone()));
                req.find_live = Some(find);
                Some(Comeback::Resume(Box::new(req)))
            }
            _ => t.launch_spawn().cloned().map(|s| Comeback::Fresh(Box::new(t.meta.clone()), s)),
        }
    }

    /// Start `back` in pane `id` (held open, reloading, meanwhile).
    fn run_comeback(&mut self, id: PaneId, back: Comeback) {
        match back {
            Comeback::Resume(mut req) => {
                req.into = Some(id);
                self.svc.launch(*req);
            }
            Comeback::Fresh(meta, spawn) => {
                let mut meta = *meta;
                meta.sid = uuid::Uuid::new_v4().to_string();
                meta.started = std::time::Instant::now();
                if let Some(old) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                    old.on_exit();
                    // the new session cleans up what the old one staged
                    meta.cleanup = std::mem::take(&mut old.meta.cleanup);
                }
                let term = Term::new(meta, spawn, self.svc.clone());
                self.panes.insert(id, Box::new(term));
                self.agent_state.remove(&id);
                self.done.remove(&id);
                self.last_tick.remove(&id);
                self.start_now(id);
            }
        }
    }

    fn restarted(&mut self, result: Result<(), String>) {
        let comebacks = std::mem::take(&mut self.comebacks);
        if let Err(e) = result {
            // nothing to come back to: the held panes close as their sessions ended
            for (id, _) in comebacks {
                if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                    t.reloading = false;
                }
            }
            self.raise(Kind::Error, format!("couldn't restart the session server: {e}"));
            return;
        }
        let n = comebacks.len();
        for (id, back) in comebacks {
            if self.panes.contains_key(&id) {
                self.run_comeback(id, back);
            }
        }
        let s = if n == 1 { "" } else { "s" };
        self.toast(Kind::Info, if n == 0 { "session server restarted".to_string() } else { format!("session server restarted · {n} session{s} coming back") });
    }
}
