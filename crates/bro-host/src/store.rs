//! What brings sessions back after the server stopped: `v2-host-sessions.json` in bro's folder, readable by you
//! alone. The running server keeps it current with each live session's `keep` (the window's metadata without its
//! secrets). A server asked to stop clears its sessions from it first, so whatever is still listed when the next
//! server starts was running when one stopped without being asked to (a crash, the computer shutting down): that
//! server takes them over as lost sessions, and they stay in the file until a window brings them back or lets them
//! go.

use crate::SessionInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

#[derive(Serialize, Deserialize, Default, Debug)]
pub(crate) struct File {
    #[serde(default)]
    pub sessions: Vec<Kept>,
    #[serde(default)]
    pub lost: Vec<SessionInfo>,
}

/// One running session, as it would come back.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct Kept {
    pub id: String,
    pub keep: Value,
    pub rows: u16,
    pub cols: u16,
}

impl Kept {
    fn lost(self) -> SessionInfo {
        SessionInfo { id: self.id, pid: None, rows: self.rows, cols: self.cols, exited: true, code: None, blob: self.keep }
    }
}

pub(crate) fn path(dir: &Path) -> PathBuf {
    dir.join("v2-host-sessions.json")
}

pub(crate) fn load(dir: &Path) -> File {
    std::fs::read(path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub(crate) fn write(dir: &Path, file: &File) -> std::io::Result<()> {
    crate::record::write_private(&path(dir), &serde_json::to_vec_pretty(file).unwrap_or_default())
}

/// A starting server's lost sessions: what the last one left listed (it didn't stop cleanly), plus what it hadn't
/// handed over yet. Rewritten at once with no running sessions, so a crash of this server doesn't count them twice.
pub(crate) fn take_over(dir: &Path) -> Vec<SessionInfo> {
    let old = load(dir);
    let mut lost = old.lost;
    lost.extend(old.sessions.into_iter().filter(|k| !k.keep.is_null()).map(Kept::lost));
    let _ = write(dir, &File { sessions: vec![], lost: lost.clone() });
    lost
}

/// Writes the file on a thread a moment after the last change, so a burst of changes is one write.
pub(crate) struct Store {
    tx: Sender<()>,
}

impl Store {
    /// `snapshot` says what the file holds now (None: don't write, the server is stopping).
    pub fn start(dir: PathBuf, snapshot: impl Fn() -> Option<File> + Send + 'static) -> Store {
        let (tx, rx) = mpsc::channel::<()>();
        let _ = std::thread::Builder::new().name("bro-host-store".into()).spawn(move || {
            while rx.recv().is_ok() {
                std::thread::sleep(Duration::from_millis(150));
                while rx.try_recv().is_ok() {}
                if let Some(file) = snapshot() {
                    let _ = write(&dir, &file);
                }
            }
        });
        Store { tx }
    }

    pub fn changed(&self) {
        let _ = self.tx.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_server_left_running_comes_back_once_as_lost() {
        let dir = tempfile::tempdir().unwrap();
        let kept = |id: &str, keep: Value| Kept { id: id.into(), keep, rows: 24, cols: 80 };
        write(dir.path(), &File { sessions: vec![kept("a", serde_json::json!({"name": "a"})), kept("x", Value::Null)], lost: vec![] }).unwrap();
        let lost = take_over(dir.path());
        assert_eq!(lost.len(), 1, "nothing to bring back is left out: {lost:?}");
        assert_eq!(lost[0].id, "a");
        assert!(lost[0].exited && lost[0].blob["name"] == "a");
        // the next server takes over the same ones, not two copies
        let again = take_over(dir.path());
        assert_eq!(again.len(), 1);
        assert!(load(dir.path()).sessions.is_empty());
    }
}
