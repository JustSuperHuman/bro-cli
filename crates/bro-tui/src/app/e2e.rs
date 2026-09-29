//! End-to-end: a Just Terminal–style client talks to bro's real bridge over `/ws` while a real bro App runs
//! real PTY sessions (shells and, if installed, Claude Code). Covers hello/profiles, subscribe → snapshot,
//! input → output, remote create, remote + local resize, and kill.
//!
//! Spawns processes, binds loopback ports and starts Claude Code with your login, so it's opt-in:
//! `cargo test -p bro-tui e2e -- --ignored --nocapture --test-threads=1`

use super::*;
use crate::services::{Avail, Services, load_settings};
use futures_util::{SinkExt, StreamExt};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::{Value, json};
use std::sync::mpsc::{Receiver as StdRx, channel};
use tokio_tungstenite::tungstenite::Message;

/// The phone: a websocket client on its own tokio runtime, bridged to sync channels.
struct Client {
    out: tokio::sync::mpsc::UnboundedSender<Value>,
    inbox: StdRx<Value>,
    /// every message received so far
    seen: Vec<Value>,
}

impl Client {
    fn connect(url: String) -> Client {
        let (out, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let (in_tx, inbox) = channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
            rt.block_on(async move {
                let (ws, _) = tokio_tungstenite::connect_async(url).await.expect("ws connect");
                let (mut tx, mut rx) = ws.split();
                loop {
                    tokio::select! {
                        m = out_rx.recv() => match m {
                            Some(v) => { if tx.send(Message::Text(v.to_string().into())).await.is_err() { break; } }
                            None => break,
                        },
                        m = rx.next() => match m {
                            Some(Ok(Message::Text(t))) => { if let Ok(v) = serde_json::from_str::<Value>(&t) && in_tx.send(v).is_err() { break; } }
                            Some(Ok(_)) => {}
                            _ => break,
                        },
                    }
                }
            });
        });
        Client { out, inbox, seen: vec![] }
    }
    fn send(&self, v: Value) {
        let _ = self.out.send(v);
    }
    fn drain(&mut self) {
        while let Ok(v) = self.inbox.try_recv() {
            self.seen.push(v);
        }
    }
    /// All `output` text received for a session, ANSI stripped.
    fn output(&self, sid: &str) -> String {
        strip(&self.seen.iter().filter(|v| v["type"] == "output" && v["sessionId"] == sid).filter_map(|v| v["data"].as_str()).collect::<String>())
    }
}

/// Remove ANSI escape sequences (CSI, OSC, two-byte ESC) for text matching.
fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\x07' || (c == '\x1b' && it.peek() == Some(&'\\')) {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

struct Harness {
    app: App,
    rx: StdRx<Event>,
    term: Terminal<TestBackend>,
    client: Option<Client>,
}

impl Harness {
    /// Run the app's loop (events → state → draw) for a while, like `App::run` without a real terminal.
    fn pump(&mut self, ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            if let Ok(e) = self.rx.recv_timeout(Duration::from_millis(15)) {
                self.app.handle(e);
                while let Ok(e) = self.rx.try_recv() {
                    self.app.handle(e);
                }
            }
            self.app.handle(Event::Tick);
            self.app.after_events();
            let _ = self.term.draw(|f| self.app.draw(f));
            if let Some(c) = &mut self.client {
                c.drain();
            }
        }
    }
    /// Pump until `cond` holds (or time out). Returns whether it held.
    fn until(&mut self, secs: u64, mut cond: impl FnMut(&mut Harness) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            self.pump(60);
            if cond(self) {
                return true;
            }
        }
        false
    }
    fn client(&mut self) -> &mut Client {
        self.client.as_mut().expect("client")
    }
    fn pane_of(&self, sid: &str) -> Option<PaneId> {
        self.app.panes.iter().find(|(_, p)| p.as_term_ref().is_some_and(|t| t.meta.sid == sid)).map(|(id, _)| *id)
    }
    fn size_of(&self, sid: &str) -> Option<(u16, u16)> {
        self.pane_of(sid).and_then(|id| self.app.panes.get(&id)).and_then(|p| p.as_term_ref()).map(|t| t.size())
    }
    fn resize_screen(&mut self, w: u16, h: u16) {
        self.term = Terminal::new(TestBackend::new(w, h)).expect("backend");
    }
    /// Session ids the bridge has announced (hello + session/sessions events).
    fn bridge_sessions(&mut self) -> Vec<Value> {
        let mut out: Vec<Value> = vec![];
        for v in &self.client().seen {
            let list: Vec<Value> = match v["type"].as_str() {
                Some("hello") | Some("sessions") => v["sessions"].as_array().cloned().unwrap_or_default(),
                Some("session") => vec![v["session"].clone()],
                _ => vec![],
            };
            for s in list {
                out.retain(|o| o["id"] != s["id"]);
                out.push(s);
            }
        }
        out
    }
}

fn marker_cmd(shell: &str, tag: &str) -> (String, String) {
    // the echoed command line doesn't contain the result, so seeing it proves the shell ran it
    if shell.contains("pwsh") || shell.contains("powershell") {
        (format!("Write-Output (\"{tag}-\" + (6*7))\r"), format!("{tag}-42"))
    } else if shell.contains("cmd") {
        (format!("set /a 6*7 >nul & echo {tag}-%=exitcode%\r"), format!("{tag}-"))
    } else {
        (format!("echo {tag}-$((6*7))\r"), format!("{tag}-42"))
    }
}

#[test]
#[ignore]
fn e2e_bridge_client_and_resizing() {
    crate::testkit::isolate();
    let dir = crate::util::bro_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("v2.toml"), "[bridge]\nport = 0\nbind = \"127.0.0.1\"\n[proxy]\nport = 0\n").unwrap();
    let (settings, _) = load_settings();
    let (tx, rx) = channel();
    let svc = Services::start(settings, tx.clone());
    let app = App::new(svc.clone(), tx, Opts::default());
    let mut h = Harness { app, rx, term: Terminal::new(TestBackend::new(140, 40)).unwrap(), client: None };

    // --- the bridge comes up
    assert!(h.until(20, |h| matches!(h.app.svc.state().bridge.status, Avail::Ready(ref s) if s.running)), "bridge never started: {}", svc.state().bridge.status.why());
    let (port, token) = match &svc.state().bridge.status {
        Avail::Ready(s) => (s.port, s.token.clone()),
        _ => unreachable!(),
    };
    println!("bridge on :{port}");

    // --- a local shell in bro, on screen
    let local = h.app.open_shell(Some(std::env::temp_dir()), Place::Tab);
    h.pump(1500);
    let local_sid = h.app.panes.get(&local).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap();
    let shell_prog = crate::util::default_shell(None).0;

    // --- the phone connects (token like a LAN client would present)
    h.client = Some(Client::connect(format!("ws://127.0.0.1:{port}/ws?token={token}")));
    assert!(h.until(10, |h| h.client().seen.iter().any(|v| v["type"] == "hello")), "no hello");
    let hello = h.client().seen.iter().find(|v| v["type"] == "hello").cloned().unwrap();
    let profiles: Vec<String> = hello["profiles"].as_array().unwrap().iter().filter_map(|p| p["id"].as_str().map(String::from)).collect();
    println!("hello: {} session(s), profiles {profiles:?}", hello["sessions"].as_array().map(|a| a.len()).unwrap_or(0));
    assert!(hello["sessions"].as_array().unwrap().iter().any(|s| s["id"] == local_sid.as_str()), "bro's session is mirrored in hello");
    assert!(profiles.contains(&"shell".to_string()), "bro advertises its launch profiles");

    // --- subscribe → snapshot, input → output
    h.client().send(json!({ "type": "subscribe", "sessionId": local_sid }));
    assert!(h.until(10, |h| h.client().seen.iter().any(|v| v["type"] == "snapshot" && v["sessionId"] == local_sid.as_str())), "no snapshot");
    let (cmd, want) = marker_cmd(&shell_prog, "bro-local");
    h.client().send(json!({ "type": "input", "sessionId": local_sid, "data": cmd }));
    let sid = local_sid.clone();
    assert!(h.until(15, |h| h.client().output(&sid).contains(&want)), "phone input didn't run in bro's shell; output: {}", h.client().output(&local_sid));
    println!("input → output ok");

    // --- resize: the local view wins while visible; the phone's size applies when hidden
    let local_size = h.size_of(&local_sid).unwrap();
    h.client().send(json!({ "type": "resize", "sessionId": local_sid, "cols": 60, "rows": 20 }));
    h.pump(600);
    assert_eq!(h.size_of(&local_sid), Some(local_size), "visible pane keeps the local size");
    h.app.run_act(crate::keymap::Act::Usage); // another tab in front
    h.pump(300);
    h.client().send(json!({ "type": "resize", "sessionId": local_sid, "cols": 60, "rows": 20 }));
    let sid = local_sid.clone();
    assert!(h.until(5, |h| h.size_of(&sid) == Some((20, 60))), "hidden pane follows the phone: {:?}", h.size_of(&local_sid));
    println!("remote resize while hidden ok");
    h.app.focus_pane(local);
    h.resize_screen(120, 36);
    h.pump(500);
    let back = h.size_of(&local_sid).unwrap();
    assert_ne!(back, (20, 60), "back on screen: local size again");
    let sid = local_sid.clone();
    assert!(h.until(5, |h| h.bridge_sessions().iter().any(|s| s["id"] == sid.as_str() && s["cols"] == back.1 && s["rows"] == back.0)), "bridge told about the local size {back:?}");
    println!("local resize mirrored to the bridge: {back:?}");

    // --- the phone creates a shell
    let before: Vec<String> = h.bridge_sessions().iter().filter_map(|s| s["id"].as_str().map(String::from)).collect();
    h.client().send(json!({ "type": "create", "profileId": "shell", "title": "from-phone", "cwd": std::env::temp_dir() }));
    let mut new_sid = String::new();
    assert!(
        h.until(20, |h| {
            let found = h.bridge_sessions().into_iter().find(|s| s["id"].as_str().is_some_and(|id| !before.contains(&id.to_string())));
            if let Some(s) = found {
                new_sid = s["id"].as_str().unwrap_or_default().to_string();
            }
            !new_sid.is_empty()
        }),
        "remote create never showed up"
    );
    assert!(h.pane_of(&new_sid).is_some(), "bro opened a pane for it");
    h.client().send(json!({ "type": "subscribe", "sessionId": new_sid, "slot": "second" }));
    let (cmd, want) = marker_cmd(&shell_prog, "bro-remote");
    h.pump(1500);
    h.client().send(json!({ "type": "input", "sessionId": new_sid, "data": cmd }));
    let sid = new_sid.clone();
    assert!(h.until(15, |h| h.client().output(&sid).contains(&want)), "remote-created shell didn't run input: {}", h.client().output(&new_sid));
    println!("remote create + input ok ({new_sid})");
    h.client().send(json!({ "type": "kill", "sessionId": new_sid }));
    let sid = new_sid.clone();
    assert!(h.until(10, |h| h.pane_of(&sid).is_none()), "remote kill closed the pane");
    println!("remote kill ok");

    // --- Claude Code, if it's installed: remote create, render, resize both ways, kill
    if crate::util::which("claude").is_some() && profiles.iter().any(|p| p == "claude" || p.starts_with("claude:")) {
        let proj = std::env::temp_dir().join("bro-e2e-claude");
        let _ = std::fs::create_dir_all(&proj);
        let before: Vec<String> = h.bridge_sessions().iter().filter_map(|s| s["id"].as_str().map(String::from)).collect();
        h.client().send(json!({ "type": "create", "profileId": "claude", "title": "claude-from-phone", "cwd": proj }));
        let mut csid = String::new();
        let ok = h.until(40, |h| {
            if let Some(s) = h.bridge_sessions().into_iter().find(|s| s["id"].as_str().is_some_and(|id| !before.contains(&id.to_string()))) {
                csid = s["id"].as_str().unwrap_or_default().to_string();
            }
            !csid.is_empty()
        });
        let errors: Vec<String> = h.client().seen.iter().filter(|v| v["type"] == "error").map(|v| v.to_string()).collect();
        let toasts: Vec<String> = h.app.toasts.items.iter().map(|t| t.text.clone()).collect();
        assert!(ok, "claude session never appeared; errors {errors:?} toasts {toasts:?}");
        h.client().send(json!({ "type": "subscribe", "sessionId": csid, "slot": "claude" }));
        let looks_like_claude = |t: &str| {
            let t = t.to_lowercase();
            ["claude code", "trust", "welcome", "? for shortcuts", "try \""].iter().any(|m| t.contains(m))
        };
        let sid = csid.clone();
        assert!(h.until(45, |h| looks_like_claude(&h.client().output(&sid))), "claude UI never streamed to the phone: {:?}", h.client().output(&csid).chars().take(600).collect::<String>());
        println!("claude streamed to the phone ({csid}), size {:?}", h.size_of(&csid));
        // hidden in bro (created in the background): the phone's size applies
        for (cols, rows) in [(80u16, 24u16), (50, 30), (100, 20)] {
            let n0 = h.client().output(&csid).len();
            h.client().send(json!({ "type": "resize", "sessionId": csid, "cols": cols, "rows": rows }));
            let sid = csid.clone();
            assert!(h.until(8, |h| h.size_of(&sid) == Some((rows, cols))), "claude pty didn't follow the phone to {cols}x{rows}: {:?}", h.size_of(&csid));
            let sid = csid.clone();
            let redrew = h.until(10, |h| h.client().output(&sid).len() > n0);
            assert!(h.pane_of(&csid).is_some(), "claude died after resizing to {cols}x{rows}");
            println!("  phone resize {cols}x{rows}: pty ok, redrew={redrew}");
        }
        // now on screen in bro: local resizes win and stream to the phone
        let pane = h.pane_of(&csid).unwrap();
        h.app.focus_pane(pane);
        for (w, hh) in [(150u16, 45u16), (100, 30), (70, 24)] {
            h.resize_screen(w, hh);
            let n0 = h.client().output(&csid).len();
            h.pump(1200);
            let size = h.size_of(&csid).unwrap();
            let sid = csid.clone();
            let mirrored = h.until(6, |h| h.bridge_sessions().iter().any(|s| s["id"] == sid.as_str() && s["cols"] == size.1 && s["rows"] == size.0));
            let grew = h.client().output(&csid).len() > n0;
            assert!(h.pane_of(&csid).is_some(), "claude died after a local resize to {w}x{hh}");
            assert!(mirrored, "bridge not told about local size {size:?}");
            println!("  local screen {w}x{hh}: pty {size:?}, mirrored to bridge, redrew={grew}");
        }
        // the phone re-subscribes and gets a snapshot of the current screen
        h.client().send(json!({ "type": "subscribe", "sessionId": csid, "slot": "claude" }));
        let sid = csid.clone();
        assert!(h.until(10, |h| h.client().seen.iter().rev().any(|v| v["type"] == "snapshot" && v["sessionId"] == sid.as_str() && looks_like_claude(&strip(v["screen"].as_str().unwrap_or_default())))), "snapshot after resizes lacks claude's screen");
        let screen = crate::testkit::dump(h.term.backend().buffer());
        println!("bro's own render of the claude pane (last frame, first lines):\n{}", screen.lines().take(14).collect::<Vec<_>>().join("\n"));
        h.client().send(json!({ "type": "kill", "sessionId": csid }));
        let sid = csid.clone();
        assert!(h.until(15, |h| h.pane_of(&sid).is_none()), "remote kill of claude");
        println!("claude: create, stream, resize (remote + local), snapshot, kill ok");
    } else {
        println!("claude not installed or no claude profile: skipped the Claude part (which = {:?})", crate::util::which("claude"));
    }
    h.app.shutdown();
}
