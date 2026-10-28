//! bro's session server: a headless bro process (`bro host`) that owns every session's pseudo-terminal, process
//! tree and a screen model, so sessions keep running when the window showing them closes, crashes or updates.
//!
//! The terminal UI and the desktop app are its clients and work the same way: a session's raw output streams to
//! the window, which parses and draws it with its own emulator exactly as it does for a session it runs itself.
//! Only bytes cross the process boundary (a loopback socket), so drawing, scrolling and selection cost the same.
//! Attaching to a running session first sends a snapshot that rebuilds its screen and scrollback in a fresh
//! emulator; the server answers the cursor-position query itself, so a program never waits on a window.
//!
//! Who connects ([`Role`]): one *primary* drives the server at a time and runs bro's automation (a window, or the
//! background bro that stands in while no window is open); a window that connects takes over from the last one.
//! *Viewers* (a second window, say the terminal UI over SSH) watch and type into the same sessions. Any number of
//! clients can watch a session; whoever types into it controls it, and its size is that client's. Queries
//! (`bro host status`, `bro host stop`) connect without either.
//!
//! The server keeps what each session needs to come back (no secrets) in `~/.bro/v2-host-sessions.json`. A server
//! that stopped without being asked to (a crash, the computer shutting down) leaves its sessions there, and the
//! next one offers them as [`HostInfo::lost`] so a window can bring them back.
//!
//! Discovery: the running server records `{pid, port, token, protocol, version}` in `~/.bro/v2-host.json`
//! (written atomically, readable by you alone) and holds `~/.bro/v2-host.lock` for its lifetime.

pub mod answer;
pub mod client;
pub mod record;
mod screen;
pub mod server;
pub mod service;
mod store;
mod wire;

#[cfg(test)]
mod tests;

pub use client::{Client, ClientEvent, Remote};
pub use server::{ServeOpts, serve};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

/// Bumped whenever the wire format changes: a window refuses a server speaking another protocol (restarting the
/// server from settings replaces it with this bro's).
pub const PROTOCOL: u32 = 2;

/// A program to start on a new pseudo-terminal.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SpawnRequest {
    /// The window's id for the session (it keeps it across reattaches).
    pub id: String,
    /// Resolved program (the window resolves it on its own PATH).
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The complete environment: the server clears its own, so a session sees the window's, not the server's.
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
    /// Whatever the window wants back when it reattaches (its session metadata). Kept in memory only: it can
    /// hold keys.
    pub blob: Value,
    /// What brings the session back after the server stopped: the window's metadata without its secrets. Written
    /// to disk (`v2-host-sessions.json`); `Null`: nothing to bring back.
    #[serde(default)]
    pub keep: Value,
}

/// A session the server holds.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub pid: Option<u32>,
    pub rows: u16,
    pub cols: u16,
    /// The program exited (its exit code, when it had one, in `code`).
    pub exited: bool,
    pub code: Option<i32>,
    pub blob: Value,
}

/// How a client connects.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A window: drives the server, taking over from whoever did.
    #[default]
    Window,
    /// The background bro: drives the server while no window does, and steps aside for one.
    Background,
    /// Watches and types into the sessions beside the primary.
    Viewer,
    /// Only asks (status, stop).
    Query,
}

/// A client the server has connected.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub pid: u32,
    pub role: Role,
}

/// Who's connected: the primary (a window or the background bro) and how many viewers.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Peers {
    pub primary: Option<Peer>,
    pub viewers: usize,
}

/// What the server tells a window that connects.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HostInfo {
    pub protocol: u32,
    pub pid: u32,
    /// The bro version the server runs (an older one keeps running after an update until it's restarted).
    pub version: String,
    /// Unix ms the server started.
    pub started_ms: u64,
    /// Oldest first.
    pub sessions: Vec<SessionInfo>,
    /// Small values a window keeps across its own restarts (the translating proxy's token).
    #[serde(default)]
    pub values: HashMap<String, String>,
    /// Sessions that were running when a server last stopped without being asked to (crashed, or the computer
    /// shut down), with what brings them back (their `keep`), until a window brings them back or lets them go.
    #[serde(default)]
    pub lost: Vec<SessionInfo>,
    /// Who else is connected.
    #[serde(default)]
    pub peers: Peers,
    /// This connection's role, as granted.
    #[serde(default)]
    pub role: Role,
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
