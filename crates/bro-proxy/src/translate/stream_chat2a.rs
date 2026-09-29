//! Streaming: OpenAI Chat Completions chunks → Anthropic SSE events.
//!
//! State machine. Anthropic content blocks are strictly sequential, so at most one
//! block is open at a time: reasoning → thinking block, content → text block, each
//! tool call → its own tool_use block (opened once its name is known). A block is
//! closed as soon as a different kind of output arrives.

use super::{SIG_CHAT, chat_finish_to_stop, usage_from_chat};
use crate::anthropic::{
    ContentBlock, Delta, DeltaUsage, MessageDeltaBody, MessagesResponse, StreamEvent, Usage,
};
use crate::errors::ProxyError;
use crate::openai::{ChatChunk, ChatUsage};
use crate::util::gen_id;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Open {
    None,
    Text(usize),
    Thinking(usize),
    Tool { chat_index: usize, block: usize },
}

#[derive(Debug, Default)]
struct PendingTool {
    chat_index: usize,
    id: Option<String>,
    name: Option<String>,
    args: String,
    opened: bool,
    closed: bool,
}

pub struct ChatToAnthropic {
    model: String,
    started: bool,
    finished: bool,
    next_index: usize,
    open: Open,
    tools: Vec<PendingTool>,
    finish_reason: Option<String>,
    usage: Option<ChatUsage>,
}

impl ChatToAnthropic {
    pub fn new(client_model: &str) -> Self {
        ChatToAnthropic {
            model: client_model.to_string(),
            started: false,
            finished: false,
            next_index: 0,
            open: Open::None,
            tools: Vec::new(),
            finish_reason: None,
            usage: None,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Feed one SSE `data:` payload.
    pub fn push(&mut self, data: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        if data.trim() == "[DONE]" {
            self.finish_into(&mut out);
            return out;
        }
        let chunk: ChatChunk = match serde_json::from_str(data) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("skipping malformed chat chunk: {e}");
                return out;
            }
        };
        if let Some(err) = &chunk.error {
            let e = ProxyError::from_stream_error(err);
            self.start(&mut out, &chunk.id);
            self.close_open(&mut out);
            out.push(StreamEvent::error(e.kind.anthropic_type(), e.message));
            self.finished = true;
            return out;
        }
        self.start(&mut out, &chunk.id);
        if let Some(u) = chunk.usage {
            self.usage = Some(u);
        }
        for choice in chunk.choices.into_iter().filter(|c| c.index == 0) {
            let d = choice.delta;
            if let Some(r) = d
                .reasoning_content
                .or(d.reasoning)
                .filter(|s| !s.is_empty())
            {
                let idx = match self.open {
                    Open::Thinking(i) => i,
                    _ => {
                        self.close_open(&mut out);
                        self.open_block(
                            &mut out,
                            ContentBlock::Thinking {
                                thinking: String::new(),
                                signature: None,
                            },
                        )
                    }
                };
                self.open = Open::Thinking(idx);
                out.push(StreamEvent::ContentBlockDelta {
                    index: idx,
                    delta: Delta::ThinkingDelta { thinking: r },
                });
            }
            if let Some(text) = d.content.or(d.refusal).filter(|s| !s.is_empty()) {
                let idx = match self.open {
                    Open::Text(i) => i,
                    _ => {
                        self.close_open(&mut out);
                        self.open_block(&mut out, ContentBlock::text(""))
                    }
                };
                self.open = Open::Text(idx);
                out.push(StreamEvent::ContentBlockDelta {
                    index: idx,
                    delta: Delta::TextDelta { text },
                });
            }
            for (pos, tc) in d.tool_calls.into_iter().flatten().enumerate() {
                let chat_index = tc.index.unwrap_or(pos);
                let f = tc.function.unwrap_or_default();
                self.tool_delta(
                    &mut out,
                    chat_index,
                    tc.id,
                    f.name,
                    f.arguments.unwrap_or_default(),
                );
            }
            if let Some(fr) = choice.finish_reason {
                self.finish_reason = Some(fr);
            }
        }
        out
    }

    /// Upstream closed. Emits the closing events if [DONE] never came.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if !self.finished {
            self.finish_into(&mut out);
        }
        out
    }

    fn start(&mut self, out: &mut Vec<StreamEvent>, id: &str) {
        if self.started {
            return;
        }
        self.started = true;
        let id = if id.is_empty() {
            gen_id("msg_")
        } else {
            format!("msg_{}", id.trim_start_matches("chatcmpl-"))
        };
        let mut message = MessagesResponse::new(id, &self.model);
        message.usage = Usage {
            input_tokens: 0,
            output_tokens: 1,
            ..Default::default()
        };
        out.push(StreamEvent::MessageStart { message });
        out.push(StreamEvent::Ping);
    }

    fn open_block(&mut self, out: &mut Vec<StreamEvent>, block: ContentBlock) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        out.push(StreamEvent::ContentBlockStart {
            index,
            content_block: block,
        });
        index
    }

    fn close_open(&mut self, out: &mut Vec<StreamEvent>) {
        match std::mem::replace(&mut self.open, Open::None) {
            Open::None => {}
            Open::Text(i) => out.push(StreamEvent::ContentBlockStop { index: i }),
            Open::Thinking(i) => {
                out.push(StreamEvent::ContentBlockDelta {
                    index: i,
                    delta: Delta::SignatureDelta {
                        signature: SIG_CHAT.into(),
                    },
                });
                out.push(StreamEvent::ContentBlockStop { index: i });
            }
            Open::Tool { chat_index, block } => {
                if let Some(t) = self.tools.iter_mut().find(|t| t.chat_index == chat_index) {
                    t.closed = true;
                }
                out.push(StreamEvent::ContentBlockStop { index: block });
            }
        }
    }

    fn tool_delta(
        &mut self,
        out: &mut Vec<StreamEvent>,
        chat_index: usize,
        id: Option<String>,
        name: Option<String>,
        args: String,
    ) {
        // A new id at a known index means a new call (some servers reuse index 0).
        let known = self.tools.iter().position(|t| {
            t.chat_index == chat_index
                && !(id.is_some() && t.id.is_some() && t.id != id && t.opened)
        });
        let pos = match known {
            Some(p) => p,
            None => {
                if let Some(old) = self.tools.iter_mut().find(|t| t.chat_index == chat_index) {
                    old.chat_index = usize::MAX; // retire
                }
                self.tools.push(PendingTool {
                    chat_index,
                    ..Default::default()
                });
                self.tools.len() - 1
            }
        };
        {
            let t = &mut self.tools[pos];
            if t.id.is_none() {
                t.id = id.filter(|s| !s.is_empty());
            }
            if let Some(n) = name.filter(|s| !s.is_empty()) {
                t.name.get_or_insert_with(String::new).push_str(&n);
            }
        }
        let t = &self.tools[pos];
        if t.closed {
            if !args.is_empty() {
                tracing::warn!("dropping late arguments for already-closed tool call {chat_index}");
            }
            return;
        }
        if !t.opened {
            if t.name.is_none() {
                self.tools[pos].args.push_str(&args);
                return;
            }
            self.close_open(out);
            let t = &mut self.tools[pos];
            t.opened = true;
            let block = ContentBlock::ToolUse {
                id: t.id.clone().unwrap_or_else(|| gen_id("toolu_")),
                name: t.name.clone().unwrap_or_default(),
                input: Value::Object(Default::default()),
                cache_control: None,
            };
            let buffered = std::mem::take(&mut t.args);
            let index = self.open_block(out, block);
            self.open = Open::Tool {
                chat_index,
                block: index,
            };
            if !buffered.is_empty() {
                out.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: Delta::InputJsonDelta {
                        partial_json: buffered,
                    },
                });
            }
        } else if !matches!(self.open, Open::Tool { chat_index: c, .. } if c == chat_index) {
            // Interleaved parallel calls: the earlier call was closed by another block.
            if !args.is_empty() {
                tracing::warn!("dropping interleaved arguments for tool call {chat_index}");
            }
            return;
        }
        if !args.is_empty()
            && let Open::Tool { block, .. } = self.open
        {
            out.push(StreamEvent::ContentBlockDelta {
                index: block,
                delta: Delta::InputJsonDelta { partial_json: args },
            });
        }
    }

    fn finish_into(&mut self, out: &mut Vec<StreamEvent>) {
        self.start(out, "");
        // Tools that never got a name still deserve a block.
        for i in 0..self.tools.len() {
            if !self.tools[i].opened && !self.tools[i].closed {
                self.close_open(out);
                let t = &mut self.tools[i];
                t.opened = true;
                let block = ContentBlock::ToolUse {
                    id: t.id.clone().unwrap_or_else(|| gen_id("toolu_")),
                    name: t.name.clone().unwrap_or_default(),
                    input: Value::Object(Default::default()),
                    cache_control: None,
                };
                let args = std::mem::take(&mut t.args);
                let chat_index = t.chat_index;
                let index = self.open_block(out, block);
                if !args.is_empty() {
                    out.push(StreamEvent::ContentBlockDelta {
                        index,
                        delta: Delta::InputJsonDelta { partial_json: args },
                    });
                }
                self.open = Open::Tool {
                    chat_index,
                    block: index,
                };
            }
        }
        self.close_open(out);
        let has_tools = self.tools.iter().any(|t| t.opened);
        let usage = self.usage.as_ref().map(usage_from_chat).unwrap_or_default();
        out.push(StreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some(
                    chat_finish_to_stop(self.finish_reason.as_deref(), has_tools).into(),
                ),
                stop_sequence: None,
            },
            usage: DeltaUsage {
                output_tokens: usage.output_tokens,
                input_tokens: Some(usage.input_tokens),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: usage.cache_read_input_tokens,
            },
        });
        out.push(StreamEvent::MessageStop);
        self.finished = true;
    }
}
