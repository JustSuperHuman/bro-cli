//! Real local ConPTY lifecycle tests. They use copies of the test runner, not an agent or user login.

use super::*;
use std::path::Path;
use std::process::{Command, Stdio};

const WORKER: &str = "panes::term::process_tests::session_process_worker";

fn wait_pid(marker: &Path) -> u32 {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(pid) = std::fs::read_to_string(marker).ok().and_then(|s| s.trim().parse().ok()) {
            return pid;
        }
        assert!(Instant::now() < until, "session did not write {}", marker.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            return false;
        }
        let running = WaitForSingleObject(process, 0) == WAIT_TIMEOUT;
        CloseHandle(process);
        running
    }
}

fn assert_dead(pid: u32) {
    let until = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(pid), "session descendant {pid} survived");
}

fn descendants(root: u32) -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    let mut rows = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        assert_ne!(snapshot, INVALID_HANDLE_VALUE);
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut next = Process32FirstW(snapshot, &mut entry) != 0;
        while next {
            rows.push((entry.th32ProcessID, entry.th32ParentProcessID));
            next = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    let mut tree = vec![root];
    let mut i = 0;
    while i < tree.len() {
        for &(pid, parent) in &rows {
            if parent == tree[i] && !tree.contains(&pid) {
                tree.push(pid);
            }
        }
        i += 1;
    }
    tree.remove(0);
    tree
}

#[test]
#[ignore = "helper process invoked by the ConPTY lifecycle tests"]
fn session_process_worker() {
    use std::os::windows::process::CommandExt;
    let Ok(mode) = std::env::var("SESSION_PROCESS_TEST_MODE") else { return };
    if mode == "leaf" {
        std::thread::sleep(Duration::from_secs(120));
        return;
    }
    let marker = std::env::var_os("SESSION_PROCESS_TEST_MARKER").unwrap();
    std::fs::write(PathBuf::from(&marker).with_extension("root"), std::process::id().to_string()).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", WORKER, "--ignored", "--nocapture"])
        .env("SESSION_PROCESS_TEST_MODE", "leaf")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    std::fs::write(marker, child.id().to_string()).unwrap();
    if mode == "exit" {
        std::process::exit(7);
    }
    child.wait().unwrap();
}

fn session(mode: &str, marker: &Path) -> Term {
    crate::testkit::isolate();
    let (tx, _rx) = std::sync::mpsc::channel();
    let svc = Services::offline(Default::default(), tx.clone());
    let cwd = marker.parent().unwrap().to_path_buf();
    let meta = Meta {
        sid: uuid::Uuid::new_v4().to_string(),
        harness: None,
        profile: None,
        store: None,
        model: None,
        label: "local lifecycle test".into(),
        name: None,
        renamed: false,
        project: svc.project_for(&cwd),
        cwd,
        started: Instant::now(),
        route_id: None,
        cleanup: vec![],
        orchestrator: false,
    };
    let spawn = Spawn {
        program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
        args: ["--exact", WORKER, "--ignored", "--nocapture"].map(String::from).to_vec(),
        env: vec![
            ("SESSION_PROCESS_TEST_MODE".into(), mode.into()),
            ("SESSION_PROCESS_TEST_MARKER".into(), marker.to_string_lossy().into_owned()),
        ],
        ..Spawn::default()
    };
    let mut term = Term::new(meta, spawn, svc);
    term.ensure_started(24, 80, Waker::new(1, tx));
    assert!(term.error.is_none(), "session startup failed: {:?}", term.error);
    term
}

#[test]
fn conpty_close_ends_hidden_descendants_and_preserves_sibling_session() {
    let dir = tempfile::tempdir().unwrap();
    let sibling = session("stay", &dir.path().join("sibling"));
    let sibling_pid = wait_pid(&dir.path().join("sibling"));
    let mut term = session("stay", &dir.path().join("leaf"));
    let pid = wait_pid(&dir.path().join("leaf"));
    assert!(alive(pid));
    term.kill();
    assert_dead(pid);
    assert!(alive(sibling_pid), "closing one pane ended its sibling");
    drop(sibling);
    assert_dead(sibling_pid);
}

#[test]
fn conpty_fast_natural_exit_ends_descendants_before_poll_or_pane_drop() {
    let dir = tempfile::tempdir().unwrap();
    for attempt in 0..8 {
        let marker = dir.path().join(format!("leaf-{attempt}"));
        let mut term = session("exit", &marker);
        let pid = wait_pid(&marker);
        // Keep Term alive and do not poll it: the root watcher must clean up independently.
        assert_dead(pid);
        let until = Instant::now() + Duration::from_secs(3);
        while term.exit_code().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(term.exit_code(), Some(7));
        term.kill(); // Must still clean up after exit_code has cached the root's exit.
        assert!(term.closed_signal().load(Ordering::SeqCst));
    }
}

#[test]
#[ignore = "verifies the installed bro.exe in an isolated demo ConPTY after installation"]
fn conpty_installed_bro_force_close_ends_live_session_tree() {
    let installed = std::env::var_os("BRO_VERIFY_EXE").expect("set BRO_VERIFY_EXE to the installed bro.exe");
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("descendant.pid");
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 45,
            cols: 140,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(installed);
    cmd.args(["--new", "--demo"]);
    cmd.cwd(dir.path());
    cmd.env("BRO_DIR", dir.path().join(".bro"));
    cmd.env_remove("BRO_BRIDGE_URL");
    cmd.env_remove("BRO_SESSION_ID");
    cmd.env("SESSION_PROCESS_TEST_MODE", "stay");
    cmd.env("SESSION_PROCESS_TEST_MARKER", &marker);
    // Bro itself has no outer job or watcher here. Killing only Bro must close its own job handles.
    let child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    struct Owner(Box<dyn Child + Send + Sync>);
    impl Drop for Owner {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut owner = Owner(child);
    let owner_pid = owner.0.process_id().unwrap();
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured = output.clone();
    let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
    let replies = writer.clone();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let drain = std::thread::spawn(move || {
        let mut buf = [0; 8192];
        let mut tail = Vec::new();
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            // ConPTY's inherited-cursor startup waits for the terminal host to answer DSR.
            tail.extend_from_slice(&buf[..n]);
            let requests = tail.windows(4).filter(|bytes| *bytes == b"\x1b[6n").count();
            for _ in 0..requests {
                let mut reply = replies.lock();
                reply.write_all(b"\x1b[1;1R").unwrap();
                reply.flush().unwrap();
            }
            if tail.len() > 3 {
                tail.drain(..tail.len() - 3);
            }
            let mut out = captured.lock();
            if out.len() < 256 * 1024 {
                out.extend_from_slice(&buf[..n]);
            }
        }
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut screen = vt100::Parser::new(45, 140, 0);
    let mut consumed = 0;
    loop {
        {
            let out = output.lock();
            screen.process(&out[consumed..]);
            consumed = out.len();
        }
        if screen.screen().contents().contains("Welcome to") {
            break;
        }
        assert!(owner.0.try_wait().unwrap().is_none(), "fresh Bro exited during startup");
        assert!(
            Instant::now() < until,
            "fresh Bro did not draw its demo session: {}",
            screen.screen().contents()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let exe = std::env::current_exe().unwrap();
    let (shell, _) = crate::util::default_shell(None);
    let input = if shell.contains("cmd") {
        format!("\"{}\" --exact {WORKER} --ignored --nocapture\r", exe.display())
    } else {
        format!("& '{}' --exact {WORKER} --ignored --nocapture\r", exe.display().to_string().replace('\'', "''"))
    };
    writer.lock().write_all(input.as_bytes()).unwrap();
    writer.lock().flush().unwrap();
    let descendant = wait_pid(&marker);
    let worker = wait_pid(&marker.with_extension("root"));
    assert!(alive(worker) && alive(descendant));
    let owned = descendants(owner_pid);
    assert!(owned.contains(&worker) && owned.contains(&descendant));
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    assert_dead(worker);
    assert_dead(descendant);
    for &pid in &owned {
        assert_dead(pid);
    }
    drop(writer);
    drop(pair.master);
    drain.join().unwrap();
    println!(
        "installed Bro {owner_pid}: all {} descendants ended after Bro was force-closed: {owned:?}",
        owned.len()
    );
}
