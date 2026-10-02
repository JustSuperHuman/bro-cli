//! GPT-Live mode: speech in, speech out, over one Live session
//! (`wss://api.openai.com/v1/live/sessions`) with client delegation.
//!
//! GPT-Live is full duplex and paces itself by the input stream, so while a
//! session is open the microphone stream never stops: real audio while the
//! hotkey is held, silence otherwise (muting the input freezes the session and
//! it would never speak a late result). When the model hands work to the app
//! (`session.delegation.created`, which carries no text) the words heard since
//! the previous delegation become [`VoiceEvent::Delegation`]; the host runs
//! them through its own agent and answers with [`crate::Voice::delegation_result`],
//! which GPT-Live then speaks. Sessions bill per connected second, so an idle
//! session closes and the next press opens a new one (audio spoken while it
//! connects is buffered, not lost).
//!
//! [`Protocol`] is the socket-free part (transcripts, delegation timing) so it
//! can be tested; [`run`] owns the WebSocket on its own thread.

use crate::playback::Player;
use crate::worker::Msg;
use crate::{Shared, VoiceEvent};
use base64::Engine;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

pub const DEFAULT_LIVE_MODEL: &str = "gpt-live-1";
pub const DEFAULT_LIVE_URL: &str = "wss://api.openai.com/v1/live/sessions";
pub const DEFAULT_LIVE_VOICE: &str = "cedar";
/// PCM rate sent to and received from GPT-Live.
pub const LIVE_RATE: u32 = 24_000;

/// Words are complete once no transcript delta arrived for this long.
const HEARD_QUIET: Duration = Duration::from_millis(350);
/// Words nobody delegated are shown after this long without more.
const HEARD_FLUSH: Duration = Duration::from_millis(1500);
/// A delegation waits at most this long for the transcript to settle.
const DELEGATION_WAIT: Duration = Duration::from_millis(1200);
const SAID_QUIET: Duration = Duration::from_millis(900);
const START_TIMEOUT: Duration = Duration::from_secs(15);
const READ_POLL: Duration = Duration::from_millis(10);
/// Delegations unanswered for this long no longer keep the session open.
const DELEGATION_TTL: Duration = Duration::from_secs(600);

/// GPT-Live settings; `Some` in [`crate::VoiceConfig::live`] selects live mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveConfig {
    pub model: String,
    pub voice: String,
    /// The voice's standing instructions (persona, when to delegate).
    pub instructions: String,
    pub url: String,
    /// Close the session after this long with nothing going on.
    pub idle_secs: u32,
}

impl Default for LiveConfig {
    fn default() -> Self {
        LiveConfig {
            model: DEFAULT_LIVE_MODEL.into(),
            voice: DEFAULT_LIVE_VOICE.into(),
            instructions: "You are a voice assistant. Delegate anything that needs tools or knowledge of the user's \
                           workspace; while it runs say two or three words at most, then speak the result briefly."
                .into(),
            url: DEFAULT_LIVE_URL.into(),
            idle_secs: 90,
        }
    }
}

/// What the worker (and the host, through the worker) sends the session.
pub enum Out {
    /// 24 kHz PCM16 little-endian, already silenced when the key is up.
    Audio(Vec<u8>),
    Talking(bool),
    Json(Value),
    Close,
}

/// What handling a server event asks for.
#[derive(Debug, PartialEq)]
pub enum Action {
    Play(Vec<u8>),
    Emit(VoiceEvent),
}

/// Transcript assembly and delegation timing, free of the socket.
#[derive(Default)]
pub struct Protocol {
    /// Words heard since the last delegation (the delegation's request).
    request: String,
    /// Words heard since the last Heard event.
    heard: String,
    last_heard: Option<Instant>,
    said: String,
    last_said: Option<Instant>,
    /// Delegation waiting for its words to settle: (id, created).
    waiting: Option<(String, Instant)>,
    /// Delegations handed to the host and not yet answered.
    pub open: HashSet<String>,
    opened_at: Vec<(String, Instant)>,
    /// Last event that means the conversation is still going.
    pub last_activity: Option<Instant>,
    /// The hotkey is held: the user may still be mid-sentence.
    pub talking: bool,
}

impl Protocol {
    pub fn on_server(&mut self, event: &Value, now: Instant) -> Vec<Action> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let mut actions = Vec::new();
        match kind {
            k if k.ends_with("output_audio.delta") || k == "response.audio.delta" => {
                if let Some(bytes) = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
                {
                    actions.push(Action::Play(bytes));
                }
            }
            "session.input_transcript.delta" => {
                let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                self.request.push_str(delta);
                self.heard.push_str(delta);
                self.last_heard = Some(now);
                self.last_activity = Some(now);
            }
            "session.output_transcript.delta" | "response.text.delta" => {
                self.said.push_str(event.get("delta").and_then(Value::as_str).unwrap_or(""));
                self.last_said = Some(now);
                self.last_activity = Some(now);
            }
            "session.delegation.created" => {
                if let Some(id) = event.pointer("/delegation/id").and_then(Value::as_str) {
                    // A second delegation before the first was dispatched: send
                    // the first with what we have.
                    if let Some((previous, _)) = self.waiting.take() {
                        actions.extend(self.dispatch(previous));
                    }
                    self.waiting = Some((id.to_string(), now));
                }
                self.last_activity = Some(now);
            }
            "error" => {
                let message = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("GPT-Live reported an error")
                    .to_string();
                actions.push(Action::Emit(VoiceEvent::Error(format!("GPT-Live: {message}"))));
            }
            _ => {}
        }
        actions
    }

    fn dispatch(&mut self, id: String) -> Vec<Action> {
        let request = std::mem::take(&mut self.request).trim().to_string();
        self.open.insert(id.clone());
        self.opened_at.push((id.clone(), Instant::now()));
        let mut actions = Vec::new();
        // The request is the settled utterance: show it as heard now rather
        // than in pieces as late transcript words trickle in.
        if !std::mem::take(&mut self.heard).trim().is_empty() && !request.is_empty() {
            actions.push(Action::Emit(VoiceEvent::Heard(request.clone())));
        }
        actions.push(Action::Emit(VoiceEvent::Delegation { id, request }));
        actions
    }

    /// Time-based flushes; call often.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let mut actions = Vec::new();
        // Pauses between words are normal while the key is held.
        let heard_quiet = !self.talking && self.last_heard.is_none_or(|at| now.duration_since(at) >= HEARD_QUIET);
        if let Some((_, created)) = &self.waiting
            && (heard_quiet || now.duration_since(*created) >= DELEGATION_WAIT)
        {
            let (id, _) = self.waiting.take().expect("checked");
            actions.extend(self.dispatch(id));
        }
        // Words that weren't delegated (small talk): shown once they have
        // clearly stopped arriving.
        let heard_done = !self.talking && self.last_heard.is_some_and(|at| now.duration_since(at) >= HEARD_FLUSH);
        if heard_done && self.waiting.is_none() && !self.heard.trim().is_empty() {
            actions.push(Action::Emit(VoiceEvent::Heard(std::mem::take(&mut self.heard).trim().to_string())));
        }
        if self.last_said.is_some_and(|at| now.duration_since(at) >= SAID_QUIET) && !self.said.trim().is_empty() {
            actions.push(Action::Emit(VoiceEvent::Said(std::mem::take(&mut self.said).trim().to_string())));
            self.last_said = None;
        }
        let stale: Vec<String> = self
            .opened_at
            .iter()
            .filter(|(_, at)| now.duration_since(*at) >= DELEGATION_TTL)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.answered(&id);
        }
        actions
    }

    pub fn answered(&mut self, id: &str) {
        self.open.remove(id);
        self.opened_at.retain(|(open, _)| open != id);
    }

    pub fn busy(&self) -> bool {
        self.waiting.is_some() || !self.open.is_empty()
    }
}

pub fn session_start(cfg: &LiveConfig) -> Value {
    json!({
        "type": "session.start",
        "session": {
            "model": cfg.model,
            "instructions": cfg.instructions,
            "audio": {
                "format": { "type": "audio/pcm", "rate": LIVE_RATE },
                "output": { "voice": cfg.voice }
            },
            "delegation": { "type": "client" }
        }
    })
}

pub fn commentary(delegation_id: &str, content: &str) -> Value {
    json!({ "type": "session.commentary.append", "delegation_id": delegation_id, "content": content })
}

pub fn thinking(delegation_id: &str, content: &str) -> Value {
    json!({ "type": "session.thinking.append", "delegation_id": delegation_id, "content": content })
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn set_read_timeout(ws: &mut Socket, timeout: Option<Duration>) {
    let _ = match ws.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout),
        MaybeTlsStream::Rustls(stream) => stream.get_mut().set_read_timeout(timeout),
        _ => Ok(()),
    };
}

fn send(ws: &mut Socket, value: &Value) -> Result<(), String> {
    ws.send(Message::text(value.to_string())).map_err(|e| format!("GPT-Live connection lost: {e}"))
}

enum Read {
    Event(Value),
    Nothing,
    Closed(Option<String>),
}

fn read(ws: &mut Socket) -> Read {
    match ws.read() {
        Ok(Message::Text(text)) => serde_json::from_str(&text).map(Read::Event).unwrap_or(Read::Nothing),
        Ok(Message::Close(_)) => Read::Closed(None),
        Ok(_) => Read::Nothing,
        Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
            Read::Nothing
        }
        Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => Read::Closed(None),
        Err(e) => Read::Closed(Some(format!("GPT-Live connection lost: {e}"))),
    }
}

fn connect(cfg: &LiveConfig, key: &str) -> Result<Socket, String> {
    let mut request = tungstenite::client::IntoClientRequest::into_client_request(cfg.url.as_str())
        .map_err(|e| format!("bad GPT-Live URL {}: {e}", cfg.url))?;
    let auth = format!("Bearer {key}").parse().map_err(|_| "the API key has characters a header can't carry".to_string())?;
    request.headers_mut().insert("Authorization", auth);
    let uri = request.uri().clone();
    let host = uri.host().ok_or_else(|| format!("bad GPT-Live URL {}", cfg.url))?.to_string();
    let port = uri.port_u16().unwrap_or(443);
    let tcp = TcpStream::connect((host.as_str(), port)).map_err(|e| format!("could not reach GPT-Live ({host}): {e}"))?;
    let _ = tcp.set_nodelay(true);
    // An explicit crypto provider: bro links rustls with both ring and
    // aws-lc-rs, and rustls refuses (panics) to choose one on its own.
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS setup failed: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tungstenite::Connector::Rustls(Arc::new(tls));
    let (mut ws, _) = tungstenite::client_tls_with_config(request, tcp, None, Some(connector)).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)) => {
            let body = response.body().as_ref().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            format!("GPT-Live refused the connection ({}): {}", response.status(), body.chars().take(300).collect::<String>())
        }
        other => format!("could not reach GPT-Live: {other}"),
    })?;
    set_read_timeout(&mut ws, Some(READ_POLL));
    Ok(ws)
}

/// The session's life on its own thread: connect, start, pump audio and
/// events until closed or idle, then tell the worker (`Msg::LiveClosed`).
pub fn run(
    shared: Arc<Shared>,
    cfg: LiveConfig,
    key: String,
    player: Option<Arc<Player>>,
    outgoing: Receiver<Out>,
    worker: Sender<Msg>,
    generation: u64,
) {
    // A panic must not leave the worker talking into a dead session.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session(&shared, &cfg, &key, player.as_deref(), &outgoing)
    }))
    .unwrap_or_else(|panic| {
        let what = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        Err(format!("the voice session crashed: {what}"))
    });
    if let Err(message) = result {
        shared.log(format!("live: ended with error: {message}"));
        shared.emit(VoiceEvent::Error(message));
    }
    let _ = worker.send(Msg::LiveClosed(generation));
}

fn session(shared: &Shared, cfg: &LiveConfig, key: &str, player: Option<&Player>, outgoing: &Receiver<Out>) -> Result<(), String> {
    shared.log(format!("live: connecting to {} ({}, voice {})", cfg.url, cfg.model, cfg.voice));
    let mut ws = connect(cfg, key)?;
    shared.log("live: connected; session.start");
    send(&mut ws, &session_start(cfg))?;
    let started = Instant::now();
    loop {
        match read(&mut ws) {
            Read::Event(event) if event["type"] == "session.started" => {
                shared.log(format!("live: session.started after {} ms", started.elapsed().as_millis()));
                break;
            }
            Read::Event(event) if event["type"] == "error" => {
                let message = event.pointer("/error/message").and_then(Value::as_str).unwrap_or("session.start failed");
                return Err(format!("GPT-Live: {message}"));
            }
            Read::Closed(reason) => return Err(reason.unwrap_or_else(|| "GPT-Live closed the session at start".into())),
            _ if started.elapsed() > START_TIMEOUT => return Err("GPT-Live did not start a session in 15 s".into()),
            _ => {}
        }
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let mut protocol = Protocol { last_activity: Some(Instant::now()), ..Protocol::default() };
    let mut talking = false;
    let idle = Duration::from_secs(cfg.idle_secs.max(10) as u64);
    let (mut sent_bytes, mut loud_bytes, mut heard_bytes) = (0usize, 0usize, 0usize);
    loop {
        // Everything queued for the socket first: audio must keep flowing.
        loop {
            match outgoing.try_recv() {
                Ok(Out::Audio(pcm)) => {
                    sent_bytes += pcm.len();
                    if pcm.chunks_exact(2).any(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > 300) {
                        loud_bytes += pcm.len();
                    }
                    send(&mut ws, &json!({ "type": "session.input_audio.append", "audio": engine.encode(&pcm) }))?
                }
                Ok(Out::Talking(now)) => {
                    shared.log(format!(
                        "live: talking={now}; mic sent so far {:.1}s ({:.1}s with sound), speech received {:.1}s",
                        sent_bytes as f32 / 48_000.0,
                        loud_bytes as f32 / 48_000.0,
                        heard_bytes as f32 / 48_000.0
                    ));
                    talking = now;
                    protocol.talking = now;
                    protocol.last_activity = Some(Instant::now());
                }
                Ok(Out::Json(value)) => {
                    if value["type"] == "session.commentary.append"
                        && let Some(id) = value["delegation_id"].as_str()
                    {
                        protocol.answered(id);
                        protocol.last_activity = Some(Instant::now());
                    }
                    send(&mut ws, &value)?;
                }
                Ok(Out::Close) | Err(TryRecvError::Disconnected) => {
                    shared.log("live: closed by bro");
                    let _ = send(&mut ws, &json!({ "type": "session.close" }));
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        match read(&mut ws) {
            Read::Event(event) => {
                let kind = event["type"].as_str().unwrap_or("");
                if !kind.ends_with("audio.delta") && !kind.ends_with("transcript.delta") {
                    shared.log(format!("live: <- {}", event.to_string().chars().take(400).collect::<String>()));
                }
                for action in protocol.on_server(&event, Instant::now()) {
                    match action {
                        Action::Play(pcm) => {
                            heard_bytes += pcm.len();
                            if let Some(player) = player {
                                player.push_pcm16(&pcm);
                            }
                        }
                        Action::Emit(event) => shared.emit(event),
                    }
                }
            }
            Read::Closed(reason) => {
                shared.log(format!("live: socket closed ({reason:?})"));
                return reason.map_or(Ok(()), Err);
            }
            Read::Nothing => {}
        }
        for action in protocol.tick(Instant::now()) {
            if let Action::Emit(event) = action {
                shared.emit(event);
            }
        }
        let speaking = player.is_some_and(|p| p.pending_secs() > 0.2);
        if !talking && !speaking && !protocol.busy() && protocol.last_activity.is_some_and(|at| at.elapsed() >= idle) {
            shared.log("live: idle, closing");
            let _ = send(&mut ws, &json!({ "type": "session.close" }));
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn delegation_waits_for_the_words_then_carries_them() {
        let t0 = Instant::now();
        let mut p = Protocol { talking: true, ..Protocol::default() };
        p.on_server(&json!({ "type": "session.input_transcript.delta", "delta": " Open a session" }), at(t0, 0));
        // A pause mid-sentence while the key is held is not the end.
        assert!(p.tick(at(t0, 900)).is_empty());
        p.talking = false;
        let created = json!({ "type": "session.delegation.created", "delegation": { "id": "d1", "type": "delegation", "target": "client" } });
        assert!(p.on_server(&created, at(t0, 100)).is_empty());
        p.on_server(&json!({ "type": "session.input_transcript.delta", "delta": " in justgains" }), at(t0, 150));
        // Words still arriving: not yet.
        assert!(p.tick(at(t0, 300)).is_empty());
        let actions = p.tick(at(t0, 600));
        assert!(actions.contains(&Action::Emit(VoiceEvent::Delegation { id: "d1".into(), request: "Open a session in justgains".into() })));
        assert!(actions.contains(&Action::Emit(VoiceEvent::Heard("Open a session in justgains".into()))));
        assert!(p.busy());
        p.answered("d1");
        assert!(!p.busy());
        // The next delegation starts from fresh words.
        p.on_server(&json!({ "type": "session.input_transcript.delta", "delta": "what's Mal doing" }), at(t0, 2000));
        p.on_server(&json!({ "type": "session.delegation.created", "delegation": { "id": "d2" } }), at(t0, 2100));
        let actions = p.tick(at(t0, 2500));
        assert_eq!(
            actions,
            vec![
                Action::Emit(VoiceEvent::Heard("what's Mal doing".into())),
                Action::Emit(VoiceEvent::Delegation { id: "d2".into(), request: "what's Mal doing".into() })
            ]
        );
    }

    #[test]
    fn a_delegation_never_waits_forever_for_words() {
        let t0 = Instant::now();
        let mut p = Protocol::default();
        p.on_server(&json!({ "type": "session.delegation.created", "delegation": { "id": "d1" } }), at(t0, 0));
        for ms in (0..1100).step_by(50) {
            p.on_server(&json!({ "type": "session.input_transcript.delta", "delta": "x" }), at(t0, ms));
            let events = p.tick(at(t0, ms));
            assert!(!events.iter().any(|a| matches!(a, Action::Emit(VoiceEvent::Delegation { .. }))), "{ms}");
        }
        let events = p.tick(at(t0, 1250));
        assert!(events.iter().any(|a| matches!(a, Action::Emit(VoiceEvent::Delegation { .. }))));
    }

    #[test]
    fn speech_and_transcripts_come_out() {
        let t0 = Instant::now();
        let mut p = Protocol::default();
        let audio = base64::engine::general_purpose::STANDARD.encode([1u8, 0, 2, 0]);
        assert_eq!(p.on_server(&json!({ "type": "session.output_audio.delta", "delta": audio }), t0), vec![Action::Play(vec![1, 0, 2, 0])]);
        p.on_server(&json!({ "type": "session.output_transcript.delta", "delta": " On it." }), at(t0, 10));
        assert!(p.tick(at(t0, 500)).is_empty());
        assert_eq!(p.tick(at(t0, 1000)), vec![Action::Emit(VoiceEvent::Said("On it.".into()))]);
        let error = p.on_server(&json!({ "type": "error", "error": { "message": "bad voice" } }), t0);
        assert_eq!(error, vec![Action::Emit(VoiceEvent::Error("GPT-Live: bad voice".into()))]);
    }

    /// The real session loop against GPT-Live: TTS speech streamed in real
    /// time with the key "held", silence after release, the delegation
    /// answered like the host would, and the answer spoken back.
    #[test]
    #[ignore = "network: needs OPENAI_API_KEY"]
    fn live_round_trip_through_a_delegation() {
        use std::sync::Mutex;
        use std::sync::atomic::AtomicBool;
        let key = crate::transcribe::resolve_key("OPENAI_API_KEY").expect("OPENAI_API_KEY");
        let pcm = ureq::post("https://api.openai.com/v1/audio/speech")
            .header("Authorization", &format!("Bearer {key}"))
            .send_json(json!({ "model": "gpt-4o-mini-tts", "input": "Open a session in justgains.", "voice": "alloy", "response_format": "pcm" }))
            .expect("tts")
            .body_mut()
            .read_to_vec()
            .expect("tts body");
        let events = Arc::new(Mutex::new(Vec::<VoiceEvent>::new()));
        let sink = events.clone();
        let cfg = LiveConfig { idle_secs: 10, ..LiveConfig::default() };
        let shared = Arc::new(Shared {
            cfg: crate::VoiceConfig { live: Some(cfg.clone()), ..crate::VoiceConfig::default() },
            prompt: Mutex::new(None),
            keywords: Mutex::new(Vec::new()),
            stopped: AtomicBool::new(false),
            on_event: Box::new(move |event| sink.lock().unwrap().push(event)),
        });
        let (out, outgoing) = std::sync::mpsc::channel();
        let (worker, closed) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || run(shared, cfg, key, None, outgoing, worker, 1));
        let started = Instant::now();
        out.send(Out::Talking(true)).unwrap();
        let frame = LIVE_RATE as usize / 25 * 2; // 40 ms of pcm16
        let mut speech = pcm.chunks(frame);
        let mut answered = false;
        let mut released = false;
        while started.elapsed() < Duration::from_secs(40) {
            match speech.next() {
                Some(chunk) => out.send(Out::Audio(chunk.to_vec())).unwrap(),
                None => {
                    if !released {
                        out.send(Out::Talking(false)).unwrap();
                        released = true;
                    }
                    out.send(Out::Audio(vec![0; frame])).unwrap();
                }
            }
            std::thread::sleep(Duration::from_millis(40));
            let delegation = events.lock().unwrap().iter().find_map(|event| match event {
                VoiceEvent::Delegation { id, request } => Some((id.clone(), request.clone())),
                _ => None,
            });
            if let Some((id, request)) = delegation.filter(|_| !answered) {
                eprintln!("[{:.1}s] delegation: {request:?}", started.elapsed().as_secs_f32());
                answered = true;
                out.send(Out::Json(commentary(&id, "Opened a Claude session in justgains named Toast."))).unwrap();
            }
            if answered && events.lock().unwrap().iter().any(|e| matches!(e, VoiceEvent::Said(said) if said.contains("Toast"))) {
                break;
            }
        }
        out.send(Out::Close).unwrap();
        thread.join().unwrap();
        assert!(matches!(closed.try_recv(), Ok(Msg::LiveClosed(1))));
        let events = events.lock().unwrap();
        eprintln!("{:#?}", events.iter().filter(|e| !matches!(e, VoiceEvent::Level(_))).collect::<Vec<_>>());
        assert!(answered, "GPT-Live never delegated");
        let request = events.iter().find_map(|e| match e { VoiceEvent::Delegation { request, .. } => Some(request.to_lowercase()), _ => None }).unwrap();
        assert!(request.contains("session"), "{request}");
        assert!(events.iter().any(|e| matches!(e, VoiceEvent::Said(said) if said.contains("Toast"))), "the answer was not spoken");
        assert!(!events.iter().any(|e| matches!(e, VoiceEvent::Error(_))), "errors: {events:?}");
    }

    #[test]
    fn session_start_uses_client_delegation() {
        let start = session_start(&LiveConfig::default());
        assert_eq!(start["session"]["model"], "gpt-live-1");
        assert_eq!(start["session"]["delegation"]["type"], "client");
        assert_eq!(start["session"]["audio"]["format"]["rate"], 24000);
        assert_eq!(commentary("d", "done")["type"], "session.commentary.append");
    }
}
