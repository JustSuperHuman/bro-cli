//! One bro at a time: the running bro listens on loopback and records `{pid, port, token}` in
//! `~/.bro/v2-instance.json`. Starting `bro` again in another folder hands that folder to the running bro (it
//! opens it as a project) and exits — like opening a folder in an editor that's already open. `bro --new`
//! skips the handoff.

use crate::pane::Event;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Serialize, Deserialize)]
struct Record {
    pid: u32,
    port: u16,
    token: String,
}

fn path() -> PathBuf {
    crate::util::bro_dir().join("v2-instance.json")
}

/// Ask a running bro to open `dir`. Ok(true) = it did (this process should exit); Ok(false) = nobody's running.
pub fn hand_off(dir: &Path) -> anyhow::Result<bool> {
    hand_off_via(&path(), dir)
}

fn hand_off_via(file: &Path, dir: &Path) -> anyhow::Result<bool> {
    let Some(rec) = std::fs::read_to_string(file).ok().and_then(|s| serde_json::from_str::<Record>(&s).ok()) else { return Ok(false) };
    if rec.pid == std::process::id() {
        return Ok(false);
    }
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, rec.port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else { return Ok(false) };
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    writeln!(s, "{}\topen\t{}", rec.token, dir.display())?;
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply)?;
    // "ok <window>": bring the running bro's terminal window to the front
    let mut parts = reply.split_whitespace();
    if parts.next() != Some("ok") {
        return Ok(false);
    }
    if let Some(h) = parts.next().and_then(|h| h.parse::<isize>().ok()).filter(|h| *h != 0) {
        window::bring_to_front(h);
    }
    Ok(true)
}

/// Removes the record when the running bro exits.
pub struct Guard {
    token: String,
    file: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        // only remove it if it's still ours (a second `bro --new` may have taken over)
        if let Some(rec) = std::fs::read_to_string(&self.file).ok().and_then(|s| serde_json::from_str::<Record>(&s).ok())
            && rec.token == self.token
        {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

/// Listen for handoffs; each becomes `Event::OpenProject`. Keep the guard alive for the app's lifetime.
pub fn serve(tx: Sender<Event>) -> Option<Guard> {
    serve_via(path(), tx)
}

fn serve_via(file: PathBuf, tx: Sender<Event>) -> Option<Guard> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let rec = Record { pid: std::process::id(), port, token: token.clone() };
    crate::util::atomic_write(&file, serde_json::to_string_pretty(&rec).ok()?.as_bytes()).ok()?;
    let want = token.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            let mut r = BufReader::new(&stream);
            if r.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.trim_end().splitn(3, '\t');
            let (Some(tok), Some("open"), Some(dir)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let mut w = &stream;
            if tok != want {
                let _ = writeln!(w, "denied");
                continue;
            }
            let ok = tx.send(Event::OpenProject(PathBuf::from(dir))).is_ok();
            if ok {
                let _ = writeln!(w, "ok {}", window::terminal_window());
            } else {
                let _ = writeln!(w, "closing");
            }
        }
    });
    Some(Guard { token, file })
}

/// The terminal window this bro runs in, and raising another bro's.
mod window {
    /// The top-level window hosting this console (Windows Terminal owns the pseudo-console window it gives
    /// ConPTY apps; classic conhost returns its own window). 0 when there isn't a visible one.
    #[cfg(windows)]
    pub fn terminal_window() -> isize {
        use windows_sys::Win32::System::Console::GetConsoleWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor, IsWindowVisible};
        // SAFETY: plain Win32 queries on handles the system gives us; null / invisible handles are filtered.
        unsafe {
            let h = GetConsoleWindow();
            if h.is_null() {
                return 0;
            }
            let root = GetAncestor(h, GA_ROOTOWNER);
            let root = if root.is_null() { h } else { root };
            if IsWindowVisible(root) == 0 { 0 } else { root as isize }
        }
    }

    #[cfg(not(windows))]
    pub fn terminal_window() -> isize {
        0
    }

    /// Restore (if minimised) and focus a window. Windows only lets the foreground process move the focus;
    /// a tap of Alt is the documented way for the process the user just typed into to hand it over.
    #[cfg(windows)]
    pub fn bring_to_front(h: isize) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, VK_MENU, keybd_event};
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, SW_RESTORE, SetForegroundWindow, ShowWindow};
        let hwnd = h as windows_sys::Win32::Foundation::HWND;
        // SAFETY: the handle came from the running bro; IsWindow guards against a stale one.
        unsafe {
            if IsWindow(hwnd) == 0 {
                return;
            }
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            keybd_event(VK_MENU as u8, 0, 0, 0);
            keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
            SetForegroundWindow(hwnd);
        }
    }

    #[cfg(not(windows))]
    pub fn bring_to_front(_h: isize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_launch_hands_its_folder_to_the_running_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("v2-instance.json");
        assert!(!hand_off_via(&file, Path::new("/x")).unwrap(), "nobody running");
        let (tx, rx) = std::sync::mpsc::channel();
        let guard = serve_via(file.clone(), tx).expect("listening");
        // pretend the handoff comes from another process
        let mut rec: Record = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        rec.pid += 1;
        std::fs::write(&file, serde_json::to_string(&rec).unwrap()).unwrap();
        assert!(hand_off_via(&file, Path::new("/some/project")).unwrap());
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::OpenProject(p) => assert_eq!(p, PathBuf::from("/some/project")),
            _ => panic!("expected OpenProject"),
        }
        // a wrong token is refused
        let good = rec.token.clone();
        rec.token = "nope".into();
        std::fs::write(&file, serde_json::to_string(&rec).unwrap()).unwrap();
        assert!(!hand_off_via(&file, Path::new("/x")).unwrap());
        // the guard only removes its own record
        rec.token = good;
        std::fs::write(&file, serde_json::to_string(&rec).unwrap()).unwrap();
        drop(guard);
        assert!(!file.exists());
    }
}
