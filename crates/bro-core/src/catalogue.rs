//! Model lists for the launcher: the live OpenRouter catalogue (v1-compatible cache at
//! `~/.bro/openrouter.cache.json`), the ChatGPT models a Codex login can use (Codex's own
//! `models_cache.json`), and every other provider's models from the catalogue.
use crate::paths;
use crate::providers::Provider;
use crate::util::{read_json, write_json_pretty};
use anyhow::bail;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";

/// One pickable model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRow {
    pub id: String,
    pub name: String,
    /// context window in tokens
    pub context: Option<u64>,
    /// USD per million (prompt, completion)
    pub pricing: Option<(f64, f64)>,
    pub reasoning: bool,
    /// unix seconds the model appeared (OpenRouter)
    pub created: Option<i64>,
}

impl ModelRow {
    fn from_value(v: &Value) -> Option<ModelRow> {
        let id = v.get("id")?.as_str()?.to_string();
        if id.is_empty() {
            return None;
        }
        let pricing = v.get("pricing").and_then(|p| Some((p.get("prompt")?.as_f64()?, p.get("completion")?.as_f64()?)));
        Some(ModelRow {
            name: v.get("name").and_then(Value::as_str).unwrap_or(&id).to_string(),
            context: v.get("context").and_then(Value::as_u64),
            pricing,
            reasoning: v.get("reasoning").and_then(Value::as_bool).unwrap_or(false),
            created: v.get("created").and_then(Value::as_i64),
            id,
        })
    }
}

/// `~/.bro/openrouter.cache.json` (shared with bro v1).
pub fn openrouter_cache_path() -> PathBuf {
    paths::bro_dir().join("openrouter.cache.json")
}

/// The cached OpenRouter catalogue (newest first) and its age in seconds.
pub fn openrouter_cached() -> Option<(Vec<ModelRow>, u64)> {
    let path = openrouter_cache_path();
    let Value::Array(rows) = read_json(&path)? else { return None };
    let models: Vec<ModelRow> = rows.iter().filter_map(ModelRow::from_value).collect();
    if models.is_empty() {
        return None;
    }
    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()).unwrap_or(u64::MAX);
    Some((models, age))
}

/// v1 `mapOpenRouterModels`: `/api/v1/models` `data` → cache rows, newest first.
pub fn map_openrouter(data: &Value) -> Vec<Value> {
    let Some(items) = data.as_array() else { return vec![] };
    let mut items: Vec<&Value> = items.iter().filter(|m| m.get("id").and_then(Value::as_str).is_some_and(|s| !s.is_empty())).collect();
    let created = |m: &Value| m.get("created").and_then(Value::as_i64).unwrap_or(0);
    let id = |m: &Value| m.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    items.sort_by(|a, b| created(b).cmp(&created(a)).then_with(|| id(a).cmp(&id(b))));
    items
        .into_iter()
        .map(|m| {
            let mut row = json!({ "id": id(m), "name": m.get("name").and_then(Value::as_str).unwrap_or(&id(m)) });
            if let Some(c) = m.get("created").and_then(Value::as_i64) {
                row["created"] = json!(c);
            }
            if let Some(c) = m.get("context_length").and_then(Value::as_u64) {
                row["context"] = json!(c);
            }
            let per_million = |v: Option<&Value>| -> Option<f64> {
                let n = match v? {
                    Value::String(s) => s.parse::<f64>().ok()?,
                    Value::Number(n) => n.as_f64()?,
                    _ => return None,
                };
                (n.is_finite() && n >= 0.0).then(|| (n * 1e6 * 1e4).round() / 1e4)
            };
            let p = m.get("pricing");
            if let (Some(a), Some(b)) = (per_million(p.and_then(|p| p.get("prompt"))), per_million(p.and_then(|p| p.get("completion")))) {
                row["pricing"] = json!({ "prompt": a, "completion": b });
            }
            let reasoning = m.get("reasoning").and_then(Value::as_bool).unwrap_or(false)
                || m.get("supported_parameters").and_then(Value::as_array).is_some_and(|a| a.iter().any(|s| s == "reasoning"));
            if reasoning {
                row["reasoning"] = json!(true);
            }
            row
        })
        .collect()
}

/// Fetch the live OpenRouter catalogue into the cache. Blocking. Returns the model count.
pub fn refresh_openrouter() -> anyhow::Result<usize> {
    let resp = crate::http::get_json(OPENROUTER_URL, &[], Duration::from_secs(8))?;
    if !resp.ok() {
        bail!("OpenRouter models: HTTP {}", resp.status);
    }
    let rows = map_openrouter(resp.body.get("data").unwrap_or(&Value::Null));
    if rows.is_empty() {
        bail!("OpenRouter returned no models");
    }
    write_json_pretty(&openrouter_cache_path(), &Value::Array(rows.clone()))?;
    Ok(rows.len())
}

/// Models a Codex login can pick: Codex's own `models_cache.json` (listed ones), else a
/// small static list.
pub fn codex_models(codex_home: &Path) -> Vec<ModelRow> {
    let listed: Vec<ModelRow> = read_json(&codex_home.join("models_cache.json"))
        .and_then(|v| v.get("models").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter(|m| m.get("visibility").and_then(Value::as_str).is_none_or(|v| v == "list"))
        .filter_map(|m| {
            let id = m.get("slug").or_else(|| m.get("id")).and_then(Value::as_str)?.to_string();
            let name = m.get("display_name").and_then(Value::as_str).unwrap_or(&id).to_string();
            let context = m.get("context_window").and_then(Value::as_u64);
            Some(ModelRow { id, name, context, pricing: None, reasoning: true, created: None })
        })
        .collect();
    if !listed.is_empty() {
        return listed;
    }
    ["gpt-5.5", "gpt-5.2-codex", "gpt-5.2"]
        .iter()
        .map(|id| ModelRow { id: id.to_string(), name: id.to_string(), context: None, pricing: None, reasoning: true, created: None })
        .collect()
}

/// A provider's models: the live catalogue for OpenRouter (when cached), else models.json.
pub fn provider_models(p: &Provider) -> Vec<ModelRow> {
    if p.id == "openrouter"
        && let Some((rows, _)) = openrouter_cached()
    {
        return rows;
    }
    p.models
        .iter()
        .filter(|m| !m.id.is_empty())
        .map(|m| ModelRow {
            id: m.id.clone(),
            name: m.name.clone().unwrap_or_else(|| m.id.clone()),
            context: m.context,
            pricing: None,
            reasoning: false,
            created: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    #[test]
    fn maps_openrouter_like_v1_and_round_trips_through_the_cache() {
        let _s = sandbox();
        let data = json!([
            {"id": "old/one", "name": "Old", "created": 10, "context_length": 8000, "pricing": {"prompt": "0.000001", "completion": "0.000002"}},
            {"id": "new/two", "created": 20, "pricing": {"prompt": "-1", "completion": "0"}, "supported_parameters": ["tools", "reasoning"]},
            {"id": ""}
        ]);
        let rows = map_openrouter(&data);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "new/two", "newest first");
        assert!(rows[0].get("pricing").is_none(), "-1 is no price");
        assert_eq!(rows[0]["reasoning"], true);
        assert_eq!(rows[1]["pricing"]["prompt"], 1.0);
        write_json_pretty(&openrouter_cache_path(), &Value::Array(rows)).unwrap();
        let (models, age) = openrouter_cached().unwrap();
        assert_eq!(models[1], ModelRow { id: "old/one".into(), name: "Old".into(), context: Some(8000), pricing: Some((1.0, 2.0)), reasoning: false, created: Some(10) });
        assert!(age < 60);
    }

    #[test]
    fn codex_models_from_cache_or_fallback() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!codex_models(dir.path()).is_empty(), "static fallback");
        std::fs::write(
            dir.path().join("models_cache.json"),
            r#"{"models":[{"slug":"gpt-6-astra","display_name":"GPT-6 Astra","visibility":"list"},{"slug":"hidden","visibility":"hide"}]}"#,
        )
        .unwrap();
        let m = codex_models(dir.path());
        assert_eq!(m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gpt-6-astra"]);
        assert_eq!(m[0].name, "GPT-6 Astra");
    }
}
