use super::owned_output;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const WORKER: &str = "orchestrator::claude::process_tests::owned_command_worker";

#[test]
#[ignore = "helper process invoked by async command lifecycle tests"]
fn owned_command_worker() {
    use std::os::windows::process::CommandExt;
    let Ok(mode) = std::env::var("BRO_ASYNC_PROCESS_TEST_MODE") else { return };
    if mode == "leaf" {
        std::thread::sleep(Duration::from_secs(120));
        return;
    }
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", WORKER, "--ignored", "--nocapture"])
        .env("BRO_ASYNC_PROCESS_TEST_MODE", "leaf")
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    std::fs::write(std::env::var_os("BRO_ASYNC_PROCESS_TEST_MARKER").unwrap(), child.id().to_string()).unwrap();
    if mode == "exit" {
        std::process::exit(7);
    }
    std::thread::sleep(Duration::from_secs(120));
}

fn command(mode: &str, marker: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", WORKER, "--ignored", "--nocapture"])
        .env("BRO_ASYNC_PROCESS_TEST_MODE", mode)
        .env("BRO_ASYNC_PROCESS_TEST_MARKER", marker)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

async fn wait_pid(marker: &Path) -> u32 {
    let until = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(marker).ok().and_then(|s| s.trim().parse().ok()) {
            return pid;
        }
        assert!(tokio::time::Instant::now() < until, "command did not start its descendant");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn assert_dead(pid: u32) {
    use std::os::windows::process::CommandExt;
    let ps = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/tasklist.exe");
    let output = Command::new(ps)
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains(&format!("\"{pid}\"")), "async task descendant {pid} survived: {text}");
}

#[tokio::test]
async fn cancelling_an_async_command_ends_its_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("leaf");
    let mut cmd = command("stay", &marker);
    let task = tokio::spawn(async move { owned_output(&mut cmd).await });
    let pid = wait_pid(&marker).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_dead(pid).await;
}

#[tokio::test]
async fn natural_command_exit_cleans_inherited_output_handles() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("leaf");
    let output = tokio::time::timeout(Duration::from_secs(10), owned_output(&mut command("exit", &marker)))
        .await
        .expect("a descendant held the finished command's output open")
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_dead(wait_pid(&marker).await).await;
}
