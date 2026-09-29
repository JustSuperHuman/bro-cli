//! Streaming plumbing: upstream SSE → hub (Anthropic) events → inbound SSE, with
//! usage/error tapping, keepalives and request recording.

use crate::Dialect;
use crate::anthropic::{StreamEvent, Usage, merge_delta_usage};
use crate::errors::{ErrorKind, ProxyError};
use crate::events::Recorder;
use crate::openai::{ChatUsage, ResponsesUsage};
use crate::sse::{SseEvent, SseParser, comment, encode};
use crate::translate::reasoning_cache::ReasoningCache;
use crate::translate::stream_a2chat::{AnthropicToChat, ChatOut};
use crate::translate::stream_a2responses::AnthropicToResponses;
use crate::translate::stream_chat2a::ChatToAnthropic;
use crate::translate::stream_responses2a::ResponsesToAnthropic;
use crate::translate::{usage_from_chat, usage_from_responses};
use crate::upstream::ByteStream;
use axum::body::Body;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

/// Upstream stream → hub events.
pub(crate) enum Decoder {
    Anthropic { terminated: bool },
    Chat(ChatToAnthropic),
    Responses(ResponsesToAnthropic),
}

impl Decoder {
    pub fn new(upstream: Dialect, client_model: &str) -> Self {
        match upstream {
            Dialect::Anthropic => Decoder::Anthropic { terminated: false },
            Dialect::Chat => Decoder::Chat(ChatToAnthropic::new(client_model)),
            Dialect::Responses => Decoder::Responses(ResponsesToAnthropic::new(client_model)),
        }
    }

    pub fn push(&mut self, ev: &SseEvent) -> Vec<StreamEvent> {
        match self {
            Decoder::Anthropic { terminated } => {
                if *terminated {
                    return vec![];
                }
                match serde_json::from_str::<StreamEvent>(&ev.data) {
                    Ok(StreamEvent::Unknown) | Err(_) => vec![],
                    Ok(e) => {
                        if matches!(e, StreamEvent::MessageStop | StreamEvent::Error { .. }) {
                            *terminated = true;
                        }
                        vec![e]
                    }
                }
            }
            Decoder::Chat(t) => t.push(&ev.data),
            Decoder::Responses(t) => t.push(&ev.data),
        }
    }

    pub fn finish(&mut self) -> Vec<StreamEvent> {
        match self {
            Decoder::Anthropic { terminated } => {
                if *terminated {
                    vec![]
                } else {
                    *terminated = true;
                    vec![StreamEvent::error(
                        "api_error",
                        "upstream stream ended unexpectedly",
                    )]
                }
            }
            Decoder::Chat(t) => t.finish(),
            Decoder::Responses(t) => t.finish(),
        }
    }
}

/// Hub events → inbound SSE bytes.
pub(crate) enum Encoder {
    Anthropic { started: bool, done: bool },
    Chat(AnthropicToChat),
    Responses(AnthropicToResponses),
}

impl Encoder {
    pub fn new(
        inbound: Dialect,
        client_model: &str,
        include_usage: bool,
        custom_tools: HashSet<String>,
        cache: Arc<ReasoningCache>,
    ) -> Self {
        match inbound {
            Dialect::Anthropic => Encoder::Anthropic {
                started: false,
                done: false,
            },
            Dialect::Chat => Encoder::Chat(AnthropicToChat::new(
                client_model,
                include_usage,
                Some(cache),
            )),
            Dialect::Responses => {
                Encoder::Responses(AnthropicToResponses::new(client_model, custom_tools))
            }
        }
    }

    pub fn push(&mut self, ev: &StreamEvent) -> Vec<Bytes> {
        match self {
            Encoder::Anthropic { started, done } => {
                if *done || matches!(ev, StreamEvent::Unknown) {
                    return vec![];
                }
                if matches!(ev, StreamEvent::MessageStart { .. }) {
                    *started = true;
                }
                if matches!(ev, StreamEvent::MessageStop | StreamEvent::Error { .. }) {
                    *done = true;
                }
                let data = serde_json::to_string(ev).unwrap_or_default();
                vec![encode(Some(ev.name()), &data)]
            }
            Encoder::Chat(t) => t.push(ev).into_iter().map(chat_bytes).collect(),
            Encoder::Responses(t) => t
                .push(ev)
                .into_iter()
                .map(|(k, v)| encode(Some(&k), &v.to_string()))
                .collect(),
        }
    }

    pub fn finish(&mut self) -> Vec<Bytes> {
        match self {
            Encoder::Anthropic { done, .. } => {
                if *done {
                    vec![]
                } else {
                    self.push(&StreamEvent::error(
                        "api_error",
                        "upstream stream ended unexpectedly",
                    ))
                }
            }
            Encoder::Chat(t) => t.finish().into_iter().map(chat_bytes).collect(),
            Encoder::Responses(t) => t
                .finish()
                .into_iter()
                .map(|(k, v)| encode(Some(&k), &v.to_string()))
                .collect(),
        }
    }

    pub fn keepalive(&self) -> Bytes {
        match self {
            Encoder::Anthropic {
                started: true,
                done: false,
            } => encode(Some("ping"), r#"{"type":"ping"}"#),
            _ => comment("keepalive"),
        }
    }
}

fn chat_bytes(o: ChatOut) -> Bytes {
    match o {
        ChatOut::Chunk(v) => encode(None, &v.to_string()),
        ChatOut::Done => encode(None, "[DONE]"),
    }
}

/// Decoder + encoder with usage/error tapping on the hub events.
pub(crate) struct Pipeline {
    parser: SseParser,
    decoder: Decoder,
    encoder: Encoder,
    pub usage: Usage,
    pub error: Option<ProxyError>,
    /// Content has started (errors after this can't change the HTTP status)
    pub committed: bool,
    pub early_error: Option<ProxyError>,
    pub ended: bool,
}

impl Pipeline {
    pub fn new(decoder: Decoder, encoder: Encoder) -> Self {
        Pipeline {
            parser: SseParser::new(),
            decoder,
            encoder,
            usage: Usage::default(),
            error: None,
            committed: false,
            early_error: None,
            ended: false,
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Bytes> {
        let mut out = Vec::new();
        for sse in self.parser.push(chunk) {
            for ev in self.decoder.push(&sse) {
                self.handle(ev, &mut out);
            }
        }
        out
    }

    pub fn feed_events(&mut self, evs: Vec<StreamEvent>) -> Vec<Bytes> {
        let mut out = Vec::new();
        for ev in evs {
            self.handle(ev, &mut out);
        }
        out
    }

    /// Inject a transport failure.
    pub fn fail(&mut self, msg: &str) -> Vec<Bytes> {
        self.feed_events(vec![StreamEvent::error("api_error", msg)])
    }

    pub fn finish(&mut self) -> Vec<Bytes> {
        let mut out = Vec::new();
        for sse in self.parser.finish() {
            for ev in self.decoder.push(&sse) {
                self.handle(ev, &mut out);
            }
        }
        for ev in self.decoder.finish() {
            self.handle(ev, &mut out);
        }
        out.extend(self.encoder.finish());
        self.ended = true;
        out
    }

    pub fn keepalive(&self) -> Bytes {
        self.encoder.keepalive()
    }

    fn handle(&mut self, ev: StreamEvent, out: &mut Vec<Bytes>) {
        match &ev {
            StreamEvent::MessageStart { message } => self.usage = message.usage.clone(),
            StreamEvent::MessageDelta { usage, .. } => {
                merge_delta_usage(&mut self.usage, usage);
                self.committed = true;
            }
            StreamEvent::Error { error } => {
                let kind = ErrorKind::from_anthropic_type(&error.kind).unwrap_or(ErrorKind::Api);
                let e = ProxyError::new(kind, error.message.clone());
                if !self.committed && self.early_error.is_none() {
                    self.early_error = Some(e.clone());
                }
                self.error = Some(e);
                self.ended = true;
            }
            StreamEvent::MessageStop => {
                self.committed = true;
                self.ended = true;
            }
            StreamEvent::ContentBlockStart { .. } | StreamEvent::ContentBlockDelta { .. } => {
                self.committed = true
            }
            _ => {}
        }
        out.extend(self.encoder.push(&ev));
    }
}

/// Read upstream until the translated stream commits (or fails early), so an
/// upstream error before any content becomes a proper HTTP error status. Gives up
/// waiting after `limit` (long silent reasoning) so the client gets headers.
pub(crate) async fn prime(
    pipe: &mut Pipeline,
    upstream: &mut ByteStream,
    limit: Duration,
) -> Result<Vec<Bytes>, ProxyError> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut pending = Vec::new();
    while !pipe.committed && !pipe.ended {
        let next = match tokio::time::timeout_at(deadline, upstream.next()).await {
            Ok(n) => n,
            Err(_) => break,
        };
        match next {
            None => {
                pending.extend(pipe.finish());
                break;
            }
            Some(Err(e)) => {
                return Err(ProxyError::api(format!("upstream stream: {e}")).with_status(502));
            }
            Some(Ok(chunk)) => pending.extend(pipe.feed(&chunk)),
        }
        if let Some(e) = pipe.early_error.take() {
            return Err(e);
        }
    }
    if let Some(e) = pipe.early_error.take() {
        return Err(e);
    }
    Ok(pending)
}

pub(crate) fn translated_body(
    mut upstream: ByteStream,
    mut pipe: Pipeline,
    pending: Vec<Bytes>,
    mut rec: Recorder,
    keepalive: Duration,
) -> Body {
    let s = async_stream::stream! {
        for b in pending {
            yield Ok::<Bytes, Infallible>(b);
        }
        let mut transport_err: Option<String> = None;
        while !pipe.ended {
            match tokio::time::timeout(keepalive, upstream.next()).await {
                Err(_) => yield Ok(pipe.keepalive()),
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    transport_err = Some(format!("upstream stream: {e}"));
                    break;
                }
                Ok(Some(Ok(chunk))) => {
                    for b in pipe.feed(&chunk) {
                        yield Ok(b);
                    }
                }
            }
        }
        if let Some(e) = &transport_err {
            for b in pipe.fail(e) {
                yield Ok(b);
            }
        }
        for b in pipe.finish() {
            yield Ok(b);
        }
        rec.set_usage(&pipe.usage);
        let err = transport_err.or_else(|| pipe.error.as_ref().map(|e| e.message.clone()));
        if err.is_some() {
            rec.mark_upstream_error();
        }
        rec.finish(200, err);
    };
    Body::from_stream(s)
}

// ------------------------------------------------------------------ passthrough

/// Watches same-dialect SSE for usage and errors without touching the bytes.
pub(crate) struct Tap {
    dialect: Dialect,
    parser: SseParser,
    pub usage: Usage,
    pub error: Option<String>,
}

impl Tap {
    pub fn new(dialect: Dialect) -> Self {
        Tap {
            dialect,
            parser: SseParser::new(),
            usage: Usage::default(),
            error: None,
        }
    }

    pub fn observe(&mut self, chunk: &[u8]) {
        for ev in self.parser.push(chunk) {
            self.event(&ev);
        }
    }

    pub fn finish(&mut self) {
        for ev in self.parser.finish() {
            self.event(&ev);
        }
    }

    fn event(&mut self, ev: &SseEvent) {
        let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
            return;
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        match self.dialect {
            Dialect::Anthropic => match kind {
                "message_start" => {
                    if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                        self.usage = serde_json::from_value(u.clone()).unwrap_or_default();
                    }
                }
                "message_delta" => {
                    if let Some(u) = v
                        .get("usage")
                        .and_then(|u| serde_json::from_value(u.clone()).ok())
                    {
                        merge_delta_usage(&mut self.usage, &u);
                    }
                }
                "error" => self.error = Some(error_message(&v)),
                _ => {}
            },
            Dialect::Chat => {
                if let Some(u) = v.get("usage").filter(|u| !u.is_null())
                    && let Ok(u) = serde_json::from_value::<ChatUsage>(u.clone())
                {
                    self.usage = usage_from_chat(&u);
                }
                if v.get("error").is_some() {
                    self.error = Some(error_message(&v));
                }
            }
            Dialect::Responses => match kind {
                "response.completed" | "response.incomplete" => {
                    if let Some(u) = v.get("response").and_then(|r| r.get("usage"))
                        && let Ok(u) = serde_json::from_value::<ResponsesUsage>(u.clone())
                    {
                        self.usage = usage_from_responses(&u);
                    }
                }
                "response.failed" | "error" => {
                    self.error = Some(error_message(v.get("response").unwrap_or(&v)));
                }
                _ => {}
            },
        }
    }
}

fn error_message(v: &Value) -> String {
    v.get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| v.get("message").and_then(Value::as_str))
        .unwrap_or("upstream stream error")
        .to_string()
}

/// Usage from a complete (non-streaming) JSON body of the given dialect.
pub(crate) fn usage_from_body(dialect: Dialect, v: &Value) -> Option<Usage> {
    let u = v.get("usage")?.clone();
    match dialect {
        Dialect::Anthropic => serde_json::from_value(u).ok(),
        Dialect::Chat => serde_json::from_value::<ChatUsage>(u)
            .ok()
            .map(|u| usage_from_chat(&u)),
        Dialect::Responses => serde_json::from_value::<ResponsesUsage>(u)
            .ok()
            .map(|u| usage_from_responses(&u)),
    }
}

pub(crate) fn passthrough_body(
    mut upstream: ByteStream,
    dialect: Dialect,
    mut rec: Recorder,
) -> Body {
    let s = async_stream::stream! {
        let mut tap = Tap::new(dialect);
        let mut transport_err = None;
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(b) => {
                    tap.observe(&b);
                    yield Ok::<Bytes, Infallible>(b);
                }
                Err(e) => {
                    transport_err = Some(format!("upstream stream: {e}"));
                    break;
                }
            }
        }
        tap.finish();
        rec.set_usage(&tap.usage);
        let err = transport_err.or(tap.error);
        if err.is_some() {
            rec.mark_upstream_error();
        }
        rec.finish(200, err);
    };
    Body::from_stream(s)
}

/// Accumulates a Responses stream into the final response object (for
/// non-streaming Responses clients on the always-streaming ChatGPT backend).
pub(crate) async fn collect_responses_stream(
    mut upstream: ByteStream,
) -> Result<Value, ProxyError> {
    let mut parser = SseParser::new();
    let mut items: Vec<(u64, Value)> = Vec::new();
    let mut done: Option<Value> = None;
    let mut handle = |ev: SseEvent| -> Result<(), ProxyError> {
        let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
            return Ok(());
        };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.output_item.done" => {
                let idx = v
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(items.len() as u64);
                items.push((idx, v.get("item").cloned().unwrap_or_default()));
            }
            "response.completed" | "response.incomplete" => done = v.get("response").cloned(),
            "response.failed" | "error" => {
                let err = v
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .or_else(|| v.get("error"))
                    .unwrap_or(&v);
                return Err(ProxyError::from_stream_error(err));
            }
            _ => {}
        }
        Ok(())
    };
    while let Some(chunk) = upstream.next().await {
        let chunk =
            chunk.map_err(|e| ProxyError::api(format!("upstream stream: {e}")).with_status(502))?;
        for ev in parser.push(&chunk) {
            handle(ev)?;
        }
    }
    for ev in parser.finish() {
        handle(ev)?;
    }
    let mut resp =
        done.ok_or_else(|| ProxyError::api("upstream stream ended unexpectedly").with_status(502))?;
    let empty = resp
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(|a| a.is_empty());
    if empty {
        items.sort_by_key(|(i, _)| *i);
        resp["output"] = Value::Array(items.into_iter().map(|(_, v)| v).collect());
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_reads_usage_for_each_dialect() {
        let mut t = Tap::new(Dialect::Anthropic);
        t.observe(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":1,\"cache_read_input_tokens\":5}}}\n\n");
        t.observe(b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":9}}\n\n");
        assert_eq!(t.usage.input_tokens, 7);
        assert_eq!(t.usage.output_tokens, 9);
        assert_eq!(t.usage.cache_read_input_tokens, Some(5));

        let mut t = Tap::new(Dialect::Chat);
        t.observe(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n");
        assert_eq!(t.usage.input_tokens, 10);

        let mut t = Tap::new(Dialect::Responses);
        t.observe(b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":3}}}\n\n");
        assert_eq!(t.usage.output_tokens, 3);
    }

    #[test]
    fn pipeline_early_error_and_keepalive() {
        let mut p = Pipeline::new(
            Decoder::new(Dialect::Chat, "m"),
            Encoder::new(
                Dialect::Anthropic,
                "m",
                false,
                HashSet::new(),
                Arc::new(ReasoningCache::new()),
            ),
        );
        assert_eq!(p.keepalive(), comment("keepalive"));
        p.feed(b"data: {\"error\":{\"message\":\"quota\",\"code\":\"rate_limit_exceeded\"}}\n\n");
        assert_eq!(p.early_error.as_ref().unwrap().status, 429);
    }
}
