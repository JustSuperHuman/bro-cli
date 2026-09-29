//! Streaming: OpenAI Responses events → Anthropic SSE events (port of v1's
//! codex-bridge translator, extended with encrypted-reasoning round-trip,
//! custom tool calls and `*.done`-only fallbacks).

use super::{SIG_RESPONSES, a2responses::responses_stop, usage_from_responses};
use crate::anthropic::{
    ContentBlock, Delta, DeltaUsage, MessageDeltaBody, MessagesResponse, StreamEvent, Usage,
};
use crate::errors::ProxyError;
use crate::openai::{ResponsesUsage, item_text, str_field};
use crate::util::gen_id;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
enum Open {
    None,
    Text {
        index: usize,
        item: String,
        emitted: bool,
    },
    Thinking {
        index: usize,
        item: String,
    },
    Tool {
        index: usize,
        item: String,
        emitted: bool,
        custom: bool,
    },
}

pub struct ResponsesToAnthropic {
    model: String,
    started: bool,
    finished: bool,
    next_index: usize,
    open: Open,
    saw_tool: bool,
    /// Error surfaced before any content (lets callers return a proper HTTP error)
    pub early_error: Option<ProxyError>,
}

impl ResponsesToAnthropic {
    pub fn new(client_model: &str) -> Self {
        ResponsesToAnthropic {
            model: client_model.to_string(),
            started: false,
            finished: false,
            next_index: 0,
            open: Open::None,
            saw_tool: false,
            early_error: None,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }
    pub fn started(&self) -> bool {
        self.started
    }

    pub fn push(&mut self, data: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if self.finished || data.trim() == "[DONE]" {
            return out;
        }
        let ev: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return out,
        };
        let item_id = str_field(&ev, "item_id").unwrap_or("").to_string();
        match str_field(&ev, "type").unwrap_or("") {
            "response.created" | "response.in_progress" => {
                let id = ev
                    .get("response")
                    .and_then(|r| str_field(r, "id"))
                    .unwrap_or("");
                self.start(&mut out, id);
            }
            "response.output_item.added" => {
                self.start(&mut out, "");
                let item = ev.get("item").cloned().unwrap_or_default();
                let id = str_field(&item, "id").unwrap_or("").to_string();
                match str_field(&item, "type") {
                    Some(t @ ("function_call" | "custom_tool_call")) => {
                        self.saw_tool = true;
                        self.close(&mut out);
                        let index = self.open_block(
                            &mut out,
                            ContentBlock::ToolUse {
                                id: str_field(&item, "call_id")
                                    .map(str::to_string)
                                    .unwrap_or_else(|| gen_id("call_")),
                                name: str_field(&item, "name").unwrap_or_default().to_string(),
                                input: Value::Object(Default::default()),
                                cache_control: None,
                            },
                        );
                        self.open = Open::Tool {
                            index,
                            item: id,
                            emitted: false,
                            custom: t == "custom_tool_call",
                        };
                    }
                    Some("message") => {
                        self.close(&mut out);
                    }
                    _ => {}
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                self.start(&mut out, "");
                let d = str_field(&ev, "delta").unwrap_or("");
                if d.is_empty() {
                    return out;
                }
                let index = self.ensure_text(&mut out, &item_id);
                if let Open::Text { emitted, .. } = &mut self.open {
                    *emitted = true;
                }
                out.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: Delta::TextDelta { text: d.into() },
                });
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                self.start(&mut out, "");
                let d = str_field(&ev, "delta").unwrap_or("");
                let index = self.ensure_thinking(&mut out, &item_id);
                if !d.is_empty() {
                    out.push(StreamEvent::ContentBlockDelta {
                        index,
                        delta: Delta::ThinkingDelta { thinking: d.into() },
                    });
                }
            }
            "response.reasoning_summary_part.added" => {
                // Separate successive summary parts of one reasoning item.
                if let Open::Thinking { index, item } = &self.open
                    && *item == item_id
                    && ev.get("summary_index").and_then(Value::as_u64).unwrap_or(0) > 0
                {
                    out.push(StreamEvent::ContentBlockDelta {
                        index: *index,
                        delta: Delta::ThinkingDelta {
                            thinking: "\n\n".into(),
                        },
                    });
                }
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                let d = str_field(&ev, "delta").unwrap_or("");
                if d.is_empty() {
                    return out;
                }
                if let Open::Tool {
                    index,
                    emitted,
                    custom,
                    ..
                } = &mut self.open
                {
                    let partial = if *custom {
                        // A freeform input streams as the JSON document {"input": "..."}.
                        let s = serde_json::to_string(d).unwrap_or_default();
                        let inner = &s[1..s.len() - 1];
                        if *emitted {
                            inner.to_string()
                        } else {
                            format!("{{\"input\":\"{inner}")
                        }
                    } else {
                        d.to_string()
                    };
                    *emitted = true;
                    out.push(StreamEvent::ContentBlockDelta {
                        index: *index,
                        delta: Delta::InputJsonDelta {
                            partial_json: partial,
                        },
                    });
                }
            }
            "response.output_item.done" => {
                let item = ev.get("item").cloned().unwrap_or_default();
                self.item_done(&mut out, &item);
            }
            "response.completed" | "response.incomplete" => {
                self.start(&mut out, "");
                self.close(&mut out);
                let resp = ev.get("response").cloned().unwrap_or_default();
                let usage = resp
                    .get("usage")
                    .and_then(|u| serde_json::from_value::<ResponsesUsage>(u.clone()).ok())
                    .map(|u| usage_from_responses(&u))
                    .unwrap_or_default();
                out.push(StreamEvent::MessageDelta {
                    delta: MessageDeltaBody {
                        stop_reason: Some(responses_stop(&resp, self.saw_tool).into()),
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
            "response.failed" | "error" => {
                let err = ev
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .or_else(|| ev.get("error"))
                    .cloned()
                    .unwrap_or_else(|| ev.clone());
                let e = ProxyError::from_stream_error(&err);
                if !self.started {
                    self.early_error = Some(e.clone());
                }
                self.close(&mut out);
                out.push(StreamEvent::error(e.kind.anthropic_type(), e.message));
                self.finished = true;
            }
            _ => {}
        }
        out
    }

    /// Upstream ended without response.completed.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if !self.finished {
            self.close(&mut out);
            let msg = "upstream stream ended unexpectedly";
            if !self.started {
                self.early_error = Some(ProxyError::api(msg).with_status(502));
            }
            out.push(StreamEvent::error("api_error", msg));
            self.finished = true;
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
            format!("msg_{}", id.trim_start_matches("resp_"))
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

    fn ensure_text(&mut self, out: &mut Vec<StreamEvent>, item: &str) -> usize {
        if let Open::Text { index, .. } = &self.open {
            return *index;
        }
        self.close(out);
        let index = self.open_block(out, ContentBlock::text(""));
        self.open = Open::Text {
            index,
            item: item.to_string(),
            emitted: false,
        };
        index
    }

    fn ensure_thinking(&mut self, out: &mut Vec<StreamEvent>, item: &str) -> usize {
        if let Open::Thinking {
            index,
            item: open_item,
        } = &self.open
            && (open_item == item || item.is_empty())
        {
            return *index;
        }
        self.close(out);
        let index = self.open_block(
            out,
            ContentBlock::Thinking {
                thinking: String::new(),
                signature: None,
            },
        );
        self.open = Open::Thinking {
            index,
            item: item.to_string(),
        };
        index
    }

    fn close_with_signature(&mut self, out: &mut Vec<StreamEvent>, signature: String) {
        if let Open::Thinking { index, .. } = self.open {
            out.push(StreamEvent::ContentBlockDelta {
                index,
                delta: Delta::SignatureDelta { signature },
            });
            out.push(StreamEvent::ContentBlockStop { index });
            self.open = Open::None;
        }
    }

    fn close(&mut self, out: &mut Vec<StreamEvent>) {
        match std::mem::replace(&mut self.open, Open::None) {
            Open::None => {}
            Open::Thinking { index, .. } => {
                out.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: Delta::SignatureDelta {
                        signature: String::new(),
                    },
                });
                out.push(StreamEvent::ContentBlockStop { index });
            }
            Open::Text { index, .. } | Open::Tool { index, .. } => {
                out.push(StreamEvent::ContentBlockStop { index })
            }
        }
    }

    fn item_done(&mut self, out: &mut Vec<StreamEvent>, item: &Value) {
        let id = str_field(item, "id").unwrap_or("");
        match str_field(item, "type") {
            Some("reasoning") => {
                let enc = str_field(item, "encrypted_content").filter(|s| !s.is_empty());
                let signature = enc
                    .map(|e| format!("{SIG_RESPONSES}{e}"))
                    .unwrap_or_default();
                let open_here =
                    matches!(&self.open, Open::Thinking { item: i, .. } if i == id || i.is_empty());
                if open_here {
                    self.close_with_signature(out, signature);
                } else {
                    let summary: Vec<&str> = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|p| str_field(p, "text"))
                        .collect();
                    if summary.is_empty() && enc.is_none() {
                        return;
                    }
                    // No deltas were streamed: emit the whole reasoning block now so
                    // encrypted reasoning still round-trips through the client.
                    self.start(out, "");
                    let index = self.ensure_thinking(out, id);
                    let text = summary.join("\n\n");
                    if !text.is_empty() {
                        out.push(StreamEvent::ContentBlockDelta {
                            index,
                            delta: Delta::ThinkingDelta { thinking: text },
                        });
                    }
                    self.close_with_signature(out, signature);
                }
            }
            Some("message") => {
                let emitted = matches!(&self.open, Open::Text { emitted: true, .. });
                if !emitted {
                    let text = item_text(item);
                    if !text.is_empty() {
                        self.start(out, "");
                        let index = self.ensure_text(out, id);
                        out.push(StreamEvent::ContentBlockDelta {
                            index,
                            delta: Delta::TextDelta { text },
                        });
                    }
                }
                if matches!(self.open, Open::Text { .. }) {
                    self.close(out);
                }
            }
            Some(t @ ("function_call" | "custom_tool_call")) => {
                let custom = t == "custom_tool_call";
                let full = if custom {
                    json!({"input": str_field(item, "input").unwrap_or("")}).to_string()
                } else {
                    str_field(item, "arguments").unwrap_or("").to_string()
                };
                match &self.open {
                    Open::Tool {
                        index,
                        emitted,
                        custom: c,
                        ..
                    } => {
                        let index = *index;
                        if !*emitted && !full.is_empty() {
                            out.push(StreamEvent::ContentBlockDelta {
                                index,
                                delta: Delta::InputJsonDelta { partial_json: full },
                            });
                        } else if *emitted && *c {
                            // close the {"input":" wrapper opened by the first delta
                            out.push(StreamEvent::ContentBlockDelta {
                                index,
                                delta: Delta::InputJsonDelta {
                                    partial_json: "\"}".into(),
                                },
                            });
                        }
                        self.close(out);
                    }
                    _ => {
                        // done without added: emit a whole block
                        self.start(out, "");
                        self.saw_tool = true;
                        self.close(out);
                        let index = self.open_block(
                            out,
                            ContentBlock::ToolUse {
                                id: str_field(item, "call_id")
                                    .map(str::to_string)
                                    .unwrap_or_else(|| gen_id("call_")),
                                name: str_field(item, "name").unwrap_or_default().to_string(),
                                input: Value::Object(Default::default()),
                                cache_control: None,
                            },
                        );
                        if !full.is_empty() {
                            out.push(StreamEvent::ContentBlockDelta {
                                index,
                                delta: Delta::InputJsonDelta { partial_json: full },
                            });
                        }
                        out.push(StreamEvent::ContentBlockStop { index });
                    }
                }
            }
            _ => {}
        }
    }
}
