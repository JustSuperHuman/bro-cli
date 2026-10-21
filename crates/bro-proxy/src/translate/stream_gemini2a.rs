//! Streaming: Gemini `streamGenerateContent` chunks (Cloud Code Assist wraps each in
//! `{"response": …}`) → Anthropic SSE events.
//!
//! Gemini sends whole parts: thought text, text, or a complete functionCall. Thoughts become
//! a thinking block; a `thoughtSignature` lands on the thinking block in front of the part it
//! came with (`bro.gs:{sig}`, see `a2gemini`), opening an empty one when there was no thought
//! text. A functionCall is one tool_use block with its arguments in a single delta.

use super::SIG_GEMINI;
use crate::anthropic::{ContentBlock, Delta, DeltaUsage, MessageDeltaBody, MessagesResponse, StreamEvent, Usage};
use crate::errors::ProxyError;
use crate::util::gen_id;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Open {
    None,
    Text(usize),
    Thinking(usize),
}

pub struct GeminiToAnthropic {
    model: String,
    started: bool,
    finished: bool,
    next_index: usize,
    open: Open,
    tools: usize,
    finish_reason: Option<String>,
    usage: Usage,
}

/// Gemini `usageMetadata` → Anthropic usage (thoughts count as output, as Google bills them).
pub fn usage_from_gemini(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cached = n("cachedContentTokenCount");
    Usage {
        input_tokens: n("promptTokenCount").saturating_sub(cached),
        output_tokens: n("candidatesTokenCount") + n("thoughtsTokenCount"),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: (cached > 0).then_some(cached),
    }
}

/// Gemini finishReason → Anthropic stop_reason.
pub fn gemini_finish_to_stop(finish: Option<&str>, has_tool_calls: bool) -> &'static str {
    if has_tool_calls {
        return "tool_use";
    }
    match finish {
        Some("MAX_TOKENS") => "max_tokens",
        Some("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY") => "refusal",
        _ => "end_turn",
    }
}

impl GeminiToAnthropic {
    pub fn new(client_model: &str) -> Self {
        GeminiToAnthropic {
            model: client_model.to_string(),
            started: false,
            finished: false,
            next_index: 0,
            open: Open::None,
            tools: 0,
            finish_reason: None,
            usage: Usage::default(),
        }
    }

    /// Feed one SSE `data:` payload (or a whole non-streaming response body).
    pub fn push(&mut self, data: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("skipping malformed gemini chunk: {e}");
                return out;
            }
        };
        let err = v.get("error").or_else(|| v.pointer("/response/error")).filter(|e| e.is_object());
        if let Some(err) = err {
            let e = ProxyError::from_stream_error(err);
            self.start(&mut out, "");
            self.close_open(&mut out);
            out.push(StreamEvent::error(e.kind.anthropic_type(), e.message));
            self.finished = true;
            return out;
        }
        let r = v.get("response").unwrap_or(&v);
        if let Some(u) = r.get("usageMetadata") {
            self.usage = usage_from_gemini(u);
        }
        self.start(&mut out, r.get("responseId").and_then(Value::as_str).unwrap_or(""));
        let Some(cand) = r.get("candidates").and_then(Value::as_array).and_then(|c| c.first()) else { return out };
        for part in cand.pointer("/content/parts").and_then(Value::as_array).into_iter().flatten() {
            self.part(&mut out, part);
        }
        if let Some(f) = cand.get("finishReason").and_then(Value::as_str) {
            self.finish_reason = Some(f.to_string());
        }
        out
    }

    fn part(&mut self, out: &mut Vec<StreamEvent>, part: &Value) {
        let sig = part.get("thoughtSignature").and_then(Value::as_str).filter(|s| !s.is_empty());
        let text = part.get("text").and_then(Value::as_str).unwrap_or("");
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            let idx = match self.open {
                Open::Thinking(i) => i,
                _ => {
                    self.close_open(out);
                    self.open_block(out, ContentBlock::Thinking { thinking: String::new(), signature: None })
                }
            };
            self.open = Open::Thinking(idx);
            if !text.is_empty() {
                out.push(StreamEvent::ContentBlockDelta { index: idx, delta: Delta::ThinkingDelta { thinking: text.to_string() } });
            }
            if let Some(s) = sig {
                self.close_thinking(out, idx, s);
            }
            return;
        }
        if let Some(fc) = part.get("functionCall") {
            self.sign(out, sig);
            self.close_open(out);
            let id = fc.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| gen_id("toolu_"));
            let name = fc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = fc.get("args").filter(|a| a.is_object()).cloned().unwrap_or_else(|| Value::Object(Default::default()));
            let index = self.open_block(out, ContentBlock::ToolUse { id, name, input: Value::Object(Default::default()), cache_control: None });
            out.push(StreamEvent::ContentBlockDelta { index, delta: Delta::InputJsonDelta { partial_json: args.to_string() } });
            out.push(StreamEvent::ContentBlockStop { index });
            self.tools += 1;
            return;
        }
        if text.is_empty() {
            // A bare signature on an empty closing part signs a text answer: Gemini doesn't
            // need it back, and there's no part left to carry it.
            return;
        }
        if sig.is_some() {
            self.sign(out, sig);
        }
        let idx = match self.open {
            Open::Text(i) => i,
            _ => {
                self.close_open(out);
                self.open_block(out, ContentBlock::text(""))
            }
        };
        self.open = Open::Text(idx);
        out.push(StreamEvent::ContentBlockDelta { index: idx, delta: Delta::TextDelta { text: text.to_string() } });
    }

    /// Put a signature in front of the next part: on the open thinking block, else on a new empty one.
    fn sign(&mut self, out: &mut Vec<StreamEvent>, sig: Option<&str>) {
        let Some(sig) = sig else { return };
        let idx = match self.open {
            Open::Thinking(i) => i,
            _ => {
                self.close_open(out);
                self.open_block(out, ContentBlock::Thinking { thinking: String::new(), signature: None })
            }
        };
        self.close_thinking(out, idx, sig);
    }

    fn close_thinking(&mut self, out: &mut Vec<StreamEvent>, idx: usize, sig: &str) {
        out.push(StreamEvent::ContentBlockDelta { index: idx, delta: Delta::SignatureDelta { signature: format!("{SIG_GEMINI}{sig}") } });
        out.push(StreamEvent::ContentBlockStop { index: idx });
        self.open = Open::None;
    }

    /// Upstream closed: the closing events.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        if !self.started {
            out.push(StreamEvent::error("api_error", "Gemini ended the stream without a response"));
            self.finished = true;
            return out;
        }
        self.close_open(&mut out);
        out.push(StreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some(gemini_finish_to_stop(self.finish_reason.as_deref(), self.tools > 0).into()),
                stop_sequence: None,
            },
            usage: DeltaUsage {
                output_tokens: self.usage.output_tokens,
                input_tokens: Some(self.usage.input_tokens),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: self.usage.cache_read_input_tokens,
            },
        });
        out.push(StreamEvent::MessageStop);
        self.finished = true;
        out
    }

    fn start(&mut self, out: &mut Vec<StreamEvent>, id: &str) {
        if self.started {
            return;
        }
        self.started = true;
        let id = if id.is_empty() { gen_id("msg_") } else { format!("msg_{id}") };
        let mut message = MessagesResponse::new(id, &self.model);
        message.usage = Usage { input_tokens: self.usage.input_tokens, output_tokens: 1, cache_read_input_tokens: self.usage.cache_read_input_tokens, ..Default::default() };
        out.push(StreamEvent::MessageStart { message });
        out.push(StreamEvent::Ping);
    }

    fn open_block(&mut self, out: &mut Vec<StreamEvent>, block: ContentBlock) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        out.push(StreamEvent::ContentBlockStart { index, content_block: block });
        index
    }

    fn close_open(&mut self, out: &mut Vec<StreamEvent>) {
        match std::mem::replace(&mut self.open, Open::None) {
            Open::None => {}
            Open::Text(i) => out.push(StreamEvent::ContentBlockStop { index: i }),
            // Unsigned thoughts still get a (placeholder) signature, as Anthropic clients expect.
            Open::Thinking(i) => {
                out.push(StreamEvent::ContentBlockDelta { index: i, delta: Delta::SignatureDelta { signature: SIG_GEMINI.into() } });
                out.push(StreamEvent::ContentBlockStop { index: i });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::Accumulator;
    use serde_json::json;

    fn run(chunks: &[Value]) -> MessagesResponse {
        let mut d = GeminiToAnthropic::new("gemini-4-argon");
        let mut acc = Accumulator::new();
        for c in chunks {
            d.push(&c.to_string()).iter().for_each(|e| acc.push(e));
        }
        d.finish().iter().for_each(|e| acc.push(e));
        acc.finish().unwrap()
    }

    #[test]
    fn thoughts_text_and_signed_call() {
        let sig = "Q".repeat(60);
        let m = run(&[
            json!({"response": {"responseId": "r1", "candidates": [{"content": {"role": "model", "parts": [{"text": "Planning", "thought": true}]}}], "usageMetadata": {"promptTokenCount": 100}}}),
            json!({"response": {"candidates": [{"content": {"parts": [{"text": "Looking."}]}}]}}),
            json!({"response": {"candidates": [{"content": {"parts": [
                {"functionCall": {"name": "ls", "args": {"dir": "."}}, "thoughtSignature": sig},
                {"functionCall": {"id": "c2", "name": "ls", "args": {"dir": "src"}}}
            ]}, "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 100, "cachedContentTokenCount": 40, "candidatesTokenCount": 20, "thoughtsTokenCount": 30}}}),
        ]);
        assert_eq!(m.id, "msg_r1");
        assert_eq!(m.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(m.usage.input_tokens, 60);
        assert_eq!(m.usage.cache_read_input_tokens, Some(40));
        assert_eq!(m.usage.output_tokens, 50);
        let c = &m.content;
        assert!(matches!(&c[0], ContentBlock::Thinking { thinking, signature: Some(s) } if thinking == "Planning" && s == SIG_GEMINI));
        assert!(matches!(&c[1], ContentBlock::Text { text, .. } if text == "Looking."));
        assert!(matches!(&c[2], ContentBlock::Thinking { thinking, signature: Some(s) } if thinking.is_empty() && *s == format!("{SIG_GEMINI}{sig}")));
        assert!(matches!(&c[3], ContentBlock::ToolUse { name, input, .. } if name == "ls" && input["dir"] == "."));
        assert!(matches!(&c[4], ContentBlock::ToolUse { id, .. } if id == "c2"));
    }

    #[test]
    fn errors_and_empty_streams() {
        let mut d = GeminiToAnthropic::new("m");
        let ev = d.push(&json!({"error": {"code": 429, "status": "RESOURCE_EXHAUSTED", "message": "quota"}}).to_string());
        assert!(matches!(ev.last(), Some(StreamEvent::Error { error }) if error.message == "quota"));
        let mut d = GeminiToAnthropic::new("m");
        assert!(matches!(d.finish().as_slice(), [StreamEvent::Error { .. }]));
    }

    #[test]
    fn max_tokens_and_refusals() {
        assert_eq!(gemini_finish_to_stop(Some("MAX_TOKENS"), false), "max_tokens");
        assert_eq!(gemini_finish_to_stop(Some("SAFETY"), false), "refusal");
        assert_eq!(gemini_finish_to_stop(Some("STOP"), true), "tool_use");
    }
}
