//! A real server on a temporary folder, real programs on real pseudo-terminals.

use crate::client::{self, Client, ClientEvent, ConnectError, Remote};
use crate::{Role, ServeOpts, SpawnRequest, record, serve, store};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

struct Server {
    dir: tempfile::TempDir,
    done: mpsc::Receiver<()>,
}

impl Server {
    fn start() -> Server {
        Server::with(Duration::from_secs(120), |_| false)
    }

    fn with(idle: Duration, resident: fn(&Path) -> bool) -> Server {
        Server::in_dir(tempfile::tempdir().unwrap(), idle, resident)
    }

    fn in_dir(dir: tempfile::TempDir, idle: Duration, resident: fn(&Path) -> bool) -> Server {
        let opts = ServeOpts { dir: dir.path().to_path_buf(), version: "test".into(), idle, resident };
        let (tx, done) = mpsc::channel();
        std::thread::spawn(move || {
            serve(opts).unwrap();
            let _ = tx.send(());
        });
        let until = Instant::now() + Duration::from_secs(5);
        while record::read(dir.path()).is_none() {
            assert!(Instant::now() < until, "the server never wrote its record");
            std::thread::sleep(Duration::from_millis(10));
        }
        Server { dir, done }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn connect(&self) -> (Client, mpsc::Receiver<ClientEvent>) {
        self.connect_as(Role::Window)
    }

    fn connect_as(&self, role: Role) -> (Client, mpsc::Receiver<ClientEvent>) {
        let (tx, rx) = mpsc::channel();
        let c = Client::connect(self.path(), role, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        (c, rx)
    }
}

/// A shell that stays at its prompt.
fn shell() -> (String, Vec<String>) {
    if cfg!(windows) { ("cmd.exe".into(), vec![]) } else { ("/bin/sh".into(), vec![]) }
}

fn request(id: &str, (program, args): (String, Vec<String>)) -> SpawnRequest {
    SpawnRequest {
        id: id.into(),
        program,
        args,
        cwd: std::env::temp_dir(),
        env: std::env::vars().collect(),
        rows: 24,
        cols: 100,
        blob: serde_json::json!({ "name": id, "secret": "in memory only" }),
        keep: serde_json::json!({ "name": id }),
    }
}

/// Read output on a thread; `until` waits for `needle` in everything read so far.
struct Output {
    rx: mpsc::Receiver<Vec<u8>>,
    seen: Vec<u8>,
    sizes: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl Output {
    fn of(remote: &mut Remote) -> Output {
        let sizes = Arc::new(Mutex::new(vec![]));
        let s = sizes.clone();
        let mut reader = remote.take_reader_with(move |r, c| s.lock().unwrap().push((r, c))).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Output { rx, seen: vec![], sizes }
    }

    fn until(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let text = String::from_utf8_lossy(&self.seen).to_string();
            if text.contains(needle) {
                return text;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "never saw {needle:?} in {text:?}");
            if let Ok(chunk) = self.rx.recv_timeout(left) {
                self.seen.extend(chunk);
            }
        }
    }
}

fn type_line(remote: &Remote, line: &str) {
    let mut w = remote.writer();
    w.write_all(line.as_bytes()).unwrap();
    w.write_all(if cfg!(windows) { b"\r" } else { b"\n" }).unwrap();
}

fn wait_exit(remote: &Remote) -> i32 {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(code) = remote.exit_code() {
            return code;
        }
        assert!(Instant::now() < until, "the session never exited");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait(what: &str, ok: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !ok() {
        assert!(Instant::now() < until, "never: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn next(events: &mpsc::Receiver<ClientEvent>, want: impl Fn(&ClientEvent) -> bool) -> ClientEvent {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(e) if want(&e) => return e,
            Ok(_) => {}
            Err(_) => panic!("the event never came"),
        }
    }
}

#[test]
fn a_session_streams_output_takes_input_and_reports_its_exit() {
    let server = Server::start();
    let (client, _) = server.connect();
    let mut remote = client.spawn(request("a", shell())).unwrap();
    assert!(remote.pid().is_some());
    assert!(remote.controlling(), "whoever starts a session controls it");
    let mut out = Output::of(&mut remote);
    type_line(&remote, "echo bro-marker-1");
    out.until("bro-marker-1");
    type_line(&remote, "exit 3");
    assert_eq!(wait_exit(&remote), 3);
    client.shutdown(Duration::from_secs(5));
    server.done.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(record::read(server.path()).is_none(), "the record goes with the server");
}

#[test]
fn a_session_outlives_its_window_and_comes_back_with_its_screen() {
    let server = Server::start();
    {
        let (client, _) = server.connect();
        let mut remote = client.spawn(request("keep", shell())).unwrap();
        let mut out = Output::of(&mut remote);
        type_line(&remote, "echo still-here-42");
        out.until("still-here-42");
        // the window closes without ending it
        remote.forget();
    }
    std::thread::sleep(Duration::from_millis(200));
    let (client, _) = server.connect();
    let info = client.info().sessions.iter().find(|s| s.id == "keep").cloned().expect("the session is still listed");
    assert!(!info.exited);
    assert_eq!(info.blob["name"], "keep", "the window's metadata comes back");
    let mut remote = client.attach(&info);
    let mut out = Output::of(&mut remote);
    out.until("still-here-42");
    assert_eq!(out.sizes.lock().unwrap().first(), Some(&(24, 100)), "the snapshot comes at the size it was cut");
    // and it is live, not a picture of the past
    type_line(&remote, "echo after-reattach");
    out.until("after-reattach");
    remote.close();
    assert!(wait_exit(&remote) != 0 || cfg!(unix), "closing ends it");
    client.shutdown(Duration::from_secs(5));
}

#[test]
fn a_second_window_takes_over_and_the_first_hears_it() {
    let server = Server::start();
    let (first, events) = server.connect();
    // the first has to leave for the second's welcome: it does once it hears
    let leaves = std::thread::spawn(move || {
        next(&events, |e| *e == ClientEvent::TakenOver);
        assert!(!first.connected());
        drop(first);
    });
    let (second, _) = server.connect();
    leaves.join().unwrap();
    assert!(second.connected());
    // a query doesn't take over
    let status = client::status(server.path()).unwrap();
    assert_eq!(status.version, "test");
    assert_eq!(status.peers.primary.map(|p| p.role), Some(Role::Window));
    assert!(second.connected());
    second.shutdown(Duration::from_secs(5));
}

#[test]
fn the_background_bro_stands_in_for_a_window_and_steps_aside_for_one() {
    let server = Server::start();
    let (window, _) = server.connect();
    match Client::connect(server.path(), Role::Background, |_| {}) {
        Err(ConnectError::Busy) => {}
        other => panic!("a window drives it: the background bro waits, got {:?}", other.map(|_| ())),
    }
    drop(window);
    wait("the window left", || client::status(server.path()).unwrap().peers.primary.is_none());
    let (background, events) = server.connect_as(Role::Background);
    assert_eq!(client::status(server.path()).unwrap().peers.primary.map(|p| p.role), Some(Role::Background));
    // it hands over and leaves: the window's welcome waits for that
    let leaves = std::thread::spawn(move || {
        next(&events, |e| *e == ClientEvent::TakenOver);
        std::thread::sleep(Duration::from_millis(300));
        let leaving = Instant::now();
        drop(background);
        leaving
    });
    let (window, _) = server.connect();
    let welcomed = Instant::now();
    let leaving = leaves.join().unwrap();
    assert!(welcomed >= leaving, "the window started only once the background bro let go");
    assert_eq!(window.info().peers.primary.map(|p| p.role), Some(Role::Window));
    window.shutdown(Duration::from_secs(5));
}

#[test]
fn a_viewer_watches_and_types_and_whoever_types_controls_the_size() {
    let server = Server::start();
    let (window, _) = server.connect();
    let (viewer, viewer_events) = server.connect_as(Role::Viewer);
    let mut mine = window.spawn(request("shared", shell())).unwrap();
    let mut out = Output::of(&mut mine);
    let ClientEvent::Added(info) = next(&viewer_events, |e| matches!(e, ClientEvent::Added(_))) else { unreachable!() };
    assert_eq!(info.id, "shared", "a viewer hears about sessions others start");
    let mut theirs = viewer.attach(&info);
    let mut seen = Output::of(&mut theirs);
    type_line(&mine, "echo from-the-window");
    seen.until("from-the-window");
    assert!(mine.controlling() && !theirs.controlling());
    // the viewer would like another size: it takes effect once it types
    theirs.resize(30, 90);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(mine.size(), (24, 100), "watching doesn't resize anyone's session");
    type_line(&theirs, "echo from-the-viewer");
    out.until("from-the-viewer");
    wait("the viewer took control", || theirs.controlling() && !mine.controlling());
    wait("both screens took its size", || mine.size() == (30, 90) && theirs.size() == (30, 90));
    assert!(out.sizes.lock().unwrap().contains(&(30, 90)), "the window's screen resized in line with the output");
    // and back
    mine.resize(24, 100);
    type_line(&mine, "echo mine-again");
    wait("the window took it back", || mine.controlling() && theirs.size() == (24, 100));
    window.shutdown(Duration::from_secs(5));
}

#[test]
fn a_viewer_relays_to_the_primary_and_hears_when_there_is_none() {
    let server = Server::start();
    let (viewer, viewer_events) = server.connect_as(Role::Viewer);
    viewer.relay(serde_json::json!({ "launch": "claude" }));
    assert_eq!(next(&viewer_events, |e| matches!(e, ClientEvent::Unrelayed(_))), ClientEvent::Unrelayed(serde_json::json!({ "launch": "claude" })));
    let (window, window_events) = server.connect();
    next(&viewer_events, |e| matches!(e, ClientEvent::Peers(p) if p.primary.is_some()));
    viewer.relay(serde_json::json!({ "launch": "codex" }));
    assert_eq!(next(&window_events, |e| matches!(e, ClientEvent::Relayed(_))), ClientEvent::Relayed(serde_json::json!({ "launch": "codex" })));
    window.shutdown(Duration::from_secs(5));
}

#[test]
fn renames_reach_the_other_clients() {
    let server = Server::start();
    let (window, _) = server.connect();
    let (viewer, viewer_events) = server.connect_as(Role::Viewer);
    let remote = window.spawn(request("r", shell())).unwrap();
    remote.set_blob(serde_json::json!({ "name": "renamed" }), serde_json::json!({ "name": "renamed" }));
    let ClientEvent::Blob { id, blob } = next(&viewer_events, |e| matches!(e, ClientEvent::Blob { .. })) else { unreachable!() };
    assert_eq!((id.as_str(), blob["name"].as_str()), ("r", Some("renamed")));
    drop(viewer);
    window.shutdown(Duration::from_secs(5));
}

#[test]
fn stopping_the_server_ends_its_sessions_and_leaves_nothing_to_bring_back() {
    let server = Server::start();
    let (client, events) = server.connect();
    let remote = client.spawn(request("s", shell())).unwrap();
    wait("the session is written down", || store::load(server.path()).sessions.iter().any(|k| k.id == "s"));
    let on_disk = std::fs::read_to_string(store::path(server.path())).unwrap();
    assert!(!on_disk.contains("in memory only"), "only what's safe to keep goes to disk: {on_disk}");
    assert!(client::stop(server.path(), Duration::from_secs(5)));
    server.done.recv_timeout(Duration::from_secs(5)).unwrap();
    // asked for: the window hears that, not a connection lost (it doesn't try to bring the server back)
    next(&events, |e| *e == ClientEvent::Stopped);
    assert!(!events.try_iter().any(|e| e == ClientEvent::Lost));
    // killed by the server on its way out (its code arrives first) or gone with the connection (-1)
    assert_ne!(wait_exit(&remote), 0, "the window hears its session went with the server");
    assert!(!client::stop(server.path(), Duration::from_secs(1)), "nothing left to stop");
    assert!(store::load(server.path()).sessions.is_empty(), "asked to stop: nothing was lost");
}

#[test]
fn sessions_a_server_left_running_come_back_as_lost_until_a_window_takes_them() {
    let dir = tempfile::tempdir().unwrap();
    // what a server that crashed (or went down with the computer) left behind
    let kept = store::Kept { id: "gone".into(), keep: serde_json::json!({ "name": "gone" }), rows: 30, cols: 120 };
    store::write(dir.path(), &store::File { sessions: vec![kept], lost: vec![] }).unwrap();
    let server = Server::in_dir(dir, Duration::from_secs(120), |_| false);
    let (client, _) = server.connect();
    let lost = &client.info().lost;
    assert_eq!(lost.len(), 1);
    assert!(lost[0].id == "gone" && lost[0].exited && lost[0].blob["name"] == "gone" && (lost[0].rows, lost[0].cols) == (30, 120));
    assert!(client.info().sessions.is_empty(), "nothing runs: they're for bringing back");
    client.forget_lost(vec!["gone".into()]);
    wait("let go of", || client::status(server.path()).unwrap().lost.is_empty());
    wait("written down", || store::load(server.path()).lost.is_empty());
    client.shutdown(Duration::from_secs(5));
}

#[test]
fn a_quiet_window_stays_connected() {
    // the hello's timeout must not outlive the hello (it did on Windows, on a cloned socket), and the heartbeat
    // keeps a quiet connection from looking dead
    let server = Server::start();
    let (client, events) = server.connect();
    std::thread::sleep(Duration::from_secs(7));
    assert!(!events.try_iter().any(|e| e == ClientEvent::Lost), "the window heard nothing bad");
    assert!(client.connected());
    assert!(client.list(Duration::from_secs(5)).is_some(), "and the server still answers it");
    client.shutdown(Duration::from_secs(5));
}

#[test]
fn a_server_gone_silent_counts_as_lost() {
    // something that says hello and then nothing: a stuck server
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    record::write(dir.path(), &record::Record { pid: 1, port, token: "t".into(), protocol: crate::PROTOCOL, version: "stuck".into() }).unwrap();
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let _ = crate::wire::read_frame(&mut sock);
        crate::wire::write_all(&mut sock, &crate::wire::json(crate::wire::WELCOME, &crate::HostInfo::default())).unwrap();
        std::thread::sleep(Duration::from_secs(30));
    });
    let (tx, events) = mpsc::channel();
    let started = Instant::now();
    let _client = Client::connect(dir.path(), Role::Window, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    assert_eq!(events.recv_timeout(Duration::from_secs(10)).unwrap(), ClientEvent::Lost);
    assert!(started.elapsed() >= Duration::from_secs(2), "not before it had been quiet a while");
}

#[test]
fn an_idle_server_stops_unless_it_starts_at_sign_in() {
    let server = Server::with(Duration::from_millis(600), |_| false);
    server.done.recv_timeout(Duration::from_secs(10)).expect("an idle server stops");
    assert!(record::read(server.path()).is_none());

    let resident = Server::with(Duration::from_millis(600), |_| true);
    assert!(resident.done.recv_timeout(Duration::from_secs(3)).is_err(), "one bro starts at sign-in stays");
    assert!(client::stop(resident.path(), Duration::from_secs(5)));
}

#[test]
fn values_survive_a_reconnect() {
    let server = Server::start();
    let (client, _) = server.connect();
    client.set_value("proxy_token", "abc");
    assert!(client.list(Duration::from_secs(5)).is_some(), "the set is in before the list answers");
    drop(client);
    let (client, _) = server.connect();
    assert_eq!(client.value("proxy_token").as_deref(), Some("abc"));
    client.shutdown(Duration::from_secs(5));
}

#[test]
fn a_spawn_that_fails_says_why() {
    let server = Server::start();
    let (client, _) = server.connect();
    let missing = request("x", (PathBuf::from("definitely-not-a-program-bro").to_string_lossy().to_string(), vec![]));
    match client.spawn(missing) {
        // Windows' ConPTY reports a missing program at spawn
        Err(e) => assert!(e.contains("couldn't start"), "{e}"),
        // a Unix pty forks first: the child exits at once instead
        Ok(remote) => assert!(wait_exit(&remote) != 0),
    }
    client.shutdown(Duration::from_secs(5));
}
