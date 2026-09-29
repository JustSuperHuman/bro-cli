//! Streaming: Anthropic SSE events → OpenAI Chat Completions chunks.
//!
//! Emits the role in the first chunk, one chunk per delta, a final chunk with
//! `finish_reason`, an optional usage chunk (when the client asked for
//! `stream_options.include_usage`) and `[DONE]`.

use super::reasoning_cache::ReasoningCache;
use super::{chat_usage_json, stop_to_chat_finish};
use crate::anthropic::{ContentBlock, Delta, StreamEvent, Usage, merge_delta_usage};
use crate::errors::{ErrorKind, ProxyError};
use crate::util::now_secs;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// One outgoing SSE payload (`data: ...`).
#[derive(Debug, Clone, PartialEq)]
pub enum ChatOut {
    Chunk(Value),
    Done,
}

pub struct AnthropicToChat {
    id: String,
    created: i64,
    model: String,
    include_usage: bool,
    sent_role: bool,
    /// Anthropic block index → chat tool_calls index
    tool_index: HashMap<usize, usize>,
    next_tool: usize,
    first_tool_id: Option<String>,
    /// Signed thinking of this turn, stashed under the first tool call id
    reasoning_blocks: Vec<ContentBlock>,
    open_thinking: Option<(usize, String, String)>,
    usage: Usage,
    stop_reason: Option<String>,
    finished: bool,
    cache: Option<Arc<ReasoningCache>>,
}

impl AnthropicToChat {
    pub fn new(
        client_model: &str,
        include_usage: bool,
        cache: Option<Arc<ReasoningCache>>,
    ) -> Self {
        AnthropicToChat {
            id: crate::util::gen_id("chatcmpl-"),
            created: now_secs(),
            model: client_model.to_string(),
            include_usage,
            sent_role: false,
            tool_index: HashMap::new(),
            next_tool: 0,
            first_tool_id: None,
            reasoning_blocks: vec![],
            open_thinking: None,
            usage: Usage::default(),
            stop_reason: None,
            finished: false,
            cache,
        }
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> ChatOut {
        ChatOut::Chunk(json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{"index": 0, "delta": delta, "logprobs": null, "finish_reason": finish}],
        }))
    }

    fn role_chunk(&mut self, out: &mut Vec<ChatOut>) {
        if !self.sent_role {
            self.sent_role = true;
            out.push(self.chunk(json!({"role": "assistant", "content": ""}), None));
        }
    }

    pub fn push(&mut self, ev: &StreamEvent) -> Vec<ChatOut> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        match ev {
            StreamEvent::MessageStart { message } => {
                self.id = format!("chatcmpl-{}", message.id.trim_start_matches("msg_"));
                self.usage = message.usage.clone();
                self.role_chunk(&mut out);
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                self.role_chunk(&mut out);
                match content_block {
                    ContentBlock::ToolUse { id, name, .. } => {
                        let ti = self.next_tool;
                        self.next_tool += 1;
                        self.tool_index.insert(*index, ti);
                        if self.first_tool_id.is_none() {
                            self.first_tool_id = Some(id.clone());
                        }
                        out.push(self.chunk(
                            json!({"tool_calls": [{"index": ti, "id": id, "type": "function",
                                "function": {"name": name, "arguments": ""}}]}),
                            None,
                        ));
                    }
                    ContentBlock::Thinking { thinking, .. } => {
                        self.open_thinking = Some((*index, thinking.clone(), String::new()));
                        if !thinking.is_empty() {
                            out.push(self.chunk(json!({"reasoning_content": thinking}), None));
                        }
                    }
                    ContentBlock::RedactedThinking { .. } => {
                        self.reasoning_blocks.push(content_block.clone())
                    }
                    ContentBlock::Text { text, .. } if !text.is_empty() => {
                        out.push(self.chunk(json!({"content": text}), None));
                    }
                    _ => {}
                }
            }
            StreamEvent::ContentBlockDelta { index, delta } => match delta {
                Delta::TextDelta { text } => out.push(self.chunk(json!({"content": text}), None)),
                Delta::ThinkingDelta { thinking } => {
                    if let Some((_, t, _)) = self.open_thinking.as_mut() {
                        t.push_str(thinking);
                    }
                    out.push(self.chunk(json!({"reasoning_content": thinking}), None));
                }
                Delta::SignatureDelta { signature } => {
                    if let Some((_, _, s)) = self.open_thinking.as_mut() {
                        s.push_str(signature);
                    }
                }
                Delta::InputJsonDelta { partial_json } => {
                    if let Some(ti) = self.tool_index.get(index) {
                        out.push(self.chunk(
                            json!({"tool_calls": [{"index": ti, "function": {"arguments": partial_json}}]}),
                            None,
                        ));
                    }
                }
                _ => {}
            },
            StreamEvent::ContentBlockStop { index } => {
                if self
                    .open_thinking
                    .as_ref()
                    .is_some_and(|(i, _, _)| i == index)
                    && let Some((_, thinking, signature)) = self.open_thinking.take()
                {
                    self.reasoning_blocks.push(ContentBlock::Thinking {
                        thinking,
                        signature: Some(signature),
                    });
                }
            }
            StreamEvent::MessageDelta { delta, usage } => {
                merge_delta_usage(&mut self.usage, usage);
                if delta.stop_reason.is_some() {
                    self.stop_reason = delta.stop_reason.clone();
                }
            }
            StreamEvent::MessageStop => self.finish_into(&mut out),
            StreamEvent::Error { error } => {
                let kind = ErrorKind::from_anthropic_type(&error.kind).unwrap_or(ErrorKind::Api);
                let e = ProxyError::new(kind, error.message.clone());
                out.push(ChatOut::Chunk(e.body_for(crate::Dialect::Chat)));
                out.push(ChatOut::Done);
                self.finished = true;
            }
            StreamEvent::Ping | StreamEvent::Unknown => {}
        }
        out
    }

    /// Upstream ended; close out if message_stop never arrived.
    pub fn finish(&mut self) -> Vec<ChatOut> {
        let mut out = Vec::new();
        if !self.finished {
            self.finish_into(&mut out);
        }
        out
    }

    fn finish_into(&mut self, out: &mut Vec<ChatOut>) {
        self.role_chunk(out);
        if let (Some(cache), Some(first)) = (&self.cache, &self.first_tool_id) {
            cache.put(
                first,
                ReasoningCache::reasoning_blocks(&self.reasoning_blocks),
            );
        }
        let finish = stop_to_chat_finish(self.stop_reason.as_deref());
        out.push(self.chunk(json!({}), Some(finish)));
        if self.include_usage {
            out.push(ChatOut::Chunk(json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [],
                "usage": chat_usage_json(&self.usage),
            })));
        }
        out.push(ChatOut::Done);
        self.finished = true;
    }
}
