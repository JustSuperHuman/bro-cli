//! Model mapping (client model → upstream model) and `/v1/models` listings.

use crate::state::AppState;
use crate::translate::EFFORT_ORDER;
use crate::upstream::chatgpt;
use crate::util::now_secs;
use crate::{Dialect, Route, Upstream};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Short Claude aliases the CLI accepts but the API doesn't (v1 pool table).
const CLAUDE_ALIASES: &[(&str, &str)] = &[
    ("opus", "claude-opus-4-8"),
    ("sonnet", "claude-sonnet-5"),
    ("haiku", "claude-haiku-4-5-20251001"),
];

pub(crate) fn is_claude_name(m: &str) -> bool {
    let m = m.to_lowercase();
    m.contains("claude")
        || m.starts_with("opus")
        || m.starts_with("sonnet")
        || m.starts_with("haiku")
}

pub(crate) fn is_openai_name(m: &str) -> bool {
    let m = m.to_lowercase();
    let m = m.rsplit('/').next().unwrap_or(&m).to_string();
    m.starts_with("gpt-")
        || m.starts_with("chatgpt")
        || m.starts_with("codex")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
}

pub(crate) fn is_small_name(m: &str) -> bool {
    let m = m.to_lowercase();
    ["haiku", "mini", "small", "nano", "spark", "flash-lite"]
        .iter()
        .any(|k| m.contains(k))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Resolved {
    pub model: String,
    /// From a `model:effort` suffix
    pub effort: Option<String>,
    /// Efforts the upstream model accepts (ChatGPT model list), if known
    pub efforts: Vec<String>,
}

/// Pure mapping rules:
/// 1. `route.model_map` exact match;
/// 2. a foreign-family alias (claude-* to an OpenAI upstream, gpt-* to Anthropic)
///    becomes `small_model` for haiku/mini/small/nano names, else `default_model`;
/// 3. otherwise pass through (bare opus/sonnet/haiku expand for Anthropic upstreams).
///
/// A `:effort` suffix (`gpt-5.5:high`, v1 convention) is split off for OpenAI upstreams.
pub(crate) fn map_model(route: &Route, client_model: &str) -> Resolved {
    if let Some((_, to)) = route
        .model_map
        .iter()
        .find(|(from, _)| from == client_model)
    {
        return split_effort(to, route.upstream.dialect());
    }
    let dialect = route.upstream.dialect();
    let Resolved { model, effort, .. } = split_effort(client_model, dialect);
    if let Some((_, to)) = route.model_map.iter().find(|(from, _)| *from == model) {
        return Resolved {
            model: to.clone(),
            effort,
            efforts: vec![],
        };
    }
    let foreign = match dialect {
        Dialect::Anthropic => is_openai_name(&model),
        _ => is_claude_name(&model),
    };
    let pick = |small: bool| -> Option<String> {
        if small {
            route
                .small_model
                .clone()
                .or_else(|| route.default_model.clone())
        } else {
            route.default_model.clone()
        }
    };
    let mapped = if model.is_empty() {
        pick(false)
    } else if foreign {
        pick(is_small_name(&model))
    } else {
        None
    };
    let model = mapped.unwrap_or_else(|| {
        if dialect == Dialect::Anthropic
            && let Some((_, full)) = CLAUDE_ALIASES.iter().find(|(a, _)| *a == model)
        {
            return full.to_string();
        }
        model
    });
    // A mapped target may itself carry an effort suffix.
    let inner = split_effort(&model, dialect);
    Resolved {
        model: inner.model,
        effort: inner.effort.or(effort),
        efforts: vec![],
    }
}

fn split_effort(model: &str, dialect: Dialect) -> Resolved {
    if dialect != Dialect::Anthropic
        && let Some((slug, suffix)) = model.rsplit_once(':')
        && EFFORT_ORDER.contains(&suffix)
    {
        return Resolved {
            model: slug.to_string(),
            effort: Some(suffix.to_string()),
            efforts: vec![],
        };
    }
    Resolved {
        model: model.to_string(),
        effort: None,
        efforts: vec![],
    }
}

/// [`map_model`] plus ChatGPT specifics: unmapped foreign aliases fall back to the
/// subscription's model list (first model / a mini one), and supported efforts.
pub(crate) async fn resolve(state: &AppState, route: &Route, client_model: &str) -> Resolved {
    let mut r = map_model(route, client_model);
    if let Upstream::ChatGptCodex { codex_home } = &route.upstream {
        let list = chatgpt::models(state, codex_home).await;
        if !list.iter().any(|m| m.id == r.model) && (is_claude_name(&r.model) || r.model.is_empty())
        {
            let small = is_small_name(&r.model);
            let fallback = if small {
                list.iter().find(|m| is_small_name(&m.id)).or(list.first())
            } else {
                list.first()
            };
            if let Some(m) = fallback {
                r.model = m.id.clone();
            }
        }
        if let Some(info) = list.iter().find(|m| m.id == r.model) {
            r.efforts = info.efforts.clone();
        }
    }
    r
}

/// Model ids to advertise for a route.
pub(crate) async fn list(state: &AppState, route: &Route) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |id: &str, name: &str| {
        if !id.is_empty() && !out.iter().any(|(i, _)| i == id) {
            out.push((id.to_string(), name.to_string()));
        }
    };
    for m in route.default_model.iter().chain(route.small_model.iter()) {
        add(m, m);
    }
    for (from, to) in &route.model_map {
        add(from, from);
        add(to, to);
    }
    match &route.upstream {
        Upstream::ChatGptCodex { codex_home } => {
            for m in chatgpt::models(state, codex_home).await {
                add(&m.id, &m.name);
            }
        }
        _ => {
            for id in upstream_models(state, route).await {
                add(&id, &id);
            }
        }
    }
    out
}

/// Best-effort live list from the upstream (cached 5 min; failures cached too).
async fn upstream_models(state: &AppState, route: &Route) -> Vec<String> {
    let key = route.id.clone();
    if let Some((at, v)) = state.upstream_models.lock().get(&key)
        && at.elapsed() < Duration::from_secs(300)
    {
        return v.clone();
    }
    let call = crate::upstream::UpstreamCall::get_models();
    let fetched = tokio::time::timeout(Duration::from_secs(10), async {
        let resp = crate::upstream::send(state, route, &call).await.ok()?;
        let body = crate::upstream::read_all(resp.body).await.ok()?;
        let v: Value = serde_json::from_slice(&body).ok()?;
        let ids: Vec<String> = v
            .get("data")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        Some(ids)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    state
        .upstream_models
        .lock()
        .insert(key, (Instant::now(), fetched.clone()));
    fetched
}

/// One listing that satisfies both Anthropic (`type`, `display_name`, `has_more`)
/// and OpenAI (`object`, `owned_by`) clients.
pub(crate) fn listing_json(models: &[(String, String)], owner: &str) -> Value {
    let created = now_secs();
    let data: Vec<Value> = models
        .iter()
        .map(|(id, name)| {
            json!({
                "id": id,
                "type": "model",
                "object": "model",
                "display_name": name,
                "created_at": "2025-01-01T00:00:00Z",
                "created": created,
                "owned_by": owner,
            })
        })
        .collect();
    json!({
        "object": "list",
        "data": data,
        "has_more": false,
        "first_id": models.first().map(|m| m.0.clone()),
        "last_id": models.last().map(|m| m.0.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(upstream: Upstream) -> Route {
        Route {
            id: "r".into(),
            upstream,
            default_model: Some("gpt-5.5".into()),
            small_model: Some("gpt-5.4-mini".into()),
            model_map: vec![("claude-opus-4-5".into(), "gpt-5.6-sol:xhigh".into())],
            label: "t".into(),
        }
    }

    #[test]
    fn mapping_rules() {
        let r = route(Upstream::OpenAiChat {
            base_url: "x".into(),
            api_key: None,
        });
        assert_eq!(
            map_model(&r, "claude-opus-4-5"),
            Resolved {
                model: "gpt-5.6-sol".into(),
                effort: Some("xhigh".into()),
                efforts: vec![]
            }
        );
        assert_eq!(map_model(&r, "claude-sonnet-4-5-20250929").model, "gpt-5.5");
        assert_eq!(map_model(&r, "claude-haiku-4-5").model, "gpt-5.4-mini");
        assert_eq!(map_model(&r, "deepseek-chat").model, "deepseek-chat");
        assert_eq!(
            map_model(&r, "gpt-5.5:low"),
            Resolved {
                model: "gpt-5.5".into(),
                effort: Some("low".into()),
                efforts: vec![]
            }
        );
        assert_eq!(map_model(&r, "").model, "gpt-5.5");

        let mut a = route(Upstream::Anthropic {
            base_url: "x".into(),
            api_key: None,
            bearer: false,
        });
        a.default_model = Some("claude-sonnet-4-5".into());
        a.small_model = Some("claude-haiku-4-5".into());
        a.model_map.clear();
        assert_eq!(map_model(&a, "gpt-5").model, "claude-sonnet-4-5");
        assert_eq!(map_model(&a, "gpt-5-mini").model, "claude-haiku-4-5");
        assert_eq!(map_model(&a, "claude-opus-4-5").model, "claude-opus-4-5");
        assert_eq!(map_model(&a, "opus").model, "claude-opus-4-8");
        assert_eq!(map_model(&a, "claude-x:high").model, "claude-x:high");
        a.default_model = None;
        a.small_model = None;
        assert_eq!(map_model(&a, "gpt-5").model, "gpt-5");
    }

    #[test]
    fn listing_shape() {
        let v = listing_json(&[("a".into(), "A".into())], "bro");
        assert_eq!(v["data"][0]["type"], "model");
        assert_eq!(v["data"][0]["object"], "model");
        assert_eq!(v["first_id"], "a");
        assert_eq!(v["has_more"], false);
    }
}
