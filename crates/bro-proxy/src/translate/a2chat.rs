//! Anthropic Messages client → OpenAI Chat Completions upstream.
//! Request: Anthropic → Chat. Response: Chat → Anthropic.

use super::{
    SIG_CHAT, Target, anthropic_effort, chat_finish_to_stop, clamp_effort, is_reasoning_model,
    schema,
};
use crate::anthropic::{
    ContentBlock, MessagesRequest, MessagesResponse, Role, ToolChoice, parse_tool_input,
};
use crate::openai::{
    ChatContent, ChatFunctionCall, ChatFunctionDef, ChatMessage, ChatPart, ChatRequest,
    ChatResponse, ChatTool, ChatToolCall, ImageUrl, Stop,
};
use crate::util::{gen_id, short_hash};
use serde_json::{Value, json};

pub fn request(req: &MessagesRequest, t: &Target) -> ChatRequest {
    let mut messages = Vec::new();
    let system = req.system_text();
    if !system.is_empty() {
        messages.push(ChatMessage {
            role: "system".into(),
            content: Some(ChatContent::Text(system)),
            ..Default::default()
        });
    }
    for m in &req.messages {
        let blocks = m.content.blocks();
        match m.role {
            Role::User => user_messages(blocks, &mut messages),
            Role::Assistant => {
                if let Some(msg) = assistant_message(blocks) {
                    messages.push(msg);
                }
            }
        }
    }

    let tools: Vec<ChatTool> = req
        .tools
        .iter()
        .flatten()
        .filter(|t| t.is_custom())
        .map(|tool| ChatTool {
            kind: "function".into(),
            function: Some(ChatFunctionDef {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: Some(schema::clean(tool.input_schema.as_ref(), t.strict_schema)),
                strict: None,
            }),
        })
        .collect();
    let has_tools = !tools.is_empty();

    let tool_choice = if has_tools {
        req.tool_choice.as_ref().map(|c| match c {
            ToolChoice::Auto { .. } => json!("auto"),
            ToolChoice::Any { .. } => json!("required"),
            ToolChoice::None => json!("none"),
            ToolChoice::Tool { name, .. } => {
                json!({"type": "function", "function": {"name": name}})
            }
        })
    } else {
        None
    };
    let parallel = (has_tools
        && req
            .tool_choice
            .as_ref()
            .is_some_and(|c| c.disable_parallel()))
    .then_some(false);

    let effort = t.effort_override.clone().or_else(|| anthropic_effort(req));
    let reasoning_effort = effort
        .filter(|_| t.effort_override.is_some() || is_reasoning_model(&t.model))
        .map(|e| {
            let sup = if t.supported_efforts.is_empty() {
                vec!["low".to_string(), "medium".into(), "high".into()]
            } else {
                t.supported_efforts.clone()
            };
            clamp_effort(&e, &sup)
        });
    // Reasoning models reject sampling parameters.
    let sampling_ok = reasoning_effort.is_none();

    let stream = req.stream.unwrap_or(false);
    let (max_tokens, max_completion_tokens) = if t.max_completion_tokens {
        (None, req.max_tokens)
    } else {
        (req.max_tokens, None)
    };
    ChatRequest {
        model: t.model.clone(),
        messages,
        max_tokens,
        max_completion_tokens,
        temperature: req.temperature.filter(|_| sampling_ok),
        top_p: req.top_p.filter(|_| sampling_ok),
        stop: req
            .stop_sequences
            .clone()
            .filter(|s| !s.is_empty())
            .map(Stop::Many),
        stream: Some(stream),
        stream_options: stream.then(|| json!({"include_usage": true})),
        tools: has_tools.then_some(tools),
        functions: None,
        tool_choice,
        parallel_tool_calls: parallel,
        reasoning_effort,
        user: req.user_id().map(|u| short_hash(&u)),
        extra: Default::default(),
    }
}

/// A user turn becomes: tool messages first (they must directly follow the
/// assistant's tool_calls), then one user message with the remaining content.
fn user_messages(blocks: Vec<ContentBlock>, out: &mut Vec<ChatMessage>) {
    let mut parts: Vec<ChatPart> = Vec::new();
    let mut tool_images: Vec<ChatPart> = Vec::new();
    for b in blocks {
        match b {
            ContentBlock::Text { text, .. } if !text.is_empty() => {
                parts.push(ChatPart::Text { text })
            }
            ContentBlock::Image { source, .. } => {
                if let Some(url) = source.to_url() {
                    parts.push(ChatPart::ImageUrl {
                        image_url: ImageUrl { url, detail: None },
                    });
                }
            }
            ContentBlock::Document { source, title, .. } => {
                if let Some(text) = document_text(&source, title.as_deref()) {
                    parts.push(ChatPart::Text { text });
                }
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                let mut text = content.as_ref().map(|c| c.text()).unwrap_or_default();
                let images = content.as_ref().map(|c| c.images()).unwrap_or_default();
                if is_error {
                    text = format!("[tool error] {text}");
                }
                if text.is_empty() {
                    text = if images.is_empty() {
                        "(no output)".into()
                    } else {
                        "(see attached image)".into()
                    };
                }
                for img in images {
                    if let Some(url) = img.to_url() {
                        tool_images.push(ChatPart::ImageUrl {
                            image_url: ImageUrl { url, detail: None },
                        });
                    }
                }
                out.push(ChatMessage {
                    role: "tool".into(),
                    tool_call_id: Some(tool_use_id),
                    content: Some(ChatContent::Text(text)),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
    // Tool messages can't carry images: attach them to the following user message.
    if !tool_images.is_empty() {
        let mut v = vec![ChatPart::Text {
            text: "Images returned by the tool call(s):".into(),
        }];
        v.extend(tool_images);
        v.append(&mut parts);
        parts = v;
    }
    if parts.is_empty() {
        return;
    }
    let content = if parts.iter().all(|p| matches!(p, ChatPart::Text { .. })) {
        ChatContent::Text(
            parts
                .iter()
                .filter_map(|p| {
                    if let ChatPart::Text { text } = p {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        )
    } else {
        ChatContent::Parts(parts)
    };
    out.push(ChatMessage {
        role: "user".into(),
        content: Some(content),
        ..Default::default()
    });
}

fn document_text(source: &Value, title: Option<&str>) -> Option<String> {
    let body = match source.get("type").and_then(Value::as_str) {
        Some("text") => source.get("data").and_then(Value::as_str)?.to_string(),
        Some("content") => source.get("content").and_then(Value::as_str)?.to_string(),
        Some("url") => format!("Document: {}", source.get("url").and_then(Value::as_str)?),
        _ => {
            return Some(format!(
                "[document omitted: {} not supported by this model]",
                title.unwrap_or("attachment")
            ));
        }
    };
    Some(match title {
        Some(t) => format!("# {t}\n\n{body}"),
        None => body,
    })
}

fn assistant_message(blocks: Vec<ContentBlock>) -> Option<ChatMessage> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    for b in blocks {
        match b {
            ContentBlock::Text { text: t, .. } => text.push_str(&t),
            ContentBlock::ToolUse {
                id, name, input, ..
            } => calls.push(ChatToolCall {
                id,
                kind: "function".into(),
                function: ChatFunctionCall {
                    name,
                    arguments: serde_json::to_string(&input).unwrap_or_default(),
                },
            }),
            // Only reasoning that came from a Chat upstream is sent back as reasoning_content.
            ContentBlock::Thinking {
                thinking,
                signature,
            } if signature.as_deref() == Some(SIG_CHAT) => reasoning.push_str(&thinking),
            _ => {}
        }
    }
    if text.is_empty() && calls.is_empty() {
        return None;
    }
    let has_calls = !calls.is_empty();
    Some(ChatMessage {
        role: "assistant".into(),
        content: if text.is_empty() {
            None
        } else {
            Some(ChatContent::Text(text))
        },
        tool_calls: has_calls.then_some(calls),
        // Interleaved-thinking servers (DeepSeek) need it back within a tool loop.
        reasoning_content: (has_calls && !reasoning.is_empty()).then_some(reasoning),
        ..Default::default()
    })
}

/// Chat Completions JSON → Anthropic message.
pub fn response(resp: &ChatResponse, client_model: &str) -> MessagesResponse {
    let id = if resp.id.is_empty() {
        gen_id("msg_")
    } else {
        format!("msg_{}", resp.id.trim_start_matches("chatcmpl-"))
    };
    let mut m = MessagesResponse::new(id, client_model);
    let choice = resp.choices.first();
    let mut has_calls = false;
    if let Some(c) = choice {
        let msg = &c.message;
        if let Some(r) = msg
            .reasoning_content
            .as_ref()
            .or(msg.reasoning.as_ref())
            .filter(|r| !r.is_empty())
        {
            m.content.push(ContentBlock::Thinking {
                thinking: r.clone(),
                signature: Some(SIG_CHAT.into()),
            });
        }
        let text = msg.content.as_ref().map(|c| c.text()).unwrap_or_default();
        if !text.is_empty() {
            m.content.push(ContentBlock::text(text));
        }
        if let Some(r) = msg.refusal.as_ref().filter(|r| !r.is_empty()) {
            m.content.push(ContentBlock::text(r.clone()));
        }
        let mut calls: Vec<ChatToolCall> = msg.tool_calls.clone().unwrap_or_default();
        if let Some(fc) = &msg.function_call {
            calls.push(ChatToolCall {
                id: String::new(),
                kind: "function".into(),
                function: fc.clone(),
            });
        }
        for call in calls {
            has_calls = true;
            m.content.push(ContentBlock::ToolUse {
                id: if call.id.is_empty() {
                    gen_id("toolu_")
                } else {
                    call.id
                },
                name: call.function.name,
                input: parse_tool_input(&call.function.arguments),
                cache_control: None,
            });
        }
    }
    m.stop_reason = Some(
        chat_finish_to_stop(choice.and_then(|c| c.finish_reason.as_deref()), has_calls).into(),
    );
    if let Some(u) = &resp.usage {
        m.usage = super::usage_from_chat(u);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> MessagesRequest {
        serde_json::from_value(json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 8000,
            "stream": true,
            "system": [{"type": "text", "text": "You are helpful.", "cache_control": {"type": "ephemeral"}}],
            "thinking": {"type": "enabled", "budget_tokens": 20000},
            "metadata": {"user_id": "user_abc_session_1"},
            "tools": [
                {"name": "Read", "description": "read a file", "input_schema": {"$schema": "x", "type": "object", "properties": {"path": {"type": "string"}}}},
                {"type": "web_search_20250305", "name": "web_search", "max_uses": 3}
            ],
            "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "Look at this"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBOR"}}
                ]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "need file", "signature": "realsig"},
                    {"type": "text", "text": "Reading."},
                    {"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"path": "a.txt"}},
                    {"type": "tool_use", "id": "toolu_2", "name": "Read", "input": {"path": "b.png"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "hello"},
                    {"type": "tool_result", "tool_use_id": "toolu_2", "is_error": true, "content": [
                        {"type": "text", "text": "binary"},
                        {"type": "image", "source": {"type": "url", "url": "https://x/b.png"}}
                    ]},
                    {"type": "text", "text": "continue"}
                ]}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn translates_multi_turn_request() {
        let t = Target {
            model: "gpt-5".into(),
            max_completion_tokens: true,
            ..Default::default()
        };
        let out = serde_json::to_value(request(&fixture(), &t)).unwrap();
        assert_eq!(out["model"], "gpt-5");
        assert_eq!(out["max_completion_tokens"], 8000);
        assert!(out.get("max_tokens").is_none());
        assert_eq!(out["reasoning_effort"], "high");
        assert_eq!(out["stream_options"]["include_usage"], true);
        assert_eq!(out["parallel_tool_calls"], false);
        assert_eq!(out["tool_choice"], "auto");
        assert_eq!(out["tools"].as_array().unwrap().len(), 1);
        assert!(
            out["tools"][0]["function"]["parameters"]
                .get("$schema")
                .is_none()
        );
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(
            msgs[0],
            json!({"role": "system", "content": "You are helpful."})
        );
        assert_eq!(
            msgs[1]["content"][1]["image_url"]["url"],
            "data:image/png;base64,iVBOR"
        );
        assert_eq!(msgs[2]["content"], "Reading.");
        assert_eq!(
            msgs[2]["tool_calls"][1]["function"]["arguments"],
            "{\"path\":\"b.png\"}"
        );
        assert!(msgs[2].get("reasoning_content").is_none());
        assert_eq!(
            msgs[3],
            json!({"role": "tool", "tool_call_id": "toolu_1", "content": "hello"})
        );
        assert_eq!(msgs[4]["content"], "[tool error] binary");
        assert_eq!(msgs[5]["role"], "user");
        assert_eq!(msgs[5]["content"][1]["image_url"]["url"], "https://x/b.png");
        assert_eq!(msgs[5]["content"][2]["text"], "continue");
        assert!(out.get("temperature").is_none());
        assert_eq!(out["user"].as_str().unwrap().len(), 16);
    }

    #[test]
    fn non_reasoning_model_keeps_sampling() {
        let mut r = fixture();
        r.temperature = Some(0.5);
        let t = Target {
            model: "llama-3.3-70b".into(),
            ..Default::default()
        };
        let out = serde_json::to_value(request(&r, &t)).unwrap();
        assert!(out.get("reasoning_effort").is_none());
        assert_eq!(out["temperature"], 0.5);
        assert_eq!(out["max_tokens"], 8000);
    }

    #[test]
    fn translates_response() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "chatcmpl-9", "object": "chat.completion", "model": "deepseek-reasoner",
            "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": "Let me check.", "reasoning_content": "thinking…",
                "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "Read", "arguments": "{\"path\":\"x\"}"}}]
            }}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120, "prompt_tokens_details": {"cached_tokens": 60}}
        }))
        .unwrap();
        let m = response(&resp, "claude-sonnet-4-5");
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["id"], "msg_9");
        assert_eq!(v["model"], "claude-sonnet-4-5");
        assert_eq!(
            v["content"][0],
            json!({"type": "thinking", "thinking": "thinking…", "signature": "bro.chat"})
        );
        assert_eq!(v["content"][1]["text"], "Let me check.");
        assert_eq!(v["content"][2]["input"], json!({"path": "x"}));
        assert_eq!(v["stop_reason"], "tool_use");
        assert_eq!(
            v["usage"],
            json!({"input_tokens": 40, "output_tokens": 20, "cache_read_input_tokens": 60})
        );
    }

    #[test]
    fn chat_reasoning_echoed_in_tool_loop() {
        let r: MessagesRequest = serde_json::from_value(json!({
            "model": "m", "messages": [{"role": "assistant", "content": [
                {"type": "thinking", "thinking": "r", "signature": "bro.chat"},
                {"type": "tool_use", "id": "c1", "name": "f", "input": {}}
            ]}]
        }))
        .unwrap();
        let out = serde_json::to_value(request(&r, &Target::default())).unwrap();
        assert_eq!(out["messages"][0]["reasoning_content"], "r");
        assert!(out["messages"][0].get("content").is_none());
    }
}
