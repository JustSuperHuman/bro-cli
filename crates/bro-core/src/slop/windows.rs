//! What Windows says about a session's processes: their parents, start and CPU times, memory, paths and (for the
//! ones that need it) command lines; which have a window on screen; which listen on a TCP port and who is connected
//! to it; which temp folders running browsers and Remotion hold. And ending a group with whatever it started since.

use super::temp::InUse;
use super::{Around, Proc, Target};
use crate::proc_tree::Scope;
use std::collections::{HashMap, HashSet};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{FILETIME, HWND, INVALID_HANDLE_VALUE, LPARAM, UNICODE_STRING};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ, QueryFullProcessImageNameW};
use windows_sys::core::BOOL;

const BROWSERS: &[&str] = &["chrome.exe", "chrome-headless-shell.exe", "headless_shell.exe", "chromium.exe", "msedge.exe", "brave.exe", "firefox.exe"];
const RUNNERS: &[&str] = &["node.exe", "bun.exe", "deno.exe", "python.exe", "pythonw.exe", "py.exe", "ruby.exe"];

/// Every target's processes, in the targets' order. `cache` keeps each process's path and command line.
pub(super) fn snapshot(targets: &[Target], cache: &mut HashMap<(u32, u64), (String, String)>) -> Vec<Vec<Proc>> {
    let table = toolhelp();
    targets
        .iter()
        .map(|t| {
            let mut procs: Vec<Proc> = t
                .scope
                .pids()
                .into_iter()
                .filter_map(|pid| {
                    let (ppid, exe) = table.get(&pid)?;
                    inspect(pid, *ppid, exe, cache)
                })
                .collect();
            // a parent outside the tree that's still alive (and older than its child: not a stranger on its PID)
            // still has its child
            let inside: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
            for p in &mut procs {
                if !inside.contains(&p.ppid) && table.contains_key(&p.ppid) {
                    // one bro can't read counts as the parent: leave it be
                    p.held = started(p.ppid).is_none_or(|s| s <= p.started);
                }
            }
            procs
        })
        .collect()
}

/// pid → (parent, file name in lower case), for every process on the system.
fn toolhelp() -> HashMap<u32, (u32, String)> {
    let mut out = HashMap::new();
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return out;
    }
    let snap = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snap.as_raw_handle(), &mut e) } != 0;
    while more {
        let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
        out.insert(e.th32ProcessID, (e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len]).to_ascii_lowercase()));
        more = unsafe { Process32NextW(snap.as_raw_handle(), &mut e) } != 0;
    }
    out
}

fn filetime(t: FILETIME) -> u64 {
    (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64
}

fn open(pid: u32) -> Option<OwnedHandle> {
    let mut raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if raw.is_null() {
        raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    }
    (!raw.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(raw) })
}

/// (started, CPU ms) of a process.
fn times(h: &OwnedHandle) -> Option<(u64, u64)> {
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    (unsafe { GetProcessTimes(h.as_raw_handle(), &mut created, &mut exited, &mut kernel, &mut user) } != 0).then(|| (filetime(created), (filetime(kernel) + filetime(user)) / 10_000))
}

fn started(pid: u32) -> Option<u64> {
    times(&open(pid)?).map(|t| t.0)
}

fn inspect(pid: u32, ppid: u32, exe: &str, cache: &mut HashMap<(u32, u64), (String, String)>) -> Option<Proc> {
    let h = open(pid)?;
    let (started, cpu_ms) = times(&h)?;
    let mut mem: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let private = if unsafe { K32GetProcessMemoryInfo(h.as_raw_handle(), (&mut mem as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(), mem.cb) } != 0 {
        mem.PrivateUsage as u64
    } else {
        0
    };
    let (path, cmd) = cache
        .entry((pid, started))
        .or_insert_with(|| {
            // browsers (which process, how it was started) and script runners (which script) say something
            let cmd = if BROWSERS.contains(&exe) || RUNNERS.contains(&exe) { command_line(&h).unwrap_or_default() } else { String::new() };
            (image(&h).unwrap_or_default(), cmd)
        })
        .clone();
    Some(Proc { pid, ppid, started, cpu_ms, mem: private, exe: exe.to_string(), path, cmd, held: false })
}

fn image(h: &OwnedHandle) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    (unsafe { QueryFullProcessImageNameW(h.as_raw_handle(), 0, buf.as_mut_ptr(), &mut len) } != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// The process's command line (Windows 8.1 and later, with query-limited access).
fn command_line(h: &OwnedHandle) -> Option<String> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
    let mut words = 512usize;
    for _ in 0..3 {
        // u64s keep the UNICODE_STRING at its head aligned
        let mut buf = vec![0u64; words];
        let mut need = 0u32;
        let status = unsafe { NtQueryInformationProcess(h.as_raw_handle(), ProcessCommandLineInformation, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut need) };
        if status >= 0 {
            let us = unsafe { &*(buf.as_ptr() as *const UNICODE_STRING) };
            if us.Buffer.is_null() || us.Length == 0 {
                return None;
            }
            let chars = unsafe { std::slice::from_raw_parts(us.Buffer, us.Length as usize / 2) };
            return Some(String::from_utf16_lossy(chars));
        }
        if need as usize <= buf.len() * 8 {
            return None;
        }
        words = (need as usize).div_ceil(8) + 8;
    }
    None
}

/// End a group, then anything its processes started since the look (a renderer, a helper), a few generations deep.
/// Every process only while the session's tree holds it and only if it's the process that was seen. How many ended.
pub(super) fn end_group(scope: &Scope, members: &[&Proc]) -> u32 {
    let mut ended: HashMap<u32, u64> = HashMap::new();
    // parents first: one can't start another child once it's gone
    for p in members {
        if scope.end(p.pid, p.started) {
            ended.insert(p.pid, p.started);
        }
    }
    if ended.is_empty() {
        return 0;
    }
    for _ in 0..3 {
        let table = toolhelp();
        let mut more = false;
        for pid in scope.pids() {
            if ended.contains_key(&pid) {
                continue;
            }
            let Some(&parent_started) = table.get(&pid).and_then(|(ppid, _)| ended.get(ppid)) else { continue };
            let Some(s) = started(pid) else { continue };
            if s >= parent_started && scope.end(pid, s) {
                ended.insert(pid, s);
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    ended.len() as u32
}

/// Which processes have a window on screen, which listen on TCP ports, and who is connected to those.
pub(super) fn around() -> Around {
    let mut a = Around { windows: visible_windows(), ..Around::default() };
    tcp(&mut a);
    a
}

fn visible_windows() -> HashSet<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible};
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        let set = unsafe { &mut *(data as *mut HashSet<u32>) };
        if unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null() {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if pid != 0 {
                set.insert(pid);
            }
        }
        1
    }
    let mut set = HashSet::new();
    unsafe { EnumWindows(Some(each), &mut set as *mut HashSet<u32> as LPARAM) };
    set
}

/// An endpoint: its address as IPv6 (IPv4 as `::ffff:a.b.c.d`, as a dual-stack socket reports it) and its port.
type End = ([u8; 16], u16);

struct Conn {
    state: u32,
    pid: u32,
    local: End,
    remote: End,
}

fn tcp(a: &mut Around) {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCP_STATE_ESTAB, MIB_TCP_STATE_LISTEN, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
    };
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    let port = |p: u32| u16::from_be((p & 0xFFFF) as u16);
    let v4 = |addr: u32| {
        let mut out = [0u8; 16];
        out[10] = 0xFF;
        out[11] = 0xFF;
        out[12..].copy_from_slice(&addr.to_ne_bytes());
        out
    };
    let mut conns: Vec<Conn> = vec![];
    let t4 = table(AF_INET);
    let head4 = std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table);
    if t4.len() * 4 >= head4 {
        let base = t4.as_ptr() as *const u8;
        let n = unsafe { *(base as *const u32) } as usize;
        let rows = unsafe { base.add(head4) } as *const MIB_TCPROW_OWNER_PID;
        let room = (t4.len() * 4 - head4) / std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
        for i in 0..n.min(room) {
            let r = unsafe { std::ptr::read_unaligned(rows.add(i)) };
            conns.push(Conn { state: r.dwState, pid: r.dwOwningPid, local: (v4(r.dwLocalAddr), port(r.dwLocalPort)), remote: (v4(r.dwRemoteAddr), port(r.dwRemotePort)) });
        }
    }
    let t6 = table(AF_INET6);
    let head6 = std::mem::offset_of!(MIB_TCP6TABLE_OWNER_PID, table);
    if t6.len() * 4 >= head6 {
        let base = t6.as_ptr() as *const u8;
        let n = unsafe { *(base as *const u32) } as usize;
        let rows = unsafe { base.add(head6) } as *const MIB_TCP6ROW_OWNER_PID;
        let room = (t6.len() * 4 - head6) / std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>();
        for i in 0..n.min(room) {
            let r = unsafe { std::ptr::read_unaligned(rows.add(i)) };
            conns.push(Conn { state: r.dwState, pid: r.dwOwningPid, local: (r.ucLocalAddr, port(r.dwLocalPort)), remote: (r.ucRemoteAddr, port(r.dwRemotePort)) });
        }
    }
    let established = |c: &&Conn| c.state == MIB_TCP_STATE_ESTAB as u32;
    for c in conns.iter().filter(|c| c.state == MIB_TCP_STATE_LISTEN as u32) {
        a.listening.entry(c.pid).or_default().push(c.local.1);
    }
    // the other end of a connection on this machine is the connection whose local end is this one's remote end
    let owner: HashMap<End, u32> = conns.iter().filter(established).map(|c| (c.local, c.pid)).collect();
    for c in conns.iter().filter(established) {
        if a.listening.get(&c.pid).is_some_and(|ports| ports.contains(&c.local.1)) {
            a.clients.entry(c.pid).or_default().push(owner.get(&c.remote).copied().unwrap_or(0));
        }
    }
}

/// Every TCP connection and listener for one address family, with its owner's PID (u32s keep it aligned).
fn table(af: u32) -> Vec<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, TCP_TABLE_OWNER_PID_ALL};
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    let mut size = 0u32;
    unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, af, TCP_TABLE_OWNER_PID_ALL, 0) };
    for _ in 0..4 {
        let mut buf = vec![0u32; (size as usize + 4096).div_ceil(4)];
        let mut len = (buf.len() * 4) as u32;
        match unsafe { GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut len, 0, af, TCP_TABLE_OWNER_PID_ALL, 0) } {
            0 => return buf,
            ERROR_INSUFFICIENT_BUFFER => size = len,
            _ => return vec![],
        }
    }
    vec![]
}

/// Across the machine, not only bro's sessions: the profiles running browsers were started with, and when the
/// oldest thing of Remotion's still running started (its compositor, or a script runner whose path or command line
/// names it).
pub(super) fn temp_in_use() -> InUse {
    let mut out = InUse::default();
    let mut remotion = |h: &OwnedHandle| {
        if let Some((started, _)) = times(h) {
            let at = filetime_to_system(started);
            out.remotion = Some(out.remotion.map_or(at, |was| was.min(at)));
        }
    };
    let mut profiles = HashSet::new();
    for (pid, (_, exe)) in toolhelp() {
        let browser = BROWSERS.contains(&exe.as_str());
        let runner = RUNNERS.contains(&exe.as_str());
        if !browser && !runner && exe != "remotion.exe" {
            continue;
        }
        let Some(h) = open(pid) else { continue };
        if exe == "remotion.exe" {
            remotion(&h);
            continue;
        }
        let cmd = command_line(&h).unwrap_or_default();
        if browser {
            if let Some(p) = super::temp::profile_of(&cmd) {
                profiles.insert(p);
            }
        } else if format!("{} {cmd}", image(&h).unwrap_or_default()).to_ascii_lowercase().contains("remotion") {
            remotion(&h);
        }
    }
    out.profiles = profiles;
    out
}

/// A FILETIME (100 ns since 1601) as a `SystemTime`.
fn filetime_to_system(ft: u64) -> std::time::SystemTime {
    const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;
    std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ft.saturating_sub(UNIX_EPOCH_AS_FILETIME).saturating_mul(100))
}
