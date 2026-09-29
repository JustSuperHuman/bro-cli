//! OpenAI Chat Completions client → Anthropic Messages upstream.
//! Request: Chat → Anthropic. Response: Anthropic → Chat.

use super::reasoning_cache::ReasoningCache;
use super::{Target, apply_effort_to_anthropic, default_max_tokens, stop_to_chat_finish};
use crate::anthropic::{
    ContentBlock, ImageSource, Message, MessageContent, MessagesRequest, MessagesResponse, Role,
    SystemPrompt, Tool, ToolChoice, ToolResultContent, parse_tool_input,
};
use crate::openai::{ChatContent, ChatPart, ChatRequest};
use crate::util::now_secs;
use serde_json::{Value, json};

pub fn request(req: &ChatRequest, t: &Target, cache: &ReasoningCache) -> MessagesRequest {
    let mut system: Vec<String> = Vec::new();
    let mut msgs: Vec<Message> = Vec::new();

    for m in &req.messages {
        match m.role.as_str() {
            "system" | "developer" => {
                let s = m.content.as_ref().map(|c| c.text()).unwrap_or_default();
                if !s.is_empty() {
                    system.push(s);
                }
            }
            "user" => {
                let blocks = user_blocks(m.content.as_ref());
                push_blocks(&mut msgs, Role::User, blocks);
            }
            "assistant" => {
                let mut blocks = Vec::new();
                let mut calls = m.tool_calls.clone().unwrap_or_default();
                if let Some(fc) = &m.function_call {
                    calls.push(crate::openai::ChatToolCall {
                        id: fc.name.clone(),
                        kind: "function".into(),
                        function: fc.clone(),
                    });
                }
                // Signed thinking we handed out earlier for this tool turn.
                if let Some(first) = calls.first()
                    && let Some(saved) = cache.get(&first.id)
                {
                    blocks.extend(saved);
                }
                if let Some(c) = &m.content {
                    let text = c.text();
                    if !text.is_empty() {
                        blocks.push(ContentBlock::text(text));
                    }
                }
                for call in calls {
                    blocks.push(ContentBlock::ToolUse {
                        id: call.id,
                        name: call.function.name,
                        input: parse_tool_input(&call.function.arguments),
                        cache_control: None,
                    });
                }
                push_blocks(&mut msgs, Role::Assistant, blocks);
            }
            "tool" | "function" => {
                let id = m
                    .tool_call_id
                    .clone()
                    .or_else(|| m.name.clone())
                    .unwrap_or_default();
                let content = match &m.content {
                    Some(ChatContent::Parts(parts)) => {
                        let b = parts_to_blocks(parts);
                        if b.is_empty() {
                            None
                        } else {
                            Some(ToolResultContent::Blocks(b))
                        }
                    }
                    Some(ChatContent::Text(s)) if !s.is_empty() => {
                        Some(ToolResultContent::Text(s.clone()))
                    }
                    _ => None,
                };
                push_blocks(
                    &mut msgs,
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: id,
                        content,
                        is_error: false,
                        cache_control: None,
                    }],
                );
            }
            _ => {}
        }
    }
    finalize_messages(&mut msgs);

    let mut tools: Vec<Tool> = Vec::new();
    for tool in req.tools.iter().flatten() {
        if let Some(f) = &tool.function {
            tools.push(Tool {
                name: f.name.clone(),
                description: f.description.clone(),
                input_schema: Some(super::schema::clean(f.parameters.as_ref(), false)),
                ..Default::default()
            });
        }
    }
    for f in req.functions.iter().flatten() {
        tools.push(Tool {
            name: f.name.clone(),
            description: f.description.clone(),
            input_schema: Some(super::schema::clean(f.parameters.as_ref(), false)),
            ..Default::default()
        });
    }

    let disable = req.parallel_tool_calls == Some(false);
    let dp = disable.then_some(true);
    let tool_choice = if tools.is_empty() {
        None
    } else {
        match req.tool_choice.as_ref().or(req.extra.get("function_call")) {
            Some(Value::String(s)) if s == "required" => Some(ToolChoice::Any {
                disable_parallel_tool_use: dp,
            }),
            Some(Value::String(s)) if s == "none" => Some(ToolChoice::None),
            Some(Value::Object(o)) => {
                let name = o
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .or_else(|| o.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Some(ToolChoice::Tool {
                    name: name.to_string(),
                    disable_parallel_tool_use: dp,
                })
            }
            _ => disable.then_some(ToolChoice::Auto {
                disable_parallel_tool_use: dp,
            }),
        }
    };

    let explicit_max = req.max_completion_tokens.or(req.max_tokens);
    let mut out = MessagesRequest {
        model: t.model.clone(),
        messages: msgs,
        system: (!system.is_empty()).then(|| SystemPrompt::Text(system.join("\n\n"))),
        max_tokens: Some(explicit_max.unwrap_or_else(|| default_max_tokens(&t.model))),
        metadata: req.user.as_ref().map(|u| json!({"user_id": u})),
        stop_sequences: req
            .stop
            .clone()
            .map(|s| {
                s.into_vec()
                    .into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty()),
        stream: req.stream,
        temperature: req.temperature.map(|x| x.clamp(0.0, 1.0)),
        top_p: req.top_p,
        top_k: None,
        tools: (!tools.is_empty()).then_some(tools),
        tool_choice,
        thinking: None,
        extra: Default::default(),
    };
    let effort = t
        .effort_override
        .clone()
        .or_else(|| req.reasoning_effort.clone());
    apply_effort_to_anthropic(&mut out, effort.as_deref(), explicit_max.is_some());
    super::drop_thinking_if_unsigned(&mut out);
    out
}

fn user_blocks(c: Option<&ChatContent>) -> Vec<ContentBlock> {
    match c {
        None => vec![],
        Some(ChatContent::Text(s)) => vec![ContentBlock::text(s.clone())],
        Some(ChatContent::Parts(p)) => parts_to_blocks(p),
    }
}

pub fn parts_to_blocks(parts: &[ChatPart]) -> Vec<ContentBlock> {
    parts
        .iter()
        .filter_map(|p| match p {
            ChatPart::Text { text } => Some(ContentBlock::text(text.clone())),
            ChatPart::Refusal { refusal } => Some(ContentBlock::text(refusal.clone())),
            ChatPart::ImageUrl { image_url } => Some(ContentBlock::Image {
                source: ImageSource::from_url(&image_url.url),
                cache_control: None,
            }),
            ChatPart::File { file } => file_block(file),
            ChatPart::Unknown => None,
        })
        .collect()
}

/// `{file_data: "data:application/pdf;base64,…", filename}` → document block.
pub fn file_block(file: &Value) -> Option<ContentBlock> {
    let data = file.get("file_data").and_then(Value::as_str)?;
    let ImageSource::Base64 { media_type, data } = ImageSource::from_url(data) else {
        return None;
    };
    let title = file
        .get("filename")
        .and_then(Value::as_str)
        .map(str::to_string);
    if media_type.starts_with("image/") {
        return Some(ContentBlock::Image {
            source: ImageSource::Base64 { media_type, data },
            cache_control: None,
        });
    }
    Some(ContentBlock::Document {
        source: json!({"type": "base64", "media_type": media_type, "data": data}),
        title,
        cache_control: None,
    })
}

/// Append blocks, merging consecutive same-role turns (Anthropic requires every
/// tool_result for a turn in the single following user message).
pub fn push_blocks(msgs: &mut Vec<Message>, role: Role, blocks: Vec<ContentBlock>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = msgs.last_mut()
        && last.role == role
    {
        let mut existing =
            std::mem::replace(&mut last.content, MessageContent::Blocks(vec![])).into_blocks();
        existing.extend(blocks);
        last.content = MessageContent::Blocks(existing);
        return;
    }
    msgs.push(Message {
        role,
        content: MessageContent::Blocks(blocks),
    });
}

/// Anthropic validation rules: no empty text blocks, tool_results lead their user
/// turn, thinking leads its assistant turn, and no trailing whitespace on a final
/// assistant (prefill) turn.
pub fn finalize_messages(msgs: &mut Vec<Message>) {
    for m in msgs.iter_mut() {
        let mut blocks =
            std::mem::replace(&mut m.content, MessageContent::Blocks(vec![])).into_blocks();
        blocks.retain(|b| !matches!(b, ContentBlock::Text { text, .. } if text.trim().is_empty()));
        blocks.sort_by_key(|b| match (m.role, b) {
            (Role::User, ContentBlock::ToolResult { .. }) => 0,
            (
                Role::Assistant,
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. },
            ) => 0,
            _ => 1,
        });
        m.content = MessageContent::Blocks(blocks);
    }
    msgs.retain(|m| !matches!(&m.content, MessageContent::Blocks(b) if b.is_empty()));
    if let Some(last) = msgs.last_mut()
        && last.role == Role::Assistant
        && let MessageContent::Blocks(b) = &mut last.content
        && let Some(ContentBlock::Text { text, .. }) = b.last_mut()
    {
        let trimmed = text.trim_end().to_string();
        *text = trimmed;
    }
}

/// Anthropic message → Chat Completions JSON. Stashes signed thinking for tool turns.
pub fn response(m: &MessagesResponse, client_model: &str, cache: &ReasoningCache) -> Value {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    for b in &m.content {
        match b {
            ContentBlock::Text { text: t, .. } => text.push_str(t),
            ContentBlock::Thinking { thinking, .. } => reasoning.push_str(thinking),
            ContentBlock::ToolUse { id, name, input, .. } => calls.push(json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": serde_json::to_string(input).unwrap_or_default()}
            })),
            _ => {}
        }
    }
    if let Some(first) = calls.first().and_then(|c| c["id"].as_str()) {
        cache.put(first, ReasoningCache::reasoning_blocks(&m.content));
    }
    let mut message = json!({"role": "assistant", "content": if text.is_empty() && !calls.is_empty() { Value::Null } else { json!(text) }});
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    if !calls.is_empty() {
        message["tool_calls"] = json!(calls);
    }
    json!({
        "id": format!("chatcmpl-{}", m.id.trim_start_matches("msg_")),
        "object": "chat.completion",
        "created": now_secs(),
        "model": client_model,
        "choices": [{
            "index": 0,
            "message": message,
            "logprobs": null,
            "finish_reason": stop_to_chat_finish(m.stop_reason.as_deref()),
        }],
        "usage": super::chat_usage_json(&m.usage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ChatRequest {
        serde_json::from_value(json!({
            "model": "claude-sonnet-4-5",
            "max_completion_tokens": 4000,
            "reasoning_effort": "low",
            "temperature": 0.3,
            "stop": "END",
            "user": "u1",
            "parallel_tool_calls": false,
            "tool_choice": {"type": "function", "function": {"name": "get_weather"}},
            "tools": [{"type": "function", "function": {"name": "get_weather", "description": "w", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}],
            "messages": [
                {"role": "system", "content": "sys one"},
                {"role": "developer", "content": [{"type": "text", "text": "sys two"}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "Weather in these?"},
                    {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,/9j/"}},
                    {"type": "image_url", "image_url": {"url": "https://img/x.png"}}
                ]},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_a", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}},
                    {"id": "call_b", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Rome\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_a", "content": "sunny"},
                {"role": "tool", "tool_call_id": "call_b", "content": [{"type": "text", "text": "rain"}]},
                {"role": "user", "content": "thanks"},
                {"role": "assistant", "content": "You're welcome!  "}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn translates_multi_turn_request() {
        let cache = ReasoningCache::new();
        cache.put(
            "call_a",
            vec![ContentBlock::Thinking {
                thinking: "plan".into(),
                signature: Some("SIG".into()),
            }],
        );
        let t = Target {
            model: "claude-sonnet-4-5".into(),
            ..Default::default()
        };
        let v = serde_json::to_value(request(&fixture(), &t, &cache)).unwrap();
        assert_eq!(v["system"], "sys one\n\nsys two");
        assert_eq!(v["max_tokens"], 4000);
        assert_eq!(
            v["thinking"],
            json!({"type": "enabled", "budget_tokens": 2048})
        );
        assert!(
            v.get("temperature").is_none(),
            "thinking forbids temperature"
        );
        assert_eq!(v["stop_sequences"], json!(["END"]));
        assert_eq!(v["metadata"]["user_id"], "u1");
        assert_eq!(
            v["tool_choice"],
            json!({"type": "tool", "name": "get_weather", "disable_parallel_tool_use": true})
        );
        assert_eq!(
            v["tools"][0]["input_schema"]["properties"]["city"]["type"],
            "string"
        );
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4);
        assert_eq!(
            msgs[0]["content"][1]["source"],
            json!({"type": "base64", "media_type": "image/jpeg", "data": "/9j/"})
        );
        assert_eq!(
            msgs[0]["content"][2]["source"],
            json!({"type": "url", "url": "https://img/x.png"})
        );
        assert_eq!(
            msgs[1]["content"][0],
            json!({"type": "thinking", "thinking": "plan", "signature": "SIG"})
        );
        assert_eq!(msgs[1]["content"][1]["input"], json!({"city": "Paris"}));
        assert_eq!(msgs[1]["content"][2]["id"], "call_b");
        assert_eq!(
            msgs[2]["content"][0],
            json!({"type": "tool_result", "tool_use_id": "call_a", "content": "sunny"})
        );
        assert_eq!(msgs[2]["content"][1]["content"][0]["text"], "rain");
        assert_eq!(msgs[2]["content"][2]["text"], "thanks");
        assert_eq!(msgs[3]["content"][0]["text"], "You're welcome!");
    }

    #[test]
    fn no_thinking_keeps_temperature_clamped() {
        let mut r = fixture();
        r.reasoning_effort = None;
        r.temperature = Some(1.7);
        let v =
            serde_json::to_value(request(&r, &Target::default(), &ReasoningCache::new())).unwrap();
        assert_eq!(v["temperature"], 1.0);
        assert!(v.get("thinking").is_none());
    }

    #[test]
    fn translates_response_and_caches_signature() {
        let m: MessagesResponse = serde_json::from_value(json!({
            "id": "msg_01", "model": "claude-x", "role": "assistant", "type": "message",
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "SIG"},
                {"type": "tool_use", "id": "toolu_9", "name": "f", "input": {"a": 1}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 7, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 10}
        }))
        .unwrap();
        let cache = ReasoningCache::new();
        let v = response(&m, "claude-x", &cache);
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(v["choices"][0]["message"]["content"], Value::Null);
        assert_eq!(v["choices"][0]["message"]["reasoning_content"], "hmm");
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"a\":1}"
        );
        assert_eq!(v["usage"]["prompt_tokens"], 115);
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 100);
        assert!(cache.get("toolu_9").is_some());
    }
}
