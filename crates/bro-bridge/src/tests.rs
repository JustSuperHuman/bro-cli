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
    Focus(String),
    OpenFile(String, PathBuf, Option<u32>, Option<u32>),
    OpenInEditor(String, PathBuf, Option<u32>, Option<u32>),
    Orchestrator(crate::OrchestratorCommand),
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
                BridgeCommand::Input { id, data } => seen
                    .lock()
                    .push(Seen::Input(id, String::from_utf8_lossy(&data).into_owned())),
                BridgeCommand::Resize { id, cols, rows } => {
                    seen.lock().push(Seen::Resize(id, cols, rows))
                }
                BridgeCommand::Kill { id } => seen.lock().push(Seen::Kill(id)),
                BridgeCommand::Rename { id, title } => seen.lock().push(Seen::Rename(id, title)),
                BridgeCommand::Focus { id } => seen.lock().push(Seen::Focus(id)),
                BridgeCommand::Orchestrator(c) => seen.lock().push(Seen::Orchestrator(c)),
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
    assert!(started.elapsed() >= Duration::from_millis(150));
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
