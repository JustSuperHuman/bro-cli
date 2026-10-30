//! bro-bridge — port of Just Terminal's rust-bridge host (F:\terminal\src\cascadia\
//! TerminalConnection\rust-bridge), decoupled from Windows Terminal's C ABI.
//!
//! bro owns the PTYs; it mirrors each agent session into the bridge and executes the
//! commands remote clients send back. The wire protocol (`/ws`, `/api/*`, token auth,
//! `hello`/`snapshot`/`output`/`session` events…) stays compatible with the existing
//! Just Terminal mobile app and web client.
//!
//! Contract rule: public items here are what bro-tui builds against. Add freely;
//! do not rename or remove.
//!
//! # Layout
//!
//! | module | role |
//! |---|---|
//! | `state` | session registry + shared host state, [`BridgeCommand`] dispatch |
//! | `session` | one mirrored session: vt100 screen, replay, agent observation |
//! | `output_filter` | strips the private `OSC 1337;TerminalWeb.Agent=` envelope |
//! | `notifications` / `alerts` | BEL / OSC 9 / OSC 777 detection, history, throttle, agent done / needs-input |
//! | `server` | router, listener, embedded web client, discovery file |
//! | `auth` | token file (+ legacy Just Terminal import), request auth |
//! | `ws` | `/ws` protocol |
//! | `rest` / `rest_sessions` | `/api/*` handlers |
//! | `commands` | create-session round trip, paced compose |
//! | `project_store` / `projects` | project ids, names, order |
//! | `net` | access URLs and the pairing QR code |
//! | `folders` | `/api/folders`: where a session can start (bro's projects, recents, path completion) |
//! | `hosts` | the other bridges on this machine (`/api/hosts`), so clients switch between bro windows without pairing again |
//! | `orchestrator` | Hugh, the built-in agent over every session: a headless Claude Code process using the tools at `/mcp` |
//! | `watchdog` | per-session watchdogs: milestones, a log, and a fresh reviewer agent per wake-up on the same tools |
//! | `mod_jobs` | MCP work for a session's Claude Code mod (`/api/mod/*`): bro asks, the mod polls, does it, answers |
//!
//! # Data root
//!
//! `<data_root>` (default `~/.bro/bridge`, `$BRO_DIR/bridge` when set) holds
//! `.terminal-web-token`, `.terminal-web-projects.json`,
//! `.terminal-web-server.json` (`{pid,host,port,startedAt,runtime:"bro"}`) and
//! the orchestrator config/history, and `hosts/<id>.json` for every running
//! bridge (see `hosts`). On first start the token is imported from
//! Just Terminal (`%LOCALAPPDATA%\TerminalWeb` or its MSIX-redirected
//! `%LOCALAPPDATA%\Packages\*Terminal*\LocalCache\Local\TerminalWeb`) so
//! phones paired with Just Terminal keep working; see [`StartOptions`].

mod agents;
mod apps;
mod alerts;
mod auth;
mod commands;
mod coordination;
mod coordination_evidence;
mod coordination_worktrees;
mod conversation;
pub mod file_links;
pub mod file_preview;
pub mod insights;
mod file_search;
mod folders;
mod hosts;
mod media;
mod mod_jobs;
mod model;
mod net;
mod notifications;
mod orchestrator;
pub mod schedule;
mod output_filter;
mod profiles;
mod project_store;
mod projects;
mod prompt;
mod raw_file;
mod thumbnail;
mod waveform;
mod rest;
mod rest_sessions;
mod server;
mod session;
pub mod slash_commands;
mod focus_controls;
mod state;
mod vt_stream;
pub mod watchdog;
mod ws;

#[cfg(test)]
mod tests;

pub use auth::is_authorized;
pub use hosts::LocalHost;
pub use insights::{AgentFiles, Attention, AttentionState, Recap, RecapSource, SentFile, SessionInsights, TouchKind, TouchedFile};
pub use model::TerminalProfile;
pub use orchestrator::{MessageContext, OrchestratorLaunch, OrchestratorUpdate};
pub use watchdog::{Milestone, MilestoneStatus, ReviewerKind, ReviewerLaunch, RunRecord, Running, WatchdogPlan, WatchdogSnapshot, WatchdogUpdate, WatchdogWake, WatchedSession};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use state::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// First port to try (10001). `0` asks the OS for an ephemeral port.
    pub port: u16,
    /// Try port..port+20 when taken
    pub automatic_port: bool,
    pub bind: String,
    /// Token file, projects, notifications, attachments. Default `~/.bro/bridge`.
    pub data_root: PathBuf,
    /// Serve the embedded web client for unknown paths
    pub web_interface: bool,
}

impl BridgeConfig {
    /// `$BRO_DIR/bridge`, else `~/.bro/bridge`.
    pub fn default_data_root() -> PathBuf {
        if let Some(dir) = std::env::var_os("BRO_DIR").filter(|dir| !dir.is_empty()) {
            return PathBuf::from(dir).join("bridge");
        }
        let home = std::env::var_os("USERPROFILE")
            .filter(|home| !home.is_empty())
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        home.join(".bro").join("bridge")
    }
}

impl Default for BridgeConfig {
    /// Port 10001 (+20 automatic), all interfaces, web client on.
    fn default() -> Self {
        Self {
            port: 10001,
            automatic_port: true,
            bind: "0.0.0.0".into(),
            data_root: Self::default_data_root(),
            web_interface: true,
        }
    }
}

/// Start-up behaviour that is not part of the persistent configuration.
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// When `<data_root>` has no token yet, copy Just Terminal's (default on).
    pub import_legacy_token: bool,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            import_legacy_token: true,
        }
    }
}

/// What bro tells the bridge about a session when it starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub shell: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Project root key (bro_core::projects::ProjectKey::key) — the bridge derives its
    /// `directory-<sha>` project id from it
    pub project: Option<PathBuf>,
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    /// "claude" | "codex" | "pi" | "omp" when bro launched an agent
    pub agent: Option<String>,
    /// "orchestrator" for bro's orchestrator session (its tools never act on it).
    #[serde(default)]
    pub kind: Option<String>,
}

/// Remote client wants a new session (POST /api/sessions or ws `create`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateRequest {
    pub title: Option<String>,
    pub profile_id: Option<String>,
    pub shell: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub project_id: Option<String>,
    /// Model for an agent session ("opus"); None = the login's default.
    #[serde(default)]
    pub model: Option<String>,
}

/// A model or thinking choice from the phone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FocusControlChange {
    Model { value: String },
    Thinking { value: String },
}

/// Commands the bridge asks bro to execute on its PTYs.
pub enum BridgeCommand {
    /// A durable task needs its coordinator. The inbox deduplicates this task/revision/action.
    CoordinationWake { task_id: String, revision: u64, seq: u64, project: PathBuf, action: String },
    /// The desktop and phone share the model and thinking controls, including changes waiting for a turn to end.
    FocusControls {
        id: String,
        change: Option<FocusControlChange>,
        reply: tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
    },
    Seen { id: String, event_id: String },
    Input {
        id: String,
        data: Vec<u8>,
    },
    /// Composed input waits until bro has handed the bytes to the session before pacing the next write.
    Write {
        id: String,
        data: Vec<u8>,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Resize {
        id: String,
        cols: u16,
        rows: u16,
    },
    Kill {
        id: String,
    },
    Rename {
        id: String,
        title: String,
    },
    /// Reply with the new session id (the session must also be `register`ed).
    Create {
        req: CreateRequest,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<String>>,
    },
    /// Open or reuse a project coordinator; a source coordinator supplies its exact launch configuration.
    OpenOrchestrator {
        cwd: PathBuf,
        source: String,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<String>>,
    },
    /// Bring this session to the foreground in the TUI (optional nicety)
    Focus {
        id: String,
    },
    /// Show bro's file preview for a resolved file in this session's workspace
    /// (`POST /api/sessions/{id}/open`, e.g. an agent running `bro open`).
    OpenFile {
        id: String,
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// Open a resolved file in the desktop's editor
    /// (`POST /api/sessions/{id}/open-in-editor`, "Open on desktop").
    OpenInEditor {
        id: String,
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// For bro's orchestrator session (see [`Bridge::route_orchestrator`]).
    Orchestrator(OrchestratorCommand),
    /// Show one project's sessions tiled together in bro's window (its project tab), or every session
    /// (`None`, the All tab). `project` is the project's directory.
    Tile {
        project: Option<PathBuf>,
    },
}

/// What bro's orchestrator session is asked to do, once [`Bridge::route_orchestrator`] hands it everything
/// the built-in orchestrator (Hugh) used to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrchestratorCommand {
    /// Explicit response or steering for an in-flight request; never interpreted as a new assignment.
    Control { id: String, text: String, request_id: Option<String>, kind: String },
    /// A message from an orchestrator panel (web, phone): answer with [`Bridge::orchestrator_answer`] and `id`.
    Message { id: String, text: String, voice: bool, focused: Option<String>, tag: Option<String> },
    /// A scheduled trigger came due (bro fires it; type `prompt` in).
    Scheduled { id: String, name: String, prompt: String, persist: bool },
    /// Stop the running turn.
    Cancel,
    /// Start the orchestrator on a fresh conversation once its turn ends, then give it `handoff`.
    ClearContext { handoff: Option<String> },
}

impl std::fmt::Debug for BridgeCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CoordinationWake { task_id, revision, seq, project, action } => f.debug_struct("CoordinationWake").field("task_id", task_id).field("revision", revision).field("seq", seq).field("project", project).field("action", action).finish(),
            Self::FocusControls { id, change, .. } => f.debug_struct("FocusControls").field("id", id).field("change", change).finish(),
            Self::Seen { id, event_id } => f.debug_struct("Seen").field("id", id).field("event_id", event_id).finish(),
            Self::Input { id, data } => f
                .debug_struct("Input")
                .field("id", id)
                .field("bytes", &data.len())
                .finish(),
            Self::Write { id, data, .. } => f
                .debug_struct("Write")
                .field("id", id)
                .field("bytes", &data.len())
                .finish(),
            Self::Resize { id, cols, rows } => f
                .debug_struct("Resize")
                .field("id", id)
                .field("cols", cols)
                .field("rows", rows)
                .finish(),
            Self::Kill { id } => f.debug_struct("Kill").field("id", id).finish(),
            Self::Rename { id, title } => f
                .debug_struct("Rename")
                .field("id", id)
                .field("title", title)
                .finish(),
            Self::Create { req, .. } => f.debug_struct("Create").field("req", req).finish(),
            Self::OpenOrchestrator { cwd, source, .. } => f.debug_struct("OpenOrchestrator").field("cwd", cwd).field("source", source).finish(),
            Self::Focus { id } => f.debug_struct("Focus").field("id", id).finish(),
            Self::OpenFile { id, path, line, column } => f
                .debug_struct("OpenFile")
                .field("id", id)
                .field("path", path)
                .field("line", line)
                .field("column", column)
                .finish(),
            Self::Orchestrator(c) => f.debug_tuple("Orchestrator").field(c).finish(),
            Self::Tile { project } => f.debug_struct("Tile").field("project", project).finish(),
            Self::OpenInEditor { id, path, line, column } => f
                .debug_struct("OpenInEditor")
                .field("id", id)
                .field("path", path)
                .field("line", line)
                .field("column", column)
                .finish(),
        }
    }
}

/// A running bridge as described by `<data_root>/.terminal-web-server.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u16,
    pub pid: u32,
}

impl Endpoint {
    /// Loopback base URL (`http://127.0.0.1:<port>`); loopback needs no token.
    pub fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// Reads the discovery file a bridge writes when it binds (and removes on
/// shutdown) so local tools like `bro open` can reach it. `None` when no
/// bridge has published one under `data_root`.
pub fn read_endpoint(data_root: &std::path::Path) -> Option<Endpoint> {
    let bytes = std::fs::read(data_root.join(server::SERVER_INFO_FILE)).ok()?;
    let info: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(Endpoint {
        port: u16::try_from(info["port"].as_u64()?).ok().filter(|port| *port != 0)?,
        pid: u32::try_from(info["pid"].as_u64().unwrap_or_default()).unwrap_or_default(),
    })
}

/// A project as the remote clients (phone, web) arrange it, for bro's own project tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientProject {
    pub cwd: PathBuf,
    /// The name a client gave it; None while it goes by its folder's.
    pub name: Option<String>,
    /// Its place in the order a client arranged the projects in; None until one did.
    pub order: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    pub sound: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeStatus {
    pub running: bool,
    pub port: u16,
    /// `http://<lan-ip>:<port>/?token=…` per interface, local URL first
    pub urls: Vec<String>,
    pub token: String,
    pub clients: usize,
    pub error: Option<String>,
}

/// Cheap-to-clone handle; the host runs on its own tokio runtime thread.
#[derive(Clone)]
pub struct Bridge {
    inner: Arc<BridgeInner>,
}

struct BridgeInner {
    state: AppState,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

const THREAD_NAME: &str = "bro-bridge";
/// How long `shutdown` waits for the host thread.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

fn path_text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Bridge {
    /// Start the host. `on_command` runs on a bridge thread — push into a channel and wake the UI.
    pub fn start(
        cfg: BridgeConfig,
        on_command: Box<dyn Fn(BridgeCommand) + Send + Sync>,
    ) -> anyhow::Result<Bridge> {
        Self::start_with(cfg, StartOptions::default(), on_command)
    }

    /// [`Bridge::start`] with explicit [`StartOptions`]. Spawns the host
    /// thread and its multi-threaded runtime, and returns once the listener
    /// is bound (or with the bind error).
    pub fn start_with(
        cfg: BridgeConfig,
        options: StartOptions,
        on_command: Box<dyn Fn(BridgeCommand) + Send + Sync>,
    ) -> anyhow::Result<Bridge> {
        std::fs::create_dir_all(&cfg.data_root)?;
        let legacy = if options.import_legacy_token {
            auth::legacy_token_candidates()
        } else {
            Vec::new()
        };
        let token = auth::load_or_create_token(&cfg.data_root, &legacy);
        let state = AppState::new(cfg.clone(), token, Arc::from(on_command));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let host_state = state.clone();
        let thread = std::thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .thread_name("bro-bridge-worker")
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!("bridge runtime failed: {error}")));
                        return;
                    }
                };
                let _ = host_state.runtime.set(runtime.handle().clone());
                runtime.spawn(schedule::run(host_state.clone()));
                runtime.block_on(run_host(host_state, cfg, ready_tx));
                runtime.shutdown_timeout(Duration::from_secs(1));
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Bridge {
                inner: Arc::new(BridgeInner {
                    state,
                    thread: Mutex::new(Some(thread)),
                }),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(anyhow::anyhow!(error))
            }
            Err(_) => {
                let _ = thread.join();
                Err(anyhow::anyhow!("the bridge thread exited during start-up"))
            }
        }
    }

    fn state(&self) -> &AppState {
        &self.inner.state
    }

    pub fn register(&self, meta: SessionMeta) {
        let cwd = meta.cwd.as_deref().map(path_text).unwrap_or_default();
        let mut summary = model::TerminalSessionSummary::native(
            meta.id, meta.title, meta.shell, cwd, 0, meta.cols, meta.rows,
        );
        summary.pid = meta.pid;
        summary.args = meta.args;
        summary.kind = meta.kind;
        let hint = meta.agent.as_deref().and_then(agents::Agent::parse);
        let project = meta
            .project
            .as_deref()
            .map(path_text)
            .filter(|dir| !dir.trim().is_empty());
        self.state().register(summary, project, hint);
    }

    /// Raw PTY output bytes, in order.
    pub fn output(&self, id: &str, data: &[u8]) {
        self.state().append_output(id, data);
    }

    pub fn title(&self, id: &str, title: &str) {
        self.state().rename(id, title.to_owned());
    }

    pub fn cwd(&self, id: &str, cwd: &std::path::Path) {
        self.state().set_cwd(id, path_text(cwd));
    }

    /// The session's project root changed (see [`SessionMeta::project`]).
    pub fn set_project(&self, id: &str, project: Option<&std::path::Path>) {
        self.state().set_project_dir(id, project.map(path_text));
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        self.state().resize_from_host(id, cols, rows);
    }

    pub fn exit(&self, id: &str, code: Option<i32>) {
        self.state().exit(id, code);
    }

    pub fn unregister(&self, id: &str) {
        self.state().unregister(id);
    }

    /// Durable task state. Performs file I/O; call from a worker thread.
    pub fn coordination_snapshot(&self) -> Result<bro_core::coordination::Snapshot, String> {
        coordination::snapshot_sync(&self.state())
    }

    /// A recoverable task context packet. Performs file I/O; call off the UI thread.
    pub fn coordination_context(&self, task_id: &str) -> Result<bro_core::coordination::ContextPacket, String> {
        coordination::context_sync(&self.state(), task_id)
    }

    /// Apply a coordinator transition through the shared durable authority, off the UI thread.
    pub fn coordination_execute(&self, command_id: &str, actor: &bro_core::coordination::Actor, command: bro_core::coordination::Command) -> Result<bro_core::coordination::Outcome, String> {
        coordination::execute_sync(&self.state(), command_id, actor, command)
    }

    /// Why the agent waits and its recap (`docs/agent-insights.md`). Publishes
    /// a `session` event only when something changed, at most twice a second;
    /// unknown ids are ignored.
    pub fn set_insights(&self, id: &str, insights: SessionInsights) {
        self.state().set_insights(id, insights);
    }

    /// The files the agent sent / touched, newest first. The summary carries
    /// their counts (`agentFiles`); clients fetch `GET .../agent-files`.
    pub fn set_agent_files(&self, id: &str, files: AgentFiles) {
        self.state().set_agent_files(id, files);
    }

    /// What session `id` was last given by [`Bridge::set_agent_files`] (None for an unknown id).
    pub fn agent_files(&self, id: &str) -> Option<AgentFiles> {
        self.state().agent_files(id)
    }

    /// Replaces the launch profiles advertised to clients (default: shell,
    /// claude, codex, pi, omp) and broadcasts them.
    pub fn set_profiles(&self, profiles: Vec<TerminalProfile>) {
        self.state().set_profiles(profiles);
    }

    /// How the orchestrator starts Claude Code (program, login environment).
    /// Takes effect with its next message.
    pub fn set_orchestrator_launch(&self, launch: OrchestratorLaunch) {
        self.state().orchestrator.set_launch(launch);
    }

    /// Folders the user works in (open projects, recents, past sessions), used
    /// to resolve spoken project names ("open a session in justgains").
    pub fn set_known_folders(&self, folders: Vec<PathBuf>) {
        *self.state().known_folders.write() = folders.iter().map(|dir| path_text(dir)).collect();
    }

    /// Whether clients may open pictures, videos, sounds and PDFs from anywhere on this computer (true), or
    /// only media tied to the work: in a session's folders or a known one ([`Bridge::set_known_folders`]),
    /// what the agents sent, made or looked at, what was attached or opened, what a conversation names
    /// (false, the default). Code and text never leave the session's folders either way.
    pub fn set_media_anywhere(&self, anywhere: bool) {
        self.state().media_anywhere.store(anywhere, std::sync::atomic::Ordering::Relaxed);
    }

    /// Sends a message to the orchestrator as if typed in its panel (voice
    /// input uses this). Returns at once; progress arrives through
    /// [`Bridge::on_orchestrator_update`] and the clients' transcript.
    pub fn orchestrator_send(&self, text: String) -> anyhow::Result<()> {
        self.orchestrator_send_with(text, MessageContext::default())
    }

    /// [`Bridge::orchestrator_send`] with where the message came from: voice
    /// input passes the session the user was looking at, so Hugh can route a
    /// prompt straight to it.
    pub fn orchestrator_send_with(&self, text: String, context: MessageContext) -> anyhow::Result<()> {
        let state = self.state().clone();
        let runtime = state
            .runtime
            .get()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the bridge is not running"))?;
        runtime.spawn(async move {
            let tag = context.tag.clone();
            if let Err((_, message)) = orchestrator::send_message_with(&state, text, context).await {
                state.orchestrator.emit_failure(message, tag);
            }
        });
        Ok(())
    }

    /// Interrupts the orchestrator's running turn (and drops queued messages).
    pub fn orchestrator_cancel(&self) -> bool {
        let state = self.state().clone();
        if state.orchestrator.routed() {
            state.orchestrator.cancel_routed();
            state.send_command(BridgeCommand::Orchestrator(OrchestratorCommand::Cancel));
            return true;
        }
        match state.runtime.get().cloned() {
            Some(runtime) => {
                let _guard = runtime.enter();
                state.orchestrator.cancel()
            }
            None => false,
        }
    }

    /// Hand everything the built-in orchestrator did to bro's orchestrator session: panel messages arrive as
    /// [`OrchestratorCommand::Message`], cancel / clear as commands, and scheduled triggers fire. Off: Hugh
    /// answers panels himself again.
    pub fn route_orchestrator(&self, on: bool) {
        self.state().orchestrator.set_routed(on);
    }

    /// The orchestrator session's answer to a routed panel message (`id` from the command).
    pub fn orchestrator_answer(&self, id: &str, text: &str, ok: bool) {
        self.state().orchestrator.answer(id, text, ok);
    }

    /// The orchestrator closed: its session-only scheduled triggers go with it.
    pub fn schedule_clear_session(&self) {
        self.state().schedule.clear_session();
    }

    /// Every scheduled trigger (session ones first).
    pub fn schedules(&self) -> Vec<schedule::Trigger> {
        self.state().schedule.list()
    }

    /// A watchdog wake-up for the orchestrator session to do instead of a headless reviewer: claims the wake
    /// (the watchdog shows it running) and returns its generation and the full brief (instructions, plan,
    /// what happened). Report back with [`Bridge::watchdog_finish`].
    pub fn watchdog_brief(&self, id: &str, wake: WatchdogWake) -> Result<(u64, String), String> {
        watchdog::run::brief_for_orchestrator(self.state(), id, wake)
    }

    /// The orchestrator finished a watchdog wake-up from [`Bridge::watchdog_brief`].
    pub fn watchdog_finish(&self, id: &str, generation: u64, summary: String, ok: bool) {
        self.state().watchdogs.finish(id, generation, summary, ok);
    }

    /// Turn progress for bro's own UI. Runs on a bridge thread; don't block.
    pub fn on_orchestrator_update(&self, f: Box<dyn Fn(OrchestratorUpdate) + Send + Sync>) {
        self.state().orchestrator.on_update(Arc::from(f));
    }

    /// Start a watchdog on session `id` (replacing one it had). bro wakes it with [`Bridge::watchdog_wake`].
    pub fn watchdog_start(&self, id: &str, plan: WatchdogPlan) -> WatchdogSnapshot {
        self.state().watchdogs.start(id, plan)
    }

    /// New milestones / options for a running watchdog; its log stays. False when the session has none.
    pub fn watchdog_set_plan(&self, id: &str, plan: WatchdogPlan) -> bool {
        self.state().watchdogs.set_plan(id, plan)
    }

    /// Stop and forget the watchdog (a running review ends).
    pub fn watchdog_stop(&self, id: &str) -> bool {
        self.state().watchdogs.stop(id)
    }

    pub fn watchdog_pause(&self, id: &str, paused: bool) -> bool {
        self.state().watchdogs.set_paused(id, paused)
    }

    pub fn watchdog(&self, id: &str) -> Option<WatchdogSnapshot> {
        self.state().watchdogs.get(id)
    }

    pub fn watchdogs(&self) -> Vec<WatchdogSnapshot> {
        self.state().watchdogs.all()
    }

    /// Wake session `id`'s watchdog: a fresh reviewer starts at once on the bridge runtime. Errs when there is
    /// no watchdog, it is already reviewing, paused, finished or out of wake-ups.
    pub fn watchdog_wake(&self, id: &str, wake: WatchdogWake) -> Result<(), String> {
        watchdog::run::spawn(self.state(), id, wake)
    }

    /// Watchdog changes and progress for bro's UI. Runs on a bridge thread; don't block.
    pub fn on_watchdog_update(&self, f: Box<dyn Fn(WatchdogUpdate) + Send + Sync>) {
        self.state().watchdogs.on_update(Arc::from(f));
    }

    /// Ask the Claude Code mod running in session `session` (bro's id for the pane, the mod's
    /// `BRO_SESSION_ID`) to do `job` with Claude Code's own MCP connections (`{"kind":"read",...}`,
    /// `{"kind":"call",...}`; the mod reads the fields). `reply` runs once, on a bridge thread, with the mod's
    /// result or why there is none: nobody took the job within 10 s (the session has no bro mod listening), the
    /// mod didn't answer within 90 s, the mod's own error, or the bridge stopped. Don't block in it.
    pub fn mod_ask(
        &self,
        session: &str,
        job: serde_json::Value,
        reply: Box<dyn FnOnce(Result<serde_json::Value, String>) + Send>,
    ) {
        mod_jobs::ask(self.state(), session, job, reply);
    }

    /// The session's mod is taking jobs: it asked for work within the last ~40 s (its long poll is open or
    /// just was). Cheap: one lock and a map lookup.
    pub fn mod_listening(&self, session: &str) -> bool {
        self.state().mod_jobs.listening(session)
    }

    /// Push a notification to connected clients (and history).
    pub fn notify(&self, n: Notification) {
        let state = self.state();
        let session_title = n
            .session_id
            .as_deref()
            .and_then(|id| state.summary(id))
            .map(|s| s.title);
        state.notify(
            model::TerminalNotification {
                id: String::new(),
                at: String::new(),
                origin: "api".into(),
                session_id: n.session_id,
                session_title,
                title: Some(n.title).filter(|title| !title.is_empty()),
                body: Some(n.body).filter(|body| !body.is_empty()),
                sound: n.sound.then(|| "done".to_owned()),
            },
            false,
        );
    }

    /// Notifications the bridge raised itself (BEL / OSC 9 / OSC 777 / agent done /
    /// agent awaiting input) so the TUI can toast them too.
    pub fn on_notification(&self, f: Box<dyn Fn(Notification) + Send + Sync>) {
        self.state().notification_sinks.write().push(Arc::from(f));
    }

    /// The projects remote clients see, with the names and order they gave them. Cheap: one lock, no I/O.
    pub fn client_projects(&self) -> Vec<ClientProject> {
        self.state().client_projects()
    }

    /// How many remote client slots are subscribed to session `id` right now (web,
    /// phone). Cheap: one lock and a map lookup, fine to call every frame.
    pub fn remote_viewers(&self, id: &str) -> usize {
        self.state().remote_viewers(id)
    }

    pub fn status(&self) -> BridgeStatus {
        let state = self.state();
        let (host, port) = {
            let inner = state.inner.lock();
            (inner.host, inner.port)
        };
        let urls = net::server_access_urls(
            host,
            port,
            &state.token,
            &net::network_interface_addresses(),
        )
        .into_iter()
        .filter_map(|url| url["url"].as_str().map(str::to_owned))
        .collect();
        BridgeStatus {
            running: state.running.load(Ordering::Relaxed),
            port,
            urls,
            token: state.token.as_ref().clone(),
            clients: state.clients.load(Ordering::Relaxed),
            error: state.last_error.lock().clone(),
        }
    }

    /// Base URL local processes use to reach this bridge (`http://127.0.0.1:<port>`,
    /// or the bound address when it is a specific interface). Cheap: no
    /// interface enumeration. `None` until the listener is bound.
    pub fn local_url(&self) -> Option<String> {
        let (host, port) = {
            let inner = self.state().inner.lock();
            (inner.host, inner.port)
        };
        if port == 0 {
            return None;
        }
        Some(if host.is_unspecified() || host.is_loopback() {
            format!("http://127.0.0.1:{port}")
        } else if host.is_ipv6() {
            format!("http://[{host}]:{port}")
        } else {
            format!("http://{host}:{port}")
        })
    }

    /// Pairing QR (unicode half-blocks, one String per row) for the first LAN URL.
    /// Falls back to the local URL when no LAN interface is available. Light
    /// modules are the drawn blocks: render light-on-dark.
    pub fn pairing_qr(&self) -> Option<Vec<String>> {
        let url = self.pairing_url()?;
        net::render_qr(&url)
    }

    /// Every bridge on this machine from the last scan, this one first
    /// (`current`). Scans run while remote clients are connected; cheap.
    pub fn local_hosts(&self) -> Vec<LocalHost> {
        self.state().local_hosts()
    }

    /// The URL [`Bridge::pairing_qr`] encodes.
    pub fn pairing_url(&self) -> Option<String> {
        let status = self.status();
        status
            .urls
            .iter()
            .find(|url| url.contains("?token="))
            .or_else(|| status.urls.first())
            .cloned()
    }

    pub fn shutdown(&self) {
        let state = self.state();
        state.orchestrator.shutdown();
        for dog in state.watchdogs.all() {
            state.watchdogs.stop(&dog.session_id);
        }
        state.shutdown.send_replace(true);
        state.mod_jobs.fail_all();
        let Some(thread) = self.inner.thread.lock().take() else {
            return;
        };
        // Called from a bridge thread (e.g. inside `on_command`): joining
        // would wait on ourselves; the host still stops on its own.
        let on_bridge_thread = std::thread::current()
            .name()
            .is_some_and(|name| name.starts_with(THREAD_NAME));
        if on_bridge_thread {
            return;
        }
        let started = Instant::now();
        while !thread.is_finished() && started.elapsed() < SHUTDOWN_WAIT {
            std::thread::sleep(Duration::from_millis(10));
        }
        if thread.is_finished() {
            let _ = thread.join();
        }
    }
}

/// The host's life on its runtime: bind, announce, serve until shutdown.
async fn run_host(
    state: AppState,
    cfg: BridgeConfig,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let (listener, host, port) = match server::bind(&cfg.bind, cfg.port, cfg.automatic_port).await {
        Ok(bound) => bound,
        Err(error) => {
            *state.last_error.lock() = Some(error.clone());
            let _ = ready.send(Err(error));
            return;
        }
    };
    {
        let mut inner = state.inner.lock();
        inner.host = host;
        inner.port = port;
    }
    server::write_server_info(&state);
    hosts::register(&state);
    server::spawn_background(&state);
    state.running.store(true, Ordering::Relaxed);
    let _ = ready.send(Ok(()));
    if let Err(error) = server::serve(listener, state.clone()).await {
        *state.last_error.lock() = Some(error.to_string());
    }
    state.running.store(false, Ordering::Relaxed);
    state.mod_jobs.fail_all();
    hosts::unregister(&state);
    server::remove_server_info(&state);
}
