//! Well-known locations. Every function honours the same env overrides as bro v1 and
//! reads the environment on each call, so tests (and child processes) can redirect it.
use crate::util::{env_path, home_dir, is_within};
use std::path::PathBuf;

/// `~/.bro` (override `BRO_DIR`). config.json, state.json, caches, usage-history.json.
pub fn bro_dir() -> PathBuf {
    env_path("BRO_DIR").unwrap_or_else(|| home_dir().join(".bro"))
}

/// `~/.claude` (override `CLAUDE_CONFIG_DIR` is deliberately NOT honoured) — "claude:local".
pub fn claude_local_dir() -> PathBuf {
    home_dir().join(".claude")
}

/// `~/.claude-max-pool` (override `CLAUDE_POOL_DIR`); accounts live in `accounts/<name>`.
pub fn claude_pool_dir() -> PathBuf {
    env_path("CLAUDE_POOL_DIR").unwrap_or_else(|| home_dir().join(".claude-max-pool"))
}

/// `~/.claude-max-pool/accounts` — one `CLAUDE_CONFIG_DIR` per sub-directory.
pub fn claude_accounts_dir() -> PathBuf {
    claude_pool_dir().join("accounts")
}

/// `~/.codex` — "codex:local". Like v1, `CODEX_HOME` is honoured — except when it points
/// inside bro's own codex-profiles dir, which means bro itself is running inside a
/// profile-launched session and the machine's own login is still `~/.codex`.
pub fn codex_local_dir() -> PathBuf {
    if let Some(home) = env_path("CODEX_HOME")
        && !is_within(&codex_profiles_dir(), &home)
    {
        return home;
    }
    home_dir().join(".codex")
}

/// `~/.bro/codex-profiles` (override `BRO_CODEX_PROFILES_DIR`).
pub fn codex_profiles_dir() -> PathBuf {
    env_path("BRO_CODEX_PROFILES_DIR").unwrap_or_else(|| bro_dir().join("codex-profiles"))
}

/// Pi's agent dir: `$PI_CODING_AGENT_DIR` or `~/.pi/agent` (models.json, sessions/).
pub fn pi_agent_dir() -> PathBuf {
    env_path("PI_CODING_AGENT_DIR").unwrap_or_else(|| home_dir().join(".pi").join("agent"))
}

/// omp's agent dir: `~/.omp/agent` (models.yml, sessions/).
pub fn omp_agent_dir() -> PathBuf {
    home_dir().join(".omp").join("agent")
}

/// `~/.bro/config.json`.
pub fn config_path() -> PathBuf {
    bro_dir().join("config.json")
}

/// `~/.bro/state.json` (override `BRO_STATE_PATH`, as v1).
pub fn state_path() -> PathBuf {
    env_path("BRO_STATE_PATH").unwrap_or_else(|| bro_dir().join("state.json"))
}

/// `~/.bro/v2.toml` — v2-only settings.
pub fn settings_path() -> PathBuf {
    bro_dir().join("v2.toml")
}
