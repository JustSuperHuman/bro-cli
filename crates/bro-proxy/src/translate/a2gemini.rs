//! Anthropic Messages (hub) → Gemini `generateContent`, the `request` inside Cloud Code
//! Assist's envelope (the envelope itself is built in `upstream::gemini`).
//!
//! Thought signatures: Gemini signs its reasoning and wants the signature back on the first
//! functionCall of each model turn (on text parts it's optional). The stream decoder parks
//! each one in a thinking block (`bro.gs:{sig}`) in front of the part it came with; here the
//! one before the first call goes back onto it. OpenAI clients hoist reasoning to the front
//! of a turn, so any signature in the turn is the fallback. A lost one (an older transcript,
//! a compacted history) is replaced by Google's documented `skip_thought_signature_validator`.

use super::{SIG_GEMINI, Target, anthropic_effort};
use crate::anthropic::{ContentBlock, ImageSource, MessagesRequest, Role, ToolResultContent};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Stand-in signature Gemini accepts when the real one is gone.
pub const SKIP_SIGNATURE: &str = "skip_thought_signature_validator";
/// Shorter "signatures" are placeholders, not something Gemini issued.
const MIN_SIGNATURE: usize = 50;
const EMPTY_TOOL_OUTPUT: &str = "(empty tool output)";
const MISSING_TOOL_RESULT: &str = "[no tool result was recorded for this call]";
/// Argon's output ceiling.
const MAX_OUTPUT: u64 = 65536;

/// Argon is served per thinking level (`gemini-4-argon-low|-medium|-high`); the bare id 404s.
const LEVELED: &[&str] = &["gemini-4-argon"];
const LEVELS: [&str; 3] = ["low", "medium", "high"];

/// The model id on the wire: a leveled model gets the level from its own suffix, else from
/// the client's effort (Claude Code's thinking budget, Codex's reasoning effort), else medium.
pub fn wire_model(model: &str, effort: Option<&str>) -> String {
    let m = model.trim();
    if let Some((base, level)) = m.rsplit_once('-')
        && LEVELED.contains(&base)
        && LEVELS.contains(&level)
    {
        return m.to_string();
    }
    if !LEVELED.contains(&m) {
        return m.to_string();
    }
    let level = match effort.unwrap_or("medium") {
        "none" | "minimal" | "low" => "low",
        "high" | "xhigh" | "max" => "high",
        _ => "medium",
    };
    format!("{m}-{level}")
}

/// `LOW` / `MEDIUM` / `HIGH` for a leveled wire id.
fn thinking_level(wire: &str) -> Option<&'static str> {
    let (base, level) = wire.rsplit_once('-')?;
    if !LEVELED.contains(&base) {
        return None;
    }
    Some(match level {
        "low" => "LOW",
        "medium" => "MEDIUM",
        "high" => "HIGH",
        _ => return None,
    })
}

/// (wire model, inner request)
pub fn request(req: &MessagesRequest, t: &Target) -> (String, Value) {
    let effort = t.effort_override.clone().or_else(|| anthropic_effort(req));
    let wire = wire_model(&t.model, effort.as_deref());

    // functionResponse needs the function's name; tool_result only has the id.
    let names: HashMap<String, String> = req
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| m.content.blocks())
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, .. } => Some((id, name)),
            _ => None,
        })
        .collect();

    let mut contents: Vec<Value> = Vec::new();
    // Calls of the last model turn still waiting for a result.
    let mut open_calls: Vec<(String, String)> = Vec::new();
    for m in &req.messages {
        let blocks = m.content.blocks();
        match m.role {
            Role::User => {
                let parts = user_parts(blocks, &names, &mut open_calls);
                push_turn(&mut contents, "user", parts);
            }
            Role::Assistant => {
                close_open_calls(&mut contents, &mut open_calls);
                let (parts, calls) = model_parts(blocks);
                open_calls = calls;
                push_turn(&mut contents, "model", parts);
            }
        }
    }
    close_open_calls(&mut contents, &mut open_calls);
    // A history may not open on a function call, nor end on a model turn (prefill).
    if contents.first().is_some_and(|c| c["role"] == "model") {
        contents.insert(0, json!({ "role": "user", "parts": [{ "text": "(continue)" }] }));
    }
    if contents.last().is_none_or(|c| c["role"] == "model") {
        contents.push(json!({ "role": "user", "parts": [{ "text": "(continue)" }] }));
    }

    let mut out = Map::new();
    out.insert("contents".into(), Value::Array(contents));
    let system = req.system_text();
    if !system.is_empty() {
        out.insert("systemInstruction".into(), json!({ "role": "user", "parts": [{ "text": system }] }));
    }
    let decls: Vec<Value> = req
        .tools
        .iter()
        .flatten()
        .filter(|tool| tool.is_custom())
        .map(|tool| {
            let mut d = json!({ "name": tool.name, "parameters": gemini_schema(tool.input_schema.as_ref()) });
            if let Some(desc) = tool.description.as_deref().filter(|d| !d.is_empty()) {
                d["description"] = json!(desc);
            }
            d
        })
        .collect();
    if !decls.is_empty() {
        out.insert("tools".into(), json!([{ "functionDeclarations": decls }]));
    }

    let mut config = Map::new();
    config.insert("maxOutputTokens".into(), json!(req.max_tokens.unwrap_or(MAX_OUTPUT).min(MAX_OUTPUT)));
    if let Some(s) = req.stop_sequences.as_ref().filter(|s| !s.is_empty()) {
        config.insert("stopSequences".into(), json!(s));
    }
    let mut thinking = json!({ "includeThoughts": true });
    match thinking_level(&wire) {
        Some(level) => thinking["thinkingLevel"] = json!(level),
        // Thinking models other than Argon: sampling stays the model's own.
        None => {
            if let Some(v) = req.temperature {
                config.insert("temperature".into(), json!(v));
            }
            if let Some(v) = req.top_p {
                config.insert("topP".into(), json!(v));
            }
        }
    }
    config.insert("thinkingConfig".into(), thinking);
    out.insert("generationConfig".into(), Value::Object(config));
    if let Some(key) = t.cache_key.as_deref() {
        out.insert("sessionId".into(), json!(key));
    }
    (wire, Value::Object(out))
}

/// Append a turn, merging into the previous one when it has the same role.
fn push_turn(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut()
        && last["role"] == role
        && let Some(p) = last["parts"].as_array_mut()
    {
        // function responses lead a user turn
        if role == "user" && parts.iter().any(|p| p.get("functionResponse").is_some()) {
            let mut merged = parts;
            merged.append(p);
            *p = merged;
        } else {
            p.extend(parts);
        }
        return;
    }
    contents.push(json!({ "role": role, "parts": parts }));
}

/// Calls of a model turn the client never answered: Gemini needs one response per call.
fn close_open_calls(contents: &mut Vec<Value>, open: &mut Vec<(String, String)>) {
    if open.is_empty() {
        return;
    }
    let parts = open.drain(..).map(|(id, name)| function_response(&id, &name, MISSING_TOOL_RESULT, false, vec![])).collect();
    push_turn(contents, "user", parts);
}

fn function_response(id: &str, name: &str, text: &str, is_error: bool, images: Vec<Value>) -> Value {
    let mut fr = json!({
        "id": id,
        "name": name,
        "response": if is_error { json!({ "error": text }) } else { json!({ "result": text }) },
    });
    // Argon rejects image parts beside a functionResponse: they go inside it.
    if !images.is_empty() {
        fr["parts"] = Value::Array(images);
    }
    json!({ "functionResponse": fr })
}

fn image_part(source: &ImageSource) -> Option<Value> {
    match source {
        ImageSource::Base64 { media_type, data } => Some(json!({ "inlineData": { "mimeType": media_type, "data": data } })),
        ImageSource::Url { url } => Some(json!({ "text": format!("[image: {url}]") })),
        ImageSource::Unknown => None,
    }
}

fn document_part(source: &Value, title: Option<&str>) -> Option<Value> {
    let kind = source.get("type").and_then(Value::as_str);
    let media = source.get("media_type").and_then(Value::as_str).unwrap_or("application/pdf");
    let text = match kind {
        Some("base64") => {
            let data = source.get("data").and_then(Value::as_str)?;
            return Some(json!({ "inlineData": { "mimeType": media, "data": data } }));
        }
        Some("text") => source.get("data").and_then(Value::as_str)?.to_string(),
        Some("content") => source.get("content").and_then(Value::as_str)?.to_string(),
        Some("url") => format!("Document: {}", source.get("url").and_then(Value::as_str)?),
        _ => format!("[document omitted: {}]", title.unwrap_or("attachment")),
    };
    Some(json!({ "text": match title { Some(t) => format!("# {t}\n\n{text}"), None => text } }))
}

fn user_parts(blocks: Vec<ContentBlock>, names: &HashMap<String, String>, open: &mut Vec<(String, String)>) -> Vec<Value> {
    let mut responses = Vec::new();
    let mut parts = Vec::new();
    for b in blocks {
        match b {
            ContentBlock::Text { text, .. } if !text.is_empty() => parts.push(json!({ "text": text })),
            ContentBlock::Image { source, .. } => parts.extend(image_part(&source)),
            ContentBlock::Document { source, title, .. } => parts.extend(document_part(&source, title.as_deref())),
            ContentBlock::ToolResult { tool_use_id, content, is_error, .. } => {
                let name = names.get(&tool_use_id).cloned().unwrap_or_else(|| "unknown_function".into());
                open.retain(|(id, _)| *id != tool_use_id);
                let mut text = content.as_ref().map(ToolResultContent::text).unwrap_or_default();
                let images: Vec<Value> = content.as_ref().map(|c| c.images()).unwrap_or_default().iter().filter_map(|s| match s {
                    ImageSource::Base64 { .. } => image_part(s),
                    _ => None,
                }).collect();
                if text.is_empty() {
                    text = if images.is_empty() { EMPTY_TOOL_OUTPUT.into() } else { "(see the attached image)".into() };
                }
                responses.push(function_response(&tool_use_id, &name, &text, is_error, images));
            }
            _ => {}
        }
    }
    // Results the model is still waiting on would otherwise be missing.
    responses.extend(open.drain(..).map(|(id, name)| function_response(&id, &name, MISSING_TOOL_RESULT, false, vec![])));
    responses.extend(parts);
    responses
}

/// A real Gemini signature parked in a thinking block.
fn gemini_signature(sig: Option<&str>) -> Option<String> {
    sig?.strip_prefix(SIG_GEMINI).filter(|s| s.len() >= MIN_SIGNATURE).map(str::to_string)
}

/// (parts, the turn's function calls as (id, name))
fn model_parts(blocks: Vec<ContentBlock>) -> (Vec<Value>, Vec<(String, String)>) {
    let mut parts = Vec::new();
    let mut calls = Vec::new();
    // the latest signature so far, and the latest in the whole turn
    let mut before: Option<String> = None;
    let mut any: Option<String> = None;
    let mut first_call: Option<(usize, Option<String>)> = None;
    for b in blocks {
        match b {
            ContentBlock::Thinking { signature, .. } => {
                if let Some(sig) = gemini_signature(signature.as_deref()) {
                    before = Some(sig.clone());
                    any = Some(sig);
                }
            }
            ContentBlock::Text { text, .. } if !text.is_empty() => parts.push(json!({ "text": text })),
            ContentBlock::ToolUse { id, name, input, .. } => {
                let args = if input.is_object() { input } else { json!({}) };
                if first_call.is_none() {
                    first_call = Some((parts.len(), before.clone()));
                }
                parts.push(json!({ "functionCall": { "id": id, "name": name, "args": args } }));
                calls.push((id, name));
            }
            _ => {}
        }
    }
    if let Some((at, sig)) = first_call {
        parts[at]["thoughtSignature"] = json!(sig.or(any).unwrap_or_else(|| SKIP_SIGNATURE.into()));
    }
    (parts, calls)
}

// ------------------------------------------------------------------ tool schemas

/// Keywords Gemini's function schema accepts (everything else 400s or is noise).
const KEEP: &[&str] = &["type", "nullable", "description", "format", "enum", "properties", "items", "required", "anyOf", "minimum", "maximum", "minLength", "maxLength", "minItems", "maxItems"];
const TYPES: &[&str] = &["string", "integer", "number", "boolean", "array", "object"];
const MAX_DEPTH: usize = 24;

/// A tool's JSON schema in the subset Gemini accepts: local `$ref`s inlined, `allOf` merged,
/// `oneOf` as `anyOf`, type unions as `nullable`, `const` as a one-value enum. Always an object.
pub fn gemini_schema(schema: Option<&Value>) -> Value {
    let root = schema.cloned().unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let mut out = sanitize(&root, &root, 0);
    if out.get("type").and_then(Value::as_str) != Some("object") {
        out = json!({ "type": "object", "properties": { "input": out } });
    }
    if out.get("properties").is_none() {
        out["properties"] = json!({});
    }
    out
}

fn resolve<'a>(root: &'a Value, r: &str) -> Option<&'a Value> {
    let path = r.strip_prefix("#/")?;
    path.split('/').try_fold(root, |v, seg| v.get(seg.replace("~1", "/").replace("~0", "~")))
}

fn sanitize(v: &Value, root: &Value, depth: usize) -> Value {
    let Some(obj) = v.as_object() else { return json!({ "type": "string" }) };
    if depth > MAX_DEPTH {
        return json!({ "type": "object" });
    }
    if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
        return match resolve(root, r) {
            Some(target) => {
                let mut s = sanitize(target, root, depth + 1);
                if let Some(d) = obj.get("description") {
                    s["description"] = d.clone();
                }
                s
            }
            None => json!({ "type": "object" }),
        };
    }
    let mut src = obj.clone();
    // allOf: fold every branch into this schema.
    if let Some(Value::Array(all)) = src.remove("allOf") {
        for branch in all {
            let b = sanitize(&branch, root, depth + 1);
            let Some(b) = b.as_object() else { continue };
            for (k, val) in b {
                match (k.as_str(), src.get_mut(k)) {
                    ("properties", Some(Value::Object(p))) => {
                        if let Value::Object(more) = val {
                            p.extend(more.clone());
                        }
                    }
                    ("required", Some(Value::Array(r))) => {
                        if let Value::Array(more) = val {
                            r.extend(more.iter().cloned());
                        }
                    }
                    (_, None) => {
                        src.insert(k.clone(), val.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(one) = src.remove("oneOf")
        && !src.contains_key("anyOf")
    {
        src.insert("anyOf".into(), one);
    }
    if let Some(c) = src.remove("const")
        && let Some(s) = c.as_str()
    {
        src.insert("enum".into(), json!([s]));
    }
    let mut out = Map::new();
    // type: a union with null becomes nullable; other unions keep their first type.
    match src.get("type") {
        Some(Value::String(t)) if TYPES.contains(&t.as_str()) => {
            out.insert("type".into(), json!(t));
        }
        Some(Value::Array(ts)) => {
            let names: Vec<&str> = ts.iter().filter_map(Value::as_str).collect();
            if names.contains(&"null") {
                out.insert("nullable".into(), json!(true));
            }
            if let Some(t) = names.iter().find(|t| TYPES.contains(t)) {
                out.insert("type".into(), json!(t));
            }
        }
        _ => {}
    }
    for (k, val) in &src {
        if !KEEP.contains(&k.as_str()) || k == "type" {
            continue;
        }
        match k.as_str() {
            "properties" => {
                let props: Map<String, Value> = val.as_object().into_iter().flatten().map(|(n, s)| (n.clone(), sanitize(s, root, depth + 1))).collect();
                out.insert(k.clone(), Value::Object(props));
            }
            "items" => {
                let items = match val {
                    Value::Array(a) => a.first().map(|s| sanitize(s, root, depth + 1)).unwrap_or_else(|| json!({ "type": "string" })),
                    other => sanitize(other, root, depth + 1),
                };
                out.insert(k.clone(), items);
            }
            "anyOf" => {
                let mut branches: Vec<Value> = Vec::new();
                for b in val.as_array().into_iter().flatten() {
                    if b.get("type").and_then(Value::as_str) == Some("null") {
                        out.insert("nullable".into(), json!(true));
                    } else {
                        branches.push(sanitize(b, root, depth + 1));
                    }
                }
                match branches.len() {
                    0 => {}
                    // a lone branch is the schema itself
                    1 => {
                        if let Value::Object(b) = branches.remove(0) {
                            for (bk, bv) in b {
                                out.entry(bk).or_insert(bv);
                            }
                        }
                    }
                    _ => {
                        out.insert(k.clone(), Value::Array(branches));
                    }
                }
            }
            "enum" => {
                let vals: Vec<&Value> = val.as_array().into_iter().flatten().filter(|v| !v.is_null()).collect();
                if !vals.is_empty() && vals.iter().all(|v| v.is_string()) {
                    out.insert(k.clone(), json!(vals));
                    out.insert("type".into(), json!("string"));
                }
            }
            "format" => {
                // Gemini only knows a few string formats.
                if matches!(val.as_str(), Some("enum" | "date-time" | "int32" | "int64" | "float" | "double")) {
                    out.insert(k.clone(), val.clone());
                }
            }
            "required" => {
                out.insert(k.clone(), val.clone());
            }
            _ => {
                out.insert(k.clone(), val.clone());
            }
        }
    }
    // required may only name properties that exist
    if let Some(Value::Array(req)) = out.get("required").cloned() {
        let props = out.get("properties").and_then(Value::as_object);
        let kept: Vec<Value> = req.into_iter().filter(|r| r.as_str().is_some_and(|n| props.is_some_and(|p| p.contains_key(n)))).collect();
        if kept.is_empty() {
            out.remove("required");
        } else {
            out.insert("required".into(), Value::Array(kept));
        }
    }
    if !out.contains_key("type") && !out.contains_key("anyOf") {
        let t = if out.contains_key("properties") { "object" } else if out.contains_key("items") { "array" } else { "string" };
        out.insert("type".into(), json!(t));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(v: Value) -> MessagesRequest {
        serde_json::from_value(v).unwrap()
    }

    fn target(model: &str) -> Target {
        Target { model: model.into(), cache_key: Some("sess".into()), ..Default::default() }
    }

    #[test]
    fn wire_model_levels() {
        assert_eq!(wire_model("gemini-4-argon", None), "gemini-4-argon-medium");
        assert_eq!(wire_model("gemini-4-argon", Some("xhigh")), "gemini-4-argon-high");
        assert_eq!(wire_model("gemini-4-argon", Some("low")), "gemini-4-argon-low");
        assert_eq!(wire_model("gemini-4-argon-high", Some("low")), "gemini-4-argon-high");
        assert_eq!(wire_model("gemini-3.7-flash", Some("high")), "gemini-3.7-flash");
        assert_eq!(thinking_level("gemini-4-argon-low"), Some("LOW"));
        assert_eq!(thinking_level("gemini-3.7-flash"), None);
    }

    #[test]
    fn tool_loop_with_signatures() {
        let sig = "S".repeat(80);
        let r = req(json!({
            "model": "gemini-4-argon",
            "max_tokens": 200000,
            "system": [{"type": "text", "text": "be brief"}],
            "thinking": {"type": "enabled", "budget_tokens": 30000},
            "tools": [{"name": "ls", "description": "list", "input_schema": {"type": "object", "properties": {"dir": {"type": ["string", "null"]}}, "required": ["dir", "ghost"], "additionalProperties": false}}],
            "messages": [
                {"role": "user", "content": "list it"},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "plan", "signature": format!("bro.gs:{sig}")},
                    {"type": "tool_use", "id": "toolu_1", "name": "ls", "input": {"dir": "."}},
                    {"type": "tool_use", "id": "toolu_2", "name": "ls", "input": {"dir": "src"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "a.rs"},
                    {"type": "tool_result", "tool_use_id": "toolu_2", "content": [{"type": "text", "text": ""}], "is_error": true}
                ]}
            ]
        }));
        let (wire, body) = request(&r, &target("gemini-4-argon"));
        assert_eq!(wire, "gemini-4-argon-high");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be brief");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 65536);
        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingLevel"], "HIGH");
        assert_eq!(body["sessionId"], "sess");
        let c = body["contents"].as_array().unwrap();
        assert_eq!(c.len(), 3);
        let model = &c[1]["parts"];
        assert_eq!(model[0]["functionCall"]["name"], "ls");
        assert_eq!(model[0]["thoughtSignature"], sig.as_str());
        assert!(model[1].get("thoughtSignature").is_none());
        assert_eq!(c[2]["parts"][0]["functionResponse"]["response"]["result"], "a.rs");
        assert_eq!(c[2]["parts"][1]["functionResponse"]["response"]["error"], EMPTY_TOOL_OUTPUT);
        let params = &body["tools"][0]["functionDeclarations"][0]["parameters"];
        assert_eq!(params["properties"]["dir"], json!({"nullable": true, "type": "string"}));
        assert_eq!(params["required"], json!(["dir"]));
        assert!(params.get("additionalProperties").is_none());
    }

    #[test]
    fn lost_signature_gets_the_skip_value_and_tail_is_user() {
        let r = req(json!({
            "model": "m", "max_tokens": 10,
            "messages": [
                {"role": "user", "content": "go"},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "x", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]},
                {"role": "assistant", "content": "partial"}
            ]
        }));
        let (_, body) = request(&r, &target("gemini-4-argon"));
        let c = body["contents"].as_array().unwrap();
        assert_eq!(c[1]["parts"][0]["thoughtSignature"], SKIP_SIGNATURE);
        assert_eq!(c.last().unwrap()["role"], "user");
    }

    #[test]
    fn unanswered_calls_get_a_response() {
        let r = req(json!({
            "model": "m", "max_tokens": 10,
            "messages": [
                {"role": "user", "content": "go"},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "x", "input": {}}]},
                {"role": "user", "content": "never mind"}
            ]
        }));
        let (_, body) = request(&r, &target("gemini-4-argon"));
        let parts = &body["contents"][2]["parts"];
        assert_eq!(parts[0]["functionResponse"]["id"], "t1");
        assert_eq!(parts[1]["text"], "never mind");
    }

    #[test]
    fn schema_refs_allof_and_const() {
        let s = json!({
            "$schema": "x",
            "$defs": {"Mode": {"type": "string", "enum": ["a", "b"]}},
            "type": "object",
            "properties": {
                "mode": {"$ref": "#/$defs/Mode", "description": "which"},
                "n": {"type": "integer", "exclusiveMinimum": 0},
                "kind": {"const": "fixed"},
                "opt": {"anyOf": [{"type": "string"}, {"type": "null"}]}
            },
            "allOf": [{"properties": {"extra": {"type": "boolean"}}, "required": ["extra"]}]
        });
        let out = gemini_schema(Some(&s));
        assert_eq!(out["properties"]["mode"], json!({"type": "string", "enum": ["a", "b"], "description": "which"}));
        assert_eq!(out["properties"]["n"], json!({"type": "integer"}));
        assert_eq!(out["properties"]["kind"], json!({"type": "string", "enum": ["fixed"]}));
        assert_eq!(out["properties"]["opt"], json!({"nullable": true, "type": "string"}));
        assert_eq!(out["properties"]["extra"], json!({"type": "boolean"}));
        assert_eq!(out["required"], json!(["extra"]));
        assert!(out.get("$schema").is_none() && out.get("$defs").is_none());
    }
}
