//! Live voice: talk to bro and its agents, hands-free, from the phone app or any browser.
//!
//! The client sends its WebRTC offer to `POST /api/voice/calls`. bro creates the OpenAI Realtime
//! call with its own key, returns the SDP answer, and attaches a sideband WebSocket to that same
//! call ([`realtime`]). Audio flows directly between the client and OpenAI (echo cancellation and
//! jitter handling stay in the client's WebRTC stack); everything the model knows and does goes
//! through bro:
//!
//! - [`brain`]: the standing instructions and a compact digest of every session, refreshed into the
//!   conversation as things change.
//! - [`tools`]: bro's whole orchestrator tool set (read, message, answer, open, focus, close…) plus
//!   voice-native ones (message an agent and hear back, stop it, hand a job to the orchestrator).
//!   Destructive actions wait for the user's own spoken "yes" or a tap ([`tools::Gate`]).
//! - [`feed`]: agents finishing turns, asking questions, sessions opening and closing, orchestrator
//!   replies. Spoken proactively when the conversation is idle, never acted on by the model alone.
//!
//! The key never leaves this machine and no client ever holds it. Every client event goes out on
//! the bridge socket as `{"type":"voice", ...}` so phones and pages render captions, tool activity
//! and confirmation cards without parsing provider events.

pub(crate) mod brain;
pub(crate) mod feed;
pub(crate) mod realtime;
pub(crate) mod tools;
#[cfg(test)]
mod tests;

use crate::model::{ServerEvent, iso_now};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;

/// The realtime model when nothing else is configured.
pub const DEFAULT_MODEL: &str = "gpt-realtime-2.1";
/// The spoken voice when the client doesn't pick one.
pub const DEFAULT_VOICE: &str = "marin";
/// Speech recognition for the captions and for the confirmation gate.
pub const DEFAULT_TRANSCRIPTION: &str = "gpt-4o-mini-transcribe";
/// Voices the realtime models speak with.
pub const VOICES: [&str; 10] = ["marin", "cedar", "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"];
/// Calls one bro serves at once (a phone and a desk browser, say).
const MAX_CALLS: usize = 4;
const MAX_SDP: usize = 64 * 1024;

/// What the voice proactively speaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Announce {
    /// Every agent that finishes a turn or needs input.
    #[default]
    All,
    /// Only agents the user talked to on this call, plus anyone waiting on the user.
    Asked,
    /// Nothing; the updates still reach the model as context.
    Off,
}

impl Announce {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "all" | "everything" => Some(Self::All),
            "asked" | "mine" | "watched" => Some(Self::Asked),
            "off" | "none" | "quiet" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Asked => "asked",
            Self::Off => "off",
        }
    }
}

/// bro's live voice settings (`[voice]` in v2.toml, set through [`crate::Bridge::set_voice_config`]).
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceConfig {
    /// Phones and pages may start calls.
    pub enabled: bool,
    pub model: String,
    pub voice: String,
    pub transcription: String,
    /// Environment variable with the OpenAI key (process, then the user's saved variables).
    pub api_key_env: String,
    pub announce: Announce,
    /// The Realtime API root; tests point it at a local stand-in.
    pub api_base: String,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: DEFAULT_MODEL.into(),
            voice: DEFAULT_VOICE.into(),
            transcription: DEFAULT_TRANSCRIPTION.into(),
            api_key_env: "OPENAI_API_KEY".into(),
            announce: Announce::All,
            api_base: "https://api.openai.com/v1".into(),
        }
    }
}

/// The key, from this process or (Windows) the user's saved variables, so a key set after bro
/// started is still found. Never logged or sent to a client.
pub(crate) fn resolve_key(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        return None;
    }
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            bro_core::envfile::system_vars()
                .ok()?
                .into_iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
}

/// A live call as bro's own window shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveCall {
    pub id: String,
    /// "phone", "web", …
    pub client: String,
    pub started_at: String,
    /// connecting, listening, hearing, thinking, speaking
    pub state: String,
}

/// What a running call's task is told.
#[derive(Debug)]
pub(crate) enum Input {
    Notice(feed::Notice),
    ToolDone { call_id: String, output: Value },
    Confirm { action_id: String, approve: bool },
    Focus(Option<String>),
    /// The client's audio is connected: greet now rather than into the void.
    Ready,
    Hangup,
}

/// One live conversation.
pub(crate) struct Call {
    pub id: String,
    pub client: String,
    pub started_at: String,
    pub voice: String,
    pub focused: Mutex<Option<String>>,
    pub announce: Mutex<Announce>,
    /// Sessions this call messaged or watches: announced even when [`Announce::Asked`].
    pub watched: Mutex<Vec<String>>,
    pub state: Mutex<&'static str>,
    pub ended: AtomicBool,
    pub inbox: mpsc::UnboundedSender<Input>,
    /// Orchestrator requests this call made, by tag.
    pub asks: AtomicU64,
}

impl Call {
    pub fn send(&self, input: Input) -> bool {
        !self.ended.load(Ordering::Acquire) && self.inbox.send(input).is_ok()
    }

    pub fn watches(&self, session_id: &str) -> bool {
        self.watched.lock().iter().any(|id| id == session_id)
    }

    pub fn watch(&self, session_id: &str) {
        let mut watched = self.watched.lock();
        if !watched.iter().any(|id| id == session_id) {
            watched.push(session_id.to_owned());
        }
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "client": self.client,
            "startedAt": self.started_at,
            "voice": self.voice,
            "state": *self.state.lock(),
            "focusedSessionId": *self.focused.lock(),
            "announce": self.announce.lock().as_str(),
        })
    }
}

/// Every live call and the shared settings, in [`AppState::voice`].
#[derive(Default)]
pub(crate) struct VoiceHub {
    config: RwLock<VoiceConfig>,
    calls: Mutex<HashMap<String, Arc<Call>>>,
    feed_running: AtomicBool,
    sink_registered: AtomicBool,
    /// The latest workspace updates, for `recent_events` and `workspace_brief`.
    events: Mutex<std::collections::VecDeque<String>>,
    http: std::sync::OnceLock<reqwest::Client>,
}

impl VoiceHub {
    pub fn config(&self) -> VoiceConfig {
        self.config.read().clone()
    }

    pub fn set_config(&self, config: VoiceConfig) {
        *self.config.write() = config;
    }

    pub fn calls(&self) -> Vec<Arc<Call>> {
        self.calls.lock().values().cloned().collect()
    }

    pub fn call(&self, id: &str) -> Option<Arc<Call>> {
        self.calls.lock().get(id).cloned()
    }

    pub(crate) fn remove(&self, id: &str) {
        self.calls.lock().remove(id);
    }

    pub(crate) fn http(&self) -> reqwest::Client {
        self.http
            .get_or_init(|| {
                reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .unwrap_or_default()
            })
            .clone()
    }

    /// The live calls, cheaply (no key lookup).
    pub fn live_calls(&self) -> Vec<LiveCall> {
        let mut calls: Vec<LiveCall> = self
            .calls()
            .iter()
            .map(|call| LiveCall {
                id: call.id.clone(),
                client: call.client.clone(),
                started_at: call.started_at.clone(),
                state: (*call.state.lock()).to_owned(),
            })
            .collect();
        calls.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        calls
    }

    /// Why a call can't start right now, in words for the client.
    pub(crate) fn unavailable(&self) -> Option<String> {
        let config = self.config();
        if !config.enabled {
            return Some("Live voice is turned off in bro's settings.".into());
        }
        if resolve_key(&config.api_key_env).is_none() {
            return Some(format!(
                "bro needs an OpenAI key for live voice. Set {} on this computer, then try again.",
                config.api_key_env
            ));
        }
        None
    }

    /// The status clients show before and during calls.
    pub fn status(&self) -> Value {
        let config = self.config();
        let reason = self.unavailable();
        json!({
            "available": reason.is_none(),
            "reason": reason,
            "model": config.model,
            "voice": config.voice,
            "voices": VOICES,
            "announce": config.announce.as_str(),
            "calls": self.calls().iter().map(|call| call.json()).collect::<Vec<_>>(),
        })
    }
}

/// `{"type":"voice","callId":…,"event":…}` to every bridge client.
pub(crate) fn publish(app: &AppState, call_id: &str, mut event: Value) {
    if let Some(object) = event.as_object_mut() {
        object.insert("type".into(), json!("voice"));
        object.insert("callId".into(), json!(call_id));
        object.insert("at".into(), json!(iso_now()));
    }
    app.publish(ServerEvent::global(event));
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "message": message.into() }))).into_response()
}

// ----- HTTP ----------------------------------------------------------------------------------

pub(crate) async fn status(State(app): State<AppState>) -> Json<Value> {
    Json(app.voice.status())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartCall {
    sdp: String,
    #[serde(default)]
    voice: Option<String>,
    #[serde(default)]
    client: Option<String>,
    #[serde(default)]
    focused_session_id: Option<String>,
    #[serde(default)]
    announce: Option<String>,
}

/// `POST /api/voice/calls`: the client's SDP offer in, bro's answer out.
pub(crate) async fn start(State(app): State<AppState>, Json(body): Json<StartCall>) -> Response {
    if let Some(reason) = app.voice.unavailable() {
        return error(StatusCode::SERVICE_UNAVAILABLE, reason);
    }
    if body.sdp.trim().is_empty() || body.sdp.len() > MAX_SDP || !body.sdp.trim_start().starts_with("v=0") {
        return error(StatusCode::BAD_REQUEST, "Send the WebRTC offer's SDP.");
    }
    if app.voice.calls.lock().len() >= MAX_CALLS {
        return error(StatusCode::CONFLICT, "Too many voice calls are open. End one first.");
    }
    let config = app.voice.config();
    let Some(key) = resolve_key(&config.api_key_env) else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "bro couldn't read its OpenAI key.");
    };
    let voice = body
        .voice
        .as_deref()
        .map(str::trim)
        .filter(|voice| VOICES.contains(voice))
        .unwrap_or(config.voice.as_str())
        .to_owned();
    let announce = body.announce.as_deref().and_then(Announce::parse).unwrap_or(config.announce);
    let focused = body.focused_session_id.filter(|id| app.summary(id).is_some());
    let client = body
        .client
        .map(|client| client.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect::<String>())
        .filter(|client| !client.is_empty())
        .unwrap_or_else(|| "web".into());
    let instructions = brain::instructions(&app, focused.as_deref(), &client);
    let session = realtime::session_config(&config, &voice, &instructions, &realtime::transcription_prompt(&app));
    let created = match realtime::create_call(&app.voice.http(), &config.api_base, &key, &body.sdp, &session).await {
        Ok(created) => created,
        Err(message) => {
            tracing::warn!("bro-bridge: voice call failed to start: {message}");
            return error(StatusCode::BAD_GATEWAY, message);
        }
    };
    let (inbox, rx) = mpsc::unbounded_channel();
    let call = Arc::new(Call {
        id: created.call_id.clone(),
        client,
        started_at: iso_now(),
        voice: voice.clone(),
        focused: Mutex::new(focused),
        announce: Mutex::new(announce),
        watched: Mutex::new(Vec::new()),
        state: Mutex::new("connecting"),
        ended: AtomicBool::new(false),
        inbox,
        asks: AtomicU64::new(0),
    });
    app.voice.calls.lock().insert(call.id.clone(), call.clone());
    feed::ensure_running(&app);
    tokio::spawn(realtime::run(app.clone(), call.clone(), key, rx));
    publish(&app, &call.id, json!({"event":"started","call":call.json()}));
    Json(json!({
        "callId": call.id,
        "sdp": created.answer_sdp,
        "model": config.model,
        "voice": voice,
        "announce": announce.as_str(),
    }))
    .into_response()
}

/// `POST /api/voice/calls/{id}/hangup`.
pub(crate) async fn hangup(State(app): State<AppState>, Path(id): Path<String>) -> Response {
    match app.voice.call(&id) {
        Some(call) => {
            call.send(Input::Hangup);
            Json(json!({ "ok": true })).into_response()
        }
        None => error(StatusCode::NOT_FOUND, "That call already ended."),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CallUpdate {
    #[serde(default, deserialize_with = "nullable")]
    focused_session_id: Option<Option<String>>,
    #[serde(default)]
    announce: Option<String>,
    #[serde(default)]
    ready: bool,
}

fn nullable<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

/// `POST /api/voice/calls/{id}/context`: the chat the user has open, how chatty to be.
pub(crate) async fn update(State(app): State<AppState>, Path(id): Path<String>, Json(body): Json<CallUpdate>) -> Response {
    let Some(call) = app.voice.call(&id) else {
        return error(StatusCode::NOT_FOUND, "That call already ended.");
    };
    if body.ready {
        call.send(Input::Ready);
    }
    if let Some(focused) = body.focused_session_id {
        let focused = focused.filter(|id| app.summary(id).is_some());
        call.send(Input::Focus(focused));
    }
    if let Some(mode) = body.announce.as_deref() {
        let Some(mode) = Announce::parse(mode) else {
            return error(StatusCode::BAD_REQUEST, "announce is all, asked or off.");
        };
        *call.announce.lock() = mode;
        publish(&app, &call.id, json!({"event":"announce","announce":mode.as_str()}));
    }
    Json(call.json()).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfirmBody {
    action_id: String,
    approve: bool,
}

/// `POST /api/voice/calls/{id}/confirm`: a tap on the confirmation card.
pub(crate) async fn confirm(State(app): State<AppState>, Path(id): Path<String>, Json(body): Json<ConfirmBody>) -> Response {
    match app.voice.call(&id) {
        Some(call) if call.send(Input::Confirm { action_id: body.action_id, approve: body.approve }) => {
            Json(json!({ "ok": true })).into_response()
        }
        _ => error(StatusCode::NOT_FOUND, "That call already ended."),
    }
}

/// `GET /voice`: the browser client (any desktop browser; phones use the app).
pub(crate) async fn page() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::HeaderName::from_static("permissions-policy"), "microphone=(self)"),
        ],
        include_str!("page.html"),
    )
        .into_response()
}
