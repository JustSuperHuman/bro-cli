//! The model catalog behind the orchestrator's model picker.
//!
//! OpenRouter's `/models` is public and rich (pricing, context, supported
//! parameters); a custom OpenAI-compatible endpoint usually only lists ids.
//! Both are normalised to `CatalogModel`, and `recommended` picks the strong
//! coding models from whatever the catalog currently contains rather than
//! pinning ids that churn every few weeks.

use super::llm;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub description: String,
    pub context_length: u64,
    /// USD per million prompt tokens (0 when unknown).
    pub prompt_price: f64,
    /// USD per million completion tokens (0 when unknown).
    pub completion_price: f64,
    pub tools: bool,
    pub reasoning: bool,
    pub created: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub fetched_at: String,
    pub source: String,
    pub models: Vec<CatalogModel>,
}

fn price_per_million(value: Option<&Value>) -> f64 {
    let per_token = match value {
        Some(Value::String(text)) => text.parse::<f64>().unwrap_or(0.0),
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        _ => 0.0,
    };
    (per_token * 1_000_000.0 * 1000.0).round() / 1000.0
}

fn provider_of(id: &str) -> String {
    id.split('/').next().unwrap_or(id).to_string()
}

fn parse_model(value: &Value, openrouter: bool) -> Option<CatalogModel> {
    let id = value.get("id")?.as_str()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let supported: Vec<&str> = value
        .get("supported_parameters")
        .and_then(Value::as_array)
        .map(|parameters| parameters.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let outputs_text = value
        .pointer("/architecture/output_modalities")
        .and_then(Value::as_array)
        .map(|modalities| modalities.iter().any(|modality| modality == "text"))
        .unwrap_or(true);
    if !outputs_text {
        return None;
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&id)
        .to_string();
    Some(CatalogModel {
        provider: provider_of(&id),
        name,
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(|text| text.chars().take(280).collect())
            .unwrap_or_default(),
        context_length: value
            .get("context_length")
            .and_then(Value::as_u64)
            .or_else(|| {
                value
                    .pointer("/top_provider/context_length")
                    .and_then(Value::as_u64)
            })
            .unwrap_or(0),
        prompt_price: price_per_million(value.pointer("/pricing/prompt")),
        completion_price: price_per_million(value.pointer("/pricing/completion")),
        // A custom endpoint says nothing about tools; assume yes so the
        // picker does not hide everything it serves.
        tools: if openrouter {
            supported.contains(&"tools")
        } else {
            true
        },
        reasoning: supported.contains(&"reasoning") || supported.contains(&"include_reasoning"),
        created: value.get("created").and_then(Value::as_i64).unwrap_or(0),
        id,
    })
}

pub async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    openrouter: bool,
) -> Result<Catalog, String> {
    // OpenRouter's catalog is public; only send the key to a custom host,
    // which usually requires it.
    let key = if openrouter { None } else { api_key };
    let value = llm::get_json(client, base_url, "models", key, openrouter).await?;
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.get("models").and_then(Value::as_array).cloned())
        .or_else(|| value.as_array().cloned())
        .ok_or_else(|| "the models endpoint returned no list".to_string())?;
    let mut models: Vec<CatalogModel> = entries
        .iter()
        .filter_map(|entry| parse_model(entry, openrouter))
        .collect();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    Ok(Catalog {
        fetched_at: crate::model::iso_now(),
        source: if openrouter { "openrouter" } else { "custom" }.into(),
        models,
    })
}

/// Coding-strong families, strongest first. `prefer` bumps the newest
/// member whose id contains that word (e.g. the `pro`/`coder` tier) and
/// `avoid` drops variants that are not general coding models.
struct Family {
    prefix: &'static str,
    weight: u32,
    prefer: &'static [&'static str],
    avoid: &'static [&'static str],
    /// How many members of this family make the shortlist.
    take: usize,
}

const FAMILIES: &[Family] = &[
    Family {
        prefix: "anthropic/claude-sonnet",
        weight: 100,
        prefer: &[],
        avoid: &[],
        take: 1,
    },
    Family {
        prefix: "anthropic/claude-opus",
        weight: 96,
        prefer: &[],
        avoid: &[],
        take: 1,
    },
    Family {
        prefix: "anthropic/claude-fable",
        weight: 94,
        prefer: &[],
        avoid: &[],
        take: 1,
    },
    Family {
        prefix: "openai/gpt-6",
        weight: 92,
        prefer: &[],
        avoid: &["mini", "nano"],
        take: 1,
    },
    Family {
        prefix: "openai/gpt-5",
        weight: 90,
        prefer: &["codex"],
        avoid: &["mini", "nano", "chat"],
        take: 2,
    },
    Family {
        prefix: "google/gemini",
        weight: 84,
        prefer: &["pro"],
        avoid: &[
            "lite",
            "image",
            "audio",
            "tts",
            "live",
            "embedding",
            "customtools",
            "exp",
        ],
        take: 2,
    },
    Family {
        prefix: "x-ai/grok",
        weight: 80,
        prefer: &["build", "code"],
        avoid: &["vision", "image", "mini"],
        take: 2,
    },
    Family {
        prefix: "deepseek/deepseek",
        weight: 78,
        prefer: &["pro", "coder"],
        avoid: &["vision", "exp", "prover"],
        take: 2,
    },
    Family {
        prefix: "qwen/qwen",
        weight: 74,
        prefer: &["coder", "max"],
        avoid: &["vl", "image", "flash", "audio", "omni"],
        take: 2,
    },
    Family {
        prefix: "moonshotai/kimi",
        weight: 72,
        prefer: &["code"],
        avoid: &["vl", "vision"],
        take: 2,
    },
    Family {
        prefix: "z-ai/glm",
        weight: 70,
        prefer: &[],
        avoid: &["flash", "vision"],
        take: 1,
    },
    Family {
        prefix: "minimax/minimax",
        weight: 66,
        prefer: &[],
        avoid: &["vision"],
        take: 1,
    },
    Family {
        prefix: "mistralai/devstral",
        weight: 64,
        prefer: &[],
        avoid: &[],
        take: 1,
    },
    Family {
        prefix: "mistralai/codestral",
        weight: 62,
        prefer: &[],
        avoid: &["embed", "mamba"],
        take: 1,
    },
    Family {
        prefix: "mistralai/mistral-medium",
        weight: 58,
        prefer: &[],
        avoid: &[],
        take: 1,
    },
    Family {
        prefix: "mistralai/mistral-large",
        weight: 57,
        prefer: &[],
        avoid: &[],
        take: 1,
    },
];

fn is_variant_suffix(id: &str) -> bool {
    // ":batch" is asynchronous, ":free" is heavily rate limited, and the
    // remaining suffixes are routing variants of a model already listed.
    id.contains(':')
}

fn shortlist_eligible(model: &CatalogModel) -> bool {
    model.tools && !is_variant_suffix(&model.id)
}

/// The curated shortlist: the newest strong coding model per family (plus a
/// preferred tier where one exists), the configured model always first.
pub fn recommended(models: &[CatalogModel], current: &str) -> Vec<CatalogModel> {
    let mut picks: Vec<(u32, i64, CatalogModel)> = Vec::new();
    for family in FAMILIES {
        let mut members: Vec<&CatalogModel> = models
            .iter()
            .filter(|model| {
                shortlist_eligible(model)
                    && model.id.starts_with(family.prefix)
                    && !family.avoid.iter().any(|word| {
                        model.id[family.prefix.len()..]
                            .split(['-', '.', '/', ':'])
                            .any(|part| part == *word)
                    })
            })
            .collect();
        members.sort_by_key(|model| std::cmp::Reverse(model.created));
        let mut chosen: Vec<&CatalogModel> = Vec::new();
        for word in family.prefer {
            if let Some(preferred) = members.iter().find(|model| model.id.contains(word))
                && !chosen.iter().any(|model| model.id == preferred.id)
            {
                chosen.push(preferred);
            }
        }
        for member in &members {
            if chosen.len() >= family.take {
                break;
            }
            if !chosen.iter().any(|model| model.id == member.id) {
                chosen.push(member);
            }
        }
        for (position, model) in chosen.into_iter().enumerate() {
            picks.push((
                family.weight - position as u32,
                model.created,
                model.clone(),
            ));
        }
    }
    picks.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
    let mut result: Vec<CatalogModel> = Vec::new();
    if let Some(configured) = models.iter().find(|model| model.id == current) {
        result.push(configured.clone());
    }
    for (_, _, model) in picks {
        if !result.iter().any(|existing| existing.id == model.id) {
            result.push(model);
        }
    }
    if result.is_empty() {
        // A custom endpoint with unfamiliar ids: show what it serves.
        result = models.iter().take(24).cloned().collect();
    }
    result.truncate(18);
    result
}

pub fn to_value(catalog: Option<&Catalog>, current: &str, error: Option<&str>) -> Value {
    match catalog {
        Some(catalog) => json!({
            "fetchedAt": catalog.fetched_at,
            "source": catalog.source,
            "recommended": recommended(&catalog.models, current),
            "models": catalog.models,
            "error": error
        }),
        None => json!({
            "fetchedAt": Value::Null,
            "source": Value::Null,
            "recommended": [],
            "models": [],
            "error": error
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, created: i64, tools: bool) -> CatalogModel {
        CatalogModel {
            id: id.into(),
            name: id.into(),
            provider: provider_of(id),
            description: String::new(),
            context_length: 200_000,
            prompt_price: 1.0,
            completion_price: 5.0,
            tools,
            reasoning: true,
            created,
        }
    }

    #[test]
    fn shortlist_prefers_newest_tool_capable_member_per_family() {
        let models = vec![
            model("anthropic/claude-sonnet-4.5", 10, true),
            model("anthropic/claude-sonnet-5", 20, true),
            model("anthropic/claude-sonnet-5:batch", 20, true),
            model("google/gemini-3.8-flash", 30, true),
            model("google/gemini-3-pro", 5, true),
            model("google/gemini-3.5-flash-lite", 40, true),
            model("openai/gpt-5.6-terra", 25, true),
            model("openai/gpt-5-mini", 26, true),
            model("some/other-model", 99, false),
        ];
        let picks = recommended(&models, "anthropic/claude-sonnet-5");
        let ids: Vec<&str> = picks.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids[0], "anthropic/claude-sonnet-5");
        assert!(ids.contains(&"openai/gpt-5.6-terra"));
        assert!(ids.contains(&"google/gemini-3-pro"));
        assert!(ids.contains(&"google/gemini-3.8-flash"));
        assert!(!ids.contains(&"anthropic/claude-sonnet-4.5"));
        assert!(!ids.contains(&"anthropic/claude-sonnet-5:batch"));
        assert!(!ids.contains(&"google/gemini-3.5-flash-lite"));
        assert!(!ids.contains(&"openai/gpt-5-mini"));
        assert!(!ids.contains(&"some/other-model"));
    }

    #[test]
    fn openrouter_pricing_is_normalised_per_million_tokens() {
        let parsed = parse_model(
            &json!({
                "id": "anthropic/claude-sonnet-5",
                "name": "Anthropic: Claude Sonnet 5",
                "context_length": 1000000,
                "pricing": { "prompt": "0.000002", "completion": "0.00001" },
                "supported_parameters": ["tools", "reasoning"],
                "architecture": { "output_modalities": ["text"] },
                "created": 1782843083
            }),
            true,
        )
        .unwrap();
        assert_eq!(parsed.prompt_price, 2.0);
        assert_eq!(parsed.completion_price, 10.0);
        assert!(parsed.tools && parsed.reasoning);
        assert_eq!(parsed.provider, "anthropic");
    }
}
