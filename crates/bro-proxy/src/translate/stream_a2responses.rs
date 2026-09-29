//! Streaming: Anthropic SSE events → OpenAI Responses events (what Codex CLI reads).
//!
//! Per Anthropic block: text → message item (output_item.added, content_part.added,
//! output_text.delta*, output_text.done, content_part.done, output_item.done);
//! thinking → reasoning item with one summary part whose encrypted_content carries
//! the Anthropic signature; tool_use → function_call (or custom_tool_call) item.
//! message_stop → response.completed (or response.incomplete on max_tokens).

use super::ENC_REDACTED;
use super::responses2a::{reasoning_item, response_object, tool_item};
use crate::anthropic::{ContentBlock, Delta, MessagesResponse, StreamEvent, merge_delta_usage};
use crate::errors::ErrorKind;
use crate::util::gen_id;
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Debug, Clone)]
enum Item {
    Text {
        id: String,
        text: String,
    },
    Thinking {
        id: String,
        text: String,
        signature: String,
    },
    Tool {
        id: String,
        call_id: String,
        name: String,
        args: String,
        custom: bool,
    },
}

pub struct AnthropicToResponses {
    resp_id: String,
    model: String,
    custom: HashSet<String>,
    seq: u64,
    started: bool,
    finished: bool,
    /// (anthropic block index, output_index, item)
    open: Option<(usize, usize, Item)>,
    output: Vec<Value>,
    msg: MessagesResponse,
}

impl AnthropicToResponses {
    pub fn new(client_model: &str, custom: HashSet<String>) -> Self {
        AnthropicToResponses {
            resp_id: gen_id("resp_"),
            model: client_model.to_string(),
            custom,
            seq: 0,
            started: false,
            finished: false,
            open: None,
            output: vec![],
            msg: MessagesResponse::new("", client_model),
        }
    }

    fn ev(&mut self, out: &mut Vec<(String, Value)>, kind: &str, mut body: Value) {
        body["type"] = json!(kind);
        body["sequence_number"] = json!(self.seq);
        self.seq += 1;
        out.push((kind.to_string(), body));
    }

    fn snapshot(&self, status: &str) -> Value {
        response_object(
            &self.resp_id,
            &self.model,
            status,
            self.output.clone(),
            &self.msg,
        )
    }

    fn start(&mut self, out: &mut Vec<(String, Value)>) {
        if self.started {
            return;
        }
        self.started = true;
        let mut r = self.snapshot("in_progress");
        r["usage"] = Value::Null;
        self.ev(out, "response.created", json!({"response": r.clone()}));
        self.ev(out, "response.in_progress", json!({"response": r}));
    }

    pub fn push(&mut self, ev: &StreamEvent) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        match ev {
            StreamEvent::MessageStart { message } => {
                self.resp_id = format!("resp_{}", message.id.trim_start_matches("msg_"));
                self.msg.usage = message.usage.clone();
                self.start(&mut out);
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                self.start(&mut out);
                self.close(&mut out);
                let oi = self.output.len();
                match content_block {
                    ContentBlock::Text { text, .. } => {
                        let id = gen_id("msg_");
                        self.ev(&mut out, "response.output_item.added", json!({"output_index": oi, "item": {
                            "id": id, "type": "message", "status": "in_progress", "role": "assistant", "content": []}}));
                        self.ev(&mut out, "response.content_part.added", json!({"item_id": id, "output_index": oi,
                            "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}}));
                        if !text.is_empty() {
                            self.ev(
                                &mut out,
                                "response.output_text.delta",
                                json!({"item_id": id, "output_index": oi,
                                "content_index": 0, "delta": text, "logprobs": []}),
                            );
                        }
                        self.open = Some((
                            *index,
                            oi,
                            Item::Text {
                                id,
                                text: text.clone(),
                            },
                        ));
                    }
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                    } => {
                        let id = gen_id("rs_");
                        self.ev(
                            &mut out,
                            "response.output_item.added",
                            json!({"output_index": oi, "item": {
                            "id": id, "type": "reasoning", "summary": []}}),
                        );
                        self.ev(&mut out, "response.reasoning_summary_part.added", json!({"item_id": id,
                            "output_index": oi, "summary_index": 0, "part": {"type": "summary_text", "text": ""}}));
                        if !thinking.is_empty() {
                            self.ev(
                                &mut out,
                                "response.reasoning_summary_text.delta",
                                json!({"item_id": id,
                                "output_index": oi, "summary_index": 0, "delta": thinking}),
                            );
                        }
                        self.open = Some((
                            *index,
                            oi,
                            Item::Thinking {
                                id,
                                text: thinking.clone(),
                                signature: signature.clone().unwrap_or_default(),
                            },
                        ));
                    }
                    ContentBlock::RedactedThinking { data } => {
                        let item = json!({"id": gen_id("rs_"), "type": "reasoning", "summary": [],
                            "encrypted_content": format!("{ENC_REDACTED}{data}")});
                        self.ev(
                            &mut out,
                            "response.output_item.added",
                            json!({"output_index": oi, "item": item.clone()}),
                        );
                        self.ev(
                            &mut out,
                            "response.output_item.done",
                            json!({"output_index": oi, "item": item.clone()}),
                        );
                        self.output.push(item);
                    }
                    ContentBlock::ToolUse {
                        id: call_id, name, ..
                    } => {
                        let custom = self.custom.contains(name);
                        let id = gen_id(if custom { "ctc_" } else { "fc_" });
                        let item = if custom {
                            json!({"id": id, "type": "custom_tool_call", "status": "in_progress", "call_id": call_id, "name": name, "input": ""})
                        } else {
                            json!({"id": id, "type": "function_call", "status": "in_progress", "call_id": call_id, "name": name, "arguments": ""})
                        };
                        self.ev(
                            &mut out,
                            "response.output_item.added",
                            json!({"output_index": oi, "item": item}),
                        );
                        self.open = Some((
                            *index,
                            oi,
                            Item::Tool {
                                id,
                                call_id: call_id.clone(),
                                name: name.clone(),
                                args: String::new(),
                                custom,
                            },
                        ));
                    }
                    _ => {}
                }
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                let Some((bi, oi, item)) = self.open.as_mut() else {
                    return out;
                };
                if bi != index {
                    return out;
                }
                let oi = *oi;
                match (item, delta) {
                    (Item::Text { id, text }, Delta::TextDelta { text: d }) => {
                        text.push_str(d);
                        let id = id.clone();
                        self.ev(
                            &mut out,
                            "response.output_text.delta",
                            json!({"item_id": id, "output_index": oi,
                            "content_index": 0, "delta": d, "logprobs": []}),
                        );
                    }
                    (Item::Thinking { id, text, .. }, Delta::ThinkingDelta { thinking }) => {
                        text.push_str(thinking);
                        let id = id.clone();
                        self.ev(
                            &mut out,
                            "response.reasoning_summary_text.delta",
                            json!({"item_id": id,
                            "output_index": oi, "summary_index": 0, "delta": thinking}),
                        );
                    }
                    (Item::Thinking { signature, .. }, Delta::SignatureDelta { signature: s }) => {
                        signature.push_str(s)
                    }
                    (
                        Item::Tool {
                            id, args, custom, ..
                        },
                        Delta::InputJsonDelta { partial_json },
                    ) => {
                        args.push_str(partial_json);
                        // Freeform (custom) input can only be emitted once the JSON is complete.
                        if !*custom {
                            let id = id.clone();
                            self.ev(
                                &mut out,
                                "response.function_call_arguments.delta",
                                json!({"item_id": id,
                                "output_index": oi, "delta": partial_json}),
                            );
                        }
                    }
                    _ => {}
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                if self.open.as_ref().is_some_and(|(bi, _, _)| bi == index) {
                    self.close(&mut out);
                }
            }
            StreamEvent::MessageDelta { delta, usage } => {
                merge_delta_usage(&mut self.msg.usage, usage);
                if delta.stop_reason.is_some() {
                    self.msg.stop_reason = delta.stop_reason.clone();
                }
            }
            StreamEvent::MessageStop => self.finish_into(&mut out),
            StreamEvent::Error { error } => {
                self.start(&mut out);
                self.close(&mut out);
                let kind = ErrorKind::from_anthropic_type(&error.kind).unwrap_or(ErrorKind::Api);
                let (_, code) = kind.openai_type_code();
                let mut r = self.snapshot("failed");
                r["error"] = json!({"code": code, "message": error.message});
                self.ev(&mut out, "response.failed", json!({"response": r}));
                self.finished = true;
            }
            StreamEvent::Ping | StreamEvent::Unknown => {}
        }
        out
    }

    pub fn finish(&mut self) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        if !self.finished {
            self.finish_into(&mut out);
        }
        out
    }

    fn finish_into(&mut self, out: &mut Vec<(String, Value)>) {
        self.start(out);
        self.close(out);
        let incomplete = self.msg.stop_reason.as_deref() == Some("max_tokens");
        let r = self.snapshot(if incomplete {
            "incomplete"
        } else {
            "completed"
        });
        let kind = if incomplete {
            "response.incomplete"
        } else {
            "response.completed"
        };
        self.ev(out, kind, json!({"response": r}));
        self.finished = true;
    }

    fn close(&mut self, out: &mut Vec<(String, Value)>) {
        let Some((_, oi, item)) = self.open.take() else {
            return;
        };
        let done = match item {
            Item::Text { id, text } => {
                self.ev(
                    out,
                    "response.output_text.done",
                    json!({"item_id": id, "output_index": oi,
                    "content_index": 0, "text": text, "logprobs": []}),
                );
                let part = json!({"type": "output_text", "text": text, "annotations": []});
                self.ev(
                    out,
                    "response.content_part.done",
                    json!({"item_id": id, "output_index": oi,
                    "content_index": 0, "part": part}),
                );
                json!({"id": id, "type": "message", "status": "completed", "role": "assistant", "content": [part]})
            }
            Item::Thinking {
                id,
                text,
                signature,
            } => {
                self.ev(
                    out,
                    "response.reasoning_summary_text.done",
                    json!({"item_id": id, "output_index": oi,
                    "summary_index": 0, "text": text}),
                );
                self.ev(
                    out,
                    "response.reasoning_summary_part.done",
                    json!({"item_id": id, "output_index": oi,
                    "summary_index": 0, "part": {"type": "summary_text", "text": text}}),
                );
                reasoning_item(&id, &text, Some(&signature))
            }
            Item::Tool {
                id,
                call_id,
                name,
                args,
                custom,
            } => {
                let input = crate::anthropic::parse_tool_input(&args);
                let item = tool_item(&id, &call_id, &name, &input, &self.custom);
                if custom {
                    let raw = item["input"].clone();
                    self.ev(
                        out,
                        "response.custom_tool_call_input.delta",
                        json!({"item_id": id, "output_index": oi, "delta": raw}),
                    );
                    self.ev(
                        out,
                        "response.custom_tool_call_input.done",
                        json!({"item_id": id, "output_index": oi, "input": raw}),
                    );
                } else {
                    let args = item["arguments"].clone();
                    self.ev(
                        out,
                        "response.function_call_arguments.done",
                        json!({"item_id": id, "output_index": oi, "arguments": args}),
                    );
                }
                item
            }
        };
        self.ev(
            out,
            "response.output_item.done",
            json!({"output_index": oi, "item": done.clone()}),
        );
        self.output.push(done);
    }
}
