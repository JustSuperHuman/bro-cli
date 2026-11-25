//! Which processes have a file open, and which processes a pane started: how bro tells a Codex pane's own
//! rollout from a sibling's in the same folder ([`super::live::Binder`]). Codex names no process in its rollouts,
//! but it keeps the rollout it writes open for as long as the session runs.
//!
//! Windows asks the Restart Manager (slow: up to a few seconds a file, so answers are cached and asked one at a
//! time), Linux reads `/proc`. Elsewhere bro can't tell: every function answers None.
//!
//! Blocking: call from a worker thread.

use std::collections::HashSet;
use std::path::Path;

/// `root` and every process it started (and they started), None where bro can't tell.
pub fn tree(root: u32) -> Option<HashSet<u32>> {
    let parents = imp::parents()?;
    let mut out = HashSet::from([root]);
    let mut grew = true;
    while grew {
        grew = false;
        for &(pid, parent) in &parents {
            if pid != parent && out.contains(&parent) && out.insert(pid) {
                grew = true;
            }
        }
    }
    Some(out)
}

/// The processes that have `path` open, None where bro can't tell.
pub fn held_by(path: &Path) -> Option<Vec<u32>> {
    imp::held_by(path)
}

/// Whether one of `pids` has `path` open, None where bro can't tell.
pub fn held_within(path: &Path, pids: &HashSet<u32>) -> Option<bool> {
    imp::held_within(path, pids)
}

#[cfg(windows)]
mod imp {
    use parking_lot::Mutex;
    use std::collections::{HashMap, HashSet};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::LazyLock;
    use std::time::{Duration, Instant, SystemTime};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::RestartManager::{CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources, RmStartSession};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    /// A file someone had open is asked about again after this long, whatever else holds.
    const KEEP_HELD: Duration = Duration::from_secs(10 * 60);
    /// A file nobody had open stays so until it changes (most never do: a finished session's), or this long.
    const KEEP_SHUT: Duration = Duration::from_secs(6 * 60 * 60);

    /// A process by its id and start time (ids are reused).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct Holder {
        pid: u32,
        started: u64,
    }

    /// What the Restart Manager said of one file, and the file as it was then.
    struct Verdict {
        holders: Vec<Holder>,
        len: u64,
        modified: Option<SystemTime>,
        at: Instant,
    }

    static VERDICTS: LazyLock<Mutex<HashMap<PathBuf, Verdict>>> = LazyLock::new(Default::default);
    /// One Restart Manager question at a time: a pane asking what another just asked waits for that answer.
    static ASKING: Mutex<()> = Mutex::new(());

    pub fn parents() -> Option<Vec<(u32, u32)>> {
        // SAFETY: a snapshot handle we close; PROCESSENTRY32W sized as the API asks.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut out = Vec::new();
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snap, &mut entry) != 0;
            while more {
                out.push((entry.th32ProcessID, entry.th32ParentProcessID));
                more = Process32NextW(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
            Some(out)
        }
    }

    fn filetime(t: FILETIME) -> u64 {
        (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
    }

    /// Whether the process `h` names still runs: the same one (its start time matches), not one that exited while
    /// something still holds its handle.
    fn alive(h: Holder) -> bool {
        // SAFETY: a query-only handle we close.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, h.pid);
            if process.is_null() {
                return false;
            }
            let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
            let ok = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) != 0;
            CloseHandle(process);
            ok && filetime(created) == h.started && filetime(exited) == 0
        }
    }

    /// The file's length and time, to tell whether it moved since an answer.
    fn stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
        std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()))
    }

    /// An answer still good: its holders all still running, or (nobody had it open) the file unchanged since. The
    /// length tells more than the time here: Windows keeps a file's write time while it's held open.
    fn fresh(v: &Verdict, now: (u64, Option<SystemTime>)) -> bool {
        if v.holders.is_empty() { v.at.elapsed() <= KEEP_SHUT && (v.len, v.modified) == now } else { v.at.elapsed() <= KEEP_HELD && v.holders.iter().all(|h| alive(*h)) }
    }

    /// Ask the Restart Manager who has `path` open.
    fn ask(path: &Path) -> Option<Vec<Holder>> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: buffers sized as the API asks; the session is always ended.
        unsafe {
            let mut session = 0u32;
            let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
            if RmStartSession(&mut session, 0, key.as_mut_ptr()) != 0 {
                return None;
            }
            let files = [wide.as_ptr()];
            let mut out = None;
            if RmRegisterResources(session, 1, files.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) == 0 {
                let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
                for _ in 0..4 {
                    let (mut needed, mut count, mut reasons) = (0u32, infos.len() as u32, 0u32);
                    let ptr = if infos.is_empty() { std::ptr::null_mut() } else { infos.as_mut_ptr() };
                    match RmGetList(session, &mut needed, &mut count, ptr, &mut reasons) {
                        0 => {
                            let found = infos.iter().take(count as usize).map(|i| Holder { pid: i.Process.dwProcessId, started: filetime(i.Process.ProcessStartTime) });
                            out = Some(found.collect());
                            break;
                        }
                        ERROR_MORE_DATA => infos = vec![RM_PROCESS_INFO::default(); needed as usize + 2],
                        _ => break,
                    }
                }
            }
            RmEndSession(session);
            out
        }
    }

    /// Who has `path` open: the cached answer while it's good, else the Restart Manager's.
    fn holders(path: &Path) -> Option<Vec<Holder>> {
        let now = stamp(path)?;
        let cached = |verdicts: &HashMap<PathBuf, Verdict>| verdicts.get(path).filter(|v| fresh(v, now)).map(|v| v.holders.clone());
        if let Some(h) = cached(&VERDICTS.lock()) {
            return Some(h);
        }
        let _one = ASKING.lock();
        if let Some(h) = cached(&VERDICTS.lock()) {
            return Some(h);
        }
        let found = ask(path)?;
        let mut verdicts = VERDICTS.lock();
        verdicts.retain(|_, v| v.at.elapsed() <= KEEP_SHUT);
        verdicts.insert(path.to_path_buf(), Verdict { holders: found.clone(), len: now.0, modified: now.1, at: Instant::now() });
        Some(found)
    }

    pub fn held_by(path: &Path) -> Option<Vec<u32>> {
        holders(path).map(|h| h.into_iter().map(|h| h.pid).collect())
    }

    pub fn held_within(path: &Path, pids: &HashSet<u32>) -> Option<bool> {
        holders(path).map(|h| h.iter().any(|h| pids.contains(&h.pid)))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    fn pids() -> impl Iterator<Item = u32> {
        std::fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.parse().ok())
    }

    pub fn parents() -> Option<Vec<(u32, u32)>> {
        let out: Vec<(u32, u32)> = pids()
            .filter_map(|pid| {
                // `pid (comm) state ppid …`: the name may hold spaces and parentheses, so read after the last ')'
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
                let ppid = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
                Some((pid, ppid))
            })
            .collect();
        (!out.is_empty()).then_some(out)
    }

    /// Whether `pid` has `path` (canonical) open; None when its descriptors can't be read.
    fn holds(pid: u32, path: &Path) -> Option<bool> {
        let fds = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
        Some(fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|p| p == path)))
    }

    fn canonical(path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    pub fn held_by(path: &Path) -> Option<Vec<u32>> {
        let path = canonical(path)?;
        Some(pids().filter(|&pid| holds(pid, &path) == Some(true)).collect())
    }

    pub fn held_within(path: &Path, pids: &HashSet<u32>) -> Option<bool> {
        let path = canonical(path)?;
        let mut readable = false;
        for &pid in pids {
            match holds(pid, &path) {
                Some(true) => return Some(true),
                Some(false) => readable = true,
                None => {}
            }
        }
        // none of the tree's descriptors could be read: that's not a "no"
        readable.then_some(false)
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use std::collections::HashSet;
    use std::path::Path;

    pub fn parents() -> Option<Vec<(u32, u32)>> {
        None
    }
    pub fn held_by(_path: &Path) -> Option<Vec<u32>> {
        None
    }
    pub fn held_within(_path: &Path, _pids: &HashSet<u32>) -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_holds_its_root_and_the_processes_it_started() {
        let me = std::process::id();
        let Some(mine) = tree(me) else { return }; // bro can't tell here
        assert!(mine.contains(&me));
        // full paths: other tests point PATH at their sandboxes
        let program = if cfg!(windows) {
            format!("{}\\System32\\PING.EXE", std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        } else {
            "/bin/sleep".into()
        };
        let mut child = std::process::Command::new(program)
            .args(if cfg!(windows) { &["-n", "4", "127.0.0.1"][..] } else { &["3"][..] })
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mine = tree(me).unwrap();
        let _ = child.kill();
        let _ = child.wait();
        assert!(mine.contains(&child.id()), "the child is in the tree");
        assert!(!tree(child.id()).unwrap_or_default().contains(&me), "a child's tree doesn't hold its parent");
    }

    #[test]
    fn a_file_this_process_has_open_is_held_by_it() {
        let dir = tempfile::tempdir().unwrap();
        let open = dir.path().join("open.jsonl");
        let shut = dir.path().join("shut.jsonl");
        std::fs::write(&shut, "{}\n").unwrap();
        let file = std::fs::File::create(&open).unwrap();
        let me = HashSet::from([std::process::id()]);
        let Some(held) = held_within(&open, &me) else { return }; // bro can't tell here
        assert!(held);
        assert!(held_by(&open).unwrap().contains(&std::process::id()));
        assert_eq!(held_within(&shut, &me), Some(false));
        assert_eq!(held_within(&open, &HashSet::from([u32::MAX - 1])), Some(false), "another process's tree doesn't have it");
        drop(file);
    }

    /// A child process holding a file: found as its holder, and in its parent's tree. Slow (a PowerShell start and
    /// a few Restart Manager questions).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn a_childs_open_file_is_found_and_forgotten_when_it_exits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let script = format!("$f=[IO.File]::Open('{}','Open','ReadWrite','ReadWrite'); Start-Sleep 30", path.display());
        let mut child = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).spawn().unwrap();
        let mine = tree(std::process::id()).unwrap();
        assert!(mine.contains(&child.id()));
        let mut found = false;
        for _ in 0..20 {
            // the cache keeps "nobody" until the file changes: touch it so each try asks again
            std::fs::OpenOptions::new().append(true).open(&path).and_then(|mut f| std::io::Write::write_all(&mut f, b" ")).ok();
            if held_by(&path).unwrap().contains(&child.id()) {
                found = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        assert!(found, "the child holds it");
        assert_eq!(held_within(&path, &mine), Some(true));
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(held_within(&path, &mine), Some(false), "a holder that exited is asked about again");
    }

    /// Read-only, against this machine's running Codex panes: `BRO_HOLDERS_PANE` is a pane's pid,
    /// `BRO_HOLDERS_FILES` rollouts (`;`-separated). Prints which of them the pane's tree has open.
    #[test]
    #[ignore]
    fn live_pane_rollouts() {
        let (Ok(pane), Ok(files)) = (std::env::var("BRO_HOLDERS_PANE"), std::env::var("BRO_HOLDERS_FILES")) else { return };
        let mine = tree(pane.parse().unwrap()).unwrap();
        println!("tree of {pane}: {mine:?}");
        for f in files.split(';') {
            let t = std::time::Instant::now();
            println!("{f}: held by {:?}, by the pane: {:?} ({:?})", held_by(Path::new(f)), held_within(Path::new(f), &mine), t.elapsed());
        }
    }
}
