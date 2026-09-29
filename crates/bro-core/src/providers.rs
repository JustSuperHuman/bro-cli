//! Provider catalogue: bundled `models.json` (copied from bro v1, `include_str!`), remote
//! refresh from `BRO_MODELS_URL` / GitHub raw cached at `~/.bro/models.cache.json`,
//! merged with `Config.providers` by id.
//!
//! JSON uses v1's camelCase field names (`baseUrl`, `keyEnv`, `noKey`,
//! `disable1mContext`…); snake_case spellings are accepted as aliases.
use crate::paths;
use crate::util::{env_str, read_json, strip_hash, write_json_pretty};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

/// The catalogue bundled into the binary (v1's `models.json`).
pub const BUNDLED_MODELS_JSON: &str = include_str!("models.json");
/// Default remote catalogue (override `BRO_MODELS_URL`).
pub const DEFAULT_MODELS_URL: &str = "https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/models.json";

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
    /// Empty for v1's "Default (your Claude login)" row, which has no id.
    #[serde(default)]
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
    #[serde(default)]
    pub name: String,
    pub mode: ProviderMode,
    #[serde(default, rename = "baseUrl", alias = "base_url", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, rename = "responsesBaseUrl", alias = "responses_base_url", skip_serializing_if = "Option::is_none")]
    pub responses_base_url: Option<String>,
    #[serde(default, rename = "keyEnv", alias = "key_env", skip_serializing_if = "Option::is_none")]
    pub key_env: Option<String>,
    #[serde(default, rename = "keyUrl", alias = "key_url", skip_serializing_if = "Option::is_none")]
    pub key_url: Option<String>,
    #[serde(default, rename = "noKey", alias = "no_key")]
    pub no_key: bool,
    #[serde(default, rename = "disable1mContext", alias = "disable_1m_context")]
    pub disable_1m_context: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<Model>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Provider {
    /// Display name, falling back to the id.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() { &self.id } else { &self.name }
    }

    /// A new-api relay (OpenLux, Yunwu): `"catalogue": "newapi"` with a base URL.
    pub fn is_newapi(&self) -> bool {
        self.extra.get("catalogue").and_then(Value::as_str) == Some("newapi") && self.base_url.is_some()
    }

    /// Concrete model ids (rows without an id, like v1's "Default" row, skipped).
    pub fn model_ids(&self) -> Vec<&str> {
        self.models.iter().map(|m| m.id.as_str()).filter(|id| !id.is_empty()).collect()
    }

    /// The Responses-API base URL Codex should use, if the provider serves one
    /// (v1 `codexResponsesBaseUrl`). Empty/None for Anthropic-only providers.
    pub fn responses_base(&self) -> Option<String> {
        if let Some(r) = self.responses_base_url.as_deref().filter(|s| !s.is_empty()) {
            return Some(normalize_openai_base_url(r));
        }
        let base = self.base_url.as_deref().filter(|s| !s.is_empty())?;
        match self.mode {
            ProviderMode::Openai => Some(normalize_openai_base_url(base)),
            _ if self.id == "openrouter" => {
                let b = normalize_openai_base_url(base).trim_end_matches('/').to_string();
                Some(if b.to_lowercase().ends_with("/v1") { b } else { format!("{b}/v1") })
            }
            _ => None,
        }
    }

    /// v1 `providerForModel`: a new-api relay serves Claude models on the Anthropic
    /// endpoint and everything else on OpenAI's, so the provider is reshaped for the
    /// chosen model using the cached live catalogue (`~/.bro/newapi-<id>.cache.json`).
    /// Unknown model / no cache → unchanged.
    pub fn for_model(&self, model: Option<&str>) -> Provider {
        let (Some(model), true) = (model, self.is_newapi()) else { return self.clone() };
        let cache = paths::bro_dir().join(format!("newapi-{}.cache.json", self.id));
        let Some(Value::Array(rows)) = read_json(&cache) else { return self.clone() };
        let Some(row) = rows.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(model)) else {
            return self.clone();
        };
        let endpoints: Vec<&str> =
            row.get("endpoints").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if endpoints.is_empty() {
            return self.clone();
        }
        let base = self.base_url.as_deref().unwrap_or("").trim_end_matches('/').to_string();
        let mut p = self.clone();
        if endpoints.contains(&"anthropic") {
            p.mode = ProviderMode::Anthropic;
            p.base_url = Some(base);
        } else {
            p.mode = ProviderMode::Openai;
            p.base_url = Some(format!("{base}/v1/chat/completions"));
        }
        p
    }
}

/// Strip a trailing `/chat/completions` or `/responses` (v1 `normalizeOpenAiBaseUrl`).
pub fn normalize_openai_base_url(url: &str) -> String {
    let mut s = url.trim().to_string();
    for suffix in ["/chat/completions/", "/chat/completions", "/responses/", "/responses"] {
        if s.to_lowercase().ends_with(suffix) {
            s.truncate(s.len() - suffix.len());
            break;
        }
    }
    s
}

/// v1 `withBundledProviders`: ids the fetched list lacks are added, and a known id
/// gets fields filled in only where the fetched entry says nothing.
fn with_bundled(data: Value, bundled: &Value) -> Value {
    let Some(bundled) = bundled.get("providers").and_then(Value::as_array) else { return data };
    let mut by_id: Vec<(String, &Map<String, Value>)> = bundled
        .iter()
        .filter_map(|p| Some((p.get("id")?.as_str()?.to_string(), p.as_object()?)))
        .collect();
    let mut data = match data {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let mut providers: Vec<Value> = data.get("providers").and_then(Value::as_array).cloned().unwrap_or_default();
    for p in providers.iter_mut() {
        let Some(id) = p.get("id").and_then(Value::as_str).map(str::to_string) else { continue };
        let Some(pos) = by_id.iter().position(|(bid, _)| *bid == id) else { continue };
        let (_, extra) = by_id.remove(pos);
        if let Value::Object(obj) = p {
            for (k, v) in extra {
                if !v.is_null() && obj.get(k).is_none_or(Value::is_null) {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
    }
    providers.extend(by_id.into_iter().map(|(_, m)| Value::Object(m.clone())));
    data.insert("providers".into(), Value::Array(providers));
    Value::Object(data)
}

/// v1 `mergeProviders`: same id → append models and override the listed fields;
/// new id → added.
fn merge_config(mut providers: Vec<Value>, config_providers: &[Value]) -> Vec<Value> {
    const OVERRIDE: [&str; 9] =
        ["baseUrl", "responsesBaseUrl", "mode", "keyEnv", "keyUrl", "noKey", "disable1mContext", "section", "catalogue"];
    for cp in config_providers {
        let Some(id) = cp.get("id").and_then(Value::as_str) else { continue };
        if let Some(existing) = providers.iter_mut().find(|p| p.get("id").and_then(Value::as_str) == Some(id)) {
            let Value::Object(ex) = existing else { continue };
            for f in OVERRIDE {
                if let Some(v) = cp.get(f).filter(|v| !v.is_null()) {
                    ex.insert(f.into(), v.clone());
                }
            }
            if let Some(models) = cp.get("models").and_then(Value::as_array) {
                let list = ex.entry("models").or_insert_with(|| Value::Array(vec![]));
                if !list.is_array() {
                    *list = Value::Array(vec![]);
                }
                if let Value::Array(l) = list {
                    l.extend(models.iter().cloned());
                }
            }
        } else {
            providers.push(cp.clone());
        }
    }
    providers
}

/// Merge a raw catalogue document with the user's providers and parse. Providers
/// that don't parse (unknown mode, missing id) are skipped.
pub fn merge(catalogue: &Value, cfg: &crate::config::Config) -> Vec<Provider> {
    let bundled: Value = serde_json::from_str(BUNDLED_MODELS_JSON).unwrap_or(Value::Null);
    let data = strip_hash(&with_bundled(catalogue.clone(), &bundled));
    let base = data.get("providers").and_then(Value::as_array).cloned().unwrap_or_default();
    let user = strip_hash(&Value::Array(cfg.providers.clone()));
    let merged = merge_config(base, user.as_array().map(Vec::as_slice).unwrap_or(&[]));
    merged.into_iter().filter_map(|p| serde_json::from_value::<Provider>(p).ok()).filter(|p| !p.id.is_empty()).collect()
}

/// `~/.bro/models.cache.json`.
pub fn cache_path() -> std::path::PathBuf {
    paths::bro_dir().join("models.cache.json")
}

/// Bundled + cached + user providers, merged. Never touches the network.
pub fn load(cfg: &crate::config::Config) -> Vec<Provider> {
    let catalogue = read_json(&cache_path())
        .filter(|v| v.get("providers").is_some_and(Value::is_array))
        .or_else(|| serde_json::from_str(BUNDLED_MODELS_JSON).ok())
        .unwrap_or_else(|| serde_json::json!({ "providers": [] }));
    merge(&catalogue, cfg)
}

/// Find a provider by id.
pub fn find<'a>(providers: &'a [Provider], id: &str) -> Option<&'a Provider> {
    providers.iter().find(|p| p.id == id)
}

/// The remote catalogue URL (`BRO_MODELS_URL` or GitHub raw).
pub fn remote_url() -> String {
    env_str("BRO_MODELS_URL").unwrap_or_else(|| DEFAULT_MODELS_URL.to_string())
}

/// Fetch the remote catalogue and update the cache. Blocking; call off the UI thread.
pub fn refresh() -> anyhow::Result<()> {
    let url = remote_url();
    let resp = crate::http::get_json(&url, &[("connection", "close")], Duration::from_secs(6))?;
    if !resp.ok() {
        bail!("HTTP {} from {url}", resp.status);
    }
    if !resp.body.get("providers").is_some_and(Value::is_array) {
        bail!("response was not a models list (no \"providers\" array)");
    }
    write_json_pretty(&cache_path(), &resp.body).context("writing models cache")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::util::test_env::sandbox;
    use serde_json::json;

    #[test]
    fn bundled_parses_with_camel_case_fields() {
        let _sb = sandbox();
        let ps = load(&Config::default());
        let zai = find(&ps, "zai").unwrap();
        assert_eq!(zai.mode, ProviderMode::Anthropic);
        assert_eq!(zai.base_url.as_deref(), Some("https://api.z.ai/api/anthropic"));
        assert_eq!(zai.key_env.as_deref(), Some("ZAI_API_KEY"));
        assert!(zai.disable_1m_context);
        assert!(find(&ps, "ollama").unwrap().no_key);
        assert!(find(&ps, "#example").is_none());
        assert_eq!(find(&ps, "openlux").unwrap().extra["section"], "other");
        // The "Default" row has no id.
        assert_eq!(find(&ps, "anthropic").unwrap().models[0].id, "");
    }

    #[test]
    fn user_providers_merge_by_id() {
        let _sb = sandbox();
        let cfg = Config::from_value(json!({
            "providers": [
                { "id": "openai", "baseUrl": "http://proxy/v1/chat/completions", "models": [ { "id": "gpt-x" } ] },
                { "id": "mine", "name": "Mine", "mode": "openai", "noKey": true, "models": [ { "id": "m1" }, { "#id": "gone" } ] },
                { "id": "#off", "mode": "openai" },
                { "id": "broken", "mode": "martian" }
            ]
        }));
        let ps = load(&cfg);
        let openai = find(&ps, "openai").unwrap();
        assert_eq!(openai.base_url.as_deref(), Some("http://proxy/v1/chat/completions"));
        assert!(openai.model_ids().contains(&"gpt-4o"));
        assert_eq!(openai.model_ids().last(), Some(&"gpt-x"));
        let mine = find(&ps, "mine").unwrap();
        assert!(mine.no_key);
        assert_eq!(mine.model_ids(), vec!["m1"]);
        assert!(find(&ps, "#off").is_none());
        assert!(find(&ps, "broken").is_none());
    }

    #[test]
    fn cache_wins_but_bundled_fills_gaps() {
        let _sb = sandbox();
        let cached = json!({ "providers": [ { "id": "zai", "name": "Z cached", "mode": "anthropic", "models": [] } ] });
        write_json_pretty(&cache_path(), &cached).unwrap();
        let ps = load(&Config::default());
        let zai = find(&ps, "zai").unwrap();
        assert_eq!(zai.name, "Z cached");
        assert_eq!(zai.key_env.as_deref(), Some("ZAI_API_KEY"));
        assert!(find(&ps, "openrouter").is_some());
    }

    #[test]
    fn responses_base_urls() {
        let _sb = sandbox();
        let ps = load(&Config::default());
        assert_eq!(find(&ps, "openai").unwrap().responses_base().as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(find(&ps, "openrouter").unwrap().responses_base().as_deref(), Some("https://openrouter.ai/api/v1"));
        assert_eq!(find(&ps, "zai").unwrap().responses_base(), None);
    }

    #[test]
    fn newapi_for_model() {
        let sb = sandbox();
        std::fs::create_dir_all(sb.bro()).unwrap();
        write_json_pretty(
            &sb.bro().join("newapi-openlux.cache.json"),
            &json!([ { "id": "gpt-6-astra", "endpoints": ["openai"] }, { "id": "claude-opus-5", "endpoints": ["anthropic", "openai"] } ]),
        )
        .unwrap();
        let ps = load(&Config::default());
        let lux = find(&ps, "openlux").unwrap();
        let gpt = lux.for_model(Some("gpt-6-astra"));
        assert_eq!(gpt.mode, ProviderMode::Openai);
        assert_eq!(gpt.base_url.as_deref(), Some("https://api.openlux.ai/v1/chat/completions"));
        assert_eq!(lux.for_model(Some("claude-opus-5")).mode, ProviderMode::Anthropic);
        assert_eq!(lux.for_model(Some("unknown")).mode, ProviderMode::Anthropic);
    }

    #[test]
    #[ignore = "network"]
    fn refresh_from_github() {
        let _sb = sandbox();
        unsafe { std::env::remove_var("BRO_MODELS_URL") };
        refresh().unwrap();
        assert!(!load(&Config::default()).is_empty());
    }
}
