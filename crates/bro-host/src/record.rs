//! Where the running server listens: `v2-host.json` in bro's folder, written atomically by the server once it
//! accepts connections and removed when it stops. `v2-host.lock` is held for the server's lifetime, so two
//! servers never run against one folder.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Record {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub protocol: u32,
    pub version: String,
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join("v2-host.json")
}

pub(crate) fn lock_path(dir: &Path) -> PathBuf {
    dir.join("v2-host.lock")
}

/// The record a server left, if any (it may be stale: connecting tells).
pub fn read(dir: &Path) -> Option<Record> {
    serde_json::from_slice(&std::fs::read(path(dir)).ok()?).ok()
}

/// Readable by you alone: its token lets whoever holds it run programs as you.
pub(crate) fn write(dir: &Path, r: &Record) -> std::io::Result<()> {
    write_private(&path(dir), &serde_json::to_vec_pretty(r).unwrap_or_default())
}

/// Write `path` atomically, readable by you alone. On Unix the file is made that way (temp + rename, created
/// 0600); on Windows bro's folder in your profile already is.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()));
        // a leftover keeps its old mode: start from none
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        drop(f);
        std::fs::rename(&tmp, path)
    }
    #[cfg(not(unix))]
    {
        bro_core::util::atomic_write(path, bytes)
    }
}

/// Remove the record, but only while it is still ours (a newer server may have replaced it).
pub(crate) fn remove(dir: &Path, pid: u32) {
    if read(dir).is_some_and(|r| r.pid == pid) {
        let _ = std::fs::remove_file(path(dir));
    }
}
