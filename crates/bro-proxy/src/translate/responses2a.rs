//! OpenAI Responses client (Codex CLI…) → Anthropic Messages upstream.
//! Request: Responses → Anthropic. Response: Anthropic → Responses.

use super::chat2a::{file_block, finalize_messages, push_blocks};
use super::{
    ENC_ANTHROPIC, ENC_REDACTED, Target, apply_effort_to_anthropic, default_max_tokens, schema,
};
use crate::anthropic::{
    ContentBlock, ImageSource, MessagesRequest, MessagesResponse, Role, SystemPrompt, Tool,
    ToolChoice, ToolResultContent, parse_tool_input,
};
use crate::openai::{ResponsesRequest, item_text, str_field};
use crate::util::{gen_id, now_secs};
use serde_json::{Value, json};
use std::collections::HashSet;

/// Names of Responses `custom` (freeform) tools; their calls carry a raw string.
pub fn custom_tool_names(req: &ResponsesRequest) -> HashSet<String> {
    req.tools
        .iter()
        .flatten()
        .filter(|t| str_field(t, "type") == Some("custom"))
        .filter_map(|t| str_field(t, "name").map(str::to_string))
        .collect()
}

pub fn request(req: &ResponsesRequest, t: &Target) -> MessagesRequest {
    let mut system: Vec<String> = req
        .instructions
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect();
    let mut msgs = Vec::new();

    for item in req.input_items() {
        match str_field(&item, "type") {
            Some("message") => {
                let role = str_field(&item, "role").unwrap_or("user");
                match role {
                    "system" | "developer" => {
                        let s = item_text(&item);
                        if !s.is_empty() {
                            system.push(s);
                        }
                    }
                    "assistant" => {
                        let text = item_text(&item);
                        push_blocks(&mut msgs, Role::Assistant, vec![ContentBlock::text(text)]);
                    }
                    _ => {
                        let parts = item
                            .get("content")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        push_blocks(
                            &mut msgs,
                            Role::User,
                            parts.iter().filter_map(input_part).collect(),
                        );
                    }
                }
            }
            Some("reasoning") => {
                if let Some(block) = reasoning_block(&item) {
                    push_blocks(&mut msgs, Role::Assistant, vec![block]);
                }
            }
            Some("function_call") => push_blocks(
                &mut msgs,
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: call_id(&item),
                    name: str_field(&item, "name").unwrap_or_default().to_string(),
                    input: parse_tool_input(str_field(&item, "arguments").unwrap_or("")),
                    cache_control: None,
                }],
            ),
            Some("custom_tool_call") => push_blocks(
                &mut msgs,
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: call_id(&item),
                    name: str_field(&item, "name").unwrap_or_default().to_string(),
                    input: json!({"input": str_field(&item, "input").unwrap_or("")}),
                    cache_control: None,
                }],
            ),
            Some("function_call_output") | Some("custom_tool_call_output") => {
                let content = match item.get("output") {
                    Some(Value::String(s)) if !s.is_empty() => {
                        Some(ToolResultContent::Text(s.clone()))
                    }
                    Some(Value::Array(parts)) => {
                        let b: Vec<ContentBlock> = parts.iter().filter_map(input_part).collect();
                        (!b.is_empty()).then_some(ToolResultContent::Blocks(b))
                    }
                    // Codex's older shape: {content, success}
                    Some(Value::Object(o)) => o
                        .get("content")
                        .and_then(Value::as_str)
                        .map(|s| ToolResultContent::Text(s.to_string())),
                    _ => None,
                };
                let is_error = item
                    .get("output")
                    .and_then(|o| o.get("success"))
                    .and_then(Value::as_bool)
                    == Some(false);
                push_blocks(
                    &mut msgs,
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: call_id(&item),
                        content,
                        is_error,
                        cache_control: None,
                    }],
                );
            }
            _ => {} // item_reference, web_search_call, local_shell_call…: not representable
        }
    }
    finalize_messages(&mut msgs);

    let mut tools = Vec::new();
    for tool in req.tools.iter().flatten() {
        match str_field(tool, "type") {
            Some("function") => {
                let (name, desc, params) = match tool.get("function") {
                    // tolerate Chat-style nesting
                    Some(f) => (
                        str_field(f, "name"),
                        str_field(f, "description"),
                        f.get("parameters"),
                    ),
                    None => (
                        str_field(tool, "name"),
                        str_field(tool, "description"),
                        tool.get("parameters"),
                    ),
                };
                if let Some(name) = name {
                    tools.push(Tool {
                        name: name.to_string(),
                        description: desc.map(str::to_string),
                        input_schema: Some(schema::clean(params, false)),
                        ..Default::default()
                    });
                }
            }
            Some("custom") => {
                if let Some(name) = str_field(tool, "name") {
                    let mut desc = str_field(tool, "description")
                        .unwrap_or_default()
                        .to_string();
                    if let Some(def) = tool.get("format").and_then(|f| str_field(f, "definition")) {
                        desc.push_str("\n\nThe `input` string must follow this grammar:\n");
                        desc.push_str(def);
                    }
                    tools.push(Tool {
                        name: name.to_string(),
                        description: Some(desc),
                        input_schema: Some(schema::custom_tool_schema()),
                        ..Default::default()
                    });
                }
            }
            _ => {} // hosted tools (web_search, file_search…) are not available on Anthropic
        }
    }

    let dp = (req.parallel_tool_calls == Some(false)).then_some(true);
    let tool_choice = if tools.is_empty() {
        None
    } else {
        match &req.tool_choice {
            Some(Value::String(s)) if s == "required" => Some(ToolChoice::Any {
                disable_parallel_tool_use: dp,
            }),
            Some(Value::String(s)) if s == "none" => Some(ToolChoice::None),
            Some(Value::Object(o)) if o.get("name").is_some() => Some(ToolChoice::Tool {
                name: o
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                disable_parallel_tool_use: dp,
            }),
            _ => dp.map(|_| ToolChoice::Auto {
                disable_parallel_tool_use: dp,
            }),
        }
    };

    let session = req
        .prompt_cache_key
        .clone()
        .or_else(|| req.user.clone())
        .or_else(|| {
            req.metadata
                .as_ref()
                .and_then(|m| str_field(m, "user_id"))
                .map(str::to_string)
        });
    let mut out = MessagesRequest {
        model: t.model.clone(),
        messages: msgs,
        system: (!system.is_empty()).then(|| SystemPrompt::Text(system.join("\n\n"))),
        max_tokens: Some(
            req.max_output_tokens
                .unwrap_or_else(|| default_max_tokens(&t.model)),
        ),
        metadata: session.map(|s| json!({"user_id": s})),
        stop_sequences: None,
        stream: req.stream,
        temperature: req.temperature.map(|x| x.clamp(0.0, 1.0)),
        top_p: req.top_p,
        top_k: None,
        tools: (!tools.is_empty()).then_some(tools),
        tool_choice,
        thinking: None,
        extra: Default::default(),
    };
    let effort = t.effort_override.clone().or_else(|| req.effort());
    apply_effort_to_anthropic(&mut out, effort.as_deref(), req.max_output_tokens.is_some());
    super::drop_thinking_if_unsigned(&mut out);
    out
}

fn call_id(item: &Value) -> String {
    str_field(item, "call_id")
        .or_else(|| str_field(item, "id"))
        .map(str::to_string)
        .unwrap_or_else(|| gen_id("call_"))
}

fn input_part(p: &Value) -> Option<ContentBlock> {
    match str_field(p, "type")? {
        "input_text" | "output_text" | "text" => Some(ContentBlock::text(
            str_field(p, "text").unwrap_or_default().to_string(),
        )),
        "input_image" => {
            let url = str_field(p, "image_url")
                .or_else(|| p.get("image_url").and_then(|u| str_field(u, "url")))?;
            Some(ContentBlock::Image {
                source: ImageSource::from_url(url),
                cache_control: None,
            })
        }
        "input_file" => file_block(p),
        _ => None,
    }
}

/// Our encoded reasoning items go back to Anthropic as signed thinking; foreign
/// (real OpenAI) reasoning cannot be verified by Anthropic and is dropped.
fn reasoning_block(item: &Value) -> Option<ContentBlock> {
    let enc = str_field(item, "encrypted_content")?;
    if let Some(data) = enc.strip_prefix(ENC_REDACTED) {
        return Some(ContentBlock::RedactedThinking {
            data: data.to_string(),
        });
    }
    let sig = enc.strip_prefix(ENC_ANTHROPIC)?;
    let text = item
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| str_field(p, "text"))
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(ContentBlock::Thinking {
        thinking: text,
        signature: Some(sig.to_string()),
    })
}

// ------------------------------------------------------------------ response

/// Output items for a complete Anthropic message.
pub fn output_items(m: &MessagesResponse, custom: &HashSet<String>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut texts: Vec<Value> = Vec::new();
    let flush = |out: &mut Vec<Value>, texts: &mut Vec<Value>| {
        if !texts.is_empty() {
            out.push(json!({
                "id": gen_id("msg_"), "type": "message", "status": "completed", "role": "assistant",
                "content": std::mem::take(texts)
            }));
        }
    };
    for b in &m.content {
        match b {
            ContentBlock::Text { text, .. } => {
                texts.push(json!({"type": "output_text", "text": text, "annotations": []}))
            }
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                flush(&mut out, &mut texts);
                out.push(reasoning_item(
                    &gen_id("rs_"),
                    thinking,
                    signature.as_deref(),
                ));
            }
            ContentBlock::RedactedThinking { data } => {
                flush(&mut out, &mut texts);
                out.push(json!({
                    "id": gen_id("rs_"), "type": "reasoning", "summary": [],
                    "encrypted_content": format!("{ENC_REDACTED}{data}")
                }));
            }
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                flush(&mut out, &mut texts);
                out.push(tool_item(&gen_id("fc_"), id, name, input, custom));
            }
            _ => {}
        }
    }
    flush(&mut out, &mut texts);
    out
}

pub fn reasoning_item(id: &str, thinking: &str, signature: Option<&str>) -> Value {
    let mut item = json!({
        "id": id, "type": "reasoning",
        "summary": if thinking.is_empty() { json!([]) } else { json!([{"type": "summary_text", "text": thinking}]) },
    });
    if let Some(sig) = signature.filter(|s| !s.is_empty() && !s.starts_with("bro.")) {
        item["encrypted_content"] = json!(format!("{ENC_ANTHROPIC}{sig}"));
    }
    item
}

pub fn tool_item(
    item_id: &str,
    call_id: &str,
    name: &str,
    input: &Value,
    custom: &HashSet<String>,
) -> Value {
    if custom.contains(name) {
        let raw = input
            .get("input")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| input.to_string());
        json!({"id": item_id, "type": "custom_tool_call", "status": "completed", "call_id": call_id, "name": name, "input": raw})
    } else {
        json!({
            "id": item_id, "type": "function_call", "status": "completed", "call_id": call_id, "name": name,
            "arguments": serde_json::to_string(input).unwrap_or_default()
        })
    }
}

/// The `response` object (used for JSON responses and response.completed).
pub fn response_object(
    id: &str,
    model: &str,
    status: &str,
    output: Vec<Value>,
    m: &MessagesResponse,
) -> Value {
    let mut r = json!({
        "id": id,
        "object": "response",
        "created_at": now_secs(),
        "status": status,
        "model": model,
        "output": output,
        "parallel_tool_calls": true,
        "tool_choice": "auto",
        "tools": [],
        "usage": super::responses_usage_json(&m.usage),
        "error": null,
        "incomplete_details": null,
    });
    if status == "incomplete" {
        r["incomplete_details"] = json!({"reason": "max_output_tokens"});
    }
    r
}

pub fn response(m: &MessagesResponse, client_model: &str, custom: &HashSet<String>) -> Value {
    let status = if m.stop_reason.as_deref() == Some("max_tokens") {
        "incomplete"
    } else {
        "completed"
    };
    let id = format!("resp_{}", m.id.trim_start_matches("msg_"));
    response_object(&id, client_model, status, output_items(m, custom), m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::ResponsesRequest;

    fn fixture() -> ResponsesRequest {
        serde_json::from_value(json!({
            "model": "claude-opus-4-5",
            "instructions": "You are Codex.",
            "stream": true,
            "store": false,
            "prompt_cache_key": "conv-1",
            "reasoning": {"effort": "medium", "summary": "auto"},
            "parallel_tool_calls": false,
            "tool_choice": "auto",
            "include": ["reasoning.encrypted_content"],
            "tools": [
                {"type": "function", "name": "shell", "description": "run", "strict": false,
                 "parameters": {"type": "object", "properties": {"command": {"type": "array", "items": {"type": "string"}}}}},
                {"type": "custom", "name": "apply_patch", "description": "patch", "format": {"type": "grammar", "syntax": "lark", "definition": "start: x"}},
                {"type": "web_search"}
            ],
            "input": [
                {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "env ctx"}]},
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "fix bug"},
                    {"type": "input_image", "image_url": "data:image/png;base64,AAA"}
                ]},
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "look"}], "encrypted_content": "bro.at:SIG1"},
                {"type": "reasoning", "summary": [], "encrypted_content": "gAAAA-foreign"},
                {"type": "function_call", "call_id": "toolu_1", "name": "shell", "arguments": "{\"command\":[\"ls\"]}"},
                {"type": "custom_tool_call", "call_id": "toolu_2", "name": "apply_patch", "input": "*** Begin Patch"},
                {"type": "function_call_output", "call_id": "toolu_1", "output": "a.rs"},
                {"type": "custom_tool_call_output", "call_id": "toolu_2", "output": "Done!"},
                {"role": "user", "content": "thanks"}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn translates_codex_request() {
        let req = fixture();
        let t = Target {
            model: "claude-opus-4-5".into(),
            ..Default::default()
        };
        let v = serde_json::to_value(request(&req, &t)).unwrap();
        assert_eq!(v["system"], "You are Codex.\n\nenv ctx");
        assert_eq!(v["metadata"]["user_id"], "conv-1");
        assert_eq!(
            v["thinking"],
            json!({"type": "enabled", "budget_tokens": 8192})
        );
        assert_eq!(
            v["tool_choice"],
            json!({"type": "auto", "disable_parallel_tool_use": true})
        );
        assert_eq!(v["tools"].as_array().unwrap().len(), 2);
        assert_eq!(v["tools"][1]["input_schema"]["required"], json!(["input"]));
        assert!(
            v["tools"][1]["description"]
                .as_str()
                .unwrap()
                .contains("start: x")
        );
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["content"][1]["source"]["data"], "AAA");
        assert_eq!(
            msgs[1]["content"][0],
            json!({"type": "thinking", "thinking": "look", "signature": "SIG1"})
        );
        assert_eq!(msgs[1]["content"][1]["input"], json!({"command": ["ls"]}));
        assert_eq!(
            msgs[1]["content"][2]["input"],
            json!({"input": "*** Begin Patch"})
        );
        assert_eq!(msgs[1]["content"].as_array().unwrap().len(), 3);
        assert_eq!(
            msgs[2]["content"][0],
            json!({"type": "tool_result", "tool_use_id": "toolu_1", "content": "a.rs"})
        );
        assert_eq!(msgs[2]["content"][1]["tool_use_id"], "toolu_2");
        assert_eq!(msgs[2]["content"][2]["text"], "thanks");
        assert_eq!(
            custom_tool_names(&req),
            HashSet::from(["apply_patch".to_string()])
        );
    }

    #[test]
    fn unsigned_tool_turn_disables_thinking() {
        let mut req = fixture();
        if let Value::Array(items) = &mut req.input {
            items.remove(2);
        }
        let v = serde_json::to_value(request(&req, &Target::default())).unwrap();
        assert!(v.get("thinking").is_none());
    }

    #[test]
    fn translates_response() {
        let m: MessagesResponse = serde_json::from_value(json!({
            "id": "msg_42", "model": "claude", "type": "message", "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "SIGX"},
                {"type": "redacted_thinking", "data": "RD"},
                {"type": "text", "text": "Patching."},
                {"type": "tool_use", "id": "toolu_5", "name": "apply_patch", "input": {"input": "*** Begin Patch"}},
                {"type": "tool_use", "id": "toolu_6", "name": "shell", "input": {"command": ["ls"]}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 3, "output_tokens": 4, "cache_read_input_tokens": 10}
        }))
        .unwrap();
        let custom = HashSet::from(["apply_patch".to_string()]);
        let r = response(&m, "claude-opus-4-5", &custom);
        assert_eq!(r["id"], "resp_42");
        assert_eq!(r["status"], "completed");
        let out = r["output"].as_array().unwrap();
        assert_eq!(out[0]["encrypted_content"], "bro.at:SIGX");
        assert_eq!(out[0]["summary"][0]["text"], "hmm");
        assert_eq!(out[1]["encrypted_content"], "bro.rd:RD");
        assert_eq!(out[2]["content"][0]["text"], "Patching.");
        assert_eq!(out[3]["type"], "custom_tool_call");
        assert_eq!(out[3]["input"], "*** Begin Patch");
        assert_eq!(out[4]["type"], "function_call");
        assert_eq!(out[4]["call_id"], "toolu_6");
        assert_eq!(r["usage"]["input_tokens"], 13);
        assert_eq!(r["usage"]["input_tokens_details"]["cached_tokens"], 10);
    }
}
