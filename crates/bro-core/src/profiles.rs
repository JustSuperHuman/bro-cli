//! A profile is one isolated login directory (v1 semantics, no schema file).
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// `~/.claude`
    ClaudeLocal,
    /// `~/.claude-max-pool/accounts/<name>` (a CLAUDE_CONFIG_DIR)
    ClaudeAccount,
    /// `~/.codex`
    CodexLocal,
    /// `~/.bro/codex-profiles/<name>` (a CODEX_HOME)
    CodexProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Stable id: "claude:local", "claude:<name>", "codex:local", "codex:<name>"
    pub id: String,
    pub kind: ProfileKind,
    pub name: String,
    pub dir: std::path::PathBuf,
    pub authenticated: bool,
    /// e.g. "max", "pro", "plus" (Claude subscriptionType / Codex chatgpt_plan_type)
    pub plan: Option<String>,
    /// Claude rateLimitTier, if any
    pub tier: Option<String>,
    /// Dedup identity: Claude `accountUuid:organizationUuid`, Codex account user id
    pub identity: Option<String>,
    pub email: Option<String>,
}

impl Profile {
    pub fn is_claude(&self) -> bool { matches!(self.kind, ProfileKind::ClaudeLocal | ProfileKind::ClaudeAccount) }
    pub fn is_codex(&self) -> bool { matches!(self.kind, ProfileKind::CodexLocal | ProfileKind::CodexProfile) }
}

/// All profiles, local first then alphabetical. Cheap (reads small JSON files).
pub fn list() -> Vec<Profile> { todo!() }
pub fn get(id: &str) -> Option<Profile> { todo!() }
/// Create an empty Claude account dir / seeded Codex profile (v1 `seedProfile`).
pub fn create(kind: ProfileKind, name: &str) -> anyhow::Result<Profile> { todo!() }
pub fn remove(id: &str) -> anyhow::Result<()> { todo!() }
/// `^[A-Za-z0-9._-]+$`
pub fn valid_name(name: &str) -> bool { todo!() }
/// Command that performs an interactive login for this profile inside a PTY
/// (claude with CLAUDE_CONFIG_DIR then `/login`; `codex login` with CODEX_HOME).
pub fn login_command(profile: &Profile) -> crate::launch::CommandSpec { todo!() }

#[allow(unused_imports)]
use PathBuf as _;
