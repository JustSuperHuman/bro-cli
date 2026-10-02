//! The session registry and shared host state.
//!
//! bro feeds sessions in through the [`crate::Bridge`] handle (register /
//! output / title / cwd / resize / exit / unregister, exactly what the
//! reference host received over its C ABI). Remote clients read it through
//! the HTTP and websocket handlers, and act on sessions through
//! [`AppState::dispatch`], which turns their requests into
//! [`BridgeCommand`]s for bro. Project bookkeeping lives in
//! `project_store.rs`, automatic notifications in `alerts.rs`.

use crate::agents;
use crate::model::{
    ServerEvent, TerminalNotification, TerminalProfile, TerminalProject, TerminalSessionSummary,
    empty_acp_state, iso_now,
};
use crate::net::{network_interface_addresses, server_access_urls};
use crate::notifications::NotificationCenter;
use crate::orchestrator::Orchestrator;
use crate::project_store::ProjectStore;
use crate::session::{Observed, Session, SessionView, Transition, clamp_size};
use crate::{BridgeCommand, BridgeConfig, Notification};
use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, OnceLock};
use tokio::sync::{broadcast, watch};

/// Input bytes for a session.
pub(crate) const COMMAND_INPUT: u32 = 1;
/// Resize a session's PTY.
pub(crate) const COMMAND_RESIZE: u32 = 2;
/// Terminate a session.
pub(crate) const COMMAND_KILL: u32 = 3;

pub(crate) type CommandSink = Arc<dyn Fn(BridgeCommand) + Send + Sync>;
pub(crate) type NotificationSink = Arc<dyn Fn(Notification) + Send + Sync>;

pub(crate) struct Inner {
    pub sessions: HashMap<String, Session>,
    pub profiles: Vec<TerminalProfile>,
    pub projects: ProjectStore,
    pub published_projects: Vec<TerminalProject>,
    pub acp: Value,
    pub started_at: String,
    pub host: IpAddr,
    pub port: u16,
}

/// Cheap-to-clone shared state behind every handler.
#[derive(Clone)]
pub(crate) struct AppState {
    pub inner: Arc<Mutex<Inner>>,
    pub events: broadcast::Sender<ServerEvent>,
    on_command: CommandSink,
    pub notifications: Arc<Mutex<NotificationCenter>>,
    pub notification_sinks: Arc<RwLock<Vec<NotificationSink>>>,
    /// Folders bro knows the user works in (its open projects, recents and
    /// past sessions); the orchestrator resolves spoken project names with them.
    pub known_folders: Arc<RwLock<Vec<String>>>,
    pub token: Arc<String>,
    pub data_root: Arc<PathBuf>,
    pub config: Arc<BridgeConfig>,
    pub orchestrator: Arc<Orchestrator>,
    /// The bridge runtime, once it runs (timers need it; tests without a
    /// runtime fall back to immediate behaviour).
    pub runtime: Arc<OnceLock<tokio::runtime::Handle>>,
    /// Connected `/ws` clients.
    pub clients: Arc<AtomicUsize>,
    pub running: Arc<AtomicBool>,
    pub last_error: Arc<Mutex<Option<String>>>,
    /// Flipped to true on shutdown; websocket loops close on it.
    pub shutdown: watch::Sender<bool>,
}

impl AppState {
    pub fn new(config: BridgeConfig, token: String, on_command: CommandSink) -> Self {
        let (events, _) = broadcast::channel(4096);
        let data_root = config.data_root.clone();
        let orchestrator = Arc::new(Orchestrator::new(&data_root, events.clone()));
        let projects = ProjectStore::load(&data_root);
        let published_projects = projects.visible(std::iter::empty());
        let (shutdown, _) = watch::channel(false);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                sessions: HashMap::new(),
                profiles: crate::profiles::default_profiles(),
                projects,
                published_projects,
                acp: empty_acp_state(),
                started_at: iso_now(),
                host: IpAddr::from([0, 0, 0, 0]),
                port: 0,
            })),
            events,
            on_command,
            notifications: Arc::new(Mutex::new(NotificationCenter::default())),
            notification_sinks: Arc::new(RwLock::new(Vec::new())),
            known_folders: Arc::new(RwLock::new(Vec::new())),
            token: Arc::new(token),
            data_root: Arc::new(data_root),
            config: Arc::new(config),
            orchestrator,
            runtime: Arc::new(OnceLock::new()),
            clients: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            last_error: Arc::new(Mutex::new(None)),
            shutdown,
        }
    }

    pub fn known_folders(&self) -> Vec<String> {
        self.known_folders.read().clone()
    }

    // ----- session lifecycle (fed by bro) ---------------------------------

    /// Adds or replaces a session. Re-registering keeps the screen and replay.
    pub fn register(
        &self,
        mut summary: TerminalSessionSummary,
        project_dir: Option<String>,
        agent_hint: Option<agents::Agent>,
    ) {
        let id = summary.id.clone();
        {
            let mut inner = self.inner.lock();
            if let Some(existing) = inner.sessions.get_mut(&id) {
                summary.buffered_bytes = existing.summary.buffered_bytes;
                summary.created_at = existing.summary.created_at.clone();
                summary.agent = existing.summary.agent.take();
                summary.agent_source = existing.summary.agent_source.take();
                summary.agent_activity = existing.summary.agent_activity.take();
                if (summary.cols, summary.rows) != (existing.summary.cols, existing.summary.rows) {
                    existing.resize(summary.cols, summary.rows);
                }
                existing.summary = summary.clone();
                existing.project_dir = project_dir;
                existing.agent_hint = agent_hint;
            } else {
                let mut session = Session::new(summary.clone());
                session.project_dir = project_dir;
                session.agent_hint = agent_hint;
                inner.sessions.insert(id.clone(), session);
            }
        }
        self.publish_session(&id);
        self.publish_sessions();
        self.reconcile_projects();
    }

    /// Raw PTY bytes, in order.
    pub fn append_output(&self, session_id: &str, data: &[u8]) {
        let mut bells = Vec::new();
        let (seq, visible, observed) = {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            let text = session.utf8.decode(data);
            if text.is_empty() {
                return;
            }
            let previous_agent = session.summary.agent.clone();
            let (seq, visible) = session.feed(&text, &mut bells);
            let mut observed = if session.observation_pending && session.observation_due() {
                session.observe()
            } else {
                Observed::default()
            };
            observed.changed |= previous_agent != session.summary.agent;
            (seq, visible, observed)
        };
        if seq > 0 {
            self.publish(ServerEvent::output(
                json!({
                    "type": "output",
                    "sessionId": session_id,
                    "seq": seq,
                    "data": visible,
                    "replay": false
                }),
                session_id,
            ));
        }
        self.after_observation(session_id, observed);
        self.handle_bells(session_id, bells);
    }

    /// Publishes what an observation changed and raises its notification.
    pub fn after_observation(&self, session_id: &str, observed: Observed) {
        if observed.changed {
            self.publish_session(session_id);
        }
        if let Some(transition) = observed.transition {
            self.handle_transition(session_id, transition);
        }
    }

    /// Updates the title locally (bro reported it, or a client renamed).
    pub fn rename(&self, session_id: &str, title: String) -> Option<TerminalSessionSummary> {
        let summary = {
            let mut inner = self.inner.lock();
            let session = inner.sessions.get_mut(session_id)?;
            if !title.trim().is_empty() && session.summary.title != title {
                session.summary.title = title;
                session.summary.updated_at = iso_now();
            }
            session.summary.clone()
        };
        self.publish(ServerEvent::session(
            json!({ "type": "session", "session": summary }),
            session_id,
        ));
        Some(summary)
    }

    /// Records a session's live working directory and re-derives projects.
    pub fn set_cwd(&self, session_id: &str, cwd: String) {
        let cwd = cwd.trim().to_owned();
        if cwd.is_empty() {
            return;
        }
        {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            if crate::projects::cwd_key(&session.summary.cwd) == crate::projects::cwd_key(&cwd) {
                return;
            }
            session.summary.cwd = cwd;
            session.summary.updated_at = iso_now();
        }
        self.publish_session(session_id);
        self.reconcile_projects();
    }

    /// Changes the directory a session is grouped under (bro's project root).
    pub fn set_project_dir(&self, session_id: &str, project_dir: Option<String>) {
        {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            session.project_dir = project_dir.filter(|dir| !dir.trim().is_empty());
        }
        self.reconcile_projects();
    }

    /// Pins a session to a project id (a client created it inside a project).
    pub fn set_project_id(&self, session_id: &str, project_id: Option<String>) {
        {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            session.summary.project_id = project_id;
            session.summary.updated_at = iso_now();
        }
        self.publish_session(session_id);
    }

    /// bro resized the PTY.
    pub fn resize_from_host(&self, session_id: &str, cols: u16, rows: u16) {
        {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            session.resize(cols, rows);
        }
        self.publish_session(session_id);
    }

    pub fn exit(&self, session_id: &str, exit_code: Option<i32>) {
        let summary = {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            session.summary.status = "exited".into();
            session.summary.exit_code = exit_code;
            session.summary.updated_at = iso_now();
            session.summary.clone()
        };
        self.notifications.lock().cancel_bell(session_id);
        self.publish(ServerEvent::session(
            json!({
                "type": "exit", "sessionId": session_id, "exitCode": exit_code,
                "signal": null, "session": summary
            }),
            session_id,
        ));
        self.publish_sessions();
        self.reconcile_projects();
    }

    pub fn unregister(&self, session_id: &str) {
        if self.inner.lock().sessions.remove(session_id).is_none() {
            return;
        }
        self.notifications.lock().forget(session_id);
        self.publish_sessions();
        self.reconcile_projects();
    }

    // ----- reads ----------------------------------------------------------

    pub fn summaries(&self) -> Vec<TerminalSessionSummary> {
        let mut sessions: Vec<_> = self
            .inner
            .lock()
            .sessions
            .values()
            .map(|session| session.summary.clone())
            .collect();
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        sessions
    }

    pub fn summary(&self, session_id: &str) -> Option<TerminalSessionSummary> {
        self.inner
            .lock()
            .sessions
            .get(session_id)
            .map(|session| session.summary.clone())
    }

    /// The snapshot plus the output seq it already contains.
    pub fn snapshot_with_seq(&self, session_id: &str) -> Option<(Value, u64)> {
        let mut inner = self.inner.lock();
        let session = inner.sessions.get_mut(session_id)?;
        Some((session.snapshot(), session.seq))
    }

    pub fn export(&self, session_id: &str) -> Option<Value> {
        self.inner
            .lock()
            .sessions
            .get(session_id)
            .map(Session::export)
    }

    pub fn plain_text(&self, session_id: &str) -> Option<String> {
        self.inner
            .lock()
            .sessions
            .get(session_id)
            .map(Session::plain_text)
    }

    pub fn input_context(&self, session_id: &str) -> Option<Value> {
        self.inner
            .lock()
            .sessions
            .get(session_id)
            .map(Session::input_context)
    }

    pub fn session_view(&self, session_id: &str) -> Option<SessionView> {
        self.inner
            .lock()
            .sessions
            .get(session_id)
            .map(Session::view)
    }

    pub fn session_views(&self) -> Vec<SessionView> {
        self.inner
            .lock()
            .sessions
            .values()
            .map(Session::view)
            .collect()
    }

    /// Plain text of the last `tail` rows, scrollback included.
    pub fn session_text(&self, session_id: &str, tail: usize) -> Option<String> {
        let mut inner = self.inner.lock();
        Some(
            inner
                .sessions
                .get_mut(session_id)?
                .text_with_scrollback(tail.max(1)),
        )
    }

    /// Output sequence number, which advances on every chunk a session prints.
    pub fn session_seq(&self, session_id: &str) -> Option<u64> {
        self.inner
            .lock()
            .sessions
            .get(session_id)
            .map(|session| session.seq)
    }

    /// Observes one session right now, regardless of throttling.
    pub fn observe_now(&self, session_id: &str) {
        let observed = {
            let mut inner = self.inner.lock();
            let Some(session) = inner.sessions.get_mut(session_id) else {
                return;
            };
            session.observe()
        };
        self.after_observation(session_id, observed);
    }

    /// Fingerprints every session whose output changed since its last
    /// (throttled) observation, so a TUI that went quiet still settles.
    pub fn observe_pending_sessions(&self) {
        let observed: Vec<(String, Observed)> = {
            let mut inner = self.inner.lock();
            inner
                .sessions
                .values_mut()
                .filter(|session| session.observation_pending && session.observation_due())
                .map(|session| (session.summary.id.clone(), session.observe()))
                .collect()
        };
        for (id, observed) in observed {
            self.after_observation(&id, observed);
        }
    }

    // ----- commands to bro --------------------------------------------------

    /// Hands a command to bro. A panicking callback never takes the host down.
    pub fn send_command(&self, command: BridgeCommand) {
        let sink = self.on_command.clone();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(command))).is_err() {
            tracing::warn!("bro-bridge: on_command callback panicked");
        }
    }

    /// Input / resize / kill for a known session (the reference host's
    /// native callback kinds).
    pub fn dispatch(
        &self,
        session_id: &str,
        kind: u32,
        data: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(), String> {
        if !self.inner.lock().sessions.contains_key(session_id) {
            return Err("Unknown terminal session.".into());
        }
        let id = session_id.to_owned();
        let command = match kind {
            COMMAND_INPUT => BridgeCommand::Input {
                id,
                data: data.as_bytes().to_vec(),
            },
            COMMAND_RESIZE => {
                if cols == 0 || rows == 0 {
                    return Err("cols and rows are required.".into());
                }
                let (cols, rows) = clamp_size(cols, rows);
                BridgeCommand::Resize { id, cols, rows }
            }
            COMMAND_KILL => BridgeCommand::Kill { id },
            _ => return Err("Unknown bridge command.".into()),
        };
        self.send_command(command);
        Ok(())
    }

    // ----- broadcasting ---------------------------------------------------

    pub fn publish(&self, event: ServerEvent) {
        let _ = self.events.send(event);
    }

    pub fn publish_session(&self, session_id: &str) {
        if let Some(summary) = self.summary(session_id) {
            self.publish(ServerEvent::session(
                json!({ "type": "session", "session": summary }),
                session_id,
            ));
        }
    }

    pub fn publish_sessions(&self) {
        self.publish(ServerEvent::global(
            json!({ "type": "sessions", "sessions": self.summaries() }),
        ));
    }

    pub fn set_profiles(&self, profiles: Vec<TerminalProfile>) {
        self.inner.lock().profiles = profiles;
        self.publish_profiles();
    }

    pub fn publish_profiles(&self) {
        let profiles = self.inner.lock().profiles.clone();
        self.publish(ServerEvent::global(
            json!({ "type": "profiles", "profiles": profiles }),
        ));
    }

    // ----- hello / bootstrap ------------------------------------------------

    pub fn server_info(&self) -> Value {
        let (host, port, started_at) = {
            let inner = self.inner.lock();
            (inner.host, inner.port, inner.started_at.clone())
        };
        let urls = server_access_urls(host, port, &self.token, &network_interface_addresses());
        json!({
            "pid": std::process::id(),
            "host": host.to_string(),
            "port": port,
            "startedAt": started_at,
            "urls": urls
        })
    }

    pub fn bootstrap(&self) -> Value {
        let server = self.server_info();
        let port = server["port"].clone();
        let projects = self.reconcile_projects();
        let (sessions, profiles, acp) = {
            let inner = self.inner.lock();
            let sessions: Vec<_> = inner
                .sessions
                .values()
                .map(|session| session.summary.clone())
                .collect();
            (sessions, inner.profiles.clone(), inner.acp.clone())
        };
        json!({
            "sessions": sessions,
            "profiles": profiles,
            "hostProcesses": [],
            "peerHosts": [],
            "projects": projects,
            "server": server,
            "bridgeCommands": {
                "serverUrl": format!("http://127.0.0.1:{port}"),
                "shell": "", "codex": "", "claude": ""
            },
            "orchestrator": self.orchestrator.status(None, true),
            "acp": acp
        })
    }

    pub fn hello(&self) -> Value {
        let mut hello = self.bootstrap();
        if let Some(object) = hello.as_object_mut() {
            object.insert("type".into(), json!("hello"));
            object.insert("heartbeat".into(), json!(true));
        }
        hello
    }

    // ----- notifications ------------------------------------------------------

    /// Records and broadcasts a notification; `callbacks` also hands it to
    /// the `on_notification` sinks (false for ones bro itself raised).
    pub fn notify(&self, notification: TerminalNotification, callbacks: bool) {
        let notification = self.notifications.lock().record(notification);
        let mut value = serde_json::to_value(&notification).unwrap_or_else(|_| json!({}));
        if let Some(object) = value.as_object_mut() {
            object.insert("type".into(), Value::String("notify".into()));
        }
        self.publish(ServerEvent::global(value));
        if callbacks {
            let public = Notification {
                session_id: notification.session_id.clone(),
                title: notification.title.clone().unwrap_or_default(),
                body: notification.body.clone().unwrap_or_default(),
                sound: notification.sound.is_some(),
            };
            let sinks = self.notification_sinks.read().clone();
            for sink in sinks {
                let public = public.clone();
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(public))).is_err()
                {
                    tracing::warn!("bro-bridge: on_notification callback panicked");
                }
            }
        }
    }

    pub fn notification_history(
        &self,
        since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Vec<TerminalNotification> {
        self.notifications.lock().list(since)
    }

    fn handle_transition(&self, session_id: &str, transition: Transition) {
        match transition {
            Transition::Done => self.publish_terminal_notification(
                session_id,
                "bell",
                None,
                Some("Task finished".into()),
            ),
            Transition::Awaiting { agent_label } => {
                self.publish_needs_input(session_id, agent_label)
            }
        }
    }
}
