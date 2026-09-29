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

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// First port to try (10001)
    pub port: u16,
    /// Try port..port+20 when taken
    pub automatic_port: bool,
    pub bind: String,
    /// Token file, projects, notifications, attachments. Default `~/.bro/bridge`.
    pub data_root: PathBuf,
    /// Serve the embedded web client for unknown paths
    pub web_interface: bool,
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
    Input { id: String, data: Vec<u8> },
    Resize { id: String, cols: u16, rows: u16 },
    Kill { id: String },
    Rename { id: String, title: String },
    /// Reply with the new session id (the session must also be `register`ed).
    Create { req: CreateRequest, reply: tokio::sync::oneshot::Sender<anyhow::Result<String>> },
    /// Bring this session to the foreground in the TUI (optional nicety)
    Focus { id: String },
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
    _private: (),
}

impl Bridge {
    /// Start the host. `on_command` runs on a bridge thread — push into a channel and wake the UI.
    pub fn start(cfg: BridgeConfig, on_command: Box<dyn Fn(BridgeCommand) + Send + Sync>) -> anyhow::Result<Bridge> { todo!() }
    pub fn register(&self, meta: SessionMeta) { todo!() }
    /// Raw PTY output bytes, in order.
    pub fn output(&self, id: &str, data: &[u8]) { todo!() }
    pub fn title(&self, id: &str, title: &str) { todo!() }
    pub fn cwd(&self, id: &str, cwd: &std::path::Path) { todo!() }
    pub fn resize(&self, id: &str, cols: u16, rows: u16) { todo!() }
    pub fn exit(&self, id: &str, code: Option<i32>) { todo!() }
    pub fn unregister(&self, id: &str) { todo!() }
    /// Push a notification to connected clients (and history).
    pub fn notify(&self, n: Notification) { todo!() }
    /// Notifications the bridge raised itself (BEL / OSC 9 / OSC 777 / agent done /
    /// agent awaiting input) so the TUI can toast them too.
    pub fn on_notification(&self, f: Box<dyn Fn(Notification) + Send + Sync>) { todo!() }
    pub fn status(&self) -> BridgeStatus { todo!() }
    /// Pairing QR (unicode half-blocks, one String per row) for the first LAN URL.
    pub fn pairing_qr(&self) -> Option<Vec<String>> { todo!() }
    pub fn shutdown(&self) { todo!() }
}
