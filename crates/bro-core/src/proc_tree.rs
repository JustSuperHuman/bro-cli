//! Ending a session's whole process tree, not just its first process.
//!
//! Killing the program a session started leaves behind what it started in turn: on Windows a
//! `TerminateProcess` ends one process, and agents start MCP servers, shells and dev servers with
//! their own hidden consoles, so closing the session's console doesn't reach them.
//!
//! Windows: the tree is walked from the session's process (parent ids, checked against start times
//! so a reused id can't pull in an unrelated process) and every console program in it is ended.
//! A GUI app the session opened (a browser for a login, an editor) and everything under it is left
//! running: closing a terminal shouldn't close windows you're using.
//!
//! Unix: the session's process is its own session and process-group leader (the pty made it so),
//! so the whole group gets SIGKILL. Anything that moved itself into another session or group
//! (`setsid`, `nohup … &` in some shells) is beyond reach, as it is in any terminal.
//!
//! [`track`] / [`untrack`] keep the live sessions' process ids so [`kill_all_tracked`] can end them
//! all when bro is closed without going through its own quit (the window's close button).
use parking_lot::Mutex;

static TRACKED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Remember a live session's process for [`kill_all_tracked`].
pub fn track(pid: u32) {
    let mut t = TRACKED.lock();
    if !t.contains(&pid) {
        t.push(pid);
    }
}

/// Forget it (its session ended).
pub fn untrack(pid: u32) {
    TRACKED.lock().retain(|p| *p != pid);
}

/// End every tracked session's tree.
pub fn kill_all_tracked() {
    let pids: Vec<u32> = std::mem::take(&mut *TRACKED.lock());
    for pid in pids {
        kill_tree(pid);
    }
}

/// End `root` and the console programs it started (see the module docs). Best effort.
pub fn kill_tree(root: u32) {
    untrack(root);
    imp::kill_tree(root);
}

/// The processes to end for `root`, from `(pid, parent, created, gui)` rows: `root` first, then its
/// descendants, skipping GUI processes and everything below them. A row only counts as a child
/// when it started after its parent (a parent id can be reused after the parent exits).
#[cfg_attr(not(windows), allow(dead_code))]
fn plan(root: u32, procs: &[(u32, u32, u64, bool)]) -> Vec<u32> {
    let Some(&(_, _, root_created, _)) = procs.iter().find(|p| p.0 == root) else { return vec![root] };
    let mut out = vec![root];
    let mut queue = vec![(root, root_created)];
    while let Some((parent, parent_created)) = queue.pop() {
        for &(pid, ppid, created, gui) in procs {
            if ppid != parent || pid == parent || created < parent_created || out.contains(&pid) {
                continue;
            }
            if gui {
                continue;
            }
            out.push(pid);
            queue.push((pid, created));
        }
    }
    out
}

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
    };

    /// (pid, parent pid) of every process.
    fn snapshot() -> Vec<(u32, u32)> {
        let mut out = vec![];
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut e) != 0 {
                loop {
                    out.push((e.th32ProcessID, e.th32ParentProcessID));
                    if Process32NextW(snap, &mut e) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }

    fn open(pid: u32, access: u32) -> Option<HANDLE> {
        let h = unsafe { OpenProcess(access, 0, pid) };
        (!h.is_null()).then_some(h)
    }

    /// When it started (FILETIME ticks) and whether its executable is a GUI program.
    fn details(pid: u32) -> Option<(u64, bool)> {
        let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut times: [FILETIME; 4] = unsafe { std::mem::zeroed() };
        let [created, exited, kernel, user] = &mut times;
        let ok = unsafe { GetProcessTimes(h, created, exited, kernel, user) } != 0;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let named = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0;
        unsafe { CloseHandle(h) };
        if !ok {
            return None;
        }
        let created = (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime);
        let gui = named && super::is_gui_exe(Path::new(&String::from_utf16_lossy(&buf[..len as usize])));
        Some((created, gui))
    }

    pub fn kill_tree(root: u32) {
        let rows = snapshot();
        // everything that names a process in the tree as its parent; plan() sorts out reused ids
        let mut ids = vec![root];
        let mut i = 0;
        while i < ids.len() {
            let parent = ids[i];
            for &(pid, ppid) in &rows {
                if ppid == parent && pid != parent && !ids.contains(&pid) {
                    ids.push(pid);
                }
            }
            i += 1;
        }
        let procs: Vec<(u32, u32, u64, bool)> = ids
            .iter()
            .filter_map(|&pid| {
                let ppid = rows.iter().find(|r| r.0 == pid)?.1;
                details(pid).map(|(created, gui)| (pid, ppid, created, gui))
            })
            .collect();
        for pid in super::plan(root, &procs) {
            if let Some(h) = open(pid, PROCESS_TERMINATE) {
                unsafe {
                    TerminateProcess(h, 1);
                    CloseHandle(h);
                }
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    pub fn kill_tree(root: u32) {
        let Ok(pid) = i32::try_from(root) else { return };
        if pid <= 1 {
            return;
        }
        unsafe {
            // the pty made it a session and group leader: its group is everything it runs in the foreground
            if libc::getpgid(pid) == pid {
                libc::killpg(pid, libc::SIGKILL);
            }
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    pub fn kill_tree(_root: u32) {}
}

/// The executable is a Windows GUI program (PE subsystem 2) rather than a console one.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_gui_exe(path: &std::path::Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let read = || -> std::io::Result<bool> {
        let mut f = std::fs::File::open(path)?;
        let mut dos = [0u8; 64];
        f.read_exact(&mut dos)?;
        if &dos[..2] != b"MZ" {
            return Ok(false);
        }
        let pe = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]) as u64;
        // "PE\0\0", the 20-byte file header, then the optional header: Subsystem is at offset 68
        f.seek(SeekFrom::Start(pe))?;
        let mut sig = [0u8; 4];
        f.read_exact(&mut sig)?;
        if &sig != b"PE\0\0" {
            return Ok(false);
        }
        f.seek(SeekFrom::Start(pe + 4 + 20 + 68))?;
        let mut sub = [0u8; 2];
        f.read_exact(&mut sub)?;
        Ok(u16::from_le_bytes(sub) == 2)
    };
    read().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_the_tree_without_gui_apps_or_reused_ids() {
        // 10 → 11 (shell) → 12 (node server); 10 → 13 (browser, GUI) → 14 (its helper)
        // 15 claims parent 11 but started before it: 11's id was reused, so it isn't ours
        let procs = [(10, 1, 100, false), (11, 10, 110, false), (12, 11, 120, false), (13, 10, 115, true), (14, 13, 130, false), (15, 11, 50, false), (16, 99, 140, false)];
        let mut got = plan(10, &procs);
        got.sort();
        assert_eq!(got, vec![10, 11, 12]);
        assert_eq!(plan(7, &procs), vec![7], "an unknown root is still ended");
    }

    #[test]
    fn tracking_forgets_ended_sessions() {
        track(4_000_000_001);
        track(4_000_000_001);
        assert_eq!(TRACKED.lock().iter().filter(|p| **p == 4_000_000_001).count(), 1);
        untrack(4_000_000_001);
        assert!(!TRACKED.lock().contains(&4_000_000_001));
    }

    #[cfg(windows)]
    #[test]
    fn reads_the_pe_subsystem() {
        let win = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap();
        assert!(is_gui_exe(&win.join("notepad.exe")) || is_gui_exe(&win.join("System32").join("notepad.exe")));
        assert!(!is_gui_exe(&win.join("System32").join("cmd.exe")));
        assert!(!is_gui_exe(std::path::Path::new("Z:\\no\\such.exe")));
    }

    /// A shell that starts a grandchild with its own (hidden) console, which closing the shell's
    /// console doesn't reach: ending the tree ends it.
    #[cfg(windows)]
    #[test]
    fn ends_grandchildren_with_their_own_console() {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("grandchild.pid");
        // full paths: other tests swap PATH while this one runs
        let ps = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let script = format!(
            "$p = Start-Process '{}' -ArgumentList '-NoProfile','-Command','Start-Sleep 120' -WindowStyle Hidden -PassThru; Set-Content -Path '{}' -Value $p.Id; Start-Sleep 120",
            ps.display(),
            marker.display()
        );
        let mut shell = Command::new(&ps).args(["-NoProfile", "-Command", &script]).creation_flags(0x0800_0000).spawn().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let grandchild: u32 = loop {
            if let Some(pid) = std::fs::read_to_string(&marker).ok().and_then(|s| s.trim().parse().ok()) {
                break pid;
            }
            assert!(std::time::Instant::now() < deadline, "the grandchild never started");
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let alive = |pid: u32| imp_alive(pid);
        assert!(alive(grandchild));
        kill_tree(shell.id());
        let _ = shell.wait();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!alive(grandchild), "the grandchild outlived its session");
    }

    #[cfg(windows)]
    fn imp_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            ok && code == STILL_ACTIVE as u32
        }
    }
}
