//! End-to-end tests over loopback: a real [`Bridge`] on an ephemeral port with
//! a temporary data root, driven by websocket and HTTP clients.

use crate::model::ClientMessage;
use crate::{
    Bridge, BridgeCommand, BridgeConfig, CreateRequest, Notification, SessionMeta, StartOptions,
};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Everything bro would have received through `on_command`, except `Create`
/// (answered by the harness: it registers the session, then replies).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Input(String, String),
    Resize(String, u16, u16),
    Kill(String),
    Rename(String, String),
    Create(Option<String>, Option<String>, Vec<String>),
    OpenOrchestrator(PathBuf, String),
    Focus(String),
    OpenFile(String, PathBuf, Option<u32>, Option<u32>),
    OpenInEditor(String, PathBuf, Option<u32>, Option<u32>),
    Orchestrator(crate::OrchestratorCommand),
    Tile(Option<PathBuf>),
}

struct Harness {
    bridge: Bridge,
    seen: Arc<Mutex<Vec<Seen>>>,
    notifications: Arc<Mutex<Vec<Notification>>>,
    root: tempfile::TempDir,
    port: u16,
}

impl Harness {
    fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let slot: Arc<OnceLock<Bridge>> = Arc::new(OnceLock::new());
        let on_command = {
            let seen = seen.clone();
            let slot = slot.clone();
            move |command: BridgeCommand| match command {
                BridgeCommand::FocusControls { id, change, reply } => {
                    let _ = reply.send(Ok(json!({ "sessionId": id, "change": change, "model": null, "effort": null, "models": [], "levels": [{"id":"high"}], "pending": false })));
                }
                BridgeCommand::Seen { .. } => {},
                BridgeCommand::OpenOrchestrator { cwd, source, reply } => {
                    seen.lock().push(Seen::OpenOrchestrator(cwd.clone(), source));
                    let id = "project-coordinator".to_string();
                    slot.get().unwrap().register(SessionMeta {
                        id: id.clone(), title: "Project orchestrator".into(), shell: "codex".into(),
                        args: vec![], cwd: Some(cwd.clone()), project: Some(cwd), pid: Some(8),
                        cols: 100, rows: 30, agent: Some("codex".into()), kind: Some("orchestrator".into()),
                    });
                    let _ = reply.send(Ok(id));
                }
                BridgeCommand::Input { id, data } => seen
                    .lock()
                    .push(Seen::Input(id, String::from_utf8_lossy(&data).into_owned())),
                BridgeCommand::Write { id, data, reply } => {
                    seen.lock().push(Seen::Input(id, String::from_utf8_lossy(&data).into_owned()));
                    let _ = reply.send(Ok(()));
                }
                BridgeCommand::Resize { id, cols, rows } => {
                    seen.lock().push(Seen::Resize(id, cols, rows))
                }
                BridgeCommand::Kill { id } => seen.lock().push(Seen::Kill(id)),
                BridgeCommand::Rename { id, title } => seen.lock().push(Seen::Rename(id, title)),
                BridgeCommand::Focus { id } => seen.lock().push(Seen::Focus(id)),
                BridgeCommand::Orchestrator(c) => seen.lock().push(Seen::Orchestrator(c)),
                BridgeCommand::Tile { project } => seen.lock().push(Seen::Tile(project)),
                BridgeCommand::OpenFile { id, path, line, column } => {
                    seen.lock().push(Seen::OpenFile(id, path, line, column))
                }
                BridgeCommand::OpenInEditor { id, path, line, column } => {
                    seen.lock().push(Seen::OpenInEditor(id, path, line, column))
                }
                BridgeCommand::Create { req, reply } => {
                    let CreateRequest {
                        title,
                        profile_id,
                        shell,
                        args,
                        cwd,
                        ..
                    } = req;
                    seen.lock().push(Seen::Create(
                        profile_id.clone(),
                        shell.clone(),
                        args.clone(),
                    ));
                    let slot = slot.clone();
                    // Like bro-tui: answer later, from another thread.
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(30));
                        if title.as_deref() == Some("refuse") {
                            let _ = reply.send(Err(anyhow::anyhow!("no PTY for you")));
                            return;
                        }
                        let id = format!("created-{}", uuid::Uuid::new_v4().simple());
                        slot.get().unwrap().register(SessionMeta {
                            id: id.clone(),
                            title: "pwsh".into(),
                            shell: shell.unwrap_or_else(|| "pwsh".into()),
                            args,
                            cwd,
                            project: None,
                            pid: Some(4242),
                            cols: 100,
                            rows: 30,
                            agent: None,
                            kind: None,
                        });
                        let _ = reply.send(Ok(id));
                    });
                }
            }
        };
        let config = BridgeConfig {
            port: 0,
            automatic_port: false,
            bind: "127.0.0.1".into(),
            data_root: root.path().join("bridge"),
            web_interface: true,
        };
        let bridge = Bridge::start_with(
            config,
            StartOptions {
                import_legacy_token: false,
            },
            Box::new(on_command),
        )
        .unwrap();
        let _ = slot.set(bridge.clone());
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let sink = notifications.clone();
        bridge.on_notification(Box::new(move |n| sink.lock().push(n)));
        let port = bridge.status().port;
        Self {
            bridge,
            seen,
            notifications,
            root,
            port,
        }
    }

    fn register(&self, id: &str, cwd: Option<PathBuf>, project: Option<PathBuf>) {
        self.bridge.register(SessionMeta {
            id: id.into(),
            title: format!("{id} title"),
            shell: "pwsh.exe".into(),
            args: vec!["-NoLogo".into()],
            cwd,
            project,
            pid: Some(42),
            cols: 100,
            rows: 30,
            agent: None,
            kind: None,
        });
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn register_codex(&self, id: &str) {
        use base64::Engine;
        self.register(id, None, None);
        let meta = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"v":1,"agent":"codex","state":"active"}"#);
        self.bridge.output(id, format!("\x1b]1337;TerminalWeb.Agent={meta}\x07").as_bytes());
    }

    async fn socket(&self) -> Socket {
        let (socket, _) =
            tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/ws", self.port))
                .await
                .unwrap();
        socket
    }

    async fn wait_seen(&self, count: usize) -> Vec<Seen> {
        for _ in 0..200 {
            if self.seen.lock().len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.seen.lock().clone()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.bridge.shutdown();
    }
}

async fn recv(socket: &mut Socket) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str::<Value>(&text).unwrap();
                }
                Some(Ok(_)) => continue,
                other => panic!("socket ended: {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for a websocket message")
}

async fn recv_type(socket: &mut Socket, kind: &str) -> Value {
    loop {
        let message = recv(socket).await;
        if message["type"] == kind {
            return message;
        }
    }
}

async fn recv_until(socket: &mut Socket, accept: impl Fn(&Value) -> bool) -> Value {
    loop {
        let message = recv(socket).await;
        if accept(&message) {
            return message;
        }
    }
}

async fn send(socket: &mut Socket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_subscribe_snapshot_output_ordering() {
    let harness = Harness::start();
    harness.register("s1", Some(PathBuf::from("C:\\work")), None);
    harness.bridge.output("s1", b"\x1b[32mready\x1b[m\r\n");

    let mut socket = harness.socket().await;
    let hello = recv(&mut socket).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["heartbeat"], true);
    assert_eq!(hello["sessions"][0]["id"], "s1");
    assert_eq!(hello["sessions"][0]["source"], "bridged");
    assert_eq!(hello["sessions"][0]["status"], "running");
    assert_eq!(hello["sessions"][0]["cols"], 100);
    assert!(hello["sessions"][0]["bufferedBytes"].as_u64().unwrap() > 0);
    assert_eq!(hello["server"]["port"], harness.port);
    assert_eq!(hello["server"]["urls"][0]["scope"], "local");
    assert!(
        hello["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "claude")
    );
    assert!(hello["projects"].is_array());
    assert_eq!(hello["hostProcesses"], json!([]));
    assert_eq!(hello["peerHosts"], json!([]));
    assert!(hello["orchestrator"]["state"].is_string());
    assert!(hello["acp"]["agents"].is_array());
    assert!(
        hello["bridgeCommands"]["serverUrl"]
            .as_str()
            .unwrap()
            .ends_with(&harness.port.to_string())
    );

    send(&mut socket, json!({ "type": "ping" })).await;
    assert_eq!(recv(&mut socket).await["type"], "pong");

    send(
        &mut socket,
        json!({ "type": "subscribe", "sessionId": "s1" }),
    )
    .await;
    let snapshot = recv(&mut socket).await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["sessionId"], "s1");
    assert!(snapshot["screen"].as_str().unwrap().contains("ready"));
    assert_eq!(snapshot["chunks"], json!([]));
    assert_eq!(snapshot["session"]["id"], "s1");

    // Split UTF-8 across two writes: the client still sees whole characters.
    let bytes = "more ✓\r\n".as_bytes();
    harness.bridge.output("s1", &bytes[..6]);
    harness.bridge.output("s1", &bytes[6..]);
    let first = recv_type(&mut socket, "output").await;
    assert_eq!(first["sessionId"], "s1");
    assert_eq!(first["seq"], 2);
    assert_eq!(first["data"], "more ");
    let second = recv_type(&mut socket, "output").await;
    assert_eq!(second["seq"], 3);
    assert_eq!(second["data"], "✓\r\n");
    assert_eq!(second["replay"], false);

    // A session nobody subscribed to only pings `activity`.
    harness.register("s2", None, None);
    harness.bridge.output("s2", b"quiet\r\n");
    let activity = recv_type(&mut socket, "activity").await;
    assert_eq!(
        activity,
        json!({ "type": "activity", "sessionId": "s2", "seq": 1 })
    );

    // Lifecycle events reach every client.
    harness.bridge.title("s1", "renamed by bro");
    let session = recv_type(&mut socket, "session").await;
    assert_eq!(session["session"]["title"], "renamed by bro");
    harness.bridge.exit("s1", Some(3));
    let exit = recv_type(&mut socket, "exit").await;
    assert_eq!(exit["exitCode"], 3);
    assert_eq!(exit["session"]["status"], "exited");
    harness.bridge.unregister("s1");
    let sessions = recv_until(&mut socket, |m| {
        m["type"] == "sessions"
            && m["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["id"] != "s1")
    })
    .await;
    assert_eq!(sessions["sessions"][0]["id"], "s2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_actions_become_bridge_commands() {
    let harness = Harness::start();
    harness.register("s1", None, None);
    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;

    send(
        &mut socket,
        json!({ "type": "input", "sessionId": "s1", "data": "whoami\r" }),
    )
    .await;
    send(
        &mut socket,
        json!({ "type": "resize", "sessionId": "s1", "cols": 1000, "rows": 40 }),
    )
    .await;
    send(
        &mut socket,
        json!({ "type": "rename", "sessionId": "s1", "title": "phone title" }),
    )
    .await;
    send(&mut socket, json!({ "type": "kill", "sessionId": "s1" })).await;
    // Unknown sessions and malformed messages are ignored, not fatal.
    send(
        &mut socket,
        json!({ "type": "input", "sessionId": "nope", "data": "x" }),
    )
    .await;
    socket
        .send(Message::Text("{not json".into()))
        .await
        .unwrap();
    send(
        &mut socket,
        json!({ "type": "resize", "sessionId": "s1", "cols": "wide" }),
    )
    .await;
    let seen = harness.wait_seen(4).await;
    assert_eq!(
        seen,
        vec![
            Seen::Input("s1".into(), "whoami\r".into()),
            Seen::Resize("s1".into(), 400, 40),
            Seen::Rename("s1".into(), "phone title".into()),
            Seen::Kill("s1".into()),
        ]
    );
    assert_eq!(
        recv_type(&mut socket, "session").await["session"]["title"],
        "phone title"
    );
    harness.seen.lock().clear();

    // REST equivalents.
    let client = reqwest::Client::new();
    let status = client
        .post(harness.url("/api/sessions/s1/write"))
        .json(&json!({ "data": "ls\r" }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 204);
    let status = client
        .post(harness.url("/api/sessions/s1/resize"))
        .json(&json!({ "cols": 90, "rows": 25 }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 204);
    let status = client
        .delete(harness.url("/api/sessions/s1"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 204);
    let status = client
        .post(harness.url("/api/sessions/s1/focus"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 204);
    let missing = client
        .post(harness.url("/api/sessions/missing/write"))
        .json(&json!({ "data": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert_eq!(
        missing.json::<Value>().await.unwrap()["message"],
        "Unknown terminal session."
    );
    assert_eq!(
        harness.wait_seen(4).await,
        vec![
            Seen::Input("s1".into(), "ls\r".into()),
            Seen::Resize("s1".into(), 90, 25),
            Seen::Kill("s1".into()),
            Seen::Focus("s1".into()),
        ]
    );
    harness.seen.lock().clear();

    // Create resolves through the reply channel and returns the summary.
    let work = harness.root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work_text = work.to_string_lossy().into_owned();
    let created: Value = client
        .post(harness.url("/api/sessions"))
        .json(&json!({ "title": "From phone", "profileId": "claude", "cwd": work_text }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(created["id"].as_str().unwrap().starts_with("created-"));
    assert_eq!(created["title"], "From phone");
    assert_eq!(created["cwd"], work_text);
    assert_eq!(
        created["shell"], "claude",
        "the profile filled in the shell"
    );
    assert_eq!(
        harness.seen.lock().first().cloned(),
        Some(Seen::Create(
            Some("claude".into()),
            Some("claude".into()),
            vec![]
        ))
    );

    let refused = client
        .post(harness.url("/api/sessions"))
        .json(&json!({ "title": "refuse" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    assert!(
        refused.json::<Value>().await.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("no PTY for you")
    );

    // A folder that isn't there is refused before bro is asked.
    harness.seen.lock().clear();
    let missing = client
        .post(harness.url("/api/sessions"))
        .json(&json!({ "profileId": "claude", "cwd": work.join("nope").to_string_lossy() }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 400);
    assert!(missing.json::<Value>().await.unwrap()["message"].as_str().unwrap().contains("no folder"));
    assert!(harness.seen.lock().is_empty(), "bro never saw it");

    // ws create: success shows up as a new session, failure as an error event.
    send(
        &mut socket,
        json!({ "type": "create", "shell": "cmd.exe", "args": ["/k"] }),
    )
    .await;
    recv_until(&mut socket, |m| {
        m["type"] == "sessions"
            && m["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["shell"] == "cmd.exe")
    })
    .await;
    send(&mut socket, json!({ "type": "create", "title": "refuse" })).await;
    let error = recv_type(&mut socket, "error").await;
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("no PTY for you")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compose_sends_enter_as_its_own_write_after_the_paste() {
    let harness = Harness::start();
    harness.register("s", None, None);
    // The program asked for bracketed paste.
    harness.bridge.output("s", b"\x1b[?2004h> ");
    let client = reqwest::Client::new();
    let started = std::time::Instant::now();
    let response: Value = client
        .post(harness.url("/api/sessions/s/compose"))
        .json(&json!({ "text": "line one\r\nline two", "submit": true }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(350));
    assert_eq!(response["method"], "paste");
    assert_eq!(response["submitted"], true);
    assert_eq!(
        harness.wait_seen(2).await,
        vec![
            Seen::Input("s".into(), "\x1b[200~line one\nline two\x1b[201~".into()),
            Seen::Input("s".into(), "\r".into()),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_focus_controls_roundtrip_before_a_transcript_exists_and_reject_unknown_sessions() {
    let harness = Harness::start();
    harness.register_codex("fresh");
    let client = reqwest::Client::new();
    let response = client.get(harness.url("/api/sessions/fresh/focus-controls")).send().await.unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["levels"][0]["id"], "high");
    assert!(body["model"].is_null());
    let response = client.post(harness.url("/api/sessions/fresh/focus-controls"))
        .json(&json!({"kind":"thinking","value":"high"})).send().await.unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["change"], json!({"kind":"thinking","value":"high"}));
    assert_eq!(client.get(harness.url("/api/sessions/missing/focus-controls")).send().await.unwrap().status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(client.post(harness.url("/api/sessions/fresh/focus-controls")).json(&json!({"kind":"other","value":"high"})).send().await.unwrap().status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_quips_expand_from_the_hosts_library_only_when_requested_for_an_agent() {
    let harness = Harness::start();
    harness.register_codex("agent");
    harness.register("shell", None, None);
    let client = reqwest::Client::new();
    let library: Value = client.get(harness.url("/api/quips")).send().await.unwrap().json().await.unwrap();
    let Some(quip) = library["quips"].as_array().unwrap().first() else { return };
    let trigger = quip["trigger"].as_str().unwrap();
    let short = format!("Please explain :{trigger}");
    for (id, expand, expected_expansion) in [("agent", true, true), ("agent", false, false), ("shell", true, false)] {
        harness.seen.lock().clear();
        let response = client.post(harness.url(&format!("/api/sessions/{id}/compose")))
            .json(&json!({"text":short,"submit":false,"expandQuips":expand})).send().await.unwrap();
        assert!(response.status().is_success());
        let wire = harness.seen.lock().iter().filter_map(|seen| match seen { Seen::Input(_, text) => Some(text.clone()), _ => None }).collect::<String>();
        assert_eq!(wire.contains("<bro-quip"), expected_expansion, "{wire}");
        if expected_expansion { assert_eq!(bro_core::quips::collapse(&wire).trim_start_matches("\x1b[200~").trim_end_matches("\x1b[201~"), short); }
        else { assert_eq!(wire.trim_start_matches("\x1b[200~").trim_end_matches("\x1b[201~"), short); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_slash_commands_use_raw_input_even_when_the_agent_enabled_bracketed_paste() {
    let harness = Harness::start();
    harness.register_codex("s");
    harness.bridge.output("s", b"\x1b[?2004h> ");
    let client = reqwest::Client::new();
    let response: Value = client.post(harness.url("/api/sessions/s/compose"))
        .json(&json!({"text":"/compact","submit":true,"expandQuips":true})).send().await.unwrap().json().await.unwrap();
    assert_eq!(response["method"], "raw");
    assert_eq!(*harness.seen.lock(), vec![Seen::Input("s".into(), "/compact".into()), Seen::Input("s".into(), "\r".into())]);
}

#[test]
fn refresh_host_accepts_both_spellings() {
    for spelling in ["refresh-host", "refresh_host"] {
        let message: ClientMessage = serde_json::from_value(json!({ "type": spelling })).unwrap();
        assert!(matches!(message, ClientMessage::RefreshHost), "{spelling}");
    }
    let create: ClientMessage =
        serde_json::from_value(json!({ "type": "create", "profileId": "codex", "projectId": "p" }))
            .unwrap();
    assert!(matches!(
        create,
        ClientMessage::Create {
            profile_id: Some(_),
            project_id: Some(_),
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_host_republishes_profiles() {
    let harness = Harness::start();
    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;
    send(&mut socket, json!({ "type": "refresh-host" })).await;
    let profiles = recv_type(&mut socket, "profiles").await;
    assert!(profiles["profiles"].as_array().unwrap().len() >= 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_signals_become_notifications() {
    let harness = Harness::start();
    harness.register("osc", None, None);
    harness.register("bell", None, None);
    harness.register("title", None, None);
    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;

    harness
        .bridge
        .output("osc", b"\x1b]777;notify;Build;All green\x1b\\");
    let notify = recv_type(&mut socket, "notify").await;
    assert_eq!(notify["origin"], "osc");
    assert_eq!(notify["title"], "Build");
    assert_eq!(notify["body"], "All green");
    assert_eq!(notify["sessionId"], "osc");
    assert_eq!(notify["sessionTitle"], "osc title");
    assert_eq!(notify["sound"], "done");
    assert!(notify["id"].is_string() && notify["at"].is_string());

    // Throttled: a second signal from the same session within 4 s is dropped.
    harness.bridge.output("osc", b"\x1b]9;again\x07");
    // BEL terminating a title OSC is not a bell; a bare BEL is (after 300 ms).
    harness.bridge.output("title", b"\x1b]0;just a title\x07");
    harness.bridge.output("bell", b"done\x07");
    let bell = recv_type(&mut socket, "notify").await;
    assert_eq!(bell["origin"], "bell");
    assert_eq!(bell["sessionId"], "bell");
    assert_eq!(bell["title"], "bell title");
    assert_eq!(bell["body"], "Task finished");

    let history: Vec<Value> = reqwest::get(harness.url("/api/notifications"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history.len(), 2, "{history:?}");
    assert!(history.iter().all(|n| n["sessionId"] != "title"));
    let since = chrono::DateTime::parse_from_rfc3339(history[0]["at"].as_str().unwrap())
        .unwrap()
        .timestamp_millis();
    let newer: Vec<Value> = reqwest::get(harness.url(&format!("/api/notifications?since={since}")))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(newer.len(), 1);

    let raised = harness.notifications.lock().clone();
    assert_eq!(raised.len(), 2);
    assert_eq!(raised[0].title, "Build");
    assert!(raised[1].sound);

    // bro's own notifications are broadcast but not echoed back to bro.
    harness.bridge.notify(Notification {
        session_id: Some("bell".into()),
        title: "From bro".into(),
        body: String::new(),
        sound: false,
    });
    let own = recv_type(&mut socket, "notify").await;
    assert_eq!(own["title"], "From bro");
    assert_eq!(own["origin"], "api");
    assert!(own.get("body").is_none() && own.get("sound").is_none());
    assert_eq!(harness.notifications.lock().len(), 2);

    // Hooks: POST /api/notify with query parameters only.
    let status = reqwest::Client::new()
        .post(harness.url("/api/notify?title=Hook&sound=done&sessionId=bell"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 204);
    assert_eq!(recv_type(&mut socket, "notify").await["title"], "Hook");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_awaiting_input_raises_needs_input() {
    let harness = Harness::start();
    harness.bridge.register(SessionMeta {
        id: "agent".into(),
        title: "Claude".into(),
        shell: "claude".into(),
        args: vec![],
        cwd: None,
        project: None,
        pid: None,
        cols: 100,
        rows: 30,
        agent: Some("claude".into()),
        kind: None,
    });
    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;
    harness.bridge.output(
        "agent",
        "Do you want to proceed?\r\n❯ 1. Yes\r\n  2. No\r\n\r\nEnter to confirm · Esc to cancel\x07".as_bytes(),
    );
    let notify = recv_type(&mut socket, "notify").await;
    assert_eq!(notify["title"], "Claude Code needs input");
    assert_eq!(notify["body"], "Open Claude to answer.");
    assert_eq!(notify["sound"], "attention");
    // The bell rang with the question: it was folded into "needs input".
    tokio::time::sleep(Duration::from_millis(500)).await;
    let history: Vec<Value> = reqwest::get(harness.url("/api/notifications"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history.len(), 1, "{history:?}");

    let context: Value = reqwest::get(harness.url("/api/sessions/agent/input-context"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(context["agent"], "claude");
    assert_eq!(context["prompt"]["kind"], "single-select");
    let response = reqwest::Client::new()
        .post(harness.url("/api/sessions/agent/prompt-response"))
        .json(&json!({ "promptId": context["prompt"]["id"], "action": "select", "optionId": "option-1" }))
        .send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        harness.wait_seen(1).await,
        vec![Seen::Input("agent".into(), "1\r".into())]
    );
    let stale = reqwest::Client::new()
        .post(harness.url("/api/sessions/agent/prompt-response"))
        .json(&json!({ "promptId": "prompt-old", "action": "cancel" }))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_mobile_questions_submit_one_choice_and_paste_written_answers_before_enter() {
    let harness = Harness::start();
    harness.bridge.register(SessionMeta {
        id: "codex-question".into(), title: "Codex".into(), shell: "codex".into(), args: vec![],
        cwd: None, project: None, pid: None, cols: 100, rows: 30, agent: Some("codex".into()), kind: None,
    });
    harness.bridge.output("codex-question", b"Question 1/2\r\nChoose an environment\r\n\xe2\x80\xba 1. Staging\r\n  2. Production\r\ntab to add notes | enter to submit answer | esc to interrupt");
    let url = harness.url("/api/sessions/codex-question/prompt-response");
    let context: Value = reqwest::get(harness.url("/api/sessions/codex-question/input-context")).await.unwrap().json().await.unwrap();
    assert_eq!(context["prompt"]["protocol"], "codex-question");
    let body = json!({"promptId":context["prompt"]["id"], "action":"select", "optionId":"option-2"});
    let client = reqwest::Client::new();
    assert_eq!(client.post(&url).json(&body).send().await.unwrap().status(), 200);
    assert_eq!(harness.wait_seen(1).await, vec![Seen::Input("codex-question".into(), "\x1b[B\r".into())]);
    assert_eq!(client.post(&url).json(&body).send().await.unwrap().status(), 409, "a stale mobile card cannot submit twice");

    harness.bridge.output("codex-question", b"\x1b[2J\x1b[HQuestion 2/2\r\nWhere should the files go?\r\n\xe2\x80\xba Type your answer (optional)\r\nenter to submit all | esc to interrupt");
    assert_eq!(client.post(&url).json(&body).send().await.unwrap().status(), 409, "previous question identity is stale");
    let context: Value = reqwest::get(harness.url("/api/sessions/codex-question/input-context")).await.unwrap().json().await.unwrap();
    assert_eq!(context["prompt"]["kind"], "freeform");
    let started = std::time::Instant::now();
    assert_eq!(client.post(&url).json(&json!({"promptId":context["prompt"]["id"], "action":"text", "text":"docs/\nnotes"})).send().await.unwrap().status(), 200);
    assert!(started.elapsed() >= crate::commands::submit_settle(1));
    assert_eq!(harness.wait_seen(3).await, vec![
        Seen::Input("codex-question".into(), "\x1b[B\r".into()),
        Seen::Input("codex-question".into(), "\x1b[200~docs/\nnotes\x1b[201~".into()),
        Seen::Input("codex-question".into(), "\r".into()),
    ]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_opens_queued_codex_questions_and_answers_in_the_async_editor() {
    let harness = Harness::start();
    harness.bridge.register(SessionMeta { id:"async".into(), title:"Codex".into(), shell:"codex".into(), args:vec![], cwd:None,
        project:None, pid:None, cols:100, rows:30, agent:Some("codex".into()), kind:None });
    let client = reqwest::Client::new();
    let context_url = harness.url("/api/sessions/async/input-context");
    let response_url = harness.url("/api/sessions/async/prompt-response");
    harness.bridge.output("async", "? 2 questions · 15s\r\n  ⇧← to answer".as_bytes());
    let open: Value = client.get(&context_url).send().await.unwrap().json().await.unwrap();
    assert_eq!(open["prompt"]["canCancel"], false);
    let answer = json!({"promptId":open["prompt"]["id"],"action":"select","optionId":"open-questions"});
    assert_eq!(client.post(&response_url).json(&answer).send().await.unwrap().status(), 200);
    assert_eq!(harness.wait_seen(1).await, vec![Seen::Input("async".into(), "\x1b[1;2D".into())]);

    harness.bridge.output("async", "\x1b[2J\x1b[H1 of 2\r\n\r\nWhich environment?\r\n\r\n› 1. Staging\r\n  2. Production\r\n  3. Other\r\n\r\nenter submit   ⌃] skip   ⇧→ main prompt   ⇧← next question".as_bytes());
    let choice: Value = client.get(&context_url).send().await.unwrap().json().await.unwrap();
    assert_eq!(choice["prompt"]["protocol"], "codex-question-async");
    assert_eq!(client.post(&response_url).json(&json!({"promptId":choice["prompt"]["id"],"action":"select","optionId":"option-2"})).send().await.unwrap().status(), 200);
    assert_eq!(harness.wait_seen(2).await.last(), Some(&Seen::Input("async".into(), "\x1b[B\r".into())));

    harness.bridge.output("async", "\x1b[2J\x1b[HWhere should files go?\r\n\r\nType your answer\r\n\r\nenter submit   ⌃] skip   ⇧→ main prompt".as_bytes());
    let written: Value = client.get(&context_url).send().await.unwrap().json().await.unwrap();
    assert_eq!(written["prompt"]["question"], "Where should files go?");
    assert_eq!(written["prompt"]["kind"], "freeform");
    assert_eq!(client.post(&response_url).json(&json!({"promptId":written["prompt"]["id"],"action":"text","text":"docs/"})).send().await.unwrap().status(), 200);
    assert_eq!(&harness.wait_seen(4).await[2..], &[
        Seen::Input("async".into(), "\x1b[200~docs/\x1b[201~".into()), Seen::Input("async".into(), "\r".into())]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_come_from_the_session_project_root() {
    let harness = Harness::start();
    let root = harness.root.path().join("repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    harness.register("a", Some(root.join("src")), Some(root.clone()));
    harness.register("b", Some(root.clone()), None);
    let expected =
        crate::projects::automatic_id(&crate::projects::cwd_key(&root.to_string_lossy()));

    let sessions: Vec<Value> = reqwest::get(harness.url("/api/sessions"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        sessions.iter().all(|s| s["projectId"] == expected.as_str()),
        "{sessions:?}"
    );
    let projects: Vec<Value> = reqwest::get(harness.url("/api/projects"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["id"], expected.as_str());
    assert_eq!(projects[0]["automatic"], true);
    assert_eq!(projects[0]["name"], "Repo");

    let renamed: Value = reqwest::Client::new()
        .patch(harness.url(&format!("/api/projects/{expected}")))
        .json(&json!({ "name": "My Repo" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renamed["name"], "My Repo");
    assert_eq!(
        renamed["id"],
        expected.as_str(),
        "ids are stable across renames"
    );
    let saved: Value = serde_json::from_slice(
        &std::fs::read(
            harness
                .root
                .path()
                .join("bridge/.terminal-web-projects.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["names"][0]["name"], "My Repo");
    let created = reqwest::Client::new()
        .post(harness.url("/api/projects"))
        .json(&json!({ "cwd": root.to_string_lossy() }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    assert_eq!(
        created.json::<Value>().await.unwrap()["id"],
        expected.as_str()
    );
    let bad = reqwest::Client::new()
        .post(harness.url("/api/projects"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bro_sees_the_names_and_order_clients_give_projects_and_the_orchestrator_tiles_them() {
    let harness = Harness::start();
    let one = harness.root.path().join("one");
    let two = harness.root.path().join("two");
    for dir in [&one, &two] {
        std::fs::create_dir_all(dir).unwrap();
    }
    harness.register("a", Some(one.clone()), Some(one.clone()));
    harness.register("b", Some(two.clone()), Some(two.clone()));
    let id = |dir: &PathBuf| crate::projects::automatic_id(&crate::projects::cwd_key(&dir.to_string_lossy()));
    let _ = reqwest::get(harness.url("/api/projects")).await.unwrap();
    let plain = harness.bridge.client_projects();
    assert_eq!(plain.len(), 2);
    assert!(plain.iter().all(|p| p.name.is_none() && p.order.is_none()), "folder names, nobody's order: {plain:?}");

    let client = reqwest::Client::new();
    client.patch(harness.url(&format!("/api/projects/{}", id(&two)))).json(&json!({ "name": "Second" })).send().await.unwrap();
    client.patch(harness.url("/api/projects/order")).json(&json!({ "ids": [id(&two), id(&one)] })).send().await.unwrap();
    let arranged = harness.bridge.client_projects();
    let of = |dir: &PathBuf| arranged.iter().find(|p| p.cwd == *dir).unwrap().clone();
    assert_eq!((of(&two).name.as_deref(), of(&two).order), (Some("Second"), Some(0)));
    assert_eq!((of(&one).name, of(&one).order), (None, Some(1)));

    let call = |id: u64, arguments: Value| {
        let client = client.clone();
        let url = harness.url("/mcp");
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": "show_project", "arguments": arguments } });
        async move { client.post(url).json(&body).send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let shown = call(1, json!({ "project": "Second" })).await;
    assert_eq!(shown["result"]["isError"], false, "{shown}");
    let all = call(2, json!({})).await;
    assert_eq!(all["result"]["isError"], false, "{all}");
    let seen = harness.seen.lock().clone();
    assert!(seen.contains(&Seen::Tile(Some(two.clone()))), "{seen:?}");
    assert!(seen.contains(&Seen::Tile(None)), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_discovery_qr_auth_and_shutdown() {
    let harness = Harness::start();
    let status = harness.bridge.status();
    assert!(status.running);
    assert_ne!(status.port, 0);
    assert_eq!(status.urls[0], format!("http://127.0.0.1:{}/", status.port));
    assert_eq!(status.error, None);
    let token_file = harness.root.path().join("bridge/.terminal-web-token");
    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap().trim(),
        status.token
    );
    let rows = harness.bridge.pairing_qr().unwrap();
    assert!(
        rows.len() > 10
            && rows
                .iter()
                .all(|row| row.chars().count() == rows[0].chars().count())
    );

    let info_path = harness.root.path().join("bridge/.terminal-web-server.json");
    let info: Value = serde_json::from_slice(&std::fs::read(&info_path).unwrap()).unwrap();
    assert_eq!(info["runtime"], "bro");
    assert_eq!(info["port"], status.port);
    assert_eq!(info["pid"], std::process::id());
    assert!(info["startedAt"].is_string() && info["host"] == "127.0.0.1");
    let endpoint = crate::read_endpoint(&harness.root.path().join("bridge")).unwrap();
    assert_eq!((endpoint.port, endpoint.pid), (status.port, std::process::id()));
    assert_eq!(harness.bridge.local_url(), Some(endpoint.local_url()));

    // Loopback needs no token; the embedded client is served for SPA paths.
    let health: Value = reqwest::get(harness.url("/api/health"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["runtime"], "bro");
    let page = reqwest::get(harness.url("/some/spa/route")).await.unwrap();
    if crate::server::embedded_asset("index.html").is_some() {
        assert_eq!(page.status(), 200);
        assert!(page.text().await.unwrap().contains("<html"));
    } else {
        assert_eq!(page.status(), 404);
    }
    // ACP routes explain themselves.
    let acp = reqwest::get(harness.url("/api/sessions/x/agent"))
        .await
        .unwrap();
    assert_eq!(acp.status(), 503);
    assert!(
        acp.json::<Value>().await.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("not available")
    );

    // A remote peer without the token is refused (auth check called directly).
    let remote = Some("192.168.1.9:40000".parse().unwrap());
    let uri: http::Uri = "/api/bootstrap".parse().unwrap();
    assert!(!crate::is_authorized(
        remote,
        &http::HeaderMap::new(),
        &uri,
        &status.token
    ));
    let with_token: http::Uri = format!("/ws?token={}", status.token).parse().unwrap();
    assert!(crate::is_authorized(
        remote,
        &http::HeaderMap::new(),
        &with_token,
        &status.token
    ));

    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;
    for _ in 0..100 {
        if harness.bridge.status().clients == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(harness.bridge.status().clients, 1);

    let bridge = harness.bridge.clone();
    tokio::task::spawn_blocking(move || bridge.shutdown())
        .await
        .unwrap();
    assert!(!harness.bridge.status().running);
    assert!(
        !info_path.exists(),
        "the discovery file is removed on shutdown"
    );
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return true,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .unwrap();
    assert!(closed);
    assert!(reqwest::get(harness.url("/api/health")).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_errors_are_returned() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let root = tempfile::tempdir().unwrap();
    let result = Bridge::start_with(
        BridgeConfig {
            port,
            automatic_port: false,
            bind: "127.0.0.1".into(),
            data_root: root.path().to_path_buf(),
            web_interface: false,
        },
        StartOptions {
            import_legacy_token: false,
        },
        Box::new(|_| {}),
    );
    assert!(result.is_err());
    let invalid = Bridge::start_with(
        BridgeConfig {
            port: 0,
            automatic_port: false,
            bind: "not an address".into(),
            data_root: root.path().to_path_buf(),
            web_interface: false,
        },
        StartOptions {
            import_legacy_token: false,
        },
        Box::new(|_| {}),
    );
    assert!(
        invalid
            .err()
            .unwrap()
            .to_string()
            .contains("Invalid bridge bind address")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestrator_tools_are_served_over_mcp() {
    let harness = Harness::start();
    let client = reqwest::Client::new();
    let rpc = |id: u64, method: &str, params: Value| {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    };
    let call = |body: Value| {
        let client = client.clone();
        let url = harness.url("/mcp");
        async move {
            client
                .post(url)
                .json(&body)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let init = call(rpc(1, "initialize", json!({ "protocolVersion": "2025-06-18" }))).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "bro");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let accepted = client
        .post(harness.url("/mcp"))
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 202);

    let listed = call(rpc(2, "tools/list", json!({}))).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for wanted in ["open_session", "send_input", "read_chat", "list_chats", "find_project", "wait_for_output"] {
        assert!(names.contains(&wanted), "{wanted} missing from {names:?}");
    }
    assert!(listed["result"]["tools"][0]["inputSchema"]["type"] == "object");

    // "open a session in <folder>": the next squad name, titled with the project.
    let project = harness.root.path().join("justgains");
    std::fs::create_dir_all(&project).unwrap();
    let opened = call(rpc(
        3,
        "tools/call",
        json!({ "name": "open_session", "arguments": { "project": project.to_string_lossy() } }),
    ))
    .await;
    assert_eq!(opened["result"]["isError"], false, "{opened}");
    let text = opened["result"]["content"][0]["text"].as_str().unwrap();
    let outcome: Value = serde_json::from_str(text).unwrap();
    assert_eq!(outcome["name"], "Mal");
    assert_eq!(outcome["title"], "Mal · justgains");
    assert!(harness.seen.lock().iter().any(|seen| matches!(
        seen,
        Seen::Create(Some(profile), _, _) if profile == "claude"
    )));

    // The name alone reaches the session, and a rename reaches bro's sidebar.
    let renamed = call(rpc(
        4,
        "tools/call",
        json!({ "name": "rename_session", "arguments": { "sessionId": "mal", "title": "Mal · gains" } }),
    ))
    .await;
    assert_eq!(renamed["result"]["isError"], false, "{renamed}");
    assert!(harness.seen.lock().iter().any(|seen| matches!(
        seen,
        Seen::Rename(_, title) if title == "Mal · gains"
    )));

    let unknown = call(rpc(5, "tools/call", json!({ "name": "nope", "arguments": {} }))).await;
    assert_eq!(unknown["result"]["isError"], true);
    let missing = call(rpc(6, "bogus/method", json!({}))).await;
    assert_eq!(missing["error"]["code"], -32601);

    let mut socket = harness.socket().await;
    let hello = recv_type(&mut socket, "hello").await;
    assert_eq!(hello["orchestrator"]["state"], "idle");
    assert_eq!(hello["orchestrator"]["config"]["provider"], "claude-code");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_orchestrators_open_from_a_source_and_receive_delegated_work() {
    let harness = Harness::start();
    harness.bridge.register(SessionMeta {
        id: "workspace-coordinator".into(), title: "Orchestrator".into(), shell: "codex".into(),
        args: vec![], cwd: None, project: None, pid: Some(7), cols: 100, rows: 30,
        agent: Some("codex".into()), kind: Some("orchestrator".into()),
    });
    harness.register("worker", None, None);
    let client = reqwest::Client::new();
    let call = |name: &str, arguments: Value| {
        let client = client.clone();
        let url = harness.url("/mcp");
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        async move { client.post(url).json(&body).send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let project = harness.root.path().to_path_buf();
    let opened = call("open_orchestrator", json!({ "project": project, "sourceSessionId": "workspace-coordinator", "focus": true })).await;
    assert_eq!(opened["result"]["isError"], false, "{opened}");
    let data: Value = serde_json::from_str(opened["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["id"], "project-coordinator");
    assert!(harness.seen.lock().iter().any(|s| matches!(s, Seen::OpenOrchestrator(cwd, source) if bro_core::util::same_path(cwd, &project) && source == "workspace-coordinator")));
    assert!(harness.seen.lock().contains(&Seen::Focus("project-coordinator".into())));
    let list = call("list_sessions", json!({})).await;
    let sessions: Value = serde_json::from_str(list["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let sessions = sessions.as_array().unwrap();
    assert!(sessions.iter().find(|s| s["id"] == "workspace-coordinator").unwrap()["projectPath"].is_null());
    let target = sessions.iter().find(|s| s["id"] == "project-coordinator").unwrap();
    assert_eq!(target["orchestrator"], true);
    assert!(target["projectPath"].is_string());
    let sent = call("send_input", json!({ "sessionId": "project-coordinator", "text": "review this project", "submit": false })).await;
    assert_eq!(sent["result"]["isError"], false, "a coordinator can delegate to another: {sent}");
    let invalid = call("open_orchestrator", json!({ "project": project, "sourceSessionId": "worker" })).await;
    assert_eq!(invalid["result"]["isError"], true, "a worker cannot supply coordinator settings");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestrator_session_never_acts_on_itself() {
    let harness = Harness::start();
    harness.register("worker", None, None);
    harness.bridge.register(SessionMeta {
        id: "boss".into(),
        title: "orchestrator".into(),
        shell: "claude".into(),
        args: vec![],
        cwd: None,
        project: None,
        pid: Some(7),
        cols: 100,
        rows: 30,
        agent: Some("claude".into()),
        kind: Some("orchestrator".into()),
    });
    let client = reqwest::Client::new();
    let call = |id: u64, name: &str, arguments: Value| {
        let client = client.clone();
        let url = harness.url("/mcp");
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        async move { client.post(url).json(&body).send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let text = |v: &Value| v["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();

    let listed = call(1, "list_sessions", json!({})).await;
    let sessions: Value = serde_json::from_str(&text(&listed)).unwrap();
    let boss = sessions.as_array().unwrap().iter().find(|s| s["id"] == "boss").unwrap();
    assert_eq!(boss["orchestrator"], true);
    assert!(boss["summary"].as_str().unwrap().contains("orchestrator"));

    for (i, (name, args)) in [
        ("send_input", json!({ "sessionId": "boss", "text": "hi" })),
        ("send_keys", json!({ "sessionId": "boss", "keys": ["enter"] })),
        ("close_session", json!({ "sessionId": "boss" })),
        ("wait_for_output", json!({ "sessionId": "boss", "timeoutSeconds": 1 })),
    ]
    .into_iter()
    .enumerate()
    {
        let out = call(10 + i as u64, name, args).await;
        assert_eq!(out["result"]["isError"], true, "{name}: {out}");
        assert!(text(&out).contains("orchestrator's own session"), "{name}: {out}");
    }
    // reading itself is fine
    let read = call(20, "read_session", json!({ "sessionId": "boss" })).await;
    assert_eq!(read["result"]["isError"], false, "{read}");

    // "close everything" closes the workers, never the orchestrator
    let closed = call(21, "close_all_sessions", json!({ "includeBusy": true })).await;
    assert_eq!(closed["result"]["isError"], false, "{closed}");
    let kills: Vec<String> = harness.seen.lock().iter().filter_map(|s| match s { Seen::Kill(id) => Some(id.clone()), _ => None }).collect();
    assert_eq!(kills, vec!["worker".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routed_hugh_hands_panels_schedules_and_resets_to_the_orchestrator_session() {
    let harness = Harness::start();
    harness.bridge.route_orchestrator(true);
    let client = reqwest::Client::new();
    let seen_orch = |harness: &Harness| -> Vec<crate::OrchestratorCommand> {
        harness.seen.lock().iter().filter_map(|s| match s { Seen::Orchestrator(c) => Some(c.clone()), _ => None }).collect()
    };
    let wait_for = |harness: &Harness, want: &dyn Fn(&crate::OrchestratorCommand) -> bool| {
        let until = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < until {
            if let Some(c) = seen_orch(harness).into_iter().find(|c| want(c)) {
                return Some(c);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    };

    // a panel message goes to the session, with a pending answer in the shared transcript
    let sent = client.post(harness.url("/api/orchestrator/messages")).json(&json!({ "text": "what is Toast doing?" })).send().await.unwrap();
    assert_eq!(sent.status(), 202);
    let Some(crate::OrchestratorCommand::Message { id, text, voice, .. }) = wait_for(&harness, &|c| matches!(c, crate::OrchestratorCommand::Message { .. })) else { panic!("no routed message") };
    assert_eq!((text.as_str(), voice), ("what is Toast doing?", false));
    let status: Value = client.get(harness.url("/api/orchestrator")).send().await.unwrap().json().await.unwrap();
    let items = status["transcript"].as_array().or(status["items"].as_array()).cloned().unwrap_or_default();
    assert!(items.iter().any(|i| i["role"] == "user" && i["text"] == "what is Toast doing?"), "{status}");
    assert!(items.iter().any(|i| i["id"] == id.as_str() && i["status"] == "streaming"), "{status}");
    // the session's answer lands in the panel
    harness.bridge.orchestrator_answer(&id, "Toast is running the tests.", true);
    let status: Value = client.get(harness.url("/api/orchestrator")).send().await.unwrap().json().await.unwrap();
    let items = status["transcript"].as_array().or(status["items"].as_array()).cloned().unwrap_or_default();
    assert!(items.iter().any(|i| i["id"] == id.as_str() && i["status"] == "done" && i["text"] == "Toast is running the tests."), "{status}");

    // stop and clear from a panel act on the session
    client.post(harness.url("/api/orchestrator/cancel")).send().await.unwrap();
    assert!(wait_for(&harness, &|c| *c == crate::OrchestratorCommand::Cancel).is_some());
    client.delete(harness.url("/api/orchestrator/messages")).send().await.unwrap();
    assert!(wait_for(&harness, &|c| *c == crate::OrchestratorCommand::ClearContext { handoff: None }).is_some());

    // its own tools: a fresh start with a handoff, and the schedule
    let call = |id: u64, name: &str, arguments: Value| {
        let client = client.clone();
        let url = harness.url("/mcp");
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        async move { client.post(url).json(&body).send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let text_of = |v: &Value| v["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    let cleared = call(1, "clear_own_context", json!({ "handoff": "Toast runs the tests in justgains." })).await;
    assert_eq!(cleared["result"]["isError"], false, "{cleared}");
    assert!(wait_for(&harness, &|c| *c == crate::OrchestratorCommand::ClearContext { handoff: Some("Toast runs the tests in justgains.".into()) }).is_some());

    let added = call(2, "schedule_add", json!({ "prompt": "check on Toast", "name": "toast check", "every": "30m" })).await;
    assert_eq!(added["result"]["isError"], false, "{added}");
    let listed = call(3, "schedule_list", json!({})).await;
    assert!(text_of(&listed).contains("toast check") && text_of(&listed).contains("every 30m"), "{listed}");
    let bad = call(4, "schedule_add", json!({ "prompt": "x" })).await;
    assert_eq!(bad["result"]["isError"], true);
    let removed = call(5, "schedule_remove", json!({ "id": "toast check" })).await;
    assert_eq!(removed["result"]["isError"], false, "{removed}");
    assert_eq!(text_of(&call(6, "schedule_list", json!({})).await), "Nothing is scheduled.");

    // bro's own timer fires a trigger: no agent involved
    call(7, "schedule_add", json!({ "prompt": "say hi", "name": "soon", "in": "1s" })).await;
    let fired = wait_for(&harness, &|c| matches!(c, crate::OrchestratorCommand::Scheduled { name, .. } if name == "soon"));
    assert!(matches!(fired, Some(crate::OrchestratorCommand::Scheduled { ref prompt, persist: false, .. }) if prompt == "say hi"), "{fired:?}");
    assert_eq!(text_of(&call(8, "schedule_list", json!({})).await), "Nothing is scheduled.", "a one-off is gone once it ran");
}

/// A workspace: `<root>/project` (project root) with cwd `project/app`.
fn file_workspace(harness: &Harness) -> (PathBuf, PathBuf) {
    let project = harness.root.path().join("project");
    let cwd = project.join("app");
    std::fs::create_dir_all(cwd.join("src")).unwrap();
    std::fs::write(cwd.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(project.join("README.md"), "# Read me\n").unwrap();
    harness.register("files", Some(cwd.clone()), Some(project.clone()));
    (project, cwd)
}

/// The path bro hands the TUI: canonical, without the Windows `\\?\` prefix.
fn shown(path: &std::path::Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap();
    PathBuf::from(canonical.to_string_lossy().trim_start_matches(r"\\?\"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn links_resolve_and_preview_use_the_session_roots() {
    let harness = Harness::start();
    let (project, _cwd) = file_workspace(&harness);
    let outside = harness.root.path().join("outside.txt");
    std::fs::write(&outside, "secret").unwrap();
    let client = reqwest::Client::new();
    let mut candidates: Vec<Value> = vec![
        json!({ "path": "src/main.rs" }),
        json!("README.md"),
        json!({ "path": "src" }),
        json!({ "path": "nope.rs" }),
        json!({ "path": outside.to_string_lossy() }),
    ];
    candidates.extend((0..300).map(|n| json!({ "path": format!("x{n}.rs") })));
    let body: Value = client
        .post(harness.url("/api/sessions/files/links/resolve"))
        .json(&json!({ "candidates": candidates }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 200, "capped");
    assert_eq!(results[0]["exists"], true);
    assert_eq!(results[0]["kind"], "file");
    assert_eq!(results[0]["relativePath"], "src/main.rs");
    assert_eq!(results[1]["path"], "README.md");
    assert_eq!(results[1]["relativePath"], "README.md");
    assert_eq!(
        PathBuf::from(results[1]["resolved"].as_str().unwrap()),
        shown(&project.join("README.md"))
    );
    assert_eq!(results[2]["kind"], "dir");
    assert_eq!(results[3], json!({ "path": "nope.rs", "exists": false }));
    assert_eq!(results[4]["exists"], false, "outside the roots");

    let preview: Value = client
        .get(harness.url("/api/sessions/files/file?path=README.md&line=1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["language"], "markdown");
    assert_eq!(preview["content"], "# Read me");
    let refused = client
        .get(harness.url(&format!(
            "/api/sessions/files/file?path={}",
            url::form_urlencoded::byte_serialize(outside.to_string_lossy().as_bytes())
                .collect::<String>()
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400, "file errors stay 400 for old clients");
    let unknown = client
        .post(harness.url("/api/sessions/ghost/links/resolve"))
        .json(&json!({ "candidates": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_broadcasts_to_clients_and_tells_bro() {
    let harness = Harness::start();
    let (_project, cwd) = file_workspace(&harness);
    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;
    let client = reqwest::Client::new();
    let reply: Value = client
        .post(harness.url("/api/sessions/files/open"))
        .json(&json!({ "path": "src/main.rs:12:4" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reply["opened"], true);
    assert!(reply["clients"].as_u64().unwrap() >= 1);
    let event = recv_type(&mut socket, "openFile").await;
    assert_eq!(event["sessionId"], "files");
    assert_eq!(event["path"], "src/main.rs");
    assert_eq!(event["relativePath"], "src/main.rs");
    assert_eq!(
        (event["line"].as_u64(), event["column"].as_u64()),
        (Some(12), Some(4))
    );
    assert_eq!(event["source"], "agent");
    assert!(event["at"].is_string());
    let main = shown(&cwd.join("src/main.rs"));
    assert_eq!(PathBuf::from(event["resolved"].as_str().unwrap()), main);
    let seen = harness.wait_seen(1).await;
    assert_eq!(
        seen.last(),
        Some(&Seen::OpenFile("files".into(), main.clone(), Some(12), Some(4)))
    );

    // An explicit line wins over a suffix; `source` is passed through.
    client
        .post(harness.url("/api/sessions/files/open"))
        .json(&json!({ "path": "README.md", "line": 3, "source": "user" }))
        .send()
        .await
        .unwrap();
    let event = recv_type(&mut socket, "openFile").await;
    assert_eq!(
        (
            event["path"].as_str(),
            event["line"].as_u64(),
            event["source"].as_str()
        ),
        (Some("README.md"), Some(3), Some("user"))
    );

    let editor = client
        .post(harness.url("/api/sessions/files/open-in-editor"))
        .json(&json!({ "path": "src/main.rs", "line": 2 }))
        .send()
        .await
        .unwrap();
    assert_eq!(editor.status(), 204);
    let seen = harness.wait_seen(3).await;
    assert_eq!(
        seen.last(),
        Some(&Seen::OpenInEditor("files".into(), main, Some(2), None))
    );

    for (route, body, status) in [
        ("open", json!({ "path": "missing.rs" }), 404),
        ("open", json!({ "path": "src" }), 400),
        ("open", json!({}), 400),
        ("open-in-editor", json!({ "path": "missing.rs" }), 404),
    ] {
        let response = client
            .post(harness.url(&format!("/api/sessions/files/{route}")))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{route} {body}");
        let message: Value = response.json().await.unwrap();
        assert!(message["message"].is_string());
    }
    let unknown = client
        .post(harness.url("/api/sessions/ghost/open"))
        .json(&json!({ "path": "README.md" }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_streams_files_with_ranges() {
    let harness = Harness::start();
    let (_project, cwd) = file_workspace(&harness);
    // Multi-MB, non-repeating content so offsets are checked, not just lengths.
    let big: Vec<u8> = (0..5 * 1024 * 1024 + 123u32)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    std::fs::write(cwd.join("clip one.mp4"), &big).unwrap();
    let total = big.len();
    let client = reqwest::Client::new();
    let url = harness.url("/api/sessions/files/raw?path=clip%20one.mp4");

    let preview: Value = client
        .get(harness.url("/api/sessions/files/file?path=clip%20one.mp4"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (preview["kind"].as_str(), preview["mimeType"].as_str()),
        (Some("video"), Some("video/mp4"))
    );
    assert_eq!(preview["bytes"], total);
    let media_url = preview["url"].as_str().unwrap();
    assert!(media_url.starts_with("/api/sessions/files/raw?path=") && !media_url.contains("token"));
    let via_url = client
        .get(harness.url(media_url))
        .header("range", "bytes=0-3")
        .send()
        .await
        .unwrap();
    assert_eq!(via_url.status(), 206);
    assert_eq!(via_url.bytes().await.unwrap().as_ref(), &big[..4]);

    let full = client.get(&url).send().await.unwrap();
    assert_eq!(full.status(), 200);
    let headers = full.headers().clone();
    let header = |name: &str| headers[name].to_str().unwrap().to_owned();
    assert_eq!(header("accept-ranges"), "bytes");
    assert_eq!(header("content-type"), "video/mp4");
    assert_eq!(header("content-length"), total.to_string());
    assert_eq!(header("x-content-type-options"), "nosniff");
    assert_eq!(header("content-security-policy"), "sandbox");
    assert_eq!(header("content-disposition"), "inline; filename*=UTF-8''clip%20one.mp4");
    assert!(headers.contains_key("etag") && headers.contains_key("last-modified"));
    assert_eq!(full.bytes().await.unwrap().as_ref(), big.as_slice());

    let range = |spec: String| client.get(&url).header("range", spec).send();
    let first = range("bytes=0-0".into()).await.unwrap();
    assert_eq!(first.status(), 206);
    assert_eq!(first.headers()["content-range"].to_str().unwrap(), format!("bytes 0-0/{total}"));
    assert_eq!(first.headers()["content-length"].to_str().unwrap(), "1");
    assert_eq!(first.bytes().await.unwrap().as_ref(), &big[..1]);

    let open = range("bytes=10-".into()).await.unwrap();
    assert_eq!(open.status(), 206);
    assert_eq!(
        open.headers()["content-range"].to_str().unwrap(),
        format!("bytes 10-{}/{total}", total - 1)
    );
    assert_eq!(open.bytes().await.unwrap().as_ref(), &big[10..]);

    let suffix = range("bytes=-5".into()).await.unwrap();
    assert_eq!(suffix.status(), 206);
    assert_eq!(
        suffix.headers()["content-range"].to_str().unwrap(),
        format!("bytes {}-{}/{total}", total - 5, total - 1)
    );
    assert_eq!(suffix.bytes().await.unwrap().as_ref(), &big[total - 5..]);

    let middle = range("bytes=3000000-3065535".into()).await.unwrap();
    assert_eq!(middle.headers()["content-length"].to_str().unwrap(), "65536");
    assert_eq!(middle.bytes().await.unwrap().as_ref(), &big[3_000_000..3_065_536]);

    let beyond = range(format!("bytes={total}-")).await.unwrap();
    assert_eq!(beyond.status(), 416);
    assert_eq!(beyond.headers()["content-range"].to_str().unwrap(), format!("bytes */{total}"));
    assert_eq!(beyond.headers()["accept-ranges"].to_str().unwrap(), "bytes");

    let multi = range("bytes=0-1,4-5".into()).await.unwrap();
    assert_eq!(multi.status(), 200);
    assert_eq!(multi.bytes().await.unwrap().len(), total);

    // If-Range with a stale validator ignores the range; the current one honours it.
    let etag = header("etag");
    let stale = client
        .get(&url)
        .header("range", "bytes=0-0")
        .header("if-range", "\"old\"")
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 200);
    let current = client
        .get(&url)
        .header("range", "bytes=0-0")
        .header("if-range", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(current.status(), 206);
    let cached = client.get(&url).header("if-none-match", &etag).send().await.unwrap();
    assert_eq!(cached.status(), 304);

    let head = client.head(&url).send().await.unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-length"].to_str().unwrap(), total.to_string());
    assert_eq!(head.headers()["accept-ranges"].to_str().unwrap(), "bytes");
    assert!(head.bytes().await.unwrap().is_empty());
    let head_range = client.head(&url).header("range", "bytes=-5").send().await.unwrap();
    assert_eq!(head_range.status(), 206);
    assert_eq!(head_range.headers()["content-length"].to_str().unwrap(), "5");

    let download = client.get(format!("{url}&download=1")).send().await.unwrap();
    assert_eq!(
        download.headers()["content-disposition"].to_str().unwrap(),
        "attachment; filename*=UTF-8''clip%20one.mp4"
    );

    let readme = client
        .get(harness.url("/api/sessions/files/raw?path=README.md"))
        .send()
        .await
        .unwrap();
    assert_eq!(readme.headers()["content-type"].to_str().unwrap(), "text/plain; charset=utf-8");
    assert_eq!(readme.text().await.unwrap(), "# Read me\n");

    let outside = harness.root.path().join("outside.bin");
    std::fs::write(&outside, b"secret").unwrap();
    let status = |path: String| {
        let client = client.clone();
        let url = harness.url(&path);
        async move { client.get(url).send().await.unwrap().status().as_u16() }
    };
    let raw = "/api/sessions/files/raw?path=";
    let encoded: String =
        url::form_urlencoded::byte_serialize(outside.to_string_lossy().as_bytes()).collect();
    assert_eq!(status(format!("{raw}{encoded}")).await, 403);
    assert_eq!(status(format!("{raw}nope.mp4")).await, 404);
    assert_eq!(status(format!("{raw}src")).await, 400);
    assert_eq!(status("/api/sessions/ghost/raw?path=README.md".into()).await, 404);
}

// ----- agent insights (docs/agent-insights.md §4) ------------------------------

fn blocked(reason: &str) -> crate::SessionInsights {
    crate::SessionInsights {
        attention: Some(crate::Attention {
            state: crate::AttentionState::Blocked,
            reason: Some(reason.into()),
            since: Some("2026-10-02T00:00:00.000Z".into()),
            limit: None,
            notice: None,
        }),
        recap: Some(crate::Recap {
            text: "Shipped the build".into(),
            at: None,
            source: crate::RecapSource::Claude,
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn insights_publish_on_change_only_and_debounce() {
    let harness = Harness::start();
    harness.register("agent", None, None);
    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;

    harness.bridge.set_insights("agent", blocked("Bash: npm publish"));
    let first = recv_type(&mut socket, "session").await;
    assert_eq!(first["session"]["id"], "agent");
    assert_eq!(
        first["session"]["attention"],
        json!({ "state": "blocked", "reason": "Bash: npm publish", "since": "2026-10-02T00:00:00.000Z" })
    );
    assert_eq!(first["session"]["recap"], json!({ "text": "Shipped the build", "source": "claude" }));
    assert!(first["session"].get("agentFiles").is_none(), "omitted when empty");

    // Unchanged: nothing. Two quick changes: one event, carrying the latest.
    harness.bridge.set_insights("agent", blocked("Bash: npm publish"));
    let started = std::time::Instant::now();
    harness.bridge.set_insights("agent", blocked("Question: Which DB?"));
    harness.bridge.set_insights("agent", blocked("Edit: main.rs"));
    let next = recv_type(&mut socket, "session").await;
    assert!(started.elapsed() >= Duration::from_millis(300), "debounced");
    assert_eq!(next["session"]["attention"]["reason"], "Edit: main.rs");
    let quiet = tokio::time::timeout(Duration::from_millis(900), recv_type(&mut socket, "session")).await;
    assert!(quiet.is_err(), "one event for the burst: {quiet:?}");

    // Clearing removes the fields; unknown ids are ignored.
    harness.bridge.set_insights("agent", Default::default());
    let cleared = recv_type(&mut socket, "session").await;
    assert!(cleared["session"].get("attention").is_none() && cleared["session"].get("recap").is_none());
    harness.bridge.set_insights("ghost", blocked("x"));
    let sessions: Vec<Value> = reqwest::get(harness.url("/api/sessions")).await.unwrap().json().await.unwrap();
    assert_eq!(sessions.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attention_receipts_sync_clients_and_cannot_clear_a_newer_event() {
    let harness = Harness::start();
    harness.register("agent", None, None);
    let mut event = blocked("Question: Which database?");
    event.attention.as_mut().unwrap().notice = Some(crate::insights::AttentionNotice {
        event_id: "event-1".into(), kind: crate::insights::NoticeKind::Input, unread: true, enabled: true,
    });
    harness.bridge.set_insights("agent", event.clone());
    let client = reqwest::Client::new();
    let checked: Value = client.post(harness.url("/api/sessions/agent/seen")).json(&json!({"eventId":"event-1"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(checked["attention"]["notice"]["unread"], false);
    assert_eq!(checked["attention"]["state"], "blocked");
    harness.bridge.set_insights("agent", event.clone());
    let summary: Vec<Value> = reqwest::get(harness.url("/api/sessions")).await.unwrap().json().await.unwrap();
    assert_eq!(summary[0]["attention"]["notice"]["unread"], false, "stale host updates cannot undo a receipt");
    event.attention.as_mut().unwrap().notice.as_mut().unwrap().event_id = "event-2".into();
    harness.bridge.set_insights("agent", event);
    let newer: Value = client.post(harness.url("/api/sessions/agent/seen")).json(&json!({"eventId":"event-1"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(newer["attention"]["notice"]["unread"], true);
    assert_eq!(newer["attention"]["notice"]["eventId"], "event-2");
    assert_eq!(client.post(harness.url("/api/sessions/agent/seen")).json(&json!({})).send().await.unwrap().status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_files_route_lists_relative_paths_and_counts() {
    let harness = Harness::start();
    let (project, cwd) = file_workspace(&harness);
    let outside = harness.root.path().join("outside.png");
    std::fs::write(&outside, "png").unwrap();
    let client = reqwest::Client::new();

    let empty: Value = client.get(harness.url("/api/sessions/files/agent-files")).send().await.unwrap().json().await.unwrap();
    assert_eq!(empty, json!({ "sessionId": "files", "sent": [], "touched": [] }));

    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;
    let mut touched: Vec<crate::TouchedFile> = vec![
        crate::TouchedFile { path: cwd.join("src/main.rs"), kind: crate::TouchKind::Edit, at: Some("2026-10-02T00:00:02.000Z".into()), line: Some(3) },
        crate::TouchedFile { path: project.join("README.md"), kind: crate::TouchKind::Read, at: None, line: None },
        crate::TouchedFile { path: cwd.join("gone.rs"), kind: crate::TouchKind::Delete, at: None, line: None },
    ];
    touched.extend((0..250).map(|n| crate::TouchedFile { path: cwd.join(format!("f{n}.rs")), kind: crate::TouchKind::Write, at: None, line: None }));
    harness.bridge.set_agent_files("files", crate::AgentFiles {
        transcript: Some(harness.root.path().join("t.jsonl")),
        sent: vec![crate::SentFile { path: outside.clone(), caption: Some("chart".into()), at: None, mime_type: Some("image/png".into()), is_image: true }],
        touched,
    });
    let event = recv_type(&mut socket, "session").await;
    assert_eq!(event["session"]["agentFiles"]["sent"], 1);
    assert_eq!(event["session"]["agentFiles"]["touched"], 253);
    assert!(event["session"]["agentFiles"]["updatedAt"].is_string());

    let body: Value = client.get(harness.url("/api/sessions/files/agent-files")).send().await.unwrap().json().await.unwrap();
    assert_eq!(body["sessionId"], "files");
    assert!(body["transcript"].as_str().unwrap().ends_with("t.jsonl"));
    let sent = &body["sent"][0];
    assert_eq!(sent["caption"], "chart");
    assert_eq!(sent["isImage"], true);
    assert_eq!(sent["mimeType"], "image/png");
    assert!(sent.get("relativePath").is_none(), "outside the roots");
    let list = body["touched"].as_array().unwrap();
    assert_eq!(list.len(), 200, "capped");
    assert_eq!(list[0]["relativePath"], "src/main.rs");
    assert_eq!(list[0]["kind"], "edit");
    assert_eq!(list[0]["line"], 3);
    assert_eq!(list[1]["relativePath"], "README.md");
    assert_eq!(list[2]["relativePath"], "gone.rs", "deleted files resolve lexically");
    assert_eq!(list[2]["kind"], "delete");

    let unknown = client.get(harness.url("/api/sessions/ghost/agent-files")).send().await.unwrap();
    assert_eq!(unknown.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notifications_use_the_reason_and_the_recap() {
    let harness = Harness::start();
    harness.bridge.register(SessionMeta {
        id: "agent".into(),
        title: "Claude".into(),
        shell: "claude".into(),
        args: vec![],
        cwd: None,
        project: None,
        pid: None,
        cols: 100,
        rows: 30,
        agent: Some("claude".into()),
        kind: None,
    });
    harness.register("done", None, None);
    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;

    harness.bridge.set_insights("agent", blocked("Bash: rm -rf dist"));
    harness.bridge.output(
        "agent",
        "Do you want to proceed?\r\n❯ 1. Yes\r\n  2. No\r\n\r\nEnter to confirm · Esc to cancel".as_bytes(),
    );
    let notify = recv_type(&mut socket, "notify").await;
    assert_eq!(notify["title"], "Claude Code needs input");
    assert_eq!(notify["body"], "Bash: rm -rf dist");

    // A finished turn: the recap replaces "Task finished"; a done-state reason wins over it.
    harness.bridge.set_insights("done", crate::SessionInsights {
        attention: None,
        recap: Some(crate::Recap { text: "Tests pass; ready to ship".into(), at: None, source: crate::RecapSource::LastMessage }),
    });
    harness.bridge.output("done", b"ok\x07");
    let finished = recv_type(&mut socket, "notify").await;
    assert_eq!(finished["sessionId"], "done");
    assert_eq!(finished["body"], "Tests pass; ready to ship");
    let raised = harness.notifications.lock().clone();
    assert!(raised.iter().any(|n| n.body == "Tests pass; ready to ship"));
}

/// Waits (briefly) until the bridge's remote-viewer count for `id` is `want`.
async fn viewers_become(harness: &Harness, id: &str, want: usize) -> usize {
    for _ in 0..200 {
        if harness.bridge.remote_viewers(id) == want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    harness.bridge.remote_viewers(id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_viewers_follow_subscriptions_slots_and_disconnects() {
    let harness = Harness::start();
    harness.register("s1", None, None);
    harness.register("s2", None, None);
    assert_eq!(harness.bridge.remote_viewers("s1"), 0);

    let mut phone = harness.socket().await;
    recv_type(&mut phone, "hello").await;
    send(&mut phone, json!({ "type": "subscribe", "sessionId": "s1" })).await;
    recv_type(&mut phone, "snapshot").await;
    assert_eq!(viewers_become(&harness, "s1", 1).await, 1);

    // A second client on the same session, in a named slot.
    let mut web = harness.socket().await;
    recv_type(&mut web, "hello").await;
    send(&mut web, json!({ "type": "subscribe", "sessionId": "s1", "slot": "side" })).await;
    recv_type(&mut web, "snapshot").await;
    assert_eq!(viewers_become(&harness, "s1", 2).await, 2);

    // Re-subscribing a slot to another session moves the count.
    send(&mut phone, json!({ "type": "subscribe", "sessionId": "s2" })).await;
    recv_type(&mut phone, "snapshot").await;
    assert_eq!(viewers_become(&harness, "s1", 1).await, 1);
    assert_eq!(viewers_become(&harness, "s2", 1).await, 1);

    // Unsubscribing a slot releases it; unknown slots are a no-op.
    send(&mut web, json!({ "type": "unsubscribe", "slot": "nope" })).await;
    send(&mut web, json!({ "type": "unsubscribe", "slot": "side" })).await;
    assert_eq!(viewers_become(&harness, "s1", 0).await, 0);

    // Dropping the connection releases whatever it still watched.
    drop(phone);
    assert_eq!(viewers_become(&harness, "s2", 0).await, 0);
    assert!(harness.bridge.state().viewers.lock().is_empty(), "no leaked entries");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleanup_usage_and_fleet_tools() {
    let harness = Harness::start();
    for id in ["alpha", "bravo", "charlie"] {
        harness.register(id, None, None);
    }
    let client = reqwest::Client::new();
    let kills = |harness: &Harness| {
        let mut ids: Vec<String> = harness
            .seen
            .lock()
            .iter()
            .filter_map(|seen| match seen {
                Seen::Kill(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        ids.sort();
        ids
    };

    // REST cleanup, keeping one session.
    let outcome: Value = client
        .post(harness.url("/api/sessions/cleanup"))
        .json(&json!({ "except": ["bravo"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(outcome["closed"].as_array().unwrap().len(), 2, "{outcome}");
    assert_eq!(kills(&harness), ["alpha", "charlie"]);

    // An empty DELETE closes everything.
    harness.seen.lock().clear();
    let deleted = client.delete(harness.url("/api/sessions")).send().await.unwrap();
    assert_eq!(deleted.status(), 200);
    assert_eq!(kills(&harness), ["alpha", "bravo", "charlie"]);
    let bad = client
        .post(harness.url("/api/sessions/cleanup"))
        .body("{nope")
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // Usage is served from bro's cache.
    let usage: Value = client
        .get(harness.url("/api/usage"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(usage["logins"].is_array(), "{usage}");

    // The same over MCP.
    let call = |id: u64, name: &str, arguments: Value| {
        let client = client.clone();
        let url = harness.url("/mcp");
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        async move {
            let answer: Value = client.post(url).json(&body).send().await.unwrap().json().await.unwrap();
            assert_eq!(answer["result"]["isError"], false, "{answer}");
            serde_json::from_str::<Value>(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
        }
    };
    let waited = call(1, "wait_for_sessions", json!({ "sessionIds": ["alpha", "bravo title"], "timeoutSeconds": 10 })).await;
    assert_eq!(waited["outcome"], "every tab finished", "{waited}");
    assert_eq!(waited["sessions"][1]["id"], "bravo");
    assert_eq!(waited["sessions"][0]["state"], "idle");
    let usage = call(2, "get_usage", json!({})).await;
    assert!(usage["picks"].is_object());
    harness.seen.lock().clear();
    let closed = call(3, "close_all_sessions", json!({ "except": ["charlie"] })).await;
    assert_eq!(closed["closed"].as_array().unwrap().len(), 2, "{closed}");
    assert_eq!(kills(&harness), ["alpha", "bravo"]);
}

// ----- other bridges on this machine (hosts.rs) --------------------------------

fn plain_bridge(data_root: PathBuf) -> Bridge {
    Bridge::start_with(
        BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root, web_interface: false },
        StartOptions { import_legacy_token: false },
        Box::new(|_| {}),
    )
    .unwrap()
}

fn agent_meta(id: &str, cwd: PathBuf) -> SessionMeta {
    SessionMeta {
        id: id.into(),
        title: format!("{id} title"),
        shell: "claude".into(),
        args: vec![],
        cwd: Some(cwd),
        project: None,
        pid: Some(7),
        cols: 100,
        rows: 30,
        agent: Some("claude".into()),
        kind: None,
    }
}

/// Like [`recv_until`], for events on the host scan's cadence (seconds apart).
async fn recv_slow(socket: &mut Socket, accept: impl Fn(&Value) -> bool) -> Option<Value> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let message = serde_json::from_str::<Value>(&text).unwrap();
                    if accept(&message) {
                        return message;
                    }
                }
                Some(Ok(_)) => continue,
                other => panic!("socket ended: {other:?}"),
            }
        }
    })
    .await
    .ok()
}

async fn hosts_of(port: u16) -> Vec<Value> {
    let body: Value = reqwest::get(format!("http://127.0.0.1:{port}/api/hosts")).await.unwrap().json().await.unwrap();
    body["hosts"].as_array().cloned().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridges_on_one_machine_list_each_other_for_one_tap_switching() {
    let harness = Harness::start();
    let data_root = harness.root.path().join("bridge");
    let peer = plain_bridge(data_root.clone());
    let peer_port = peer.status().port;
    assert_eq!(peer.status().token, harness.bridge.status().token, "one data root, one token");
    peer.register(agent_meta("build", harness.root.path().join("justgains")));
    peer.set_insights("build", blocked("Bash: npm publish"));
    assert_eq!(std::fs::read_dir(data_root.join("hosts")).unwrap().count(), 2, "both registered");

    // Its card: loopback callers get the token, so other data roots can hand clients over.
    let card: Value = reqwest::get(format!("http://127.0.0.1:{peer_port}/api/host")).await.unwrap().json().await.unwrap();
    assert_eq!(card["runtime"], "bro");
    assert_eq!(card["port"], peer_port);
    assert_eq!(card["bind"], "127.0.0.1");
    assert_eq!(card["token"], peer.status().token);
    assert_eq!((card["sessions"].as_u64(), card["running"].as_u64(), card["awaiting"].as_u64()), (Some(1), Some(1), Some(1)));
    assert_eq!(card["projects"], json!(["Justgains"]));

    // This bridge first, then the peer; a shared token is not repeated.
    let hosts = hosts_of(harness.port).await;
    assert_eq!(hosts.len(), 2, "{hosts:?}");
    assert_eq!(hosts[0]["port"], harness.port);
    assert_eq!(hosts[0]["current"], true);
    assert!(hosts[0].get("token").is_none());
    assert_eq!(hosts[1]["port"], peer_port);
    assert_eq!(hosts[1]["current"], false);
    assert_eq!(hosts[1]["awaiting"], 1);
    assert!(hosts[1].get("token").is_none(), "same token: clients reuse theirs");
    assert_eq!(hosts_of(peer_port).await[1]["port"], harness.port, "and the other way round");

    // `hello` carries them; a peer that changes arrives as a `hosts` event.
    let mut socket = harness.socket().await;
    let hello = recv_type(&mut socket, "hello").await;
    assert_eq!(hello["hosts"][1]["port"], peer_port);
    peer.register(agent_meta("review", harness.root.path().join("bro")));
    let update = recv_slow(&mut socket, |message| message["type"] == "hosts" && message["hosts"][1]["running"] == 2)
        .await
        .expect("a hosts event after the peer changed");
    assert_eq!(update["hosts"][0]["current"], true);

    // A bridge with its own data root and token: its token is listed.
    let other_root = harness.root.path().join("other");
    let other = plain_bridge(other_root.clone());
    let other_token = other.status().token;
    assert_ne!(other_token, harness.bridge.status().token);
    for entry in std::fs::read_dir(other_root.join("hosts")).unwrap().flatten() {
        std::fs::copy(entry.path(), data_root.join("hosts").join(entry.file_name())).unwrap();
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    let hosts = hosts_of(harness.port).await;
    let listed = hosts.iter().find(|host| host["port"] == other.status().port).expect("the other bridge");
    assert_eq!(listed["token"], other_token);

    // Gone bridges drop out and leave the registry.
    peer.shutdown();
    other.shutdown();
    assert!(!data_root.join("hosts").read_dir().unwrap().flatten().any(|entry| {
        std::fs::read_to_string(entry.path()).unwrap().contains(&format!("\"port\": {peer_port}"))
    }));
    let gone = recv_slow(&mut socket, |message| message["type"] == "hosts" && message["hosts"].as_array().is_some_and(|hosts| hosts.len() == 1))
        .await
        .expect("a hosts event once the peers left");
    assert_eq!(gone["hosts"][0]["port"], harness.port);
}

// ----- folders a session can start in (folders.rs) -----------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folders_suggest_bro_projects_and_complete_typed_paths() {
    let harness = Harness::start();
    let code = harness.root.path().join("code");
    for dir in ["JustGains", "JustGains-Mobile", "bro-cli-v2"] {
        std::fs::create_dir_all(code.join(dir)).unwrap();
    }
    harness.bridge.set_known_folders(vec![code.join("bro-cli-v2"), code.join("gone")]);
    let get = |query: String| {
        let url = harness.url("/api/folders");
        async move {
            reqwest::Client::new().get(url::Url::parse_with_params(&url, &[("q", query)]).unwrap()).send().await.unwrap().json::<Value>().await.unwrap()
        }
    };
    let paths = |body: &Value| -> Vec<String> {
        body["folders"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_owned()).collect()
    };

    // Nothing typed: where bro has been working, existing folders only.
    let suggested = get(String::new()).await;
    assert_eq!(suggested["mode"], "suggest");
    assert_eq!(paths(&suggested), [code.join("bro-cli-v2").to_string_lossy()]);
    assert_eq!(suggested["folders"][0]["name"], "bro-cli-v2");
    assert_eq!(suggested["folders"][0]["source"], "known");

    // A path: completion inside its parent.
    let sep = std::path::MAIN_SEPARATOR;
    let typed = get(format!("{}{sep}just", code.to_string_lossy())).await;
    assert_eq!(typed["mode"], "path");
    assert_eq!(paths(&typed), [code.join("JustGains").to_string_lossy(), code.join("JustGains-Mobile").to_string_lossy()]);
    let children = get(format!("{}{sep}", code.to_string_lossy())).await;
    assert_eq!(paths(&children).len(), 3);

    // A name: the spoken-name search over everything bro knows.
    let named = get("bro cli".into()).await;
    assert_eq!(named["mode"], "search");
    // (drive folders are candidates too, so only membership is certain here)
    assert!(paths(&named).contains(&code.join("bro-cli-v2").to_string_lossy().into_owned()), "{named}");
}

// ----- jobs for a session's Claude Code mod (mod_jobs.rs) ----------------------------------------------------

type ModReply = tokio::sync::oneshot::Receiver<(Result<Value, String>, String)>;

/// Asks session `session`'s mod for `job`; the receiver gets the reply and the thread it ran on.
fn ask_mod(harness: &Harness, session: &str, job: Value) -> ModReply {
    let (tx, rx) = tokio::sync::oneshot::channel();
    harness.bridge.mod_ask(
        session,
        job,
        Box::new(move |result| {
            let thread = std::thread::current().name().unwrap_or_default().to_owned();
            let _ = tx.send((result, thread));
        }),
    );
    rx
}

async fn mod_reply(reply: ModReply) -> Result<Value, String> {
    let (result, thread) = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("the reply never came")
        .expect("the reply was dropped without running");
    assert!(thread.starts_with("bro-bridge"), "replies run on a bridge thread, not {thread:?}");
    result
}

fn mod_timing(pickup: u64, answer: u64, poll: u64, listening: u64) -> crate::mod_jobs::Timing {
    crate::mod_jobs::Timing {
        pickup: Duration::from_millis(pickup),
        answer: Duration::from_millis(answer),
        poll: Duration::from_millis(poll),
        listening: Duration::from_millis(listening),
    }
}

async fn mod_jobs_of(response: reqwest::Response) -> Vec<Value> {
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["jobs"].as_array().cloned().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mod_jobs_reach_the_sessions_mod_and_its_answer_comes_back() {
    let harness = Harness::start();
    harness.bridge.state().mod_jobs.set_timing(mod_timing(5_000, 5_000, 3_000, 40_000));
    let client = reqwest::Client::new();
    let answer = |session: &str, id: &str, body: Value| {
        client.post(harness.url(&format!("/api/mod/{session}/jobs/{id}"))).json(&body).send()
    };
    assert!(!harness.bridge.mod_listening("s1"));

    // The mod's poll is open: it counts as listening, and a job reaches it at once, fields beside the id.
    let poll = tokio::spawn(client.get(harness.url("/api/mod/s1/jobs")).send());
    for _ in 0..300 {
        if harness.bridge.mod_listening("s1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(harness.bridge.mod_listening("s1"));
    assert!(!harness.bridge.mod_listening("s2"));
    let asked = std::time::Instant::now();
    let reply = ask_mod(
        &harness,
        "s1",
        json!({ "kind": "read", "server": "claude_ai_JustGains", "uri": "ui://widget/workout-v84.html" }),
    );
    let jobs = mod_jobs_of(poll.await.unwrap().unwrap()).await;
    assert!(asked.elapsed() < Duration::from_secs(2), "an open poll answers as soon as a job arrives");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["kind"], "read");
    assert_eq!(jobs[0]["server"], "claude_ai_JustGains");
    assert_eq!(jobs[0]["uri"], "ui://widget/workout-v84.html");
    let id = jobs[0]["id"].as_str().unwrap().to_owned();

    // Only that session's mod can answer it, and only ids it was given.
    assert_eq!(answer("s2", &id, json!({ "ok": true })).await.unwrap().status(), 404);
    assert_eq!(answer("s1", "nope", json!({ "ok": true })).await.unwrap().status(), 404);

    // The answer can be a widget's worth of HTML.
    let html = format!("<html>{}</html>", "x".repeat(6 * 1024 * 1024));
    let contents = json!({ "contents": [{ "uri": "ui://widget/workout-v84.html", "mimeType": "text/html;profile=mcp-app", "text": html }] });
    let answered = answer("s1", &id, json!({ "ok": true, "result": contents })).await.unwrap();
    assert_eq!(answered.status(), 200);
    let result = mod_reply(reply).await.unwrap();
    assert_eq!(result["contents"][0]["text"].as_str().unwrap().len(), html.len());
    assert_eq!(result["contents"][0]["mimeType"], "text/html;profile=mcp-app");
    assert_eq!(answer("s1", &id, json!({ "ok": true })).await.unwrap().status(), 404, "answered once");

    // Jobs queued between polls go out together; the mod's error comes back as the error.
    let call = ask_mod(&harness, "s1", json!({ "kind": "call", "server": "x", "tool": "show_workout", "arguments": { "id": 7 } }));
    let odd = ask_mod(&harness, "s1", json!("not an object"));
    let jobs = mod_jobs_of(client.get(harness.url("/api/mod/s1/jobs")).send().await.unwrap()).await;
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    assert_eq!(jobs[0]["tool"], "show_workout");
    assert_eq!(jobs[0]["arguments"]["id"], 7);
    assert_eq!(jobs[1]["job"], "not an object");
    let (first, second) = (jobs[0]["id"].as_str().unwrap(), jobs[1]["id"].as_str().unwrap());
    let refused = answer("s1", first, json!({ "ok": false, "error": "no such tool" })).await.unwrap();
    assert_eq!(refused.status(), 200);
    assert_eq!(mod_reply(call).await.unwrap_err(), "no such tool");
    answer("s1", second, json!({ "ok": false })).await.unwrap();
    assert_eq!(mod_reply(odd).await.unwrap_err(), "the bro mod couldn't do it");
    let broken = client.post(harness.url("/api/mod/s1/jobs/x")).body("{nope").send().await.unwrap();
    assert_eq!(broken.status(), 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mod_jobs_fail_when_no_mod_takes_or_answers_them() {
    let harness = Harness::start();
    let jobs = harness.bridge.state().mod_jobs.clone();
    let client = reqwest::Client::new();
    let poll = || client.get(harness.url("/api/mod/s1/jobs")).send();

    // Nobody asks for work: the job fails, saying why.
    jobs.set_timing(mod_timing(200, 5_000, 3_000, 40_000));
    let asked = std::time::Instant::now();
    let error = mod_reply(ask_mod(&harness, "s1", json!({ "kind": "read" }))).await.unwrap_err();
    assert_eq!(error, crate::mod_jobs::NOT_LISTENING);
    assert!(asked.elapsed() >= Duration::from_millis(200));

    // Taken but never answered: it times out, and a late answer finds nothing.
    jobs.set_timing(mod_timing(5_000, 300, 3_000, 40_000));
    let reply = ask_mod(&harness, "s1", json!({ "kind": "read" }));
    let taken = mod_jobs_of(poll().await.unwrap()).await;
    assert_eq!(taken.len(), 1);
    let error = mod_reply(reply).await.unwrap_err();
    assert!(error.contains("didn't answer"), "{error}");
    let id = taken[0]["id"].as_str().unwrap();
    let late = client.post(harness.url(&format!("/api/mod/s1/jobs/{id}"))).json(&json!({ "ok": true })).send().await.unwrap();
    assert_eq!(late.status(), 404);

    // A poll with nothing to do comes back empty; the mod counts as listening a while after it stops asking.
    jobs.set_timing(mod_timing(5_000, 5_000, 300, 600));
    let waited = std::time::Instant::now();
    assert!(mod_jobs_of(poll().await.unwrap()).await.is_empty());
    assert!(waited.elapsed() >= Duration::from_millis(300));
    assert!(harness.bridge.mod_listening("s1"));
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(!harness.bridge.mod_listening("s1"));

    // Stopping the bridge fails what's left, and anything asked after.
    jobs.set_timing(mod_timing(60_000, 60_000, 3_000, 40_000));
    let pending = ask_mod(&harness, "s1", json!({ "kind": "read" }));
    let bridge = harness.bridge.clone();
    tokio::task::spawn_blocking(move || bridge.shutdown()).await.unwrap();
    assert_eq!(mod_reply(pending).await.unwrap_err(), crate::mod_jobs::STOPPED);
    let after = ask_mod(&harness, "s1", json!({ "kind": "read" }));
    assert_eq!(mod_reply(after).await.unwrap_err(), crate::mod_jobs::STOPPED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mod_jobs_refuse_callers_from_other_machines() {
    use axum::extract::connect_info::MockConnectInfo;
    let harness = Harness::start();
    let token = harness.bridge.status().token;
    // The bridge's own router, reached by a peer on the LAN.
    let lan: std::net::SocketAddr = "192.168.1.9:40000".parse().unwrap();
    let app = crate::server::router(harness.bridge.state().clone()).layer(MockConnectInfo(lan));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app.into_make_service()).await });
    let url = |path: &str| format!("http://127.0.0.1:{port}{path}");
    let client = reqwest::Client::new();

    // The token opens the rest of the API, never the mod's jobs.
    let polled = client.get(url("/api/mod/s1/jobs")).header("x-terminal-web-token", &token).send().await.unwrap();
    assert_eq!(polled.status(), 403);
    let answered = client
        .post(url("/api/mod/s1/jobs/x"))
        .header("x-terminal-web-token", &token)
        .json(&json!({ "ok": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(answered.status(), 403);
    assert!(!harness.bridge.mod_listening("s1"), "a refused poll doesn't count as listening");
    let health = client.get(url("/api/health")).header("x-terminal-web-token", &token).send().await.unwrap();
    assert_eq!(health.status(), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waveform_reads_a_sound_files_peaks_and_length() {
    let harness = Harness::start();
    let (_project, cwd) = file_workspace(&harness);
    // a second of 16-bit mono silence, then a second of a loud tone
    let rate = 8_000u32;
    let samples: Vec<i16> = (0..rate * 2).map(|i| if i < rate { 0 } else { ((i as f32 * 0.3).sin() * 30_000.0) as i16 }).collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut wav = vec![];
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    for part in [&16u32.to_le_bytes()[..], &1u16.to_le_bytes(), &1u16.to_le_bytes(), &rate.to_le_bytes(), &(rate * 2).to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes()] {
        wav.extend_from_slice(part);
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(cwd.join("take one.wav"), &wav).unwrap();
    std::fs::write(cwd.join("notes.txt"), "not a sound").unwrap();
    let client = reqwest::Client::new();

    let w: Value = client.get(harness.url("/api/sessions/files/waveform?path=take%20one.wav&bars=10")).send().await.unwrap().json().await.unwrap();
    let peaks: Vec<f64> = w["peaks"].as_array().unwrap().iter().map(|p| p.as_f64().unwrap()).collect();
    assert_eq!(peaks.len(), 10);
    assert!(peaks[..4].iter().all(|p| *p < 0.01) && peaks[6..].iter().all(|p| *p > 0.9), "{peaks:?}");
    assert_eq!(w["durationMs"], 2000);

    let not_sound = client.get(harness.url("/api/sessions/files/waveform?path=notes.txt")).send().await.unwrap();
    assert_eq!(not_sound.status(), 400);
    let missing = client.get(harness.url("/api/sessions/files/waveform?path=gone.mp3")).send().await.unwrap();
    assert_eq!(missing.status(), 404);
    let unknown = client.get(harness.url("/api/sessions/ghost/waveform?path=take%20one.wav")).send().await.unwrap();
    assert_eq!(unknown.status(), 404);
}

// ----- MCP apps for the phone (apps.rs) ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_thumbnails_keep_access_checks_and_original_downloads() {
    let harness = Harness::start();
    let (_project, cwd) = file_workspace(&harness);
    harness.bridge.set_media_anywhere(false);
    let picture = cwd.join("tile.png");
    image::RgbImage::from_pixel(1200, 600, image::Rgb([40, 140, 200])).save(&picture).unwrap();
    std::fs::write(cwd.join("broken.png"), "not a picture").unwrap();
    std::fs::write(cwd.join(".env"), "TOKEN=private").unwrap();
    let outside = harness.root.path().join("outside.png");
    image::RgbImage::from_pixel(10, 10, image::Rgb([0, 0, 0])).save(&outside).unwrap();
    let client = reqwest::Client::new();
    let route = harness.url("/api/sessions/files/raw?path=tile.png&thumbnail=256");
    let response = client.get(&route).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "image/jpeg");
    let etag = response.headers()["etag"].clone();
    let bytes = response.bytes().await.unwrap();
    let image = image::load_from_memory(&bytes).unwrap();
    assert_eq!((image.width(), image.height()), (256, 128));
    let unchanged = client.get(&route).header("if-none-match", etag).send().await.unwrap();
    assert_eq!(unchanged.status(), 304);
    let original = client.get(harness.url("/api/sessions/files/raw?path=tile.png")).send().await.unwrap();
    assert_eq!(original.headers()["content-type"], "image/png");
    assert_eq!(original.bytes().await.unwrap().as_ref(), std::fs::read(&picture).unwrap());
    for (path, status) in [(".env", 422), ("../../outside.png", 403), ("missing.png", 404), ("broken.png", 422)] {
        let response = client.get(harness.url(&format!("/api/sessions/files/raw?path={path}&thumbnail=256"))).send().await.unwrap();
        assert_eq!(response.status(), status, "{path}");
    }
    let head = client.head(&route).send().await.unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-length"], bytes.len().to_string());
    assert!(head.bytes().await.unwrap().is_empty());
    let preview: Value = client.get(harness.url("/api/sessions/files/file?path=tile.png&stream=1")).send().await.unwrap().json().await.unwrap();
    assert!(preview.get("data").is_none(), "mobile previews must not expand original images into base64 JSON");
    assert!(preview["url"].as_str().unwrap().contains("/raw?path="));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_json_compresses_while_raw_ranges_stay_exact() {
    let harness = Harness::start();
    let (_project, cwd) = file_workspace(&harness);
    let text = "a long repeated line of code for mobile compression\n".repeat(500);
    std::fs::write(cwd.join("many.txt"), &text).unwrap();
    let client = reqwest::Client::new();
    let compressed = client.get(harness.url("/api/sessions/files/file?path=many.txt"))
        .header("accept-encoding", "gzip").send().await.unwrap();
    assert_eq!(compressed.headers()["content-encoding"], "gzip");
    assert!(compressed.bytes().await.unwrap().len() < text.len() / 10);
    let range = client.get(harness.url("/api/sessions/files/raw?path=many.txt"))
        .header("accept-encoding", "gzip").header("range", "bytes=0-19").send().await.unwrap();
    assert_eq!(range.status(), 206);
    assert!(!range.headers().contains_key("content-encoding"));
    assert_eq!(range.bytes().await.unwrap().as_ref(), &text.as_bytes()[..20]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inactive_output_bursts_coalesce_without_delaying_the_visible_session() {
    let harness = Harness::start();
    harness.register("visible", None, None);
    harness.register("offscreen", None, None);
    let mut socket = harness.socket().await;
    recv_type(&mut socket, "hello").await;
    send(&mut socket, json!({ "type": "subscribe", "sessionId": "visible" })).await;
    recv_type(&mut socket, "snapshot").await;
    for _ in 0..100 { harness.bridge.output("offscreen", b"a burst of terminal output\r\n"); }
    harness.bridge.output("visible", b"what you are looking at\r\n");
    let mut activity = 0;
    let mut visible = false;
    loop {
        let message = recv(&mut socket).await;
        if message["type"] == "output" {
            assert_eq!(message["sessionId"], "visible");
            visible = true;
        }
        if message["type"] == "activity" && message["sessionId"] == "offscreen" {
            activity += 1;
            if message["seq"] == 100 { break; }
        }
    }
    assert!(visible, "visible output must arrive before the background freshness tick");
    assert!(activity < 10, "a burst should produce a freshness signal, not 100 frames: {activity}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_step_serves_its_page_to_the_phone_and_its_asks_go_to_the_mod() {
    let harness = Harness::start();
    harness.bridge.state().mod_jobs.set_timing(mod_timing(200, 200, 3_000, 40_000));
    harness.register("s1", None, None);
    let transcript = harness.root.path().join("s1.jsonl");
    let result = json!({ "workoutId": "w1", "title": "Upper Body" });
    let lines = [
        json!({ "type": "assistant", "timestamp": "2026-10-28T10:00:00Z", "message": { "id": "m1", "content": [{ "type": "tool_use", "id": "t1", "name": "mcp__claude_ai_JustGains__show_workout", "input": { "workoutId": "w1" } }] } }),
        json!({ "type": "user", "timestamp": "2026-10-28T10:00:01Z", "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": result.to_string() }] }, "mcpMeta": { "_meta": { "ui": { "resourceUri": "ui://widget/workout.html" } }, "structuredContent": result } }),
    ];
    std::fs::write(&transcript, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    harness.bridge.set_agent_files("s1", crate::AgentFiles { transcript: Some(transcript), ..Default::default() });
    let client = reqwest::Client::new();

    // The conversation names the step's app.
    let conversation: Value = client.get(harness.url("/api/sessions/s1/conversation")).send().await.unwrap().json().await.unwrap();
    assert_eq!(conversation["items"][0]["steps"][0]["app"], json!({ "server": "claude_ai_JustGains", "tool": "show_workout", "resource": "ui://widget/workout.html" }));

    // No page kept, no mod listening: the phone hears why.
    let missing = client.get(harness.url("/api/sessions/s1/apps/t1")).send().await.unwrap();
    assert_eq!(missing.status(), 502);
    assert!(missing.text().await.unwrap().contains("bro mod"));

    // A kept page comes in bro's host page, with the call's input and result, in the phone's theme.
    let kept = bro_core::mcp_apps::page_path(harness.root.path(), "claude_ai_JustGains", "ui://widget/workout.html");
    std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
    std::fs::write(&kept, "<html><body>WORKOUT CARD</body></html>").unwrap();
    let page = client.get(harness.url("/api/sessions/s1/apps/t1?theme=light&bg=%23ffffff&width=390")).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    let html = page.text().await.unwrap();
    assert!(html.contains("WORKOUT CARD"), "the page");
    assert!(html.contains("\"theme\":\"light\"") && html.contains("\"platform\":\"mobile\"") && html.contains("\"width\":390"), "the phone's context");
    assert!(html.contains("\"workoutId\":\"w1\""), "the call's input and result");

    // A call that isn't there.
    assert_eq!(client.get(harness.url("/api/sessions/s1/apps/nope")).send().await.unwrap().status(), 404);

    // What the page asks of its server goes to the session's mod (none listens here), and only what a page may ask.
    let ask = |body: Value| client.post(harness.url("/api/sessions/s1/apps/t1/ask")).json(&body).send();
    let asked: Value = ask(json!({ "method": "tools/call", "params": { "name": "get_workout", "arguments": {} } })).await.unwrap().json().await.unwrap();
    assert_eq!(asked["ok"], false);
    assert!(asked["error"].as_str().unwrap().contains("bro mod"), "{asked}");
    let refused: Value = ask(json!({ "method": "sampling/createMessage" })).await.unwrap().json().await.unwrap();
    assert_eq!(refused["ok"], false);
    assert!(refused["error"].as_str().unwrap().contains("sampling/createMessage"), "{refused}");
}

// ----- the phone's gallery (media.rs) ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gallery_lists_a_sessions_media_newest_first_and_media_tied_to_the_work_opens() {
    let harness = Harness::start();
    let (_project, cwd) = file_workspace(&harness);
    harness.register("other", None, None);
    let elsewhere = harness.root.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let file = |dir: &std::path::Path, name: &str| {
        let path = dir.join(name);
        std::fs::write(&path, name).unwrap();
        path
    };
    let chart = file(&elsewhere, "chart.png");
    file(&cwd, "clip.mp4");
    let shot = file(&elsewhere, "shot.jpg");
    let pasted = file(&elsewhere, "pasted one.png");
    let voice = file(&elsewhere, "voice.mp3");
    let deleted = file(&elsewhere, "deleted.png");
    let secret = file(&elsewhere, "secret.txt");
    let transcript = harness.root.path().join("files.jsonl");
    let lines = [
        json!({ "type": "user", "timestamp": "2026-10-02T00:00:04Z", "message": { "role": "user", "content": format!("what about this \"{}\"", pasted.display()) } }),
        json!({ "type": "assistant", "timestamp": "2026-10-02T00:00:05Z", "message": { "id": "m1", "role": "assistant", "content": [{ "type": "text", "text": format!("Here's the line: `{}`", voice.display()) }] } }),
    ];
    std::fs::write(&transcript, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    let at = |s: u32| Some(format!("2026-10-02T00:00:0{s}Z"));
    let touched = |path: PathBuf, kind, s| crate::TouchedFile { path, kind, at: at(s), line: None };
    harness.bridge.set_agent_files("files", crate::AgentFiles {
        transcript: Some(transcript),
        sent: vec![
            crate::SentFile { path: chart.clone(), caption: Some("the chart".into()), at: at(3), mime_type: Some("image/png".into()), is_image: true },
            crate::SentFile { path: elsewhere.join("gone.png"), caption: None, at: at(6), mime_type: None, is_image: true },
        ],
        touched: vec![
            touched(PathBuf::from("clip.mp4"), crate::TouchKind::Write, 2),
            touched(shot.clone(), crate::TouchKind::Read, 1),
            touched(deleted, crate::TouchKind::Delete, 7),
            touched(secret.clone(), crate::TouchKind::Edit, 8),
            touched(chart.clone(), crate::TouchKind::Read, 0),
        ],
    });
    let client = reqwest::Client::new();

    // the phone attaches a picture, then an agent opens a sound that lives outside the workspace
    let attached: Value = client
        .post(harness.url("/api/sessions/files/attachments"))
        .header("content-type", "image/png")
        .body(b"png".to_vec())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let attached = PathBuf::from(attached["path"].as_str().unwrap());
    tokio::time::sleep(Duration::from_millis(5)).await;
    let take = file(&elsewhere, "take.wav");
    let open = client.post(harness.url("/api/sessions/files/open")).json(&json!({ "path": take.to_string_lossy() })).send().await.unwrap();
    assert_eq!(open.status(), 200, "media opens from anywhere");

    let body: Value = client.get(harness.url("/api/media?session=files")).send().await.unwrap().json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    let field = |key: &str| items.iter().map(|i| i[key].as_str().unwrap_or_default().to_owned()).collect::<Vec<_>>();
    let attached_name = attached.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(field("name"), ["take.wav", attached_name.as_str(), "voice.mp3", "pasted one.png", "chart.png", "clip.mp4", "shot.jpg"]);
    assert_eq!(field("source"), ["opened", "attached", "sent", "pasted", "sent", "made", "viewed"], "a file sent and read shows as sent");
    assert_eq!(field("kind"), ["audio", "image", "audio", "image", "image", "video", "image"]);
    let chart_item = &items[4];
    assert_eq!((chart_item["caption"].as_str(), chart_item["mimeType"].as_str()), (Some("the chart"), Some("image/png")));
    assert_eq!((chart_item["at"].as_str(), chart_item["sessionId"].as_str()), (Some("2026-10-02T00:00:03.000Z"), Some("files")));
    assert_eq!(chart_item["bytes"], 9);

    // every session's: the other one has none
    let all: Value = client.get(harness.url("/api/media")).send().await.unwrap().json().await.unwrap();
    assert_eq!(all["items"].as_array().unwrap().len(), items.len());
    assert_eq!(client.get(harness.url("/api/media?session=ghost")).send().await.unwrap().status(), 404);

    // what's listed streams and previews from wherever it is; text outside the workspace stays put
    let encoded = |p: &std::path::Path| url::form_urlencoded::byte_serialize(p.to_string_lossy().as_bytes()).collect::<String>();
    let raw = client.get(harness.url(&format!("/api/sessions/files/raw?path={}", encoded(&chart)))).send().await.unwrap();
    assert_eq!(raw.status(), 200);
    assert_eq!(raw.headers()["content-type"], "image/png");
    let preview: Value = client.get(harness.url(&format!("/api/sessions/files/file?path={}", encoded(&shot)))).send().await.unwrap().json().await.unwrap();
    assert_eq!((preview["kind"].as_str(), preview["relativePath"].as_str()), (Some("image"), Some("")));
    assert!(preview["url"].as_str().unwrap().starts_with("/api/sessions/files/raw?path="));
    let refused = client.get(harness.url(&format!("/api/sessions/files/raw?path={}", encoded(&secret)))).send().await.unwrap();
    assert_eq!(refused.status(), 403);

    // media is kept to what's tied to the work: what the agents sent, made or looked at, what a conversation
    // names, what was attached or opened, and anything in a folder bro knows
    let raw_status = |p: PathBuf| {
        let (client, url) = (client.clone(), harness.url(&format!("/api/sessions/files/raw?path={}", encoded(&p))));
        async move { client.get(url).send().await.unwrap().status().as_u16() }
    };
    for tied in [&voice, &pasted, &attached, &take] {
        assert_eq!(raw_status(tied.clone()).await, 200, "{}", tied.display());
    }
    let stray = file(&elsewhere, "stray.png");
    let protected = client.get(harness.url(&format!("/api/sessions/files/raw?path={}", encoded(&stray)))).send().await.unwrap();
    assert_eq!(protected.status(), 403);
    assert!(protected.text().await.unwrap().contains("Protect phone media"), "says how to open it");
    let wave = client.get(harness.url(&format!("/api/sessions/files/waveform?path={}", encoded(&voice)))).send().await.unwrap();
    assert_ne!(wave.status(), 403, "a sound a reply names has its waveform");
    let known = harness.root.path().join("known");
    std::fs::create_dir_all(&known).unwrap();
    let in_known = file(&known, "logo.webp");
    assert_eq!(raw_status(in_known.clone()).await, 403);
    harness.bridge.set_known_folders(vec![known.clone()]);
    assert_eq!(raw_status(in_known).await, 200, "a folder bro knows you work in");
    // bro can let media open from anywhere; text still can't
    harness.bridge.set_media_anywhere(true);
    assert_eq!(raw_status(stray.clone()).await, 200);
    assert_eq!(raw_status(secret.clone()).await, 403);
    harness.bridge.set_media_anywhere(false);
    assert_eq!(raw_status(stray).await, 403);
    let _ = std::fs::remove_file(attached);
}
