//! OpenAI Responses API shapes. Input/output items are kept as JSON values
//! (the item schema is large and open-ended); helpers normalize them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponsesRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// String or array of items
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ResponsesRequest {
    pub fn effort(&self) -> Option<String> {
        self.reasoning
            .as_ref()
            .and_then(|r| r.get("effort"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Input as a list of normalized items (string input → one user message;
    /// type-less `{role, content}` → message item; string content → one text part).
    pub fn input_items(&self) -> Vec<Value> {
        match &self.input {
            Value::String(s) => vec![json!({
                "type": "message", "role": "user",
                "content": [{"type": "input_text", "text": s}]
            })],
            Value::Array(items) => items.iter().map(normalize_item).collect(),
            _ => vec![],
        }
    }
}

pub fn normalize_item(item: &Value) -> Value {
    let mut item = item.clone();
    let Some(obj) = item.as_object_mut() else {
        return item;
    };
    if !obj.contains_key("type") && obj.contains_key("role") {
        obj.insert("type".into(), json!("message"));
    }
    if obj.get("type").and_then(Value::as_str) == Some("message")
        && let Some(Value::String(s)) = obj.get("content").cloned()
    {
        let kind = if obj.get("role").and_then(Value::as_str) == Some("assistant") {
            "output_text"
        } else {
            "input_text"
        };
        obj.insert("content".into(), json!([{"type": kind, "text": s}]));
    }
    item
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ResponsesUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens_details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens_details: Option<Value>,
}

impl ResponsesUsage {
    pub fn cached_tokens(&self) -> u64 {
        self.input_tokens_details
            .as_ref()
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }
    pub fn reasoning_tokens(&self) -> u64 {
        self.output_tokens_details
            .as_ref()
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }
}

/// Text of a message item's content parts (output_text / input_text / refusal).
pub fn item_text(item: &Value) -> String {
    item.get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| {
                    p.get("text")
                        .or_else(|| p.get("refusal"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

pub fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}
