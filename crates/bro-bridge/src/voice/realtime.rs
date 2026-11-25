//! OpenAI Realtime: creating the call for a client's WebRTC offer, and the sideband socket bro holds
//! on it for the call's whole life.
//!
//! [`Conductor`] is the socket-free part (what to send when, which tools may run, when the line is
//! quiet enough to speak an update) so it can be tested event by event; [`run`] owns the socket.

use super::feed::{Kind, Notice};
use super::tools::{self, Effect as ToolEffect, Verdict};
use super::{Call, Input, VoiceConfig, publish};
use crate::state::AppState;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// A slow tool answers "still running" after this long and reports its outcome later.
const SOFT_TOOL_WAIT: Duration = Duration::from_millis(3500);
/// Quiet needed after the user's last words before bro speaks an update.
const AFTER_USER: Duration = Duration::from_millis(1500);
/// Least time between two unprompted updates.
const BETWEEN_ANNOUNCEMENTS: Duration = Duration::from_secs(4);
/// Context-only updates are batched into one conversation item at most this often.
const CONTEXT_BATCH: Duration = Duration::from_secs(8);
/// Speech rate used to guess when spoken audio ends, when the provider doesn't say.
const CHARS_PER_SECOND: f32 = 14.0;
/// The greeting goes out once the client says its audio is up, or after this long regardless.
const GREET_FALLBACK: Duration = Duration::from_secs(4);
const TICK: Duration = Duration::from_millis(200);

// Directives ride in a `[bro]` system item rather than per-response `instructions`, which would
// replace the session's instructions (and with them the workspace digest) for that response.
const ANNOUNCE: &str = "[bro] Pass the [bro] update(s) above on to the user now (not the background ones): one short spoken sentence each, leading \
with the session's name (\"Toast finished: the tests pass now.\"). For an agent waiting on them, say what it asks and \
the options in one breath. Merge several into one or two sentences. No greeting, no filler, no questions of your own.";

const GREETING: &str = "[bro] The user just connected their headphones. Greet them in two or three words, then the \
state of play in one short sentence from the Sessions list in your instructions: how many agents are working and \
who is waiting on them (or that nothing is open). No questions.";

fn system_item(text: &str) -> Value {
    json!({
        "type": "conversation.item.create",
        "item": {"type": "message", "role": "system", "content": [{"type": "input_text", "text": text}]}
    })
}

/// A response bro asks for itself: never with tools, so updates can't make the model act.
fn quiet_response(kind: &str) -> Value {
    json!({"type": "response.create", "response": {"tool_choice": "none", "metadata": {"bro": kind}}})
}

/// Names speech recognition should spell exactly: the squad, the coach, the projects.
pub(crate) fn transcription_prompt(app: &AppState) -> String {
    let mut words: Vec<String> = ["bro", "Hugh", "Mal", "Pinky", "Toast", "Minty", "Lilac", "Sky", "Bitty", "Claude", "Codex"]
        .iter()
        .map(|word| word.to_string())
        .collect();
    for project in app.projects().into_iter().take(30) {
        words.push(project.name);
    }
    for summary in app.summaries().into_iter().take(30) {
        words.push(crate::orchestrator::cast::name_of(&summary.title).to_owned());
    }
    words.sort();
    words.dedup();
    format!("A developer talking to their coding agents. Names: {}.", words.join(", "))
}

/// The session the call starts with: model, voice, instructions, tools, turn-taking for headphones.
pub(crate) fn session_config(config: &VoiceConfig, voice: &str, instructions: &str, vocabulary: &str) -> Value {
    json!({
        "type": "realtime",
        "model": config.model,
        "instructions": instructions,
        "output_modalities": ["audio"],
        "audio": {
            "input": {
                "noise_reduction": {"type": "near_field"},
                "transcription": {"model": config.transcription, "prompt": vocabulary},
                "turn_detection": {"type": "semantic_vad", "eagerness": "auto", "create_response": true, "interrupt_response": true}
            },
            "output": {"voice": voice}
        },
        "tools": tools::realtime_tools(),
        "tool_choice": "auto"
    })
}

pub(crate) struct Created {
    pub call_id: String,
    pub answer_sdp: String,
}

/// `POST {base}/realtime/calls` with the client's offer and bro's session; the call id comes back
/// in the `Location` header.
pub(crate) async fn create_call(http: &reqwest::Client, base: &str, key: &str, sdp: &str, session: &Value) -> Result<Created, String> {
    let form = reqwest::multipart::Form::new().text("sdp", sdp.to_owned()).text("session", session.to_string());
    let response = http
        .post(format!("{}/realtime/calls", base.trim_end_matches('/')))
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|error| format!("Couldn't reach OpenAI: {error}"))?;
    let status = response.status();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| body.chars().take(300).collect());
        return Err(format!("OpenAI refused the call ({status}): {message}"));
    }
    let call_id = location
        .as_deref()
        .and_then(|location| location.trim_end_matches('/').rsplit('/').next())
        .map(|id| id.split('?').next().unwrap_or(id).to_owned())
        .filter(|id| !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .ok_or("OpenAI answered without a call id.")?;
    if !body.trim_start().starts_with("v=0") {
        return Err("OpenAI's answer wasn't an SDP.".into());
    }
    Ok(Created { call_id, answer_sdp: body })
}

async fn hangup_upstream(http: &reqwest::Client, base: &str, key: &str, call_id: &str) {
    let _ = http
        .post(format!("{}/realtime/calls/{call_id}/hangup", base.trim_end_matches('/')))
        .bearer_auth(key)
        .send()
        .await;
}

fn sideband_url(base: &str, call_id: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = base.strip_prefix("https://").map(|rest| format!("wss://{rest}")).unwrap_or_else(|| {
        base.strip_prefix("http://").map(|rest| format!("ws://{rest}")).unwrap_or_else(|| base.to_owned())
    });
    format!("{base}/realtime?call_id={call_id}")
}

fn tls() -> tokio_tungstenite::Connector {
    // An explicit crypto provider: bro links rustls with both ring and aws-lc-rs, and rustls refuses
    // to choose one on its own.
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
        .expect("ring supports the default TLS versions");
    tokio_tungstenite::Connector::Rustls(Arc::new(config))
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(base: &str, key: &str, call_id: &str) -> Result<Socket, String> {
    let mut last = String::new();
    for attempt in 0..4 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(400 * attempt)).await;
        }
        let mut request = sideband_url(base, call_id).into_client_request().map_err(|error| error.to_string())?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {key}").parse().map_err(|_| "The OpenAI key isn't a valid header value.".to_string())?,
        );
        let connector = request.uri().scheme_str().is_some_and(|scheme| scheme == "wss").then(tls);
        match tokio_tungstenite::connect_async_tls_with_config(request, None, true, connector).await {
            Ok((socket, _)) => return Ok(socket),
            Err(error) => last = error.to_string(),
        }
    }
    Err(format!("bro couldn't attach to the call: {last}"))
}

/// The call's life: attach, conduct, clean up.
pub(crate) async fn run(app: AppState, call: Arc<Call>, key: String, mut inbox: mpsc::UnboundedReceiver<Input>) {
    let config = app.voice.config();
    let http = app.voice.http();
    let ended = match connect(&config.api_base, &key, &call.id).await {
        Ok(socket) => conduct(&app, &call, socket, &mut inbox).await,
        Err(error) => {
            tracing::warn!("bro-bridge: {error}");
            Some(error)
        }
    };
    call.ended.store(true, Ordering::Release);
    hangup_upstream(&http, &config.api_base, &key, &call.id).await;
    app.voice.remove(&call.id);
    *call.state.lock() = "ended";
    publish(&app, &call.id, json!({"event":"ended","reason":ended}));
}

async fn conduct(app: &AppState, call: &Arc<Call>, socket: Socket, inbox: &mut mpsc::UnboundedReceiver<Input>) -> Option<String> {
    let (mut sink, mut stream) = socket.split();
    let mut conductor = Conductor::new(Instant::now());
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Input>();
    let mut ticker = tokio::time::interval(TICK);
    let mut shutdown = app.shutdown.subscribe();
    publish(app, &call.id, json!({"event":"state","state":"listening"}));
    *call.state.lock() = "listening";
    loop {
        let effects = tokio::select! {
            message = stream.next() => match message {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<Value>(&text) {
                    Ok(event) => conductor.on_server(app, call, &event, Instant::now()),
                    Err(_) => Vec::new(),
                },
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| frame.reason.to_string()).filter(|reason| !reason.is_empty()),
                Some(Ok(_)) => Vec::new(),
                Some(Err(error)) => return Some(format!("The call's connection dropped: {error}")),
                None => return None,
            },
            input = inbox.recv() => match input {
                Some(Input::Hangup) | None => return None,
                Some(input) => conductor.on_input(app, call, input, Instant::now()),
            },
            done = done_rx.recv() => match done {
                Some(input) => conductor.on_input(app, call, input, Instant::now()),
                None => Vec::new(),
            },
            _ = ticker.tick() => conductor.tick(app, call, Instant::now()),
            _ = shutdown.changed() => return Some("bro is shutting down.".into()),
        };
        for effect in effects {
            match effect {
                Effect::Send(event) => {
                    if let Err(error) = sink.send(Message::Text(event.to_string().into())).await {
                        return Some(format!("The call's connection dropped: {error}"));
                    }
                }
                Effect::Ui(event) => publish(app, &call.id, event),
                Effect::Run { call_id, name, args } => spawn_tool(app.clone(), call.clone(), done_tx.clone(), call_id, name, args),
                Effect::Hangup => return None,
            }
        }
        if let Some(state) = conductor.take_state_change() {
            *call.state.lock() = state;
            publish(app, &call.id, json!({"event":"state","state":state}));
        }
    }
}

/// Runs a tool off the call's loop. A slow one answers "still running" and reports later.
fn spawn_tool(app: AppState, call: Arc<Call>, done: mpsc::UnboundedSender<Input>, call_id: Option<String>, name: String, args: Value) {
    tokio::spawn(async move {
        let summary = tools::describe(&app, &name, &args);
        let ui_id = call_id.clone().unwrap_or_else(|| format!("tap-{}", uuid::Uuid::new_v4().simple()));
        publish(&app, &call.id, json!({"event":"tool","id":ui_id,"name":name,"summary":summary,"status":"running"}));
        let work = tools::execute(&app, &call, &name, &args);
        tokio::pin!(work);
        let finished = |result: &Result<String, String>| {
            json!({"event":"tool","id":ui_id,"name":name,"summary":summary,
                "status": if result.is_ok() { "done" } else { "failed" },
                "result": super::brain::clip(result.as_ref().unwrap_or_else(|error| error), 240)})
        };
        let report_later = |result: Result<String, String>| {
            let text = match &result {
                Ok(text) => format!("{summary} — done: {}", super::brain::clip(text, 700)),
                Err(error) => format!("{summary} — failed: {}", super::brain::clip(error, 400)),
            };
            let mut notice = Notice::new(Kind::Tool, text, None);
            notice.call = Some(call.id.clone());
            notice
        };
        match call_id {
            Some(call_id) => match tokio::time::timeout(SOFT_TOOL_WAIT, &mut work).await {
                Ok(result) => {
                    publish(&app, &call.id, finished(&result));
                    let _ = done.send(Input::ToolDone { call_id, output: tools::output(result) });
                }
                Err(_) => {
                    let _ = done.send(Input::ToolDone {
                        call_id,
                        output: json!({"ok": true, "status": "running", "result": "Started; still running in the background. bro will report the outcome. Tell the user it's underway."}),
                    });
                    let result = work.await;
                    publish(&app, &call.id, finished(&result));
                    let _ = done.send(Input::Notice(report_later(result)));
                }
            },
            None => {
                let result = work.await;
                publish(&app, &call.id, finished(&result));
                let _ = done.send(Input::Notice(report_later(result)));
            }
        }
    });
}

/// What the conductor asks of the socket loop.
#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    /// An event for the provider.
    Send(Value),
    /// A voice event for bro's clients.
    Ui(Value),
    /// Run an allowed tool; `call_id` is the model's function call, `None` for a tap.
    Run { call_id: Option<String>, name: String, args: Value },
    Hangup,
}

struct Pending {
    text: String,
    speak: bool,
}

/// Turn-taking, tools and announcements for one call, free of the socket.
pub(crate) struct Conductor {
    started: Instant,
    /// The response being generated, and whether bro asked for it (an update or the greeting).
    responding: Option<(String, bool)>,
    /// Tool call ids per response.
    response_calls: HashMap<String, Vec<String>>,
    /// Tool calls still running.
    running: HashSet<String>,
    /// Responses that finished while their tools ran.
    finished: HashSet<String>,
    /// Tool outputs are in; the model should continue once the line is free.
    continue_due: bool,
    user_speaking: bool,
    last_user: Option<Instant>,
    /// The provider reports playback (`output_audio_buffer.*`).
    playback_events: bool,
    audio_playing: bool,
    /// Estimated end of the last reply's audio, for providers that don't report playback.
    quiet_until: Instant,
    reply_chars: HashMap<String, usize>,
    pending: Vec<Pending>,
    last_context: Instant,
    last_announcement: Option<Instant>,
    pub(crate) gate: tools::Gate,
    hangup_after_reply: bool,
    /// bro sent `response.create` at this time and no `response.created` came yet.
    requested: Option<Instant>,
    greeted: bool,
    ready: bool,
    state: &'static str,
    published_state: &'static str,
}

impl Conductor {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            responding: None,
            response_calls: HashMap::new(),
            running: HashSet::new(),
            finished: HashSet::new(),
            continue_due: false,
            user_speaking: false,
            last_user: None,
            playback_events: false,
            audio_playing: false,
            quiet_until: now,
            reply_chars: HashMap::new(),
            pending: Vec::new(),
            last_context: now,
            last_announcement: None,
            gate: tools::Gate::default(),
            hangup_after_reply: false,
            requested: None,
            greeted: false,
            ready: false,
            state: "listening",
            published_state: "listening",
        }
    }

    fn set_state(&mut self) {
        self.state = if self.user_speaking {
            "hearing"
        } else if self.audio_playing || (!self.playback_events && self.responding.is_some()) {
            "speaking"
        } else if self.responding.is_some() || !self.running.is_empty() || self.continue_due {
            "thinking"
        } else {
            "listening"
        };
    }

    /// The state clients show, when it changed.
    pub fn take_state_change(&mut self) -> Option<&'static str> {
        self.set_state();
        (self.state != self.published_state).then(|| {
            self.published_state = self.state;
            self.state
        })
    }

    /// Nobody is talking, nothing is being generated, no tool is out.
    fn line_free(&self, now: Instant) -> bool {
        self.responding.is_none()
            && !self.user_speaking
            && !self.audio_playing
            && (self.playback_events || now >= self.quiet_until)
            && self.running.is_empty()
            && !self.continue_due
            && self.last_user.is_none_or(|at| now.duration_since(at) >= AFTER_USER)
    }

    pub fn on_server(&mut self, app: &AppState, call: &Call, event: &Value, now: Instant) -> Vec<Effect> {
        let kind = event["type"].as_str().unwrap_or("");
        let mut effects = Vec::new();
        match kind {
            "input_audio_buffer.speech_started" => {
                self.user_speaking = true;
                self.last_user = Some(now);
            }
            "input_audio_buffer.speech_stopped" => {
                self.user_speaking = false;
                self.last_user = Some(now);
            }
            "conversation.item.input_audio_transcription.delta" => {
                effects.push(Effect::Ui(json!({"event":"transcript","role":"user","itemId":event["item_id"],"delta":event["delta"]})));
            }
            "conversation.item.input_audio_transcription.completed" => {
                let text = event["transcript"].as_str().unwrap_or("").trim().to_owned();
                self.gate.heard(&text, now);
                self.last_user = Some(now);
                effects.push(Effect::Ui(json!({"event":"transcript","role":"user","itemId":event["item_id"],"text":text,"final":true})));
            }
            "response.created" => {
                let id = event["response"]["id"].as_str().unwrap_or("").to_owned();
                let ours = event["response"]["metadata"]["bro"].is_string();
                self.responding = Some((id, ours));
                self.requested = None;
            }
            "response.output_audio_transcript.delta" => {
                let id = event["response_id"].as_str().unwrap_or("").to_owned();
                let delta = event["delta"].as_str().unwrap_or("");
                *self.reply_chars.entry(id).or_default() += delta.chars().count();
                effects.push(Effect::Ui(json!({"event":"transcript","role":"assistant","itemId":event["item_id"],"delta":delta})));
            }
            "response.output_audio_transcript.done" => {
                effects.push(Effect::Ui(json!({"event":"transcript","role":"assistant","itemId":event["item_id"],"text":event["transcript"],"final":true})));
            }
            "output_audio_buffer.started" => {
                self.playback_events = true;
                self.audio_playing = true;
            }
            "output_audio_buffer.stopped" | "output_audio_buffer.cleared" => {
                self.playback_events = true;
                self.audio_playing = false;
            }
            "response.output_item.done" => {
                let item = &event["item"];
                if item["type"] == "function_call" {
                    let response_id = event["response_id"].as_str().unwrap_or("").to_owned();
                    let call_id = item["call_id"].as_str().unwrap_or("").to_owned();
                    let name = item["name"].as_str().unwrap_or("").to_owned();
                    let args = serde_json::from_str::<Value>(item["arguments"].as_str().unwrap_or("{}")).unwrap_or(Value::Null);
                    if !call_id.is_empty() && !self.response_calls.values().flatten().any(|id| *id == call_id) {
                        self.response_calls.entry(response_id).or_default().push(call_id.clone());
                        effects.extend(self.function_call(app, call_id, &name, args, now));
                    }
                }
            }
            "response.done" => {
                let response = &event["response"];
                let id = response["id"].as_str().unwrap_or("").to_owned();
                if self.responding.as_ref().is_some_and(|(current, _)| *current == id) || self.responding.as_ref().is_some_and(|(current, _)| current.is_empty()) {
                    self.responding = None;
                }
                let chars = self.reply_chars.remove(&id).unwrap_or(0);
                let speech = Duration::from_secs_f32(chars as f32 / CHARS_PER_SECOND);
                self.quiet_until = self.quiet_until.max(now + speech);
                if let Some(calls) = self.response_calls.get(&id) {
                    if calls.iter().any(|call_id| self.running.contains(call_id)) {
                        self.finished.insert(id);
                    } else {
                        self.response_calls.remove(&id);
                        self.continue_due = true;
                    }
                }
                if response["status"] == "failed" {
                    let message = response["status_details"]["error"]["message"].as_str().unwrap_or("The voice model hit an error.");
                    effects.push(Effect::Ui(json!({"event":"error","message":message})));
                }
            }
            "error" => {
                let message = event["error"]["message"].as_str().unwrap_or("The voice service reported an error.");
                // A response bro asked for that was refused (one was already running) never starts.
                if self.requested.take().is_some() && self.responding.as_ref().is_some_and(|(id, _)| id.is_empty()) {
                    self.responding = None;
                }
                // Harmless races (cancelling a response that already ended) aren't worth showing.
                if !message.contains("active response") {
                    tracing::warn!("bro-bridge: realtime error: {message}");
                    effects.push(Effect::Ui(json!({"event":"error","message":message})));
                }
            }
            _ => {}
        }
        effects.extend(self.tick(app, call, now));
        effects
    }

    fn output(call_id: &str, output: &Value) -> Effect {
        Effect::Send(json!({
            "type": "conversation.item.create",
            "item": {"type": "function_call_output", "call_id": call_id, "output": output.to_string()}
        }))
    }

    /// The model called a tool: answer it now, park it for the user's yes, or run it.
    fn function_call(&mut self, app: &AppState, call_id: String, name: &str, args: Value, now: Instant) -> Vec<Effect> {
        let answer = |output: Value| vec![Self::output(&call_id, &output)];
        if !args.is_object() {
            return answer(json!({"ok": false, "error": "The arguments weren't a JSON object."}));
        }
        match name {
            "confirm_action" => {
                let id = args["actionId"].as_str().unwrap_or("");
                return match self.gate.confirm(id, now) {
                    Verdict::Go(parked) => {
                        let mut effects = vec![Effect::Ui(json!({"event":"confirm_resolved","actionId":id,"approved":true}))];
                        self.running.insert(call_id.clone());
                        effects.push(Effect::Run { call_id: Some(call_id), name: parked.name, args: parked.args });
                        effects
                    }
                    Verdict::NotYet(why) => answer(json!({"ok": false, "error": why})),
                    Verdict::Declined(_) => {
                        let mut effects = answer(json!({"ok": true, "result": "The user said no, so nothing was done."}));
                        effects.push(Effect::Ui(json!({"event":"confirm_resolved","actionId":id,"approved":false})));
                        effects
                    }
                    Verdict::Unknown => answer(json!({"ok": false, "error": "Nothing is waiting under that id (it may have expired). Call the original tool again."})),
                };
            }
            "cancel_action" => {
                let id = args["actionId"].as_str().unwrap_or("");
                let found = self.gate.take(id).is_some();
                let mut effects = answer(json!({"ok": true, "result": if found { "Dropped." } else { "Nothing was waiting." }}));
                effects.push(Effect::Ui(json!({"event":"confirm_resolved","actionId":id,"approved":false})));
                return effects;
            }
            "end_call" => {
                self.hangup_after_reply = true;
                return answer(json!({"ok": true, "result": "Say a short goodbye; bro hangs up when you finish."}));
            }
            _ => {}
        }
        match tools::effect_of(name, &args) {
            None => answer(json!({"ok": false, "error": format!("There's no tool called {name}.")})),
            Some(ToolEffect::Confirm) => {
                let summary = tools::describe(app, name, &args);
                let action_id = self.gate.park(name, &args, summary.clone(), now);
                let mut effects = answer(json!({
                    "ok": true,
                    "needsConfirmation": true,
                    "actionId": action_id,
                    "ask": format!("{summary}?"),
                    "result": "Not done yet. Ask the user in a few words; after a clear yes call confirm_action with this actionId. bro checks their actual words."
                }));
                effects.push(Effect::Ui(json!({"event":"confirm","actionId":action_id,"text":summary})));
                effects
            }
            Some(_) => {
                self.running.insert(call_id.clone());
                vec![Effect::Run { call_id: Some(call_id), name: name.to_owned(), args }]
            }
        }
    }

    pub fn on_input(&mut self, app: &AppState, call: &Call, input: Input, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        match input {
            Input::ToolDone { call_id, output } => {
                self.running.remove(&call_id);
                effects.push(Self::output(&call_id, &output));
                let done: Vec<String> = self
                    .finished
                    .iter()
                    .filter(|id| self.response_calls.get(*id).is_none_or(|calls| calls.iter().all(|c| !self.running.contains(c))))
                    .cloned()
                    .collect();
                for id in done {
                    self.finished.remove(&id);
                    self.response_calls.remove(&id);
                    self.continue_due = true;
                }
            }
            Input::Notice(notice) => {
                let speak = notice.speak_for(call);
                self.pending.push(Pending { text: notice.text.clone(), speak });
                effects.push(Effect::Ui(json!({"event":"notice","kind":format!("{:?}", notice.kind).to_lowercase(),"text":notice.text,"sessionId":notice.session_id,"spoken":speak})));
            }
            Input::Confirm { action_id, approve } => {
                if let Some(parked) = self.gate.take(&action_id) {
                    effects.push(Effect::Ui(json!({"event":"confirm_resolved","actionId":action_id,"approved":approve})));
                    if approve {
                        effects.push(Effect::Run { call_id: None, name: parked.name.clone(), args: parked.args.clone() });
                        self.pending.push(Pending { text: format!("The user approved on screen: {}. Running it now.", parked.summary), speak: false });
                    } else {
                        self.pending.push(Pending { text: format!("The user declined on screen: {}. Leave it.", parked.summary), speak: true });
                    }
                }
            }
            Input::Focus(focused) => {
                *call.focused.lock() = focused.clone();
                if let Some(line) = super::brain::focus_line(app, focused.as_deref()) {
                    self.pending.push(Pending { text: line, speak: false });
                }
            }
            Input::Ready => self.ready = true,
            Input::Hangup => effects.push(Effect::Hangup),
        }
        effects.extend(self.tick(app, call, now));
        effects
    }

    pub fn tick(&mut self, _app: &AppState, _call: &Call, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        // A requested response that never started doesn't hold the line forever.
        if self.requested.is_some_and(|at| now.duration_since(at) >= Duration::from_secs(8)) {
            self.requested = None;
            if self.responding.as_ref().is_some_and(|(id, _)| id.is_empty()) {
                self.responding = None;
            }
        }
        if self.continue_due && self.responding.is_none() && !self.user_speaking {
            self.continue_due = false;
            effects.push(Effect::Send(json!({"type": "response.create"})));
            self.responding = Some((String::new(), false));
            self.requested = Some(now);
            return effects;
        }
        if !self.greeted && (self.ready || now.duration_since(self.started) >= GREET_FALLBACK) {
            self.greeted = true;
            if self.line_free(now) && self.last_user.is_none() {
                effects.push(Effect::Send(system_item(GREETING)));
                effects.push(Effect::Send(quiet_response("greeting")));
                self.responding = Some((String::new(), true));
                self.requested = Some(now);
                return effects;
            }
        }
        if self.hangup_after_reply && self.line_free(now) {
            effects.push(Effect::Hangup);
            return effects;
        }
        if self.pending.is_empty() || !self.line_free(now) {
            return effects;
        }
        let speak = self.pending.iter().any(|pending| pending.speak)
            && self.last_announcement.is_none_or(|at| now.duration_since(at) >= BETWEEN_ANNOUNCEMENTS);
        if !speak && now.duration_since(self.last_context) < CONTEXT_BATCH {
            return effects;
        }
        let mut text = self
            .pending
            .drain(..)
            .map(|pending| format!("[bro{}] {}", if pending.speak { "" } else { ", background" }, pending.text))
            .collect::<Vec<_>>()
            .join("\n");
        self.last_context = now;
        if speak {
            text.push('\n');
            text.push_str(ANNOUNCE);
        }
        effects.push(Effect::Send(system_item(&text)));
        if speak {
            self.last_announcement = Some(now);
            effects.push(Effect::Send(quiet_response("announce")));
            self.responding = Some((String::new(), true));
            self.requested = Some(now);
        }
        effects
    }
}
