//! The terminal UI on bro's session server, end to end: the real `bro` in a pseudo-terminal opens a shell, quits
//! leaving it running in the server (the background bro carries on with it), the next `bro` takes it back, and
//! restarting the server from the palette brings it back again. A second window watches and types into the same
//! session; a server that dies under bro, or while it's closed, has its sessions brought back. Everything lives in
//! a temporary bro folder.
//! Opt-in (starts real processes):
//! `cargo test -p bro-tui --test tui_sessions -- --ignored --nocapture --test-threads=1`

use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `bro` running in a pseudo-terminal, read into a screen.
struct Tui {
    child: Box<dyn Child + Send + Sync>,
    input: Arc<Mutex<Box<dyn Write + Send>>>,
    screen: Arc<Mutex<vt100::Parser>>,
}

impl Tui {
    fn launch(home: &Home) -> Tui {
        Tui::launch_with(home, &[])
    }

    fn launch_with(home: &Home, flags: &[&str]) -> Tui {
        let pair = native_pty_system().openpty(PtySize { rows: 40, cols: 140, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bro"));
        cmd.arg("--tui");
        for f in flags {
            cmd.arg(f);
        }
        cmd.arg(&home.proj);
        cmd.cwd(&home.proj);
        cmd.env("TERM", "xterm-256color");
        for (k, v) in home.env() {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let input: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
        let screen = Arc::new(Mutex::new(vt100::Parser::new(40, 140, 0)));
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (s, w) = (screen.clone(), input.clone());
        std::thread::spawn(move || {
            let _master = pair.master;
            let mut buf = [0u8; 16384];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let data = &buf[..n];
                let mut p = s.lock();
                p.process(data);
                // we're the terminal: ConPTY asks where the cursor is and waits for the answer
                if data.windows(4).any(|x| x == b"\x1b[6n") {
                    let (r, c) = p.screen().cursor_position();
                    let _ = w.lock().write_all(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
                }
            }
        });
        Tui { child, input, screen }
    }

    fn text(&self) -> String {
        self.screen.lock().screen().contents()
    }

    fn wait_for(&self, what: &str, ok: impl Fn(&str) -> bool) {
        let until = Instant::now() + Duration::from_secs(30);
        while !ok(&self.text()) {
            assert!(Instant::now() < until, "never saw {what}:\n{}", self.text());
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn keys(&self, bytes: &[u8]) {
        let mut w = self.input.lock();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    }

    /// Run an action from the command palette (alt+p; ConPTY doesn't pass ctrl+space through as the prefix).
    fn palette(&self, describe: &str) {
        self.keys(b"\x1bp");
        std::thread::sleep(Duration::from_millis(800));
        self.keys(describe.as_bytes());
        self.wait_for(describe, |s| s.contains(describe));
        std::thread::sleep(Duration::from_millis(300));
        self.keys(b"\r");
    }

    /// Quit; returns what bro printed on its way out.
    fn quit(mut self) -> String {
        self.palette("quit bro");
        let until = Instant::now() + Duration::from_secs(20);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < until, "bro didn't quit:\n{}", self.text());
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(300));
        self.text()
    }
}

/// A bro folder of the test's own.
struct Home {
    dir: tempfile::TempDir,
    proj: PathBuf,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".bro")).unwrap();
        // no setup guide over the workspace
        std::fs::write(dir.path().join(".bro").join("v2.toml"), "setup_done = true\n").unwrap();
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        Home { dir, proj }
    }

    fn bro_dir(&self) -> PathBuf {
        self.dir.path().join(".bro")
    }

    fn env(&self) -> Vec<(&'static str, PathBuf)> {
        let d = self.dir.path();
        vec![("BRO_DIR", self.bro_dir()), ("CLAUDE_POOL_DIR", d.join("pool")), ("BRO_CODEX_PROFILES_DIR", d.join("codex"))]
    }

    fn running(&self) -> usize {
        bro_host::client::status(&self.bro_dir()).map(|i| i.sessions.iter().filter(|s| !s.exited).count()).unwrap_or(0)
    }

    fn server_pid(&self) -> Option<u32> {
        bro_host::client::status(&self.bro_dir()).ok().map(|i| i.pid)
    }

    /// Who drives the server.
    fn primary(&self) -> Option<bro_host::Peer> {
        bro_host::client::status(&self.bro_dir()).ok().and_then(|i| i.peers.primary)
    }

    fn lost(&self) -> usize {
        bro_host::client::status(&self.bro_dir()).map(|i| i.lost.len()).unwrap_or(0)
    }

    fn wait(&self, what: &str, ok: impl Fn(&Home) -> bool) {
        let until = Instant::now() + Duration::from_secs(30);
        while !ok(self) {
            assert!(Instant::now() < until, "never: {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// End a process at once, as a crash or a power cut would.
fn kill_hard(pid: u32) {
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
    #[cfg(unix)]
    let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).output();
}

fn alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }
    #[cfg(unix)]
    std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().is_ok_and(|s| s.success())
}

impl Drop for Home {
    fn drop(&mut self) {
        bro_host::client::stop(&self.bro_dir(), Duration::from_secs(10));
    }
}

/// The shell's prompt is on screen.
fn prompt(screen: &str) -> bool {
    if cfg!(windows) { screen.contains("PS ") || screen.contains(">") && screen.contains(":\\") } else { screen.contains('$') || screen.contains('#') }
}

#[test]
#[ignore = "runs the real bro in a pseudo-terminal"]
fn a_shell_outlives_the_terminal_ui_and_comes_back_in_the_next_one() {
    let home = Home::new();
    let tui = Tui::launch(&home);
    tui.wait_for("the workspace", |s| s.contains("proj"));
    // alt+t: a shell
    tui.keys(b"\x1bt");
    tui.wait_for("the shell's prompt", prompt);
    assert_eq!(home.running(), 1, "the shell runs in the session server");
    let said = tui.quit();
    assert!(said.contains("1 session still running"), "{said}");
    assert_eq!(home.running(), 1, "quitting bro leaves it running");
    // the background bro carries on with it while no window is open
    home.wait("the background bro drives the server", |h| h.primary().is_some_and(|p| p.role == bro_host::Role::Background));
    let background = home.primary().unwrap().pid;

    let tui = Tui::launch(&home);
    tui.wait_for("the shell, back", |s| prompt(s) && s.contains("shell"));
    assert_eq!(home.running(), 1, "the same one, not a second");
    home.wait("the window took over", |h| h.primary().is_some_and(|p| p.role == bro_host::Role::Window));
    home.wait("the background bro stepped aside", |_| !alive(background));

    // restart the server from the palette: the shell ends and starts again in its pane
    let before = home.server_pid().unwrap();
    tui.palette("session server: restart");
    tui.wait_for("the question", |s| s.contains("restart the session server and end its 1 session?"));
    tui.keys(b"y");
    let until = Instant::now() + Duration::from_secs(30);
    while home.server_pid().is_none_or(|p| p == before) || home.running() != 1 {
        assert!(Instant::now() < until, "the server didn't come back with the shell:\n{}", tui.text());
        std::thread::sleep(Duration::from_millis(100));
    }
    tui.wait_for("a fresh shell", |s| prompt(s) && s.contains("shell") && !s.contains("restart the session server"));
    tui.quit();
    assert_eq!(home.running(), 1);
    // stopping the server ends the background bro too: it doesn't bring it back
    home.wait("the background bro is back", |h| h.primary().is_some_and(|p| p.role == bro_host::Role::Background));
    let background = home.primary().unwrap().pid;
    assert!(bro_host::client::stop(&home.bro_dir(), Duration::from_secs(10)));
    home.wait("the background bro went with the server", |_| !alive(background));
    std::thread::sleep(Duration::from_secs(2));
    assert!(home.server_pid().is_none(), "and nothing started it again");
}

#[test]
#[ignore = "runs the real bro in a pseudo-terminal"]
fn a_second_window_watches_and_types_into_the_same_session() {
    let home = Home::new();
    let tui = Tui::launch(&home);
    tui.wait_for("the workspace", |s| s.contains("proj"));
    tui.keys(b"\x1bt");
    tui.wait_for("the shell's prompt", prompt);
    let view = Tui::launch_with(&home, &["--view"]);
    view.wait_for("the same shell", |s| prompt(s) && s.contains("shell"));
    assert_eq!(home.running(), 1, "watching starts nothing");
    view.keys(b"echo from-the-second-window\r");
    tui.wait_for("what the viewer typed, in the first window", |s| s.contains("from-the-second-window"));
    let said = view.quit();
    assert!(said.contains("the sessions carry on in your other bro"), "{said}");
    assert_eq!(home.running(), 1, "the viewer leaving ends nothing");
    tui.keys(b"echo still-mine\r");
    tui.wait_for("the first window, still driving", |s| s.contains("still-mine"));
    tui.quit();
}

#[test]
#[ignore = "runs the real bro in a pseudo-terminal"]
fn a_server_that_dies_under_bro_brings_its_sessions_back() {
    let home = Home::new();
    let tui = Tui::launch(&home);
    tui.wait_for("the workspace", |s| s.contains("proj"));
    tui.keys(b"\x1bt");
    tui.wait_for("the shell's prompt", prompt);
    let before = home.server_pid().unwrap();
    let written = |h: &Home| {
        let file = std::fs::read(h.bro_dir().join("v2-host-sessions.json")).ok();
        file.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).is_some_and(|v| v["sessions"].as_array().is_some_and(|s| s.len() == 1))
    };
    home.wait("the session is written down", written);
    kill_hard(before);
    // bro connects again (starting a new server) and the shell starts again in its pane
    home.wait("a new server with the shell", |h| h.server_pid().is_some_and(|p| p != before) && h.running() == 1);
    tui.wait_for("the shell, back", |s| prompt(s) && s.contains("stopped unexpectedly"));
    assert_eq!(home.lost(), 0, "it came back: nothing left to offer");
    tui.quit();
}

#[test]
#[ignore = "runs the real bro in a pseudo-terminal"]
fn sessions_a_crash_took_down_while_bro_was_closed_are_offered_back() {
    let home = Home::new();
    let tui = Tui::launch(&home);
    tui.wait_for("the workspace", |s| s.contains("proj"));
    tui.keys(b"\x1bt");
    tui.wait_for("the shell's prompt", prompt);
    tui.quit();
    home.wait("the background bro drives the server", |h| h.primary().is_some_and(|p| p.role == bro_host::Role::Background));
    // the computer goes down: everything ends at once
    let (server, background) = (home.server_pid().unwrap(), home.primary().unwrap().pid);
    kill_hard(background);
    kill_hard(server);
    home.wait("they're gone", |_| !alive(server) && !alive(background));
    let tui = Tui::launch(&home);
    tui.wait_for("the offer", |s| s.contains("bring back the 1 session"));
    tui.keys(b"y");
    tui.wait_for("the shell, back", |s| prompt(s) && s.contains("shell") && !s.contains("bring back"));
    assert_eq!(home.running(), 1);
    assert_eq!(home.lost(), 0);
    tui.quit();
}

#[test]
#[ignore = "runs the real bro in a pseudo-terminal"]
fn quitting_with_nothing_running_stops_the_server() {
    let home = Home::new();
    let tui = Tui::launch(&home);
    tui.wait_for("the workspace", |s| s.contains("proj"));
    assert!(home.server_pid().is_some(), "bro started its server");
    let said = tui.quit();
    assert!(!said.contains("still running"), "{said}");
    assert!(home.server_pid().is_none(), "nothing to keep: the server went with bro");
}
