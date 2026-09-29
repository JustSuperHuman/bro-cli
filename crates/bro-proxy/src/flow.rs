//! Request flow for the three completion endpoints: parse, map the model, then
//! either pass through (same dialect) or translate via the Anthropic hub, send
//! upstream and render the response/stream in the inbound dialect.

use crate::anthropic::{Accumulator, MessagesRequest, MessagesResponse, events_from_message};
use crate::errors::{ErrorKind, ProxyError};
use crate::events::Recorder;
use crate::models::{self, Resolved};
use crate::openai::{ChatRequest, ChatResponse, ResponsesRequest};
use crate::pipeline::{self, Decoder, Encoder, Pipeline};
use crate::sse::SseParser;
use crate::state::AppState;
use crate::translate::{self, Target, a2chat, a2responses, chat2a, responses2a};
use crate::upstream::{self, ByteStream, Endpoint, UpstreamCall, UpstreamResponse, chatgpt};
use crate::util::short_hash;
use crate::{Dialect, Route, Upstream};
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// How long to hold a translated stream back waiting for its first content, so
/// early upstream errors can still be returned with a real HTTP status.
const PRIME_LIMIT: Duration = Duration::from_secs(10);

pub(crate) async fn handle(
    state: Arc<AppState>,
    inbound: Dialect,
    route_id: String,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(route) = state.route(&route_id) else {
        return ProxyError::new(
            ErrorKind::NotFound,
            format!("unknown proxy route '{route_id}'"),
        )
        .into_response(inbound);
    };
    let mut slot = Some(Recorder::new(state.clone(), &route, inbound));
    match run(&state, &route, inbound, &headers, &body, &mut slot).await {
        Ok(resp) => resp,
        Err(e) => {
            if let Some(rec) = slot.take() {
                rec.finish(e.status_for(inbound), Some(e.message.clone()));
            }
            e.into_response(inbound)
        }
    }
}

struct Ctx<'a> {
    state: &'a AppState,
    route: &'a Route,
    inbound: Dialect,
    headers: &'a HeaderMap,
    client_model: String,
    stream: bool,
    resolved: Resolved,
    codex_auth: Option<bro_core::creds::CodexAuth>,
    chatgpt: bool,
}

async fn run(
    state: &AppState,
    route: &Route,
    inbound: Dialect,
    headers: &HeaderMap,
    body: &Bytes,
    slot: &mut Option<Recorder>,
) -> Result<Response, ProxyError> {
    let json: Value = serde_json::from_slice(body)
        .map_err(|e| ProxyError::invalid(format!("request body is not valid JSON: {e}")))?;
    if !json.is_object() {
        return Err(ProxyError::invalid("request body must be a JSON object"));
    }
    let client_model = json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let stream = json.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let resolved = models::resolve(state, route, &client_model).await;
    if let Some(rec) = slot.as_mut() {
        rec.set_stream(stream);
        rec.set_model(&resolved.model);
    }
    let codex_auth = match &route.upstream {
        Upstream::ChatGptCodex { codex_home } => Some(state.codex_auth(codex_home, false).await?),
        _ => None,
    };
    let chatgpt = codex_auth.as_ref().is_some_and(chatgpt::is_chatgpt_login);
    let ctx = Ctx {
        state,
        route,
        inbound,
        headers,
        client_model,
        stream,
        resolved,
        codex_auth,
        chatgpt,
    };
    if inbound == route.upstream.dialect() {
        passthrough(ctx, json, slot).await
    } else {
        translated(ctx, json, slot).await
    }
}

fn endpoint_for(d: Dialect) -> Endpoint {
    match d {
        Dialect::Anthropic => Endpoint::Messages,
        Dialect::Chat => Endpoint::Chat,
        Dialect::Responses => Endpoint::Responses,
    }
}

// ------------------------------------------------------------------ passthrough

async fn passthrough(
    ctx: Ctx<'_>,
    mut json: Value,
    slot: &mut Option<Recorder>,
) -> Result<Response, ProxyError> {
    let Ctx {
        state,
        route,
        inbound,
        headers,
        stream,
        resolved,
        codex_auth,
        chatgpt,
        ..
    } = ctx;
    let obj = json.as_object_mut().expect("checked object");
    obj.insert("model".into(), json!(resolved.model));
    let mut upstream_stream = stream;
    let mut collect = false;
    match inbound {
        Dialect::Chat => {
            if let Some(e) = &resolved.effort {
                obj.insert("reasoning_effort".into(), json!(e));
            }
        }
        Dialect::Responses => {
            if let Some(e) = &resolved.effort {
                let r = obj.entry("reasoning").or_insert_with(|| json!({}));
                r["effort"] = json!(e);
            }
            if chatgpt {
                // ChatGPT backend contract (verified in v1): stateless, always streaming,
                // instructions required, no max_output_tokens / sampling params.
                obj.insert("store".into(), json!(false));
                for k in [
                    "max_output_tokens",
                    "temperature",
                    "top_p",
                    "previous_response_id",
                ] {
                    obj.remove(k);
                }
                if obj
                    .get("instructions")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    obj.insert(
                        "instructions".into(),
                        json!(a2responses::CHATGPT_DEFAULT_INSTRUCTIONS),
                    );
                }
                if !stream {
                    obj.insert("stream".into(), json!(true));
                    upstream_stream = true;
                    collect = true;
                }
            }
        }
        Dialect::Anthropic => {}
    }
    let session_key = session_of(inbound, &json);
    let call = UpstreamCall {
        endpoint: endpoint_for(inbound),
        body: Some(Bytes::from(serde_json::to_vec(&json).unwrap_or_default())),
        stream: upstream_stream,
        client_headers: if inbound == Dialect::Anthropic {
            upstream::forwardable_client_headers(headers)
        } else {
            HeaderMap::new()
        },
        session_key,
        translated: false,
        codex_auth,
    };
    let resp = upstream::send(state, route, &call).await?;
    let account = resp.account.clone();
    if let Some(rec) = slot.as_mut() {
        rec.set_account(account.clone());
    }
    if collect {
        let v = pipeline::collect_responses_stream(resp.body).await?;
        let rec = slot.take().expect("recorder");
        finish_json(rec, inbound, &v);
        return Ok(json_response(StatusCode::OK, &v, account.as_deref()));
    }
    if stream && resp.is_sse() {
        let rec = slot.take().expect("recorder");
        let headers = passthrough_headers(&resp.headers);
        let body = pipeline::passthrough_body(resp.body, inbound, rec);
        return Ok(sse_response(body, account.as_deref(), headers));
    }
    let status = resp.status;
    let headers = passthrough_headers(&resp.headers);
    let bytes = upstream::read_all(resp.body).await?;
    let rec = slot.take().expect("recorder");
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) => finish_json(rec, inbound, &v),
        Err(_) => rec.finish(status.as_u16(), None),
    }
    let mut out = (status, bytes).into_response();
    out.headers_mut().extend(headers);
    add_account(&mut out, account.as_deref());
    Ok(out)
}

fn finish_json(mut rec: Recorder, dialect: Dialect, v: &Value) {
    if let Some(u) = pipeline::usage_from_body(dialect, v) {
        rec.set_usage(&u);
    }
    rec.finish(200, None);
}

fn session_of(dialect: Dialect, v: &Value) -> Option<String> {
    let s = match dialect {
        Dialect::Anthropic => v.get("metadata").and_then(|m| m.get("user_id")),
        Dialect::Chat => v.get("user"),
        Dialect::Responses => v.get("prompt_cache_key").or_else(|| v.get("user")),
    };
    s.and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ------------------------------------------------------------------ translated

async fn translated(
    ctx: Ctx<'_>,
    json: Value,
    slot: &mut Option<Recorder>,
) -> Result<Response, ProxyError> {
    let Ctx {
        state,
        route,
        inbound,
        headers,
        client_model,
        stream,
        resolved,
        codex_auth,
        chatgpt,
    } = ctx;
    let up = route.upstream.dialect();
    let claude_oauth = matches!(
        route.upstream,
        Upstream::ClaudeOAuth { .. } | Upstream::ClaudePool { .. }
    );
    let base = match &route.upstream {
        Upstream::OpenAiChat { base_url, .. } | Upstream::OpenAiResponses { base_url, .. } => {
            base_url.to_lowercase()
        }
        _ => String::new(),
    };
    let mut target = Target {
        model: resolved.model.clone(),
        effort_override: resolved.effort.clone(),
        chatgpt,
        strict_schema: base.contains("generativelanguage.googleapis.com"),
        max_completion_tokens: base.contains("api.openai.com") || base.contains("openai.azure.com"),
        supported_efforts: resolved.efforts.clone(),
        cache_key: None,
        claude_oauth,
    };

    // 1. inbound → hub (Anthropic)
    let mut custom = HashSet::new();
    let mut include_usage = false;
    let hub: MessagesRequest = match inbound {
        Dialect::Anthropic => serde_json::from_value(json)
            .map_err(|e| ProxyError::invalid(format!("invalid Anthropic Messages request: {e}")))?,
        Dialect::Chat => {
            let req: ChatRequest = serde_json::from_value(json).map_err(|e| {
                ProxyError::invalid(format!("invalid Chat Completions request: {e}"))
            })?;
            include_usage = req.include_usage();
            chat2a::request(&req, &target, &state.reasoning)
        }
        Dialect::Responses => {
            let req: ResponsesRequest = serde_json::from_value(json)
                .map_err(|e| ProxyError::invalid(format!("invalid Responses request: {e}")))?;
            if req.previous_response_id.is_some() {
                tracing::warn!(
                    "previous_response_id is not supported by the translating proxy; send full input"
                );
            }
            custom = responses2a::custom_tool_names(&req);
            responses2a::request(&req, &target)
        }
    };
    let session = hub.user_id();
    target.cache_key = Some(
        session
            .as_deref()
            .map(short_hash)
            .unwrap_or_else(|| state.session_id.clone()),
    );

    // 2. hub → upstream
    let (body, up_stream) = match up {
        Dialect::Anthropic => {
            let mut h = hub;
            h.model = target.model.clone();
            if claude_oauth {
                translate::ensure_claude_code_system(&mut h);
            }
            translate::add_cache_breakpoints(&mut h);
            h.stream = Some(stream);
            (serde_json::to_vec(&h), stream)
        }
        Dialect::Chat => (serde_json::to_vec(&a2chat::request(&hub, &target)), stream),
        Dialect::Responses => {
            let r = a2responses::request(&hub, &target);
            let s = r.stream.unwrap_or(false);
            (serde_json::to_vec(&r), s)
        }
    };
    let body = body.map_err(|e| ProxyError::api(format!("serialize upstream request: {e}")))?;
    let call = UpstreamCall {
        endpoint: endpoint_for(up),
        body: Some(Bytes::from(body)),
        stream: up_stream,
        client_headers: if up == Dialect::Anthropic && inbound == Dialect::Anthropic {
            upstream::forwardable_client_headers(headers)
        } else {
            HeaderMap::new()
        },
        session_key: session,
        translated: inbound != Dialect::Anthropic,
        codex_auth,
    };
    let resp = upstream::send(state, route, &call).await?;
    let account = resp.account.clone();
    if let Some(rec) = slot.as_mut() {
        rec.set_account(account.clone());
    }

    // 3. upstream → inbound
    if stream {
        let decoder = Decoder::new(up, &client_model);
        let encoder = Encoder::new(
            inbound,
            &client_model,
            include_usage,
            custom,
            state.reasoning.clone(),
        );
        let mut pipe = Pipeline::new(decoder, encoder);
        let is_sse = resp.is_sse() || up_stream;
        let mut body = resp.body;
        let pending = if is_sse {
            pipeline::prime(&mut pipe, &mut body, PRIME_LIMIT).await?
        } else {
            // Upstream answered with JSON despite stream:true: synthesize the stream.
            let bytes = upstream::read_all(body).await?;
            let m = decode_json(up, &bytes, &client_model)?;
            body = Box::pin(futures_util::stream::empty());
            pipe.feed_events(events_from_message(&m))
        };
        let rec = slot.take().expect("recorder");
        let out = pipeline::translated_body(body, pipe, pending, rec, state.opts.keepalive);
        return Ok(sse_response(out, account.as_deref(), HeaderMap::new()));
    }

    let m = if resp.is_sse() || up_stream {
        accumulate(up, resp, &client_model).await?
    } else {
        let bytes = upstream::read_all(resp.body).await?;
        decode_json(up, &bytes, &client_model)?
    };
    let out = match inbound {
        Dialect::Anthropic => {
            let mut m = m.clone();
            m.model = client_model.clone();
            serde_json::to_value(&m).unwrap_or_default()
        }
        Dialect::Chat => chat2a::response(&m, &client_model, &state.reasoning),
        Dialect::Responses => responses2a::response(&m, &client_model, &custom),
    };
    let mut rec = slot.take().expect("recorder");
    rec.set_usage(&m.usage);
    rec.finish(200, None);
    Ok(json_response(StatusCode::OK, &out, account.as_deref()))
}

/// A complete upstream JSON body → hub message.
fn decode_json(
    up: Dialect,
    bytes: &[u8],
    client_model: &str,
) -> Result<MessagesResponse, ProxyError> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| {
        ProxyError::api(format!("upstream returned invalid JSON: {e}")).with_status(502)
    })?;
    if let Some(err) = v.get("error").filter(|e| e.is_object()) {
        return Err(ProxyError::from_stream_error(err));
    }
    Ok(match up {
        Dialect::Anthropic => serde_json::from_value(v).map_err(|e| {
            ProxyError::api(format!("unexpected Anthropic response: {e}")).with_status(502)
        })?,
        Dialect::Chat => {
            let r: ChatResponse = serde_json::from_value(v).map_err(|e| {
                ProxyError::api(format!("unexpected Chat Completions response: {e}"))
                    .with_status(502)
            })?;
            a2chat::response(&r, client_model)
        }
        Dialect::Responses => a2responses::response(&v, client_model),
    })
}

/// Fold an upstream stream into one hub message (non-streaming clients).
async fn accumulate(
    up: Dialect,
    resp: UpstreamResponse,
    client_model: &str,
) -> Result<MessagesResponse, ProxyError> {
    let mut body: ByteStream = resp.body;
    let mut parser = SseParser::new();
    let mut decoder = Decoder::new(up, client_model);
    let mut acc = Accumulator::new();
    while let Some(chunk) = body.next().await {
        let chunk =
            chunk.map_err(|e| ProxyError::api(format!("upstream stream: {e}")).with_status(502))?;
        for sse in parser.push(&chunk) {
            decoder.push(&sse).iter().for_each(|e| acc.push(e));
        }
        if acc.stopped || acc.error.is_some() {
            break;
        }
    }
    if !acc.stopped && acc.error.is_none() {
        for sse in parser.finish() {
            decoder.push(&sse).iter().for_each(|e| acc.push(e));
        }
        decoder.finish().iter().for_each(|e| acc.push(e));
    }
    if let Some(err) = acc.error.take() {
        let kind = ErrorKind::from_anthropic_type(&err.kind).unwrap_or(ErrorKind::Api);
        return Err(ProxyError::new(kind, err.message));
    }
    acc.finish()
        .ok_or_else(|| ProxyError::api("upstream stream ended without a message").with_status(502))
}

// ------------------------------------------------------------------ responses

const PASS_HEADERS: &[&str] = &[
    "request-id",
    "x-request-id",
    "openai-processing-ms",
    "retry-after",
];

fn passthrough_headers(h: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in h {
        let n = k.as_str();
        if n == "content-type"
            || PASS_HEADERS.contains(&n)
            || n.starts_with("anthropic-ratelimit-")
            || n.starts_with("x-ratelimit-")
        {
            out.append(k.clone(), v.clone());
        }
    }
    out
}

fn add_account(resp: &mut Response, account: Option<&str>) {
    if let Some(a) = account.and_then(|a| HeaderValue::from_str(a).ok()) {
        resp.headers_mut().insert("x-pool-account", a);
    }
}

fn json_response(status: StatusCode, v: &Value, account: Option<&str>) -> Response {
    let mut r = (status, axum::Json(v)).into_response();
    add_account(&mut r, account);
    r
}

fn sse_response(body: Body, account: Option<&str>, extra: HeaderMap) -> Response {
    let mut r = Response::new(body);
    let h = r.headers_mut();
    h.extend(extra);
    h.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    h.insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    add_account(&mut r, account);
    r
}
