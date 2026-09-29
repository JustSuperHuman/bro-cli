//! Claude subscription (OAuth) upstream: api.anthropic.com with a Claude Code login.

use super::{
    UpstreamCall, UpstreamResponse, anthropic_headers, anthropic_url, execute, finish, method_for,
    set,
};
use crate::errors::ProxyError;
use crate::state::AppState;
use http::HeaderMap;
use std::path::Path;

pub(crate) const OAUTH_BETA: &str = "oauth-2025-04-20";
const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
const CLAUDE_CLI_UA: &str = "claude-cli/2.0.0 (external, cli)";

/// Comma-merge beta flags, keeping order and dropping duplicates.
pub(crate) fn merge_betas(existing: Option<&str>, add: &[&str]) -> String {
    let mut out: Vec<String> = existing
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for a in add {
        if !out.iter().any(|x| x == a) {
            out.push(a.to_string());
        }
    }
    out.join(",")
}

pub(crate) fn oauth_headers(call: &UpstreamCall, token: &str) -> HeaderMap {
    let mut h = anthropic_headers(call);
    h.remove("x-api-key");
    set(&mut h, "authorization", &format!("Bearer {token}"));
    let mut add = vec![OAUTH_BETA];
    if call.translated {
        add.insert(0, CLAUDE_CODE_BETA);
        set(&mut h, "user-agent", CLAUDE_CLI_UA);
        set(&mut h, "x-app", "cli");
    }
    let existing = h
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    set(
        &mut h,
        "anthropic-beta",
        &merge_betas(existing.as_deref(), &add),
    );
    h
}

/// One attempt (with the single forced-refresh retry on 401) against one login.
/// Returns the raw response so the pool can classify rate limits itself.
pub(crate) async fn attempt(
    state: &AppState,
    config_dir: &Path,
    call: &UpstreamCall,
) -> Result<reqwest::Response, ProxyError> {
    let url = anthropic_url(&state.opts.anthropic_base_url, call.endpoint);
    let mut force = false;
    loop {
        let token = state.claude_token(config_dir, force).await?;
        let resp = execute(
            state,
            method_for(call.endpoint),
            &url,
            oauth_headers(call, &token),
            call.body.clone(),
        )
        .await?;
        if resp.status() == 401 && !force {
            tracing::info!(dir = %config_dir.display(), "Claude OAuth 401; refreshing token and retrying");
            force = true;
            continue;
        }
        return Ok(resp);
    }
}

pub(crate) async fn send(
    state: &AppState,
    config_dir: &Path,
    call: &UpstreamCall,
    account: Option<String>,
) -> Result<UpstreamResponse, ProxyError> {
    let resp = attempt(state, config_dir, call).await?;
    finish(resp, account).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_merge() {
        assert_eq!(
            merge_betas(Some("a, oauth-2025-04-20"), &[OAUTH_BETA]),
            "a,oauth-2025-04-20"
        );
        assert_eq!(merge_betas(None, &["x", "y"]), "x,y");
    }
}
