//! A paired remote surface for the original signed-in tab. JustBrowse owns the
//! human lease and CDP; Bro relays a bounded stream, never a browser profile.
use crate::state::AppState;
use axum::{Json, Router};
use axum::body::Body;
use axum::extract::{State, WebSocketUpgrade, ws::{Message, WebSocket}};
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;

mod handoffs;
mod outbox;
mod requests;

pub(crate) fn spawn(state: &AppState) {
    handoffs::spawn(state);
    requests::spawn(state);
}

/// A request created by an agent, shown in the desktop workspace only while held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handoff {
    pub id: String,
    pub tab_id: u32,
    pub title: String,
    pub reason: String,
    pub phase: String,
    pub viewer_available: bool,
    pub returning: bool,
    /// The bro session that asked (its `BRO_SESSION_ID`), when the agent said.
    pub session: Option<String>,
}

// JustBrowse lists only streamable pages in `tabs`. Its handoff journal also
// includes protected pages (such as edge://settings) and pages still loading.
// Keep every active request visible without trying to attach a viewer to it.
fn visible_tabs(snapshot: &Value) -> Vec<Value> {
    let mut tabs = snapshot["tabs"].as_array().cloned().unwrap_or_default();
    for handoff in snapshot["handoffs"].as_array().into_iter().flatten() {
        let Some(tab_id) = handoff["tabId"].as_u64().filter(|id| *id > 0 && *id <= u32::MAX as u64) else { continue; };
        if !matches!(handoff["phase"].as_str(), Some("requested" | "connecting" | "human"))
            || !handoff["handoffId"].as_str().is_some_and(|id| !id.is_empty() && id.len() <= 256)
        { continue; }
        let index = match tabs.iter().position(|tab| tab["tabId"].as_u64() == Some(tab_id)) {
            Some(index) => index,
            None => {
                tabs.push(json!({"tabId":tab_id, "title":"Browser request", "viewerAvailable":false}));
                tabs.len() - 1
            }
        };
        for key in ["handoffId", "phase", "reason", "broSessionId"] {
            if let Some(value) = handoff.get(key) { tabs[index][key] = value.clone(); }
        }
    }
    tabs
}

fn public_snapshot(mut snapshot: Value) -> Value {
    snapshot["tabs"] = Value::Array(visible_tabs(&snapshot));
    // Local worker journals (including page edit bodies) never reach the viewer.
    if let Some(object) = snapshot.as_object_mut() {
        object.retain(|key, _| matches!(key.as_str(), "protocol" | "provider" | "tabs"));
    }
    snapshot
}

pub(crate) fn update_pending(state: &AppState, snapshot: &Value) {
    let mut current = state.browser_handoff_pending.lock();
    let mut pending: Vec<_> = visible_tabs(snapshot).iter().filter_map(|tab| {
        let phase = tab["phase"].as_str()?;
        if !matches!(phase, "requested" | "human" | "connecting") { return None; }
        let id = tab["handoffId"].as_str()?.to_owned();
        Some(Handoff {
            returning: current.iter().any(|old| old.id == id && old.returning),
            id,
            tab_id: u32::try_from(tab["tabId"].as_u64()?).ok()?,
            title: tab["title"].as_str().unwrap_or("Browser tab").to_owned(),
            reason: tab["reason"].as_str().unwrap_or("The agent needs your input.").to_owned(),
            phase: phase.to_owned(),
            viewer_available: tab["viewerAvailable"].as_bool().unwrap_or(true),
            session: tab["broSessionId"].as_str().filter(|id| !id.is_empty() && id.len() <= 128).map(str::to_owned),
        })
    }).collect();
    pending.sort_by_key(|handoff| handoff.tab_id);
    if *current != pending {
        *current = pending;
        drop(current);
        state.send_command(crate::BridgeCommand::BrowserHandoffsChanged);
    }
}

const BACKEND: &str = "http://127.0.0.1:12306/browser/command";
const MAX_FRAME: usize = 4 * 1024 * 1024;
type Error = (StatusCode, Json<Value>);
enum InputWork { Event(Value), Flush(u64) }

/// Take the next input to deliver, folding what has piled up behind it while the browser was busy: a run of
/// typed text becomes one insert, hover moves only matter at their last position, and wheel turns add up.
/// Presses, releases, keys and flush barriers keep their order and are never merged.
fn next_input(queue: &mut std::collections::VecDeque<InputWork>) -> Option<InputWork> {
    let mut work = queue.pop_front()?;
    let InputWork::Event(event) = &mut work else { return Some(work); };
    let kind = |e: &Value| match (e["type"].as_str(), e["action"].as_str()) {
        (Some("text"), _) => 1,
        (Some("pointer"), Some("mouseMoved")) => 2,
        (Some("scroll"), _) => 3,
        _ => 0,
    };
    let first = kind(event);
    if first == 0 { return Some(work); }
    while let Some(InputWork::Event(next)) = queue.front() {
        if kind(next) != first || next.get("modifiers") != event.get("modifiers") { break; }
        match first {
            1 => {
                let (Some(a), Some(b)) = (event["text"].as_str(), next["text"].as_str()) else { break; };
                if a.len() + b.len() > 8 * 1024 { break; }
                event["text"] = json!(format!("{a}{b}"));
            }
            2 => {
                if next["buttons"] != event["buttons"] { break; }
                *event = next.clone();
            }
            _ => {
                let add = |key: &str| (event[key].as_f64().unwrap_or(0.0) + next[key].as_f64().unwrap_or(0.0)).clamp(-2500.0, 2500.0);
                let (dx, dy) = (add("deltaX"), add("deltaY"));
                let mut merged = next.clone();
                merged["deltaX"] = json!(dx); merged["deltaY"] = json!(dy);
                *event = merged;
            }
        }
        queue.pop_front();
    }
    Some(work)
}
fn failure(status: StatusCode, message: &str) -> Error { (status, Json(json!({"message": message}))) }

pub(crate) fn routes(state: AppState) -> Router<AppState> {
    let protected = Router::new()
        .route("/api/browser", get(status))
        .route("/api/browser/command", post(command))
        .route("/api/browser/stream", get(upgrade))
        .route_layer(middleware::from_fn_with_state(state, paired))
        .layer(axum::extract::DefaultBodyLimit::max(32 * 1024));
    Router::new().merge(protected)
        .route("/browser", get(page))
        .route("/browser/viewer.js", get(script))
        .route("/browser/connection.js", get(connection_script))
        .route("/browser/pointer.js", get(pointer_script))
        .route("/browser/viewer.css", get(styles))
}

async fn paired(State(state): State<AppState>, request: Request<Body>, next: Next) -> Response {
    let relayed = request.extensions().get::<crate::relay::RelayPeer>().is_some();
    if !relayed && !crate::rmm::paired_request(request.headers(), request.uri(), &state.token) {
        return failure(StatusCode::UNAUTHORIZED, "Pair with Bro before opening browser control.").into_response();
    }
    secure(next.run(request).await)
}
fn secure(mut response: Response) -> Response {
    for (key, value) in [
        ("cache-control", "no-store"), ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"), ("x-frame-options", "DENY"),
    ] { response.headers_mut().insert(key, value.parse().unwrap()); }
    response
}
async fn page() -> Response {
    let mut response = secure(Html(include_str!("browser/viewer.html")).into_response());
    response.headers_mut().insert("content-security-policy", "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src blob: data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'".parse().unwrap());
    response
}
async fn script() -> Response { secure(([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("browser/viewer.js")).into_response()) }
async fn connection_script() -> Response { secure(([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("browser/connection.mjs")).into_response()) }
async fn pointer_script() -> Response { secure(([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("browser/pointer.mjs")).into_response()) }
async fn styles() -> Response { secure(([(header::CONTENT_TYPE, "text/css; charset=utf-8")], include_str!("browser/viewer.css")).into_response()) }

fn client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(35)).build().unwrap()).clone()
}
pub(crate) async fn call(args: &Value) -> Result<Value, Error> {
    let response = client().post(BACKEND).json(args).send().await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "Open JustBrowse in your browser and click Connect. Update the extension if browser control is unavailable."))?;
    let status = response.status();
    if response.content_length().is_some_and(|size| size > (MAX_FRAME + 64 * 1024) as u64) {
        return Err(failure(StatusCode::BAD_GATEWAY, "The browser frame is too large."));
    }
    let body: Value = response.json().await.map_err(|_| failure(StatusCode::BAD_GATEWAY, "Update JustBrowse to use Bro Browser."))?;
    if !status.is_success() { return Err(failure(if status == StatusCode::CONFLICT { StatusCode::CONFLICT } else { StatusCode::SERVICE_UNAVAILABLE }, body["message"].as_str().unwrap_or("Update JustBrowse to use Bro Browser."))); }
    Ok(body)
}
async fn status() -> Result<Json<Value>, Error> {
    Ok(Json(public_snapshot(call(&json!({"action":"list"})).await?)))
}
async fn command(State(state): State<AppState>, Json(args): Json<Value>) -> Result<Json<Value>, Error> {
    if !matches!(args["action"].as_str(), Some("open" | "take" | "resume" | "release" | "heartbeat" | "input" | "frame")) {
        return Err(failure(StatusCode::BAD_REQUEST, "Unknown browser operation."));
    }
    let result = call(&args).await?;
    if matches!(args["action"].as_str(), Some("take" | "resume" | "release")) { state.browser_handoff_changed.notify_one(); }
    Ok(Json(result))
}
async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.max_message_size(32 * 1024).max_frame_size(32 * 1024).on_upgrade(move |socket| stream(socket, state)).into_response()
}

async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(message)).await, Ok(Ok(())))
}
async fn stream(mut socket: WebSocket, state: AppState) {
    let Some(Ok(Message::Text(first))) = tokio::time::timeout(Duration::from_secs(10), socket.recv()).await.ok().flatten() else { return; };
    let Ok(start) = serde_json::from_str::<Value>(&first) else { return; };
    let (Some(tab), Some(lease)) = (start["tabId"].as_u64(), start["lease"].as_str()) else { return; };
    if lease.len() > 128 { return; }
    let lease = lease.to_owned();
    let base = json!({"tabId":tab,"lease":lease});
    let mut heartbeat_args = base.clone(); heartbeat_args["action"] = json!("heartbeat");
    if let Err((_, error)) = call(&heartbeat_args).await {
        let _ = send(&mut socket, Message::Text(json!({"type":"error","message":error.0["message"]}).to_string().into())).await;
        return;
    }
    let (frame_tx, mut frame_rx) = mpsc::channel::<Result<Value, Error>>(1);
    let (ack_tx, mut ack_rx) = mpsc::channel::<u64>(1);
    // CDP input can wait for the compositor. Keep reading frame ACKs and heartbeats
    // while it runs, with a bounded, ordered queue and an explicit handback barrier.
    let (input_tx, mut input_rx) = mpsc::channel::<InputWork>(1024);
    let (input_result_tx, mut input_result_rx) = mpsc::channel::<Value>(32);
    let input_base = base.clone();
    let inputs = tokio::spawn(async move {
        let mut queue = std::collections::VecDeque::new();
        loop {
            if queue.is_empty() {
                let Some(work) = input_rx.recv().await else { break; };
                queue.push_back(work);
            }
            while let Ok(work) = input_rx.try_recv() { queue.push_back(work); }
            let Some(work) = next_input(&mut queue) else { continue; };
            let result = match work {
                InputWork::Event(event) => {
                    let mut args = input_base.clone(); args["action"] = json!("input"); args["event"] = event;
                    match call(&args).await {
                        Ok(_) => continue,
                        Err((_, error)) => json!({"type":"error","message":error.0["message"]}),
                    }
                }
                InputWork::Flush(id) => json!({"type":"flushed","id":id}),
            };
            if input_result_tx.send(result).await.is_err() { break; }
        }
    });
    let mut args = base.clone();
    let frames = tokio::spawn(async move {
        let mut after = 0;
        loop {
            args["action"] = json!("frame"); args["after"] = json!(after);
            let result = call(&args).await;
            let has_frame = result.as_ref().ok().is_some_and(|v| v["frame"].is_object());
            let failed = result.is_err();
            if frame_tx.send(result).await.is_err() || failed { break; }
            if has_frame {
                // One unacknowledged frame per screen. Slow devices skip stale CDP frames.
                let Ok(Some(sequence)) = tokio::time::timeout(Duration::from_secs(15), ack_rx.recv()).await else { break; };
                after = sequence;
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    });
    let mut shutdown = state.shutdown.subscribe();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(8));
    let mut expected_ack = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = heartbeat.tick() => {
                if call(&heartbeat_args).await.is_err() { break; }
            }
            result = input_result_rx.recv() => {
                let Some(result) = result else { break; };
                if !send(&mut socket, Message::Text(result.to_string().into())).await { break; }
            }
            frame = frame_rx.recv() => {
                let Some(frame) = frame else { break; };
                match frame {
                    Err((_, error)) => { let _ = send(&mut socket, Message::Text(json!({"type":"error","message":error.0["message"]}).to_string().into())).await; break; }
                    Ok(mut info) => {
                        if info["frame"].is_null() { continue; }
                        let data = info["frame"]["data"].take();
                        let Some(data) = data.as_str().filter(|s| s.len() <= MAX_FRAME) else { break; };
                        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else { break; };
                        expected_ack = info["frame"]["sequence"].as_u64();
                        info["type"] = json!("frame");
                        if !send(&mut socket, Message::Text(info.to_string().into())).await || !send(&mut socket, Message::Binary(bytes.into())).await { break; }
                    }
                }
            }
            message = socket.recv() => {
                let Some(Ok(message)) = message else { break; };
                match message {
                    Message::Text(text) => {
                        let Ok(value) = serde_json::from_str::<Value>(&text) else { break; };
                        match value["type"].as_str() {
                            Some("ack") => {
                                if let Some(sequence) = expected_ack.filter(|n| Some(*n) == value["sequence"].as_u64()) {
                                    expected_ack = None;
                                    if ack_tx.try_send(sequence).is_err() { break; }
                                }
                            }
                            Some("input") => {
                                let event = value["event"].clone();
                                // a stale hover or wheel turn can go; anything else would leave a key or button stuck
                                let droppable = event["type"] == "scroll" || event["action"] == "mouseMoved";
                                if input_tx.try_send(InputWork::Event(event)).is_err() && !droppable {
                                    let _ = send(&mut socket, Message::Text(json!({"type":"error","message":"Browser input is catching up. Reconnect to continue."}).to_string().into())).await;
                                    break;
                                }
                            }
                            Some("flush") => {
                                let Some(id) = value["id"].as_u64() else { break; };
                                if input_tx.try_send(InputWork::Flush(id)).is_err() { break; }
                            }
                            Some("release") => break,
                            _ => {}
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(bytes) => { if !send(&mut socket, Message::Pong(bytes)).await { break; } }
                    _ => {}
                }
            }
        }
    }
    frames.abort();
    inputs.abort();
    let mut release = base; release["action"] = json!("release");
    let _ = call(&release).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn piled_up_input_folds_typing_hover_and_wheel_but_keeps_presses_keys_and_barriers() {
        let ev = |v: Value| InputWork::Event(v);
        let mut queue: std::collections::VecDeque<_> = [
            ev(json!({"type":"text","text":"he"})), ev(json!({"type":"text","text":"llo"})),
            ev(json!({"type":"pointer","action":"mouseMoved","x":1,"y":1,"buttons":0})),
            ev(json!({"type":"pointer","action":"mouseMoved","x":5,"y":6,"buttons":0})),
            ev(json!({"type":"pointer","action":"mousePressed","x":5,"y":6,"buttons":1})),
            ev(json!({"type":"pointer","action":"mouseMoved","x":7,"y":8,"buttons":1})),
            ev(json!({"type":"scroll","x":1,"y":1,"deltaX":0,"deltaY":2000})),
            ev(json!({"type":"scroll","x":2,"y":2,"deltaX":0,"deltaY":2000})),
            InputWork::Flush(7),
            ev(json!({"type":"key","key":"Backspace","modifiers":0})),
            ev(json!({"type":"key","key":"Backspace","modifiers":0})),
            ev(json!({"type":"text","text":"a","modifiers":8})), ev(json!({"type":"text","text":"b"})),
        ].into_iter().collect();
        let mut out = vec![];
        while let Some(work) = next_input(&mut queue) {
            out.push(match work { InputWork::Event(v) => v, InputWork::Flush(id) => json!({"flush":id}) });
        }
        assert_eq!(out[0]["text"], "hello");
        assert_eq!((out[1]["x"].as_i64(), out[1]["action"].as_str()), (Some(5), Some("mouseMoved")));
        assert_eq!(out[2]["action"], "mousePressed");
        assert_eq!(out[3]["buttons"], 1, "a drag move is not folded into the hover before the press");
        assert_eq!((out[4]["deltaY"].as_f64(), out[4]["x"].as_i64()), (Some(2500.0), Some(2)));
        assert_eq!(out[5]["flush"], 7);
        assert_eq!((out[6]["key"].as_str(), out[7]["key"].as_str()), (Some("Backspace"), Some("Backspace")));
        assert_eq!((out[8]["text"].as_str(), out[9]["text"].as_str()), (Some("a"), Some("b")), "different modifiers stay apart");
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn a_handoff_hidden_from_the_streamable_tab_list_still_reaches_the_desktop() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(crate::BridgeConfig {
            port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: root.path().to_owned(), web_interface: false,
        }, "fixture".into(), Arc::new(|_| {}));
        update_pending(&state, &json!({"tabs":[], "handoffs":[{
            "tabId":42, "handoffId":"settings", "broSessionId":"owner",
            "phase":"requested", "reason":"Clear the host cache in Edge."
        }]}));
        let pending = state.browser_handoff_pending.lock();
        assert_eq!(pending.len(), 1, "a protected page must not leave an invisible handoff");
        assert_eq!(pending[0].id, "settings");
        assert_eq!(pending[0].reason, "Clear the host cache in Edge.");
        assert!(!pending[0].viewer_available);
        assert_eq!(pending[0].session.as_deref(), Some("owner"));
    }

    #[test]
    fn public_tab_picker_includes_active_hidden_requests_without_the_completion_journal() {
        let visible = public_snapshot(json!({"tabs":[{"tabId":43,"title":"Web page","phase":"agent"}], "handoffs":[
            {"tabId":42,"handoffId":"settings","phase":"requested","broSessionId":"owner","reason":"Clear cache"},
            {"tabId":44,"handoffId":"closed","phase":"agent","outcome":"tab-closed"},
            {"tabId":45,"handoffId":"","phase":"requested"},
            {"tabId":0,"handoffId":"invalid","phase":"requested"}
        ]}));
        assert!(visible.get("handoffs").is_none());
        let tabs = visible["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[1]["tabId"], 42);
        assert_eq!(tabs[1]["broSessionId"], "owner");
        assert_eq!(tabs[1]["viewerAvailable"], false);
        assert_eq!(tabs[1]["handoffId"], "settings");
    }

    #[test]
    fn public_browser_snapshot_never_exposes_local_page_request_bodies() {
        let visible = public_snapshot(json!({
            "protocol":"bro-browser-v1", "provider":"live-profile", "tabs":[],
            "requests":[{"prompt":"private page edit body", "pages":[{"before":"private snippet"}]}],
            "pageReviewRequests":[{"prompt":"another private body"}]
        }));
        assert_eq!(visible, json!({"protocol":"bro-browser-v1", "provider":"live-profile", "tabs":[]}));
        assert!(!visible.to_string().contains("private"));
    }

    #[test]
    fn active_handoff_identity_updates_a_visible_tab_without_duplicates_or_losing_its_title() {
        let tabs = visible_tabs(&json!({"tabs":[{"tabId":42,"title":"Sign in","url":"https://example.com","phase":"agent"}], "handoffs":[
            {"tabId":42,"handoffId":"current","phase":"human","reason":"Finish sign-in"},
            {"tabId":42,"handoffId":"previous","phase":"agent","outcome":"done"}
        ]}));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0]["title"], "Sign in");
        assert_eq!(tabs[0]["handoffId"], "current");
        assert_eq!(tabs[0]["phase"], "human");
        assert_ne!(tabs[0]["viewerAvailable"], false);
    }

    #[test]
    fn desktop_requests_follow_handoff_identity_and_preserve_inflight_returns() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(crate::BridgeConfig {
            port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: root.path().to_owned(), web_interface: false,
        }, "fixture".into(), Arc::new(|_| {}));
        let request = json!({"tabs":[
            {"tabId":42,"handoffId":"first","phase":"requested","title":"Vault","reason":"Unlock it here."},
            {"tabId":43,"phase":"agent"},
            {"tabId":44,"phase":"requested"}
        ]});
        update_pending(&state, &request);
        assert_eq!(state.browser_handoff_pending.lock().len(), 1);
        state.browser_handoff_pending.lock()[0].returning = true;
        update_pending(&state, &request);
        assert!(state.browser_handoff_pending.lock()[0].returning);
        update_pending(&state, &json!({"tabs":[{"tabId":42,"handoffId":"next","phase":"requested"}]}));
        assert!(!state.browser_handoff_pending.lock()[0].returning);
        update_pending(&state, &json!({"tabs":[{"tabId":42,"handoffId":"next","phase":"agent"}]}));
        assert!(state.browser_handoff_pending.lock().is_empty());
    }

    #[tokio::test]
    async fn control_requires_pairing_even_on_loopback_and_rejects_cross_site_requests() {
        let root = tempfile::tempdir().unwrap();
        let state = AppState::new(crate::BridgeConfig {
            port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: root.path().to_owned(), web_interface: false,
        }, "browser-test-token".into(), Arc::new(|_| {}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, crate::server::router(state)).await.unwrap(); });
        let http = client();
        for path in ["/api/browser", "/api/browser/stream"] {
            assert_eq!(http.get(format!("{origin}{path}")).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        }
        for (name, value) in [("Origin", "https://attacker.example"), ("Sec-Fetch-Site", "cross-site")] {
            assert_eq!(http.post(format!("{origin}/api/browser/command")).bearer_auth("browser-test-token")
                .header(name, value).json(&json!({"action":"take","tabId":1})).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        }
        assert_eq!(http.post(format!("{origin}/api/browser/command"))
            .json(&json!({"action":"frame","tabId":1,"lease":"test"})).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        let denied = http.post(format!("{origin}/api/browser/command")).bearer_auth("browser-test-token")
            .json(&json!({"action":"Runtime.evaluate"})).send().await.unwrap();
        assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
        assert_eq!(denied.headers()["cache-control"], "no-store");
        let page = http.get(format!("{origin}/browser")).send().await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert!(page.headers()["content-security-policy"].to_str().unwrap().contains("frame-ancestors 'none'"));
        assert!(page.text().await.unwrap().contains("Choose a tab"));
        let script = http.get(format!("{origin}/browser/viewer.js")).send().await.unwrap();
        assert!(script.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/javascript"));
        let connection = http.get(format!("{origin}/browser/connection.js")).send().await.unwrap();
        assert_eq!(connection.headers()["cache-control"], "no-store");
        assert!(connection.text().await.unwrap().contains("export function connectBrowser"));
        let pointer = http.get(format!("{origin}/browser/pointer.js")).send().await.unwrap();
        assert_eq!(pointer.headers()["cache-control"], "no-store");
        assert!(pointer.text().await.unwrap().contains("export function createBrowserPointer"));
        server.abort();
    }
}
