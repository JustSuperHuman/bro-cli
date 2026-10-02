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
    let created: Value = client
        .post(harness.url("/api/sessions"))
        .json(&json!({ "title": "From phone", "profileId": "claude", "cwd": "C:\\work" }))
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
    assert_eq!(created["cwd"], "C:\\work");
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
