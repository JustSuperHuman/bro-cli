//! Well-known locations. Every function honours the same env overrides as bro v1.
use std::path::PathBuf;

/// `~/.bro` (override `BRO_DIR`). config.json, state.json, caches, usage-history.json.
pub fn bro_dir() -> PathBuf { todo!() }
/// `~/.claude` (override `CLAUDE_CONFIG_DIR` is deliberately NOT honoured) — "claude:local".
pub fn claude_local_dir() -> PathBuf { todo!() }
/// `~/.claude-max-pool` (override `CLAUDE_POOL_DIR`); accounts live in `accounts/<name>`.
pub fn claude_pool_dir() -> PathBuf { todo!() }
/// `~/.codex` — "codex:local".
pub fn codex_local_dir() -> PathBuf { todo!() }
/// `~/.bro/codex-profiles` (override `BRO_CODEX_PROFILES_DIR`).
pub fn codex_profiles_dir() -> PathBuf { todo!() }
