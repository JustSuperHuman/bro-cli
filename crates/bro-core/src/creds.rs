//! OAuth credentials for Claude and Codex logins, with refresh + write-back.
//! Used by usage fetching AND by bro-proxy (upstream auth), so it must be
//! thread-safe: refreshes for the same dir are serialized by an internal lock.
use std::path::Path;

/// Returns a valid Claude OAuth access token for `config_dir`
/// (`<dir>/.credentials.json` → `claudeAiOauth`), refreshing via
/// `POST https://platform.claude.com/v1/oauth/token` when expired or `force_refresh`.
pub fn claude_access_token(config_dir: &Path, force_refresh: bool) -> anyhow::Result<String> { todo!() }

#[derive(Debug, Clone)]
pub struct CodexAuth {
    pub access_token: String,
    pub account_id: Option<String>,
    /// Present when the login is an API-key login rather than ChatGPT OAuth
    pub api_key: Option<String>,
}

/// Reads `<home>/auth.json` (ChatGPT tokens or OPENAI_API_KEY), refreshing tokens via
/// `https://auth.openai.com/oauth/token` when expired or `force_refresh`.
pub fn codex_auth(codex_home: &Path, force_refresh: bool) -> anyhow::Result<CodexAuth> { todo!() }
