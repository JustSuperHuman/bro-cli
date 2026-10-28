//! A pane on a real session server: a hosted session outlives its pane when bro quits, and the next bro's pane
//! picks it up with its screen and metadata; a second window watches it at the size whoever typed last asked for;
//! what goes to disk to bring it back holds no secrets.

use super::*;
use std::sync::mpsc;

/// A server on a folder of its own; returns the folder (the server stops with the test process).
fn server() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let opts = bro_host::ServeOpts { dir: dir.path().to_path_buf(), version: "test".into(), idle: Duration::from_secs(120), resident: |_| false };
    std::thread::spawn(move || bro_host::serve(opts));
    let until = Instant::now() + Duration::from_secs(5);
    while bro_host::record::read(dir.path()).is_none() {
        assert!(Instant::now() < until, "the server never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    dir
}

fn connect(dir: &std::path::Path) -> bro_host::Client {
    connect_as(dir, bro_host::Role::Window)
}

fn connect_as(dir: &std::path::Path, role: bro_host::Role) -> bro_host::Client {
    bro_host::Client::connect(dir, role, |_| {}).unwrap()
}

fn wait_until(what: &str, ok: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !ok() {
        assert!(Instant::now() < until, "never: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn shell() -> Spawn {
    Spawn { program: if cfg!(windows) { "cmd.exe".into() } else { "/bin/sh".into() }, ..Spawn::default() }
}

fn meta(svc: &Services) -> Meta {
    let cwd = std::env::temp_dir();
    Meta {
        sid: uuid::Uuid::new_v4().to_string(),
        harness: None,
        profile: None,
        store: None,
        model: None,
        label: "hosted test".into(),
        name: Some("keep me".into()),
        renamed: true,
        project: svc.project_for(&cwd),
        cwd,
        started: Instant::now() - Duration::from_secs(90),
        route_id: None,
        cleanup: vec![],
        orchestrator: false,
    }
}

fn wait_for(t: &Term, needle: &str) {
    let until = Instant::now() + Duration::from_secs(20);
    while !t.screen_rows().join("\n").contains(needle) {
        assert!(Instant::now() < until, "never saw {needle:?}:\n{}", t.screen_rows().join("\n"));
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_hosted_session_outlives_its_pane_and_comes_back_with_its_screen() {
    crate::testkit::isolate();
    let dir = server();
    let (tx, _rx) = mpsc::channel();
    let svc = Services::offline(Default::default(), tx.clone());
    svc.set_host_for_test(Some(connect(dir.path())));
    let meta = meta(&svc);
    let sid = meta.sid.clone();
    let mut term = Term::new(meta, shell(), svc);
    term.ensure_started(24, 80, Waker::new(1, tx.clone()));
    assert!(term.error.is_none(), "{:?}", term.error);
    assert!(term.hosted());
    term.send(b"echo hosted-marker-7\r");
    wait_for(&term, "hosted-marker-7");
    // bro quits: the session stays with the server
    term.detach();
    drop(term);

    // the next bro
    let svc = Services::offline(Default::default(), tx.clone());
    let client = connect(dir.path());
    let info = client.info().sessions.first().cloned().expect("the session kept running");
    assert!(!info.exited);
    svc.set_host_for_test(Some(client.clone()));
    let mut back = Term::reattach(&info, svc).expect("its metadata reads back");
    assert_eq!(back.meta.sid, sid, "same session id: the bridge and the sidebar keep it");
    assert_eq!(back.meta.name.as_deref(), Some("keep me"));
    assert!(back.meta.renamed);
    assert!(back.meta.started.elapsed() >= Duration::from_secs(85), "its age comes back too");
    assert!(back.hosted());
    back.ensure_started(30, 100, Waker::new(2, tx));
    assert!(back.error.is_none(), "{:?}", back.error);
    wait_for(&back, "hosted-marker-7");
    back.send(b"echo after-coming-back\r");
    wait_for(&back, "after-coming-back");

    // closing the pane ends the session on the server
    back.kill();
    let until = Instant::now() + Duration::from_secs(10);
    while client.list(Duration::from_secs(5)).is_some_and(|l| !l.is_empty()) {
        assert!(Instant::now() < until, "the server still holds the closed session");
        std::thread::sleep(Duration::from_millis(50));
    }
    client.shutdown(Duration::from_secs(5));
}

#[test]
fn a_pane_closed_before_it_picked_its_session_up_still_ends_it() {
    crate::testkit::isolate();
    let dir = server();
    let (tx, _rx) = mpsc::channel();
    let svc = Services::offline(Default::default(), tx.clone());
    svc.set_host_for_test(Some(connect(dir.path())));
    let mut term = Term::new(meta(&svc), shell(), svc);
    term.ensure_started(24, 80, Waker::new(1, tx.clone()));
    term.detach();
    drop(term);
    let svc = Services::offline(Default::default(), tx);
    let client = connect(dir.path());
    let info = client.info().sessions.first().cloned().unwrap();
    svc.set_host_for_test(Some(client.clone()));
    let mut back = Term::reattach(&info, svc).unwrap();
    // never drawn, so never attached: closing it must not leave it running unseen
    back.kill();
    let until = Instant::now() + Duration::from_secs(10);
    while client.list(Duration::from_secs(5)).is_some_and(|l| !l.is_empty()) {
        assert!(Instant::now() < until, "the server still holds it");
        std::thread::sleep(Duration::from_millis(50));
    }
    client.shutdown(Duration::from_secs(5));
}

#[test]
fn a_second_window_watches_at_the_size_of_whoever_typed_last() {
    crate::testkit::isolate();
    let dir = server();
    let (tx, _rx) = mpsc::channel();
    let svc = Services::offline(Default::default(), tx.clone());
    svc.set_host_for_test(Some(connect(dir.path())));
    let mut mine = Term::new(meta(&svc), shell(), svc);
    mine.ensure_started(24, 80, Waker::new(1, tx.clone()));
    mine.send(b"echo shared-marker\r");
    wait_for(&mine, "shared-marker");

    // another window on the same sessions
    let svc2 = Services::offline(Default::default(), tx.clone());
    let viewer = connect_as(dir.path(), bro_host::Role::Viewer);
    svc2.set_host_for_test(Some(viewer.clone()));
    let info = viewer.info().sessions.first().cloned().unwrap();
    let mut theirs = Term::reattach(&info, svc2).unwrap();
    theirs.ensure_started(40, 120, Waker::new(2, tx.clone()));
    wait_for(&theirs, "shared-marker");
    // drawn in a bigger tile, it asks for that size, but the session keeps the size of the window that typed
    theirs.resize(40, 120);
    std::thread::sleep(Duration::from_millis(300));
    theirs.poll_size_for_test();
    assert!(theirs.watching(), "the other window controls it");
    assert_eq!(theirs.pty_size(), (24, 80));
    assert!(!theirs.remote_grown(), "not a phone's minimum: another window's size");
    // typing into it takes it over, and its size with it: both screens follow
    theirs.send(b"echo from-the-viewer\r");
    wait_for(&mine, "from-the-viewer");
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        theirs.poll_size_for_test();
        mine.poll_size_for_test();
        if !theirs.watching() && theirs.pty_size() == (40, 120) && mine.pty_size() == (40, 120) && mine.watching() {
            break;
        }
        assert!(Instant::now() < until, "the viewer never took the session's size: {:?} / {:?}", theirs.pty_size(), mine.pty_size());
        std::thread::sleep(Duration::from_millis(20));
    }
    mine.kill();
    viewer.shutdown(Duration::from_secs(5));
}

#[test]
fn what_goes_to_disk_to_bring_a_session_back_keeps_no_secrets() {
    crate::testkit::isolate();
    let dir = server();
    let (tx, _rx) = mpsc::channel();
    let svc = Services::offline(Default::default(), tx.clone());
    let client = connect(dir.path());
    svc.set_host_for_test(Some(client.clone()));
    let spawn = Spawn { env: vec![("SOME_API_KEY".into(), "sk-secret-123".into())], ..shell() };
    let mut term = Term::new(meta(&svc), spawn, svc);
    term.ensure_started(24, 80, Waker::new(1, tx));
    assert!(term.error.is_none(), "{:?}", term.error);
    let file = dir.path().join("v2-host-sessions.json");
    wait_until("the server wrote it down", || std::fs::read_to_string(&file).is_ok_and(|s| s.contains("keep me")));
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(!on_disk.contains("sk-secret-123"), "the environment stays in memory: {on_disk}");
    // and what's there makes a pane again
    let saved: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
    let info = bro_host::SessionInfo { id: "x".into(), rows: 24, cols: 80, exited: true, blob: saved["sessions"][0]["keep"].clone(), ..Default::default() };
    let svc = Services::offline(Default::default(), mpsc::channel().0);
    let back = Term::from_saved(&info, svc).expect("a pane comes back from what was kept");
    assert_eq!(back.meta.name.as_deref(), Some("keep me"));
    assert!(back.launch_spawn().is_some_and(|s| s.env.is_empty()));
    term.kill();
    client.shutdown(Duration::from_secs(5));
}

#[test]
fn the_terminal_ui_answers_what_its_own_sessions_ask() {
    // a session bro runs itself (no session server, no desktop emulator): device attributes, colours, the cursor
    let (tx, _rx) = mpsc::channel();
    let svc = Services::offline(Default::default(), tx);
    let term = Term::new(meta(&svc), shell(), svc);
    term.set_answers_for_test(false);
    let parser = term.parser_for_test();
    let mut p = parser.lock();
    p.process(b"ab\x1b[6n\x1b[c\x1b]11;?\x07");
    let replies = String::from_utf8(p.callbacks_mut().answers.take()).unwrap();
    assert!(replies.starts_with("\x1b[1;3R\x1b[?6c\x1b]11;rgb:"), "{replies:?}");
    drop(p);
    // hosted: the server answers, once
    term.set_answers_for_test(true);
    let mut p = parser.lock();
    p.process(b"\x1b[c\x1b[6n");
    assert!(p.callbacks_mut().answers.take().is_empty());
}
