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
    Ok(reply.trim() == "ok")
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
            let _ = writeln!(w, "{}", if ok { "ok" } else { "closing" });
        }
    });
    Some(Guard { token, file })
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
