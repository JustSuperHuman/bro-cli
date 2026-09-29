//! Turns "harness × profile × provider × model × cwd" into a concrete command.
//! No process spawning here (the TUI spawns into a PTY). May write harness config
//! files that v1 also wrote (pi models.json, omp models.yml) — never ccr.
use crate::Harness;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    #[default]
    Default,
    /// claude `--permission-mode auto`
    Auto,
    /// claude `--dangerously-skip-permissions`, codex bypass flag, omp `--yolo`
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resume {
    pub session_id: String,
    /// Fork instead of continuing (cross-profile resume always forks)
    pub fork: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub harness: Harness,
    /// Login to run as ("claude:work", "codex:local"). None + provider = key-based provider.
    pub profile_id: Option<String>,
    /// Provider id from the catalogue, or "pool" (Claude account pool via proxy).
    /// None = the profile's native backend.
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub cwd: PathBuf,
    pub resume: Option<Resume>,
    pub permission: Permission,
    pub browser: crate::browser::BrowserMode,
    pub extra_args: Vec<String>,
}

/// Where the running bro-proxy lives, so cross-format launches can be routed through it.
#[derive(Debug, Clone, Default)]
pub struct LaunchCtx {
    /// e.g. "http://127.0.0.1:3458"
    pub proxy_base: Option<String>,
    /// Token the proxy expects from local clients
    pub proxy_token: Option<String>,
}

/// A proxy route the caller must register with bro-proxy before spawning.
/// Harness talks to `{proxy_base}/r/{route_id}/v1/...`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRequest {
    pub route_id: String,
    /// Provider id, "pool", "codex:<profile>" (ChatGPT backend) or "claude:<profile>" (OAuth)
    pub upstream: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub cwd: PathBuf,
    /// Short human label for tabs, e.g. "claude · work · opus"
    pub label: String,
    /// Route to register with the proxy (cross-format provider launches)
    pub route: Option<RouteRequest>,
    /// Temp files to delete when the session exits (staged cross-profile resumes)
    pub cleanup: Vec<PathBuf>,
}

pub fn build(spec: &LaunchSpec, ctx: &LaunchCtx) -> anyhow::Result<CommandSpec> { todo!() }

/// Resolve an executable on PATH (handles .cmd/.exe shims on Windows).
pub fn which(program: &str) -> Option<PathBuf> { todo!() }
