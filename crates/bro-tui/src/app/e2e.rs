//! End-to-end: a Just Terminal–style client talks to bro's real bridge over `/ws` while a real bro App runs
//! real PTY sessions (shells and, if installed, Claude Code). Covers hello/profiles, subscribe → snapshot,
//! input → output, remote create, remote + local resize, and kill.
//!
//! Spawns processes, binds loopback ports and starts Claude Code with your login, so it's opt-in:
//! `cargo test -p bro-tui e2e -- --ignored --nocapture --test-threads=1`

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

/// The orchestrator for real: the bridge starts off, alt+O opens the launcher on the orchestrator tab, Enter
/// turns the bridge on and starts Claude Code in a PTY with the orchestrator's prompt and bro's tools. Told what
/// to do in its own pane, it types a command into another session ("Toast", a shell) through those tools.
/// `cargo test -p bro-tui e2e_orchestrator -- --ignored --nocapture`
#[test]
#[ignore]
fn e2e_orchestrator_drives_another_session() {
    if bro_core::launch::which("claude").is_none() {
        println!("claude isn't installed: skipped");
        return;
    }
    crate::testkit::isolate();
    let dir = crate::util::bro_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("v2.toml"), "[bridge]\nenabled = false\nport = 0\nbind = \"127.0.0.1\"\n[proxy]\nport = 0\n").unwrap();
    let (settings, _) = load_settings();
    assert!(!settings.bridge.enabled);
    let (tx, rx) = channel();
    let svc = Services::start(settings, tx.clone());
    crate::orchestrator_state::clear();
    // a real bro: it remembers the orchestrator for next time
    let app = App::new(svc.clone(), tx, Opts { load_recents: true, ..Opts::default() });
    let mut h = Harness { app, rx, term: Terminal::new(TestBackend::new(160, 46)).unwrap(), client: None };

    // a worker: a shell named Toast
    let worker = h.app.open_shell(Some(std::env::temp_dir()), Place::Tab);
    if let Some(t) = h.app.panes.get_mut(&worker).and_then(|p| p.as_term()) {
        t.meta.name = Some("Toast".into());
        t.meta.renamed = true;
    }
    h.pump(1500);
    let worker_sid = h.app.panes.get(&worker).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap();

    // alt+O: the launcher on the orchestrator tab, on Claude Code; Enter starts it
    h.app.key(KeyEvent::new(KeyCode::Char('O'), KeyModifiers::ALT | KeyModifiers::SHIFT));
    let Overlay::Launcher(l) = &h.app.overlay else { panic!("alt+O should open the launcher") };
    assert!(l.orchestrator);
    if let Overlay::Launcher(l) = &mut h.app.overlay
        && l.harness != bro_core::Harness::Claude
    {
        l.harness = bro_core::Harness::Claude;
        l.list_sel = l.smart_sel();
    }
    h.app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(h.until(40, |h| h.app.orchestrator_pane().is_some()), "the orchestrator never opened: {:?}", h.app.toasts.latest().map(|t| t.text.clone()));
    assert!(svc.state().bridge.enabled, "starting the orchestrator turned the bridge on");
    let boss = h.app.orchestrator_pane().unwrap();
    assert!(matches!(&h.app.rows()[0], crate::sidebar::Row::Live { info, .. } if info.pane == boss));
    let bridge = svc.bridge_local_url().expect("bridge url");
    println!("bridge {bridge}, orchestrator up");

    // let Claude Code reach its prompt, then tell it what to do
    h.pump(12_000);
    let shell = crate::util::default_shell(None).0;
    let (cmd, want) = marker_cmd(&shell, "ORCHE2E");
    let ask = format!("Use your bro tools: in the session named Toast, run this exact command with send_input (submit it): {}", cmd.trim_end());
    h.app.focus_pane(boss);
    if let Some(t) = h.app.panes.get_mut(&boss).and_then(|p| p.as_term()) {
        t.send(ask.as_bytes());
    }
    h.pump(800);
    if let Some(t) = h.app.panes.get_mut(&boss).and_then(|p| p.as_term()) {
        t.send(b"\r");
    }
    let read = crate::ctl_cmd::CtlArgs { tool: Some("read_session".into()), arguments: json!({ "sessionId": worker_sid, "lines": 60 }) };
    let ok = h.until(180, |_| crate::ctl_cmd::run(&read, Some(&bridge)).is_ok_and(|s| s.contains(&want)));
    let orch_screen = crate::ctl_cmd::run(&crate::ctl_cmd::CtlArgs { tool: Some("read_session".into()), arguments: json!({ "sessionId": h.app.panes.get(&boss).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap(), "lines": 40 }) }, Some(&bridge));
    assert!(ok, "Toast never ran it. Orchestrator screen:\n{}", orch_screen.unwrap_or_default());
    println!("Toast ran the orchestrator's command");
    let launched_as = h.app.panes.get(&boss).and_then(|p| p.as_term_ref()).map(|t| (t.meta.harness, t.meta.profile.clone())).unwrap();
    h.app.shutdown();
    assert!(crate::orchestrator_state::load().is_some(), "quitting bro keeps the orchestrator for next time");

    // reopen: a new bro brings it back with the same params, before anything else is open
    let (settings, _) = load_settings();
    let (tx, rx) = channel();
    let svc = Services::start(settings, tx.clone());
    let app = App::new(svc.clone(), tx, Opts { load_recents: true, ..Opts::default() });
    let mut h = Harness { app, rx, term: Terminal::new(TestBackend::new(160, 46)).unwrap(), client: None };
    h.app.restore_orchestrator();
    assert!(h.until(40, |h| h.app.orchestrator_pane().is_some()), "the orchestrator didn't come back: {:?}", h.app.toasts.latest().map(|t| t.text.clone()));
    let boss = h.app.orchestrator_pane().unwrap();
    let again = h.app.panes.get(&boss).and_then(|p| p.as_term_ref()).map(|t| (t.meta.harness, t.meta.profile.clone())).unwrap();
    assert_eq!(again, launched_as);
    h.pump(4000);
    assert!(h.app.panes.contains_key(&boss), "it keeps running");
    println!("the orchestrator came back after a reopen");
    // you close it: the next bro starts without it
    h.app.close(boss);
    assert!(crate::orchestrator_state::load().is_none());
    h.app.shutdown();
}

/// Push-to-talk for real, with no orchestrator running: the words start one on the login with the most room,
/// wait for it to reach its prompt, and go in marked as spoken with the session you were looking at. Said while
/// looking at "Toast" (a shell), a command is passed straight on to Toast.
/// `cargo test -p bro-tui e2e_voice -- --ignored --nocapture`
#[test]
#[ignore]
fn e2e_voice_starts_the_orchestrator_and_reaches_the_session_you_look_at() {
    crate::testkit::isolate();
    let dir = crate::util::bro_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("v2.toml"), "[bridge]\nport = 0\nbind = \"127.0.0.1\"\n[proxy]\nport = 0\n").unwrap();
    let (settings, _) = load_settings();
    let (tx, rx) = channel();
    let svc = Services::start(settings, tx.clone());
    let app = App::new(svc.clone(), tx, Opts::default());
    let mut h = Harness { app, rx, term: Terminal::new(TestBackend::new(160, 46)).unwrap(), client: None };
    assert!(h.until(30, |h| h.app.svc.state().profiles.ready().is_some() && h.app.svc.bridge_local_url().is_some()), "profiles / bridge never loaded");
    h.pump(3000); // usage readings
    let pick = h.app.roomiest_login();
    println!("roomiest login: {pick:?}");

    let worker = h.app.open_shell(Some(std::env::temp_dir()), Place::Tab);
    if let Some(t) = h.app.panes.get_mut(&worker).and_then(|p| p.as_term()) {
        t.meta.name = Some("Toast".into());
        t.meta.renamed = true;
    }
    h.pump(1500);
    let worker_sid = h.app.panes.get(&worker).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap();
    let shell = crate::util::default_shell(None).0;
    let (cmd, want) = marker_cmd(&shell, "VOICEE2E");

    svc.orchestrator_send_voice(format!("run {}", cmd.trim_end()), Some(worker_sid.clone()), None);
    assert!(h.until(40, |h| h.app.orchestrator_pane().is_some()), "no orchestrator: {:?}", h.app.toasts.latest().map(|t| t.text.clone()));
    let boss = h.app.orchestrator_pane().unwrap();
    let t = h.app.panes.get(&boss).and_then(|p| p.as_term_ref()).unwrap();
    println!("orchestrator on {:?} {:?}", t.meta.harness, t.meta.profile);
    assert_eq!(t.meta.profile, pick, "started on the roomiest login");
    assert_eq!(h.app.focused(), Some(worker), "you stay where you were");

    let bridge = svc.bridge_local_url().unwrap();
    let read = crate::ctl_cmd::CtlArgs { tool: Some("read_session".into()), arguments: json!({ "sessionId": worker_sid, "lines": 60 }) };
    let ok = h.until(240, |_| crate::ctl_cmd::run(&read, Some(&bridge)).is_ok_and(|s| s.contains(&want)));
    let boss_sid = h.app.panes.get(&boss).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap();
    let screen = crate::ctl_cmd::run(&crate::ctl_cmd::CtlArgs { tool: Some("read_session".into()), arguments: json!({ "sessionId": boss_sid, "lines": 40 }) }, Some(&bridge)).unwrap_or_default();
    assert!(ok, "Toast never ran it. Orchestrator screen:\n{screen}");
    println!("the spoken command reached Toast through the orchestrator");
    h.app.shutdown();
}

/// Everything Hugh and the watchdog did, done by a real orchestrator session (Claude Code in a PTY):
/// a phone/web panel message is answered in the panel's transcript; a trigger scheduled through bro's tools is
/// fired by bro's own timer and acted on; the orchestrator's context is cleared after its turn, keeping only its
/// handoff notes; and a watchdog wake-up is reviewed by the orchestrator and logged as the watchdog's run.
/// `cargo test -p bro-tui e2e_inbox -- --ignored --nocapture`
#[test]
#[ignore]
fn e2e_inbox_panels_schedule_fresh_start_and_watchdog_go_through_the_orchestrator() {
    if bro_core::launch::which("claude").is_none() {
        println!("claude isn't installed: skipped");
        return;
    }
    crate::testkit::isolate();
    let dir = crate::util::bro_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("v2.toml"), "[bridge]\nport = 0\nbind = \"127.0.0.1\"\n[proxy]\nport = 0\n").unwrap();
    let (settings, _) = load_settings();
    let (tx, rx) = channel();
    let svc = Services::start(settings, tx.clone());
    let app = App::new(svc.clone(), tx, Opts::default());
    let mut h = Harness { app, rx, term: Terminal::new(TestBackend::new(160, 46)).unwrap(), client: None };
    assert!(h.until(30, |h| h.app.svc.state().profiles.ready().is_some() && h.app.svc.bridge_local_url().is_some()), "profiles / bridge never loaded");
    let bridge = svc.bridge_local_url().unwrap();
    let worker = h.app.open_shell(Some(std::env::temp_dir()), Place::Tab);
    if let Some(t) = h.app.panes.get_mut(&worker).and_then(|p| p.as_term()) {
        t.meta.name = Some("Toast".into());
        t.meta.renamed = true;
    }
    h.pump(1500);
    let worker_sid = h.app.panes.get(&worker).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()).unwrap();
    // a Claude orchestrator
    let spec = bro_core::launch::LaunchSpec {
        harness: bro_core::Harness::Claude,
        profile_id: None,
        provider_id: None,
        model: None,
        cwd: std::env::temp_dir(),
        resume: None,
        permission: bro_core::launch::Permission::Skip,
        browser: bro_core::browser::BrowserMode::Off,
        extra_args: vec![],
        orchestrator: true,
    };
    h.app.launch_spec(spec, Place::Tab);
    assert!(h.until(40, |h| h.app.orchestrator_pane().is_some()), "no orchestrator");
    h.pump(10_000);
    let http = |method: &str, path: &str, body: Option<serde_json::Value>| -> serde_json::Value {
        let url = format!("{bridge}{path}");
        let r = match (method, body) {
            ("GET", _) => bro_core::http::get_json(&url, &[], Duration::from_secs(10)).map(|r| r.body),
            (_, Some(b)) => bro_core::http::post_json(&url, &b, Duration::from_secs(10)).map(|r| r.body),
            _ => unreachable!(),
        };
        r.unwrap_or_default()
    };
    let panel_answer = |h: &mut Harness, ask: &str| -> String {
        http("POST", "/api/orchestrator/messages", Some(json!({ "text": ask })));
        let mut answer = String::new();
        h.until(150, |_| {
            let status = http("GET", "/api/orchestrator", None);
            let items = status["transcript"].as_array().or(status["items"].as_array()).cloned().unwrap_or_default();
            let last = items.iter().rev().find(|i| i["role"] == "assistant" || i["role"] == "error").cloned().unwrap_or_default();
            if last["status"] == "done" || last["status"] == "error" {
                answer = last["text"].as_str().unwrap_or_default().to_string();
                return true;
            }
            false
        });
        answer
    };

    // 1. a panel message, answered in the panel
    let a1 = panel_answer(&mut h, "Remember this secret code for later: MANGO. Reply with exactly: PANEL-OK");
    println!("panel → {a1}");
    assert!(a1.contains("PANEL-OK"), "panel answer: {a1:?}");

    // 2. a trigger, scheduled through bro's tools and fired by bro's own timer
    let shell = crate::util::default_shell(None).0;
    let (cmd, want) = marker_cmd(&shell, "SCHEDE2E");
    let add = crate::ctl_cmd::CtlArgs { tool: Some("schedule_add".into()), arguments: json!({ "name": "e2e", "in": "3s", "prompt": format!("In the session named Toast, run this exact command with send_input: {}", cmd.trim_end()) }) };
    println!("{}", crate::ctl_cmd::run(&add, Some(&bridge)).unwrap());
    let read = crate::ctl_cmd::CtlArgs { tool: Some("read_session".into()), arguments: json!({ "sessionId": worker_sid, "lines": 60 }) };
    assert!(h.until(150, |_| crate::ctl_cmd::run(&read, Some(&bridge)).is_ok_and(|s| s.contains(&want))), "the scheduled prompt never ran in Toast");
    println!("scheduled trigger fired and ran in Toast");
    h.pump(8000);

    // 3. a fresh start: the secret is gone, the handoff stays
    let clear = crate::ctl_cmd::CtlArgs { tool: Some("clear_own_context".into()), arguments: json!({ "handoff": "The handoff word is PINEAPPLE." }) };
    crate::ctl_cmd::run(&clear, Some(&bridge)).unwrap();
    assert!(h.until(30, |h| h.app.toasts.items.iter().any(|t| t.text.contains("fresh conversation"))), "never cleared");
    h.pump(12_000);
    let a3 = panel_answer(&mut h, "Answer in one line: what is the secret code I gave you earlier (say UNKNOWN if you don't know), and what is the handoff word?");
    println!("after the fresh start → {a3}");
    assert!(a3.contains("PINEAPPLE"), "handoff kept: {a3:?}");
    assert!(!a3.contains("MANGO"), "context cleared: {a3:?}");

    // 4. a watchdog wake-up, reviewed by the orchestrator
    svc.watchdog_start(&worker_sid, bro_bridge::WatchdogPlan { milestones: vec![bro_bridge::Milestone::new("Toast has printed the SCHEDE2E marker")], ..Default::default() }).unwrap();
    h.pump(500);
    h.app.wake_watchdog(worker, "the user asked for a review now".into(), true);
    let done = h.until(300, |h| h.app.svc.watchdog(&worker_sid).is_some_and(|w| w.running.is_none() && !w.runs.is_empty()));
    let w = svc.watchdog(&worker_sid).unwrap();
    println!("watchdog runs: {:?}", w.runs.iter().map(|r| (r.reviewer.clone(), r.ok, r.summary.clone())).collect::<Vec<_>>());
    assert!(done, "the review never finished");
    assert!(w.runs[0].ok && w.runs[0].reviewer == "the orchestrator", "{:?}", w.runs[0]);
    h.app.shutdown();
}
