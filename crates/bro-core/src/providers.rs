//! Provider catalogue: bundled `models.json` (copied from bro v1, `include_str!`), remote
//! refresh from `BRO_MODELS_URL` / GitHub raw cached at `~/.bro/models.cache.json`,
//! merged with `Config.providers` by id.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderMode {
    /// First-party Anthropic (Claude subscription / API key)
    Native,
    /// Speaks Anthropic Messages natively (OpenRouter, z.ai, …)
    Anthropic,
    /// Speaks OpenAI Chat Completions (and maybe Responses)
    Openai,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub context: Option<u64>,
    /// Anything else from models.json (pricing, ratings, tiers…)
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub mode: ProviderMode,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub responses_base_url: Option<String>,
    #[serde(default)]
    pub key_env: Option<String>,
    #[serde(default)]
    pub key_url: Option<String>,
    #[serde(default)]
    pub no_key: bool,
    #[serde(default)]
    pub disable_1m_context: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<Model>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Bundled + cached + user providers, merged. Never touches the network.
pub fn load(cfg: &crate::config::Config) -> Vec<Provider> { todo!() }
/// Fetch the remote catalogue and update the cache. Blocking; call off the UI thread.
pub fn refresh() -> anyhow::Result<()> { todo!() }
