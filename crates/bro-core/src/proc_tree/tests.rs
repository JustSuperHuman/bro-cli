use super::*;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

#[cfg(windows)]
const WORKER: &str = "proc_tree::tests::process_tree_worker";

#[cfg(windows)]
fn command(mode: &str, marker: &Path) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", WORKER, "--ignored", "--nocapture"])
        .env("BRO_PROCESS_TEST_MODE", mode)
        .env("BRO_PROCESS_TEST_MARKER", marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

#[cfg(windows)]
fn wait_pid(marker: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(pid) = std::fs::read_to_string(marker).ok().and_then(|s| s.trim().parse().ok()) {
            return pid;
        }
        assert!(Instant::now() < deadline, "worker did not write {}", marker.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
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

#[cfg(windows)]
fn assert_dead(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(pid), "owned process {pid} survived cleanup");
}

// The worker is another copy of this test executable. It runs no agent, network, or user command.
#[cfg(windows)]
#[test]
#[ignore = "helper process invoked by the process ownership tests"]
fn process_tree_worker() {
    use std::os::windows::process::CommandExt;
    let Ok(mode) = std::env::var("BRO_PROCESS_TEST_MODE") else { return };
    let marker = PathBuf::from(std::env::var_os("BRO_PROCESS_TEST_MARKER").unwrap());
    if mode == "leaf" {
        std::fs::write(&marker, std::process::id().to_string()).unwrap();
    } else if mode == "owner" {
        let (mut child, _tree) = ProcessTree::spawn(&mut command("intermediate-root", &marker)).unwrap();
        std::fs::write(marker.with_extension("root"), child.id().to_string()).unwrap();
        child.wait().unwrap();
        return;
    } else if mode == "intermediate-root" {
        let mut child = command("launcher", &marker).creation_flags(0x0800_0000).spawn().unwrap();
        child.wait().unwrap();
    } else {
        let mut leaf = command("leaf", &marker);
        // Keep the root's stdout pipe open in the descendant to exercise EOF-independent cleanup.
        leaf.stdout(Stdio::inherit());
        if let Some(exe) = std::env::var_os("BRO_PROCESS_TEST_GUI") {
            let args: Vec<_> = leaf.get_args().map(|s| s.to_os_string()).collect();
            leaf = Command::new(exe);
            leaf.args(args).env("BRO_PROCESS_TEST_MODE", "leaf").env("BRO_PROCESS_TEST_MARKER", &marker);
        }
        let child = leaf.creation_flags(0x0800_0000).spawn().unwrap();
        if mode == "exit-root" {
            std::fs::write(&marker, child.id().to_string()).unwrap();
            std::process::exit(7);
        }
        wait_pid(&marker);
        if mode == "launcher" {
            return;
        }
    }
    std::thread::sleep(Duration::from_secs(120));
}

#[cfg(windows)]
#[test]
fn dropping_session_ends_children_after_an_intermediate_launcher_exits() {
    let dir = tempfile::tempdir().unwrap();
    let (mut sibling, sibling_tree) = ProcessTree::spawn(&mut command("leaf", &dir.path().join("sibling"))).unwrap();
    let sibling_pid = wait_pid(&dir.path().join("sibling"));
    let (mut root, tree) = ProcessTree::spawn(&mut command("intermediate-root", &dir.path().join("leaf"))).unwrap();
    let pid = wait_pid(&dir.path().join("leaf"));
    assert!(alive(pid));
    drop(tree);
    root.wait().unwrap();
    assert_dead(pid);
    assert!(alive(sibling_pid), "closing one session must preserve another session");
    drop(sibling_tree);
    sibling.wait().unwrap();
}

#[cfg(windows)]
#[test]
fn root_exit_ends_descendants_without_waiting_for_output_eof_or_owner_drop() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("leaf");
    let mut cmd = command("exit-root", &marker);
    cmd.stdout(Stdio::piped());
    let (mut root, _tree) = ProcessTree::spawn(&mut cmd).unwrap();
    let pid = wait_pid(&marker);
    assert_eq!(root.wait().unwrap().code(), Some(7));
    assert_dead(pid);
    // _tree remains alive: the root watcher, not dropping the session, ended its children.
}

#[cfg(windows)]
#[test]
fn gui_subsystem_children_are_owned_by_the_session_too() {
    use std::io::{Read, Seek, SeekFrom, Write};
    let dir = tempfile::tempdir().unwrap();
    let gui = dir.path().join("gui-worker.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &gui).unwrap();
    let mut exe = std::fs::OpenOptions::new().read(true).write(true).open(&gui).unwrap();
    exe.seek(SeekFrom::Start(60)).unwrap();
    let mut offset = [0; 4];
    exe.read_exact(&mut offset).unwrap();
    exe.seek(SeekFrom::Start(u32::from_le_bytes(offset) as u64 + 4 + 20 + 68)).unwrap();
    exe.write_all(&2u16.to_le_bytes()).unwrap(); // IMAGE_SUBSYSTEM_WINDOWS_GUI
    drop(exe);
    let marker = dir.path().join("gui-leaf");
    let mut cmd = command("gui-root", &marker);
    cmd.env("BRO_PROCESS_TEST_GUI", &gui);
    let (mut root, tree) = ProcessTree::spawn(&mut cmd).unwrap();
    let pid = wait_pid(&marker);
    assert!(alive(pid));
    drop(tree);
    root.wait().unwrap();
    assert_dead(pid);
}

#[cfg(windows)]
#[test]
fn force_killing_owner_closes_its_jobs_without_running_destructors() {
    use std::os::windows::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("leaf");
    // No outer ProcessTree: its watcher would mask failure of the inner job's kill-on-close flag.
    let mut owner = command("owner", &marker).creation_flags(0x0800_0000).spawn().unwrap();
    let pid = wait_pid(&marker);
    let root = wait_pid(&marker.with_extension("root"));
    assert!(alive(pid));
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert_dead(pid);
    assert_dead(root);
}

#[cfg(unix)]
#[test]
fn cleanup_signals_the_group_after_its_leader_exits() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("leaf");
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "sleep 120 & echo $! > \"$1\"; exit 7", "sh"])
        .arg(&marker)
        .stdout(Stdio::piped());
    let (mut root, mut tree) = ProcessTree::spawn(&mut cmd).unwrap();
    assert_eq!(root.wait().unwrap().code(), Some(7));
    let pid: i32 = std::fs::read_to_string(marker).unwrap().trim().parse().unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    tree.terminate();
    let mut stdout = root.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut output = Vec::new();
        tx.send(stdout.read_to_end(&mut output)).ok();
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the descendant kept its output pipe open")
        .unwrap();
}
