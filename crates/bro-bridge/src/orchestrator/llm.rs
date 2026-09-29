//! Streaming chat completions against any OpenAI-compatible endpoint
//! (OpenRouter by default). Only the subset the orchestrator needs: text and
//! reasoning deltas, tool-call assembly across chunks, usage, and readable
//! error messages.

use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// Time allowed for the response headers to arrive.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest silence tolerated between two streamed chunks.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct ChatRequest<'a> {
    pub base_url: &'a str,
    pub api_key: &'a str,
    pub model: &'a str,
    pub openrouter: bool,
    pub reasoning: Option<Value>,
    pub messages: &'a [Value],
    pub tools: &'a [Value],
}

#[derive(Clone, Debug, Default)]
pub struct StreamedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Default)]
pub struct Completion {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<StreamedToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Option<Value>,
    pub model: Option<String>,
}

pub fn openrouter_headers(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header("HTTP-Referer", "https://github.com/microsoft/terminal")
        .header("X-Title", "bro Orchestrator")
}

/// Turns an error body into one line a person can act on.
pub fn error_message(status: u16, body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let detail = parsed
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.pointer("/error/metadata/raw"))
                .or_else(|| value.get("message"))
                .or_else(|| value.get("error"))
        })
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| body.trim().chars().take(400).collect());
    let hint = match status {
        401 => " (the API key was rejected)",
        402 => " (the account has no credit)",
        403 => " (this key may not use that model)",
        404 => " (unknown model or endpoint path)",
        429 => " (rate limited; try again shortly)",
        _ => "",
    };
    if detail.is_empty() {
        format!("HTTP {status}{hint}")
    } else {
        format!("HTTP {status}: {detail}{hint}")
    }
}

/// `GET {base}/{path}` returning JSON, with the same auth/attribution headers
/// the chat call uses.
pub async fn get_json(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    api_key: Option<&str>,
    openrouter: bool,
) -> Result<Value, String> {
    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let mut builder = client.get(&url).header("accept", "application/json");
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        builder = builder.bearer_auth(key);
    }
    if openrouter {
        builder = openrouter_headers(builder);
    }
    let response = tokio::time::timeout(CONNECT_TIMEOUT, builder.send())
        .await
        .map_err(|_| format!("timed out contacting {url}"))?
        .map_err(|error| format!("could not reach {url}: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("could not read the response from {url}: {error}"))?;
    if !status.is_success() {
        return Err(error_message(status.as_u16(), &body));
    }
    serde_json::from_str(&body).map_err(|error| format!("{url} returned malformed JSON: {error}"))
}

fn apply_choice(
    value: &Value,
    completion: &mut Completion,
    calls: &mut BTreeMap<usize, StreamedToolCall>,
) {
    if let Some(model) = value.get("model").and_then(Value::as_str)
        && !model.is_empty()
    {
        completion.model = Some(model.to_string());
    }
    if let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) {
        completion.usage = Some(usage.clone());
    }
    let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return;
    };
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        completion.finish_reason = Some(reason.to_string());
    }
    let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) else {
        return;
    };
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        completion.text.push_str(text);
    }
    for key in ["reasoning", "reasoning_content"] {
        if let Some(text) = delta.get(key).and_then(Value::as_str) {
            completion.reasoning.push_str(text);
        }
    }
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for (position, call) in tool_calls.iter().enumerate() {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .map(|index| index as usize)
                .unwrap_or(calls.len() + position);
            let entry = calls.entry(index).or_default();
            if let Some(id) = call.get("id").and_then(Value::as_str)
                && !id.is_empty()
            {
                entry.id = id.to_string();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str)
                && entry.name.is_empty()
            {
                entry.name = name.to_string();
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                entry.arguments.push_str(arguments);
            }
        }
    }
}

fn finish(mut completion: Completion, calls: BTreeMap<usize, StreamedToolCall>) -> Completion {
    completion.tool_calls = calls
        .into_iter()
        .filter(|(_, call)| !call.name.is_empty())
        .map(|(index, mut call)| {
            if call.id.is_empty() {
                call.id = format!("call_{index}");
            }
            call
        })
        .collect();
    completion
}

/// Streams one chat completion. `on_delta` receives the text and reasoning
/// accumulated so far every time either grows.
pub async fn stream_chat(
    client: &reqwest::Client,
    request: ChatRequest<'_>,
    mut on_delta: impl FnMut(&str, &str),
) -> Result<Completion, String> {
    let url = format!(
        "{}/chat/completions",
        request.base_url.trim_end_matches('/')
    );
    let mut body = json!({
        "model": request.model,
        "messages": request.messages,
        "stream": true,
        "stream_options": { "include_usage": true }
    });
    if !request.tools.is_empty() {
        body["tools"] = json!(request.tools);
        body["tool_choice"] = json!("auto");
        // Claude and Codex both stream partial tool arguments; enabling
        // parallel calls lets a model read several tabs in one step.
        body["parallel_tool_calls"] = json!(true);
    }
    if request.openrouter {
        body["usage"] = json!({ "include": true });
    }
    if let Some(reasoning) = request.reasoning {
        body["reasoning"] = reasoning;
    }

    let mut builder = client
        .post(&url)
        .bearer_auth(request.api_key)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(&body);
    if request.openrouter {
        builder = openrouter_headers(builder);
    }

    let response = tokio::time::timeout(CONNECT_TIMEOUT, builder.send())
        .await
        .map_err(|_| "timed out waiting for the model endpoint to answer".to_string())?
        .map_err(|error| format!("could not reach {url}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(error_message(status.as_u16(), &text));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut completion = Completion::default();
    let mut calls = BTreeMap::new();

    // Endpoints that ignore `stream` answer with one JSON document.
    if content_type.contains("application/json") {
        let value: Value = response
            .json()
            .await
            .map_err(|error| format!("malformed completion: {error}"))?;
        if let Some(error) = value.get("error") {
            return Err(error_message(status.as_u16(), &error.to_string()));
        }
        apply_choice(&value, &mut completion, &mut calls);
        on_delta(&completion.text, &completion.reasoning);
        return Ok(finish(completion, calls));
    }

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    'outer: loop {
        let chunk = match tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await {
            Err(_) => return Err("the model stopped streaming (no data for 120s)".into()),
            Ok(None) => break,
            Ok(Some(Err(error))) => return Err(format!("the stream failed: {error}")),
            Ok(Some(Ok(received))) => received,
        };
        buffer.extend_from_slice(&chunk);
        while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let raw: Vec<u8> = buffer.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                break 'outer;
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| error.to_string());
                return Err(format!("the model endpoint reported an error: {message}"));
            }
            let before = (completion.text.len(), completion.reasoning.len());
            apply_choice(&value, &mut completion, &mut calls);
            if before != (completion.text.len(), completion.reasoning.len()) {
                on_delta(&completion.text, &completion.reasoning);
            }
        }
    }
    Ok(finish(completion, calls))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_fragments_are_assembled_by_index() {
        let mut completion = Completion::default();
        let mut calls = BTreeMap::new();
        let chunks = [
            json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "call_a", "function": { "name": "read_session", "arguments": "{\"sess" } }] } }] }),
            json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "ionId\":\"x\"}" } }] } }] }),
            json!({ "choices": [{ "delta": { "content": "Looking." }, "finish_reason": "tool_calls" }], "usage": { "prompt_tokens": 10 } }),
        ];
        for chunk in chunks {
            apply_choice(&chunk, &mut completion, &mut calls);
        }
        let completion = finish(completion, calls);
        assert_eq!(completion.text, "Looking.");
        assert_eq!(completion.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(completion.tool_calls.len(), 1);
        assert_eq!(completion.tool_calls[0].id, "call_a");
        assert_eq!(completion.tool_calls[0].name, "read_session");
        assert_eq!(completion.tool_calls[0].arguments, "{\"sessionId\":\"x\"}");
        assert_eq!(completion.usage.unwrap()["prompt_tokens"], 10);
    }

    #[test]
    fn error_bodies_become_readable_messages() {
        assert_eq!(
            error_message(
                401,
                r#"{"error":{"message":"No auth credentials found","code":401}}"#
            ),
            "HTTP 401: No auth credentials found (the API key was rejected)"
        );
        assert_eq!(error_message(500, ""), "HTTP 500");
    }
}
