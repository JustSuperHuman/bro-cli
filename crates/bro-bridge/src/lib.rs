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
//! | `orchestrator` | Hugh, the built-in agent over every session: a headless Claude Code process using the tools at `/mcp` |
//!
//! # Data root
//!
//! `<data_root>` (default `~/.bro/bridge`, `$BRO_DIR/bridge` when set) holds
//! `.terminal-web-token`, `.terminal-web-projects.json`,
//! `.terminal-web-server.json` (`{pid,host,port,startedAt,runtime:"bro"}`) and
//! the orchestrator config/history. On first start the token is imported from
//! Just Terminal (`%LOCALAPPDATA%\TerminalWeb` or its MSIX-redirected
//! `%LOCALAPPDATA%\Packages\*Terminal*\LocalCache\Local\TerminalWeb`) so
//! phones paired with Just Terminal keep working; see [`StartOptions`].

mod agents;
mod alerts;
mod auth;
mod commands;
mod file_preview;
mod file_search;
mod model;
mod net;
mod notifications;
mod orchestrator;
mod output_filter;
mod profiles;
mod project_store;
mod projects;
mod prompt;
mod rest;
mod rest_sessions;
mod server;
mod session;
mod slash_commands;
mod state;
mod vt_stream;
mod ws;

#[cfg(test)]
mod tests;

pub use auth::is_authorized;
pub use model::TerminalProfile;
pub use orchestrator::{MessageContext, OrchestratorLaunch, OrchestratorUpdate};

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
}

/// Commands the bridge asks bro to execute on its PTYs.
pub enum BridgeCommand {
    Input {
        id: String,
        data: Vec<u8>,
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
    /// Bring this session to the foreground in the TUI (optional nicety)
    Focus {
        id: String,
    },
}

impl std::fmt::Debug for BridgeCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input { id, data } => f
                .debug_struct("Input")
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
            Self::Focus { id } => f.debug_struct("Focus").field("id", id).finish(),
        }
    }
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
        match state.runtime.get().cloned() {
            Some(runtime) => {
                let _guard = runtime.enter();
                state.orchestrator.cancel()
            }
            None => false,
        }
    }

    /// Turn progress for bro's own UI. Runs on a bridge thread; don't block.
    pub fn on_orchestrator_update(&self, f: Box<dyn Fn(OrchestratorUpdate) + Send + Sync>) {
        self.state().orchestrator.on_update(Arc::from(f));
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

    /// Pairing QR (unicode half-blocks, one String per row) for the first LAN URL.
    /// Falls back to the local URL when no LAN interface is available. Light
    /// modules are the drawn blocks: render light-on-dark.
    pub fn pairing_qr(&self) -> Option<Vec<String>> {
        let url = self.pairing_url()?;
        net::render_qr(&url)
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
        state.shutdown.send_replace(true);
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
    server::spawn_background(&state);
    state.running.store(true, Ordering::Relaxed);
    let _ = ready.send(Ok(()));
    if let Err(error) = server::serve(listener, state.clone()).await {
        *state.last_error.lock() = Some(error.to_string());
    }
    state.running.store(false, Ordering::Relaxed);
    server::remove_server_info(&state);
}
