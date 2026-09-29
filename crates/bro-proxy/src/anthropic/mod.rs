//! Anthropic Messages dialect: types plus the two stream helpers every path shares —
//! folding a stream into one message (non-streaming clients on streaming upstreams)
//! and expanding a message into a stream (streaming clients on JSON upstreams).

pub mod types;

pub use types::*;

use serde_json::Value;

/// Newer Claude Code builds put `{"role":"system"}` entries inside `messages` (e.g. mid-
/// conversation reminders). The hub types only model user/assistant turns, so hoist those
/// entries into the top-level `system` blocks, in order. Passthrough requests are not touched.
pub fn hoist_system_messages(req: &mut Value) {
    let Some(messages) = req.get_mut("messages").and_then(Value::as_array_mut) else { return };
    if !messages.iter().any(|m| m.get("role").and_then(Value::as_str) == Some("system")) {
        return;
    }
    let mut hoisted = Vec::new();
    messages.retain(|m| {
        if m.get("role").and_then(Value::as_str) != Some("system") {
            return true;
        }
        match m.get("content") {
            Some(Value::String(t)) => hoisted.push(serde_json::json!({"type": "text", "text": t})),
            Some(Value::Array(blocks)) => hoisted.extend(
                blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).cloned(),
            ),
            _ => {}
        }
        false
    });
    if hoisted.is_empty() {
        return;
    }
    let mut system = match req.get_mut("system").map(Value::take) {
        Some(Value::String(t)) if !t.is_empty() => vec![serde_json::json!({"type": "text", "text": t})],
        Some(Value::Array(blocks)) => blocks,
        _ => Vec::new(),
    };
    system.extend(hoisted);
    req["system"] = Value::Array(system);
}

/// Folds Anthropic stream events into a complete [`MessagesResponse`].
#[derive(Debug, Default)]
pub struct Accumulator {
    msg: Option<MessagesResponse>,
    /// Raw partial JSON per tool_use block index
    json: Vec<(usize, String)>,
    pub error: Option<ErrorBody>,
    pub stopped: bool,
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, ev: &StreamEvent) {
        match ev {
            StreamEvent::MessageStart { message } => {
                let mut m = message.clone();
                m.content.clear();
                self.msg = Some(m);
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                let m = self
                    .msg
                    .get_or_insert_with(|| MessagesResponse::new("msg_unknown", ""));
                while m.content.len() < *index {
                    m.content.push(ContentBlock::text(""));
                }
                if m.content.len() == *index {
                    m.content.push(content_block.clone());
                } else {
                    m.content[*index] = content_block.clone();
                }
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                let Some(m) = self.msg.as_mut() else { return };
                let Some(block) = m.content.get_mut(*index) else {
                    return;
                };
                match (block, delta) {
                    (ContentBlock::Text { text, .. }, Delta::TextDelta { text: d }) => {
                        text.push_str(d)
                    }
                    (
                        ContentBlock::Thinking { thinking, .. },
                        Delta::ThinkingDelta { thinking: d },
                    ) => thinking.push_str(d),
                    (
                        ContentBlock::Thinking { signature, .. },
                        Delta::SignatureDelta { signature: s },
                    ) => signature.get_or_insert_with(String::new).push_str(s),
                    (ContentBlock::ToolUse { .. }, Delta::InputJsonDelta { partial_json }) => {
                        match self.json.iter_mut().find(|(i, _)| i == index) {
                            Some((_, buf)) => buf.push_str(partial_json),
                            None => self.json.push((*index, partial_json.clone())),
                        }
                    }
                    _ => {}
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                if let Some(pos) = self.json.iter().position(|(i, _)| i == index) {
                    let (_, raw) = self.json.remove(pos);
                    if let Some(ContentBlock::ToolUse { input, .. }) =
                        self.msg.as_mut().and_then(|m| m.content.get_mut(*index))
                    {
                        *input = parse_tool_input(&raw);
                    }
                }
            }
            StreamEvent::MessageDelta { delta, usage } => {
                let Some(m) = self.msg.as_mut() else { return };
                if delta.stop_reason.is_some() {
                    m.stop_reason = delta.stop_reason.clone();
                }
                if delta.stop_sequence.is_some() {
                    m.stop_sequence = delta.stop_sequence.clone();
                }
                merge_delta_usage(&mut m.usage, usage);
            }
            StreamEvent::MessageStop => self.stopped = true,
            StreamEvent::Error { error } => self.error = Some(error.clone()),
            StreamEvent::Ping | StreamEvent::Unknown => {}
        }
    }

    pub fn finish(mut self) -> Option<MessagesResponse> {
        let pending = std::mem::take(&mut self.json);
        let mut m = self.msg?;
        for (index, raw) in pending {
            if let Some(ContentBlock::ToolUse { input, .. }) = m.content.get_mut(index) {
                *input = parse_tool_input(&raw);
            }
        }
        Some(m)
    }
}

pub fn merge_delta_usage(u: &mut Usage, d: &DeltaUsage) {
    // message_delta usage is cumulative
    if d.output_tokens > 0 || u.output_tokens == 0 {
        u.output_tokens = d.output_tokens;
    }
    if let Some(v) = d.input_tokens {
        u.input_tokens = v;
    }
    if d.cache_creation_input_tokens.is_some() {
        u.cache_creation_input_tokens = d.cache_creation_input_tokens;
    }
    if d.cache_read_input_tokens.is_some() {
        u.cache_read_input_tokens = d.cache_read_input_tokens;
    }
}

/// Tool arguments as JSON; empty → `{}`; invalid JSON is kept as a string under `_raw`
/// so nothing the model produced is silently lost.
pub fn parse_tool_input(raw: &str) -> Value {
    let t = raw.trim();
    if t.is_empty() {
        return Value::Object(Default::default());
    }
    match serde_json::from_str::<Value>(t) {
        Ok(v @ Value::Object(_)) => v,
        Ok(other) => serde_json::json!({ "_raw": other }),
        Err(_) => serde_json::json!({ "_raw": raw }),
    }
}

/// Expand a complete message into the stream a real Anthropic server would send.
pub fn events_from_message(m: &MessagesResponse) -> Vec<StreamEvent> {
    let mut start = m.clone();
    start.content.clear();
    start.stop_reason = None;
    start.stop_sequence = None;
    let mut start_usage = m.usage.clone();
    start_usage.output_tokens = start_usage.output_tokens.min(1);
    start.usage = start_usage;
    let mut out = vec![
        StreamEvent::MessageStart { message: start },
        StreamEvent::Ping,
    ];
    for (index, block) in m.content.iter().enumerate() {
        let (start_block, deltas): (ContentBlock, Vec<Delta>) = match block {
            ContentBlock::Text { text, .. } => (
                ContentBlock::text(""),
                vec![Delta::TextDelta { text: text.clone() }],
            ),
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                let mut d = vec![Delta::ThinkingDelta {
                    thinking: thinking.clone(),
                }];
                d.push(Delta::SignatureDelta {
                    signature: signature.clone().unwrap_or_default(),
                });
                (
                    ContentBlock::Thinking {
                        thinking: String::new(),
                        signature: None,
                    },
                    d,
                )
            }
            ContentBlock::ToolUse {
                id, name, input, ..
            } => (
                ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: Value::Object(Default::default()),
                    cache_control: None,
                },
                vec![Delta::InputJsonDelta {
                    partial_json: serde_json::to_string(input).unwrap_or_default(),
                }],
            ),
            other => (other.clone(), vec![]),
        };
        out.push(StreamEvent::ContentBlockStart {
            index,
            content_block: start_block,
        });
        for delta in deltas {
            out.push(StreamEvent::ContentBlockDelta { index, delta });
        }
        out.push(StreamEvent::ContentBlockStop { index });
    }
    out.push(StreamEvent::MessageDelta {
        delta: MessageDeltaBody {
            stop_reason: m.stop_reason.clone(),
            stop_sequence: m.stop_sequence.clone(),
        },
        usage: DeltaUsage {
            output_tokens: m.usage.output_tokens,
            input_tokens: Some(m.usage.input_tokens),
            cache_creation_input_tokens: m.usage.cache_creation_input_tokens,
            cache_read_input_tokens: m.usage.cache_read_input_tokens,
        },
    });
    out.push(StreamEvent::MessageStop);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_message_through_events() {
        let m: MessagesResponse = serde_json::from_value(json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-x",
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                {"type": "text", "text": "hi"},
                {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"cmd": "ls"}}
            ],
            "stop_reason": "tool_use", "stop_sequence": null,
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 3}
        }))
        .unwrap();
        let mut acc = Accumulator::new();
        for e in events_from_message(&m) {
            acc.push(&e);
        }
        assert_eq!(acc.finish().unwrap(), m);
    }

    #[test]
    fn unknown_blocks_parse() {
        let b: Vec<ContentBlock> = serde_json::from_value(
            json!([{"type": "server_tool_use", "id": "x"}, {"type": "text", "text": "a"}]),
        )
        .unwrap();
        assert_eq!(b[0], ContentBlock::Unknown);
        let e: StreamEvent = serde_json::from_value(json!({"type": "ping"})).unwrap();
        assert_eq!(e, StreamEvent::Ping);
    }

    #[test]
    fn image_source_url_roundtrip() {
        let s = ImageSource::from_url("data:image/jpeg;base64,AAAA");
        assert_eq!(
            s,
            ImageSource::Base64 {
                media_type: "image/jpeg".into(),
                data: "AAAA".into()
            }
        );
        assert_eq!(s.to_url().unwrap(), "data:image/jpeg;base64,AAAA");
        assert!(matches!(
            ImageSource::from_url("https://x/y.png"),
            ImageSource::Url { .. }
        ));
    }

    #[test]
    fn bad_tool_json_is_preserved() {
        assert_eq!(parse_tool_input(""), json!({}));
        assert_eq!(parse_tool_input("{\"a\":1"), json!({"_raw": "{\"a\":1"}));
    }
}

#[cfg(test)]
mod hoist_tests {
    use super::hoist_system_messages;
    use serde_json::json;

    #[test]
    fn hoists_inline_system_messages_into_system_blocks() {
        let mut req = json!({
            "model": "m",
            "max_tokens": 16,
            "system": "base",
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "system", "content": "reminder"},
                {"role": "assistant", "content": "yo"},
                {"role": "system", "content": [{"type": "text", "text": "late"}]}
            ]
        });
        hoist_system_messages(&mut req);
        assert_eq!(req["messages"].as_array().unwrap().len(), 2);
        let texts: Vec<_> = req["system"].as_array().unwrap().iter().map(|b| b["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["base", "reminder", "late"]);
        let req2: super::MessagesRequest = serde_json::from_value(req).unwrap();
        assert_eq!(req2.messages.len(), 2);
    }
}
