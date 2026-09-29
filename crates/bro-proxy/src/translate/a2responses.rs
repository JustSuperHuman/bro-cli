//! Anthropic Messages client → OpenAI Responses upstream (incl. the ChatGPT Codex
//! backend; details ported from bro v1's codex-bridge.js).
//! Request: Anthropic → Responses. Response: Responses → Anthropic.

use super::{SIG_RESPONSES, Target, anthropic_effort, clamp_effort, is_reasoning_model, schema};
use crate::anthropic::{
    ContentBlock, MessagesRequest, MessagesResponse, Role, ToolChoice, parse_tool_input,
};
use crate::openai::{ResponsesRequest, ResponsesUsage, item_text, str_field};
use crate::util::gen_id;
use serde_json::{Value, json};

pub const CHATGPT_DEFAULT_INSTRUCTIONS: &str = "You are a helpful coding agent.";

pub fn request(req: &MessagesRequest, t: &Target) -> ResponsesRequest {
    let system = req.system_text();
    let input = input_items(req);

    let tools: Vec<Value> = req
        .tools
        .iter()
        .flatten()
        .filter(|t| t.is_custom())
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description.clone().unwrap_or_default(),
                "strict": false,
                "parameters": schema::clean(tool.input_schema.as_ref(), t.strict_schema),
            })
        })
        .collect();
    let has_tools = !tools.is_empty();
    let tool_choice = match &req.tool_choice {
        _ if !has_tools => None,
        None => Some(json!("auto")),
        Some(ToolChoice::Auto { .. }) => Some(json!("auto")),
        Some(ToolChoice::Any { .. }) => Some(json!("required")),
        Some(ToolChoice::None) => Some(json!("none")),
        Some(ToolChoice::Tool { name, .. }) => Some(json!({"type": "function", "name": name})),
    };
    let parallel = has_tools.then(|| {
        !req.tool_choice
            .as_ref()
            .is_some_and(|c| c.disable_parallel())
    });

    // Reasoning: ChatGPT always (v1); OpenAI API only for reasoning models or when asked.
    let wanted = t.effort_override.clone().or_else(|| anthropic_effort(req));
    let reasoning_on = t.chatgpt || t.effort_override.is_some() || is_reasoning_model(&t.model);
    let reasoning = reasoning_on.then(|| {
        let default_sup = ["low", "medium", "high", "xhigh"]
            .map(String::from)
            .to_vec();
        let sup = if t.supported_efforts.is_empty() {
            &default_sup
        } else {
            &t.supported_efforts
        };
        let effort = clamp_effort(wanted.as_deref().unwrap_or("medium"), sup);
        json!({"effort": effort, "summary": "auto"})
    });
    let sampling_ok = !reasoning_on;

    ResponsesRequest {
        model: t.model.clone(),
        instructions: if system.is_empty() {
            t.chatgpt.then(|| CHATGPT_DEFAULT_INSTRUCTIONS.to_string())
        } else {
            Some(system)
        },
        input: Value::Array(input),
        tools: if has_tools || t.chatgpt {
            Some(tools)
        } else {
            None
        },
        tool_choice: tool_choice.or_else(|| t.chatgpt.then(|| json!("auto"))),
        parallel_tool_calls: parallel.or_else(|| t.chatgpt.then_some(true)),
        include: reasoning
            .is_some()
            .then(|| vec!["reasoning.encrypted_content".to_string()]),
        reasoning,
        // The ChatGPT backend rejects max_output_tokens.
        max_output_tokens: if t.chatgpt { None } else { req.max_tokens },
        temperature: req.temperature.filter(|_| sampling_ok),
        top_p: req.top_p.filter(|_| sampling_ok),
        stream: Some(t.chatgpt || req.stream.unwrap_or(false)),
        store: Some(false),
        prompt_cache_key: t.cache_key.clone(),
        user: None,
        metadata: None,
        previous_response_id: None,
        extra: Default::default(),
    }
}

/// Flatten Anthropic messages into Responses input items.
pub fn input_items(req: &MessagesRequest) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut pending: Option<(Role, Vec<Value>)> = None;
    fn flush(items: &mut Vec<Value>, pending: &mut Option<(Role, Vec<Value>)>) {
        if let Some((role, content)) = pending.take()
            && !content.is_empty()
        {
            let role = if role == Role::User {
                "user"
            } else {
                "assistant"
            };
            items.push(json!({"type": "message", "role": role, "content": content}));
        }
    }
    let push = |items: &mut Vec<Value>,
                pending: &mut Option<(Role, Vec<Value>)>,
                role: Role,
                part: Value| {
        if pending.as_ref().is_none_or(|(r, _)| *r != role) {
            flush(items, pending);
            *pending = Some((role, vec![]));
        }
        if let Some((_, c)) = pending.as_mut() {
            c.push(part);
        }
    };

    for m in &req.messages {
        for block in m.content.blocks() {
            match (m.role, block) {
                (Role::Assistant, ContentBlock::Text { text, .. }) if !text.is_empty() => push(
                    &mut items,
                    &mut pending,
                    Role::Assistant,
                    json!({"type": "output_text", "text": text}),
                ),
                (
                    Role::Assistant,
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    },
                ) => {
                    flush(&mut items, &mut pending);
                    items.push(json!({
                        "type": "function_call", "call_id": id, "name": name,
                        "arguments": serde_json::to_string(&input).unwrap_or_default()
                    }));
                }
                (
                    Role::Assistant,
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                    },
                ) => {
                    // Only reasoning that came from a Responses upstream can go back.
                    if let Some(enc) = signature
                        .as_deref()
                        .and_then(|s| s.strip_prefix(SIG_RESPONSES))
                    {
                        flush(&mut items, &mut pending);
                        let summary = if thinking.is_empty() {
                            json!([])
                        } else {
                            json!([{"type": "summary_text", "text": thinking}])
                        };
                        items.push(json!({"type": "reasoning", "summary": summary, "encrypted_content": enc}));
                    }
                }
                (Role::User, ContentBlock::Text { text, .. }) if !text.is_empty() => push(
                    &mut items,
                    &mut pending,
                    Role::User,
                    json!({"type": "input_text", "text": text}),
                ),
                (Role::User, ContentBlock::Image { source, .. }) => {
                    if let Some(url) = source.to_url() {
                        push(
                            &mut items,
                            &mut pending,
                            Role::User,
                            json!({"type": "input_image", "image_url": url}),
                        )
                    }
                }
                (Role::User, ContentBlock::Document { source, title, .. }) => {
                    if let Some(part) = document_part(&source, title.as_deref()) {
                        push(&mut items, &mut pending, Role::User, part)
                    }
                }
                (
                    Role::User,
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    },
                ) => {
                    flush(&mut items, &mut pending);
                    let text = content.as_ref().map(|c| c.text()).unwrap_or_default();
                    let images = content.as_ref().map(|c| c.images()).unwrap_or_default();
                    let prefix = if is_error { "[tool error] " } else { "" };
                    let output = if images.is_empty() {
                        json!(format!(
                            "{prefix}{}",
                            if text.is_empty() {
                                "(no output)"
                            } else {
                                &text
                            }
                        ))
                    } else {
                        let mut parts =
                            vec![json!({"type": "input_text", "text": format!("{prefix}{text}")})];
                        parts.extend(
                            images
                                .iter()
                                .filter_map(|i| i.to_url())
                                .map(|u| json!({"type": "input_image", "image_url": u})),
                        );
                        Value::Array(parts)
                    };
                    items.push(json!({"type": "function_call_output", "call_id": tool_use_id, "output": output}));
                }
                _ => {}
            }
        }
        flush(&mut items, &mut pending);
    }
    flush(&mut items, &mut pending);
    items
}

fn document_part(source: &Value, title: Option<&str>) -> Option<Value> {
    match str_field(source, "type")? {
        "base64" => Some(json!({
            "type": "input_file",
            "filename": title.unwrap_or("document.pdf"),
            "file_data": format!("data:{};base64,{}", str_field(source, "media_type").unwrap_or("application/pdf"), str_field(source, "data")?),
        })),
        "text" => Some(json!({"type": "input_text", "text": str_field(source, "data")?})),
        "url" => Some(json!({"type": "input_file", "file_url": str_field(source, "url")?})),
        _ => None,
    }
}

/// A complete Responses object (JSON response or accumulated stream) → Anthropic message.
pub fn response(resp: &Value, client_model: &str) -> MessagesResponse {
    let id = str_field(resp, "id")
        .map(|s| format!("msg_{}", s.trim_start_matches("resp_")))
        .unwrap_or_else(|| gen_id("msg_"));
    let mut m = MessagesResponse::new(id, client_model);
    let mut saw_tool = false;
    for item in resp
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match str_field(item, "type") {
            Some("reasoning") => {
                let summary: Vec<&str> = item
                    .get("summary")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|p| str_field(p, "text"))
                    .collect();
                let mut text = summary.join("\n\n");
                if text.is_empty() {
                    text = item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|p| str_field(p, "text"))
                        .collect::<Vec<_>>()
                        .join("");
                }
                let enc = str_field(item, "encrypted_content");
                if text.is_empty() && enc.is_none() {
                    continue;
                }
                m.content.push(ContentBlock::Thinking {
                    thinking: text,
                    signature: Some(
                        enc.map(|e| format!("{SIG_RESPONSES}{e}"))
                            .unwrap_or_default(),
                    ),
                });
            }
            Some("message") => {
                let text = item_text(item);
                if !text.is_empty() {
                    m.content.push(ContentBlock::text(text));
                }
            }
            Some("function_call") => {
                saw_tool = true;
                m.content.push(ContentBlock::ToolUse {
                    id: str_field(item, "call_id")
                        .map(str::to_string)
                        .unwrap_or_else(|| gen_id("call_")),
                    name: str_field(item, "name").unwrap_or_default().to_string(),
                    input: parse_tool_input(str_field(item, "arguments").unwrap_or("")),
                    cache_control: None,
                });
            }
            Some("custom_tool_call") => {
                saw_tool = true;
                m.content.push(ContentBlock::ToolUse {
                    id: str_field(item, "call_id")
                        .map(str::to_string)
                        .unwrap_or_else(|| gen_id("call_")),
                    name: str_field(item, "name").unwrap_or_default().to_string(),
                    input: json!({"input": str_field(item, "input").unwrap_or("")}),
                    cache_control: None,
                });
            }
            _ => {}
        }
    }
    m.stop_reason = Some(responses_stop(resp, saw_tool).into());
    if let Some(u) = resp
        .get("usage")
        .and_then(|u| serde_json::from_value::<ResponsesUsage>(u.clone()).ok())
    {
        m.usage = super::usage_from_responses(&u);
    }
    m
}

pub fn responses_stop(resp: &Value, saw_tool: bool) -> &'static str {
    if saw_tool {
        return "tool_use";
    }
    let incomplete = resp
        .get("incomplete_details")
        .and_then(|d| str_field(d, "reason"));
    match (str_field(resp, "status"), incomplete) {
        (_, Some("max_output_tokens")) => "max_tokens",
        (_, Some("content_filter")) => "refusal",
        _ => "end_turn",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> MessagesRequest {
        serde_json::from_value(json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 32000,
            "system": [{"type": "text", "text": "Be terse."}, {"type": "text", "text": "Use tools."}],
            "thinking": {"type": "enabled", "budget_tokens": 31999},
            "tools": [{"name": "Bash", "description": "run", "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}}],
            "tool_choice": {"type": "any"},
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "list files"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "QUJD"}}
                ]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "use ls", "signature": "bro.rs:ENC123"},
                    {"type": "thinking", "thinking": "anthropic", "signature": "EqQBCkgIBRAB"},
                    {"type": "text", "text": "Running ls."},
                    {"type": "tool_use", "id": "call_1", "name": "Bash", "input": {"command": "ls"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": [{"type": "text", "text": "a.txt"}]},
                    {"type": "text", "text": "and?"}
                ]}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn translates_request_for_chatgpt() {
        let t = Target {
            model: "gpt-5.5".into(),
            chatgpt: true,
            cache_key: Some("sess".into()),
            supported_efforts: vec!["low".into(), "medium".into(), "high".into()],
            ..Default::default()
        };
        let v = serde_json::to_value(request(&fixture(), &t)).unwrap();
        assert_eq!(v["instructions"], "Be terse.\n\nUse tools.");
        assert_eq!(v["reasoning"], json!({"effort": "high", "summary": "auto"}));
        assert_eq!(v["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(v["store"], false);
        assert_eq!(v["stream"], true);
        assert_eq!(v["prompt_cache_key"], "sess");
        assert!(v.get("max_output_tokens").is_none());
        assert_eq!(v["tool_choice"], "required");
        assert_eq!(v["parallel_tool_calls"], true);
        assert_eq!(v["tools"][0]["parameters"]["required"], json!(["command"]));
        let input = v["input"].as_array().unwrap();
        assert_eq!(
            input[0]["content"][1],
            json!({"type": "input_image", "image_url": "data:image/png;base64,QUJD"})
        );
        assert_eq!(
            input[1],
            json!({"type": "reasoning", "summary": [{"type": "summary_text", "text": "use ls"}], "encrypted_content": "ENC123"})
        );
        assert_eq!(
            input[2],
            json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Running ls."}]})
        );
        assert_eq!(input[3]["type"], "function_call");
        assert_eq!(input[3]["arguments"], "{\"command\":\"ls\"}");
        assert_eq!(
            input[4],
            json!({"type": "function_call_output", "call_id": "call_1", "output": "a.txt"})
        );
        assert_eq!(input[5]["content"][0]["text"], "and?");
        assert_eq!(input.len(), 6);
    }

    #[test]
    fn plain_openai_model_gets_sampling_and_max() {
        let mut r = fixture();
        r.thinking = None;
        r.temperature = Some(0.1);
        let t = Target {
            model: "gpt-4.1".into(),
            ..Default::default()
        };
        let v = serde_json::to_value(request(&r, &t)).unwrap();
        assert!(v.get("reasoning").is_none());
        assert_eq!(v["temperature"], 0.1);
        assert_eq!(v["max_output_tokens"], 32000);
        assert!(v.get("instructions").is_some());
    }

    #[test]
    fn translates_response() {
        let resp = json!({
            "id": "resp_abc", "status": "completed",
            "output": [
                {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "a"}, {"type": "summary_text", "text": "b"}], "encrypted_content": "E1"},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Done", "annotations": []}]},
                {"type": "function_call", "call_id": "call_z", "name": "Bash", "arguments": "{\"command\":\"pwd\"}"}
            ],
            "usage": {"input_tokens": 50, "input_tokens_details": {"cached_tokens": 20}, "output_tokens": 9, "total_tokens": 59}
        });
        let m = serde_json::to_value(response(&resp, "claude-sonnet-4-5")).unwrap();
        assert_eq!(m["id"], "msg_abc");
        assert_eq!(
            m["content"][0],
            json!({"type": "thinking", "thinking": "a\n\nb", "signature": "bro.rs:E1"})
        );
        assert_eq!(m["content"][1]["text"], "Done");
        assert_eq!(m["content"][2]["input"]["command"], "pwd");
        assert_eq!(m["stop_reason"], "tool_use");
        assert_eq!(
            m["usage"],
            json!({"input_tokens": 30, "output_tokens": 9, "cache_read_input_tokens": 20})
        );
        let inc = json!({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": []});
        assert_eq!(
            response(&inc, "m").stop_reason.as_deref(),
            Some("max_tokens")
        );
    }
}
