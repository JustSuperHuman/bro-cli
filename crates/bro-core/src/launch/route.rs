//! Proxy routes: stable ids and resolving a [`super::RouteRequest`]'s `upstream` string
//! into concrete connection details for bro-proxy (which can't depend on bro-core's
//! catalogue logic itself).
use crate::config::Config;
use crate::profiles::{self, ProfileKind};
use crate::providers::{self, ProviderMode, normalize_openai_base_url};
use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Short stable id for (upstream, model): `r` + 10 hex chars of SHA-256.
pub fn route_id(upstream: &str, model: Option<&str>) -> String {
    format!("r{}", crate::util::short_hash(&format!("{upstream}\u{0}{}", model.unwrap_or("")), 10))
}

/// Mirrors `bro_proxy::Upstream` one-for-one so the TUI can map it field by field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpstreamTarget {
    /// OpenAI Chat Completions at `{base_url}/chat/completions`
    OpenAiChat { base_url: String, api_key: Option<String> },
    /// OpenAI Responses at `{base_url}/responses`
    OpenAiResponses { base_url: String, api_key: Option<String> },
    /// ChatGPT Codex backend with this login's OAuth tokens
    ChatGptCodex { codex_home: PathBuf },
    /// Anthropic Messages with a key (`bearer` = send as `Authorization: Bearer`, as
    /// Claude Code does with `ANTHROPIC_AUTH_TOKEN`)
    Anthropic { base_url: String, api_key: Option<String>, bearer: bool },
    /// api.anthropic.com with a Claude subscription login
    ClaudeOAuth { config_dir: PathBuf },
    /// The Claude account pool (signed-in pool accounts)
    ClaudePool { config_dirs: Vec<PathBuf> },
}

/// Resolve an upstream string ("pool", "claude:<p>", "codex:<p>" or a provider id; an
/// optional model picks a new-api relay's per-model endpoint).
pub fn resolve_upstream(upstream: &str, model: Option<&str>, cfg: &Config) -> anyhow::Result<UpstreamTarget> {
    if upstream == "pool" {
        let dirs: Vec<PathBuf> = profiles::list_claude()
            .into_iter()
            .filter(|p| p.kind == ProfileKind::ClaudeAccount && p.authenticated)
            .map(|p| p.dir)
            .collect();
        if dirs.is_empty() {
            bail!("the Claude account pool has no signed-in accounts");
        }
        return Ok(UpstreamTarget::ClaudePool { config_dirs: dirs });
    }
    if let Some((kind, name)) = profiles::parse_id(upstream) {
        let dir = profiles::dir_for(kind, name);
        return Ok(match kind {
            ProfileKind::ClaudeLocal | ProfileKind::ClaudeAccount => UpstreamTarget::ClaudeOAuth { config_dir: dir },
            ProfileKind::CodexLocal | ProfileKind::CodexProfile => UpstreamTarget::ChatGptCodex { codex_home: dir },
        });
    }
    let all = providers::load(cfg);
    let p = providers::find(&all, upstream).ok_or_else(|| anyhow!("unknown provider {upstream:?}"))?.for_model(model);
    let key = cfg.key_for(&p.id, p.key_env.as_deref());
    let base = p.base_url.clone().unwrap_or_default();
    Ok(match p.mode {
        ProviderMode::Native if key.is_none() => UpstreamTarget::ClaudeOAuth { config_dir: crate::paths::claude_local_dir() },
        ProviderMode::Native => UpstreamTarget::Anthropic {
            base_url: if base.is_empty() { "https://api.anthropic.com".into() } else { base },
            api_key: key,
            bearer: false,
        },
        ProviderMode::Anthropic => UpstreamTarget::Anthropic { base_url: base, api_key: key, bearer: true },
        ProviderMode::Openai => UpstreamTarget::OpenAiChat { base_url: normalize_openai_base_url(&base), api_key: key },
    })
}
