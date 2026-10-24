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
            if let Some(o) = m.pointer("/top_provider/max_completion_tokens").and_then(Value::as_u64) {
                row["maxOutput"] = json!(o);
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

/// How big a model is, in tokens, for a harness that has to be told (Claude Code and Codex
/// assume a default window for a model they don't know, and warn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    pub context: Option<u64>,
    pub max_output: Option<u64>,
}

impl From<&crate::providers::Model> for Limits {
    fn from(m: &crate::providers::Model) -> Limits {
        Limits { context: m.context, max_output: m.extra.get("maxOutput").and_then(Value::as_u64) }
    }
}

/// Model sizes bro knows without asking anyone: the cached OpenRouter catalogue
/// (refreshed in the background, see [`refresh_openrouter`]).
pub struct KnownLimits(Vec<(String, Limits)>);

impl KnownLimits {
    /// Reads the cache; none (or a broken one) knows nothing.
    pub fn load() -> KnownLimits {
        let Some(Value::Array(rows)) = read_json(&openrouter_cache_path()) else { return KnownLimits(vec![]) };
        KnownLimits(
            rows.iter()
                .filter_map(|r| {
                    let limits = Limits { context: r.get("context").and_then(Value::as_u64), max_output: r.get("maxOutput").and_then(Value::as_u64) };
                    Some((r.get("id")?.as_str()?.to_lowercase(), limits))
                })
                .collect(),
        )
    }

    /// Limits for `model`: what `listed` (the provider's own catalogue, the login's model list) says,
    /// each gap filled from OpenRouter. OpenRouter matches the exact id (`z-ai/glm-5.3`), else the
    /// id without vendor, variant or thinking level (`glm-5.3`, `gemini-4-argon-medium`); when that
    /// matches several models the smallest wins, since too small a window only compacts early
    /// and too big a one overflows the backend. Implausible numbers are dropped, so the harness
    /// keeps its own default.
    pub fn get(&self, model: &str, listed: Option<Limits>) -> Limits {
        let want = model.to_lowercase();
        let mut hits: Vec<Limits> = self.0.iter().filter(|(id, _)| *id == want).map(|(_, l)| *l).collect();
        if hits.is_empty() {
            let short = bare_model_id(&want);
            hits = self.0.iter().filter(|(id, _)| bare_model_id(id) == short).map(|(_, l)| *l).collect();
        }
        let smallest = |f: fn(&Limits) -> Option<u64>| hits.iter().filter_map(f).min();
        let listed = listed.unwrap_or_default();
        const MAX: u64 = 20_000_000;
        let context = listed.context.or_else(|| smallest(|l| l.context)).filter(|c| (4_096..=MAX).contains(c));
        let max_output = listed.max_output.or_else(|| smallest(|l| l.max_output)).filter(|o| (256..=context.unwrap_or(MAX)).contains(o));
        Limits { context, max_output }
    }
}

/// `google/gemini-4-argon-high:free` → `gemini-4-argon`: what's left to match across catalogues.
fn bare_model_id(id: &str) -> &str {
    let id = id.rsplit('/').next().unwrap_or(id);
    let id = id.split(':').next().unwrap_or(id);
    ["-high", "-medium", "-low"].into_iter().find_map(|s| id.strip_suffix(s)).unwrap_or(id)
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
    fn limits_come_from_the_provider_then_openrouter_smallest_first() {
        let _sb = sandbox();
        assert_eq!(KnownLimits::load().get("glm-5.3", None), Limits::default());
        let data = json!([
            { "id": "z-ai/glm-5.3", "context_length": 202752, "top_provider": { "max_completion_tokens": 131072 } },
            { "id": "acme/glm-5.3:free", "context_length": 131072 },
            { "id": "google/gemini-4-argon", "context_length": 1048576, "top_provider": { "max_completion_tokens": 65536 } },
            { "id": "acme/huge", "context_length": 999999999, "top_provider": { "max_completion_tokens": 999999999 } },
        ]);
        write_json_pretty(&openrouter_cache_path(), &Value::Array(map_openrouter(&data))).unwrap();
        let known = KnownLimits::load();
        assert_eq!(known.get("z-ai/glm-5.3", None), Limits { context: Some(202752), max_output: Some(131072) });
        assert_eq!(known.get("glm-5.3", None), Limits { context: Some(131072), max_output: Some(131072) });
        assert_eq!(known.get("gemini-4-argon-medium", None), Limits { context: Some(1048576), max_output: Some(65536) });
        assert_eq!(known.get("acme/huge", None), Limits::default());
        assert_eq!(known.get("never-heard-of-it", None), Limits::default());
        // the provider's own numbers win; OpenRouter only fills gaps
        let listed = Limits { context: Some(500000), max_output: None };
        assert_eq!(known.get("gemini-4-argon", Some(listed)), Limits { context: Some(500000), max_output: Some(65536) });
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
