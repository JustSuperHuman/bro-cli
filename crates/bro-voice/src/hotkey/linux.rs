//! Linux backend: raw evdev key events from `/dev/input/event*`.
//!
//! evdev sits below the display server, so it works the same on X11, Wayland
//! (which has no global key hook) and the console, but it needs read access to
//! the device nodes (the `input` group or a udev `uaccess` rule). It can only
//! observe, never swallow: the hotkey always reaches the focused app / desktop.
//! Keyboards plugged in later are picked up by a periodic rescan.

use super::machine::{KeyEvent, KeyState, Machine};
use super::{Listener, MODIFIERS, SignalFn, VK_APPS, VK_CAPITAL, VK_CONTROL_L, VK_CONTROL_R, VK_F13, VK_INSERT};
use super::{VK_MENU_L, VK_MENU_R, VK_PAUSE, VK_SCROLL, VK_SHIFT_L, VK_SHIFT_R, VK_WIN_L, VK_WIN_R};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const EV_KEY: u16 = 0x01;
/// Codes from here up are mouse / touch buttons, not keys.
const BTN_MISC: u16 = 0x100;
const NATIVE: u32 = 0x1_0000;
const RESCAN: Duration = Duration::from_secs(2);

/// Canonical (VK) code -> evdev `KEY_*` code.
fn native(vk: u32) -> Option<u16> {
    Some(match vk {
        VK_WIN_L => 125,
        VK_WIN_R => 126,
        VK_CONTROL_L => 29,
        VK_CONTROL_R => 97,
        VK_MENU_L => 56,
        VK_MENU_R => 100,
        VK_SHIFT_L => 42,
        VK_SHIFT_R => 54,
        VK_CAPITAL => 58,
        VK_SCROLL => 70,
        VK_PAUSE => 119,
        VK_INSERT => 110,
        VK_APPS => 127, // KEY_COMPOSE, the menu key
        v if (VK_F13..VK_F13 + 12).contains(&v) => 183 + (v - VK_F13) as u16,
        _ => return None,
    })
}

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    kind: u16,
    code: u16,
    value: i32,
}

struct Devices {
    open: HashMap<PathBuf, File>,
    denied: usize,
}

fn scan(devices: &mut Devices) {
    let Ok(dir) = std::fs::read_dir("/dev/input") else { return };
    devices.denied = 0;
    for entry in dir.flatten() {
        let path = entry.path();
        let is_event = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event"));
        if !is_event || devices.open.contains_key(&path) {
            continue;
        }
        match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(&path) {
            Ok(file) => {
                devices.open.insert(path, file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => devices.denied += 1,
            Err(_) => {}
        }
    }
}

pub fn listen(vk: u32, _swallow: bool, min_ms: u32, signal: SignalFn) -> anyhow::Result<Listener> {
    let hotkey = native(vk).ok_or_else(|| anyhow::anyhow!("that voice hotkey has no Linux key code"))?;
    let mut devices = Devices { open: HashMap::new(), denied: 0 };
    scan(&mut devices);
    if devices.open.is_empty() {
        if devices.denied > 0 {
            anyhow::bail!(
                "voice hotkey needs read access to /dev/input/event*: run `sudo usermod -aG input $USER` and log in again (or add a udev uaccess rule)"
            );
        }
        anyhow::bail!("voice hotkey: no input devices under /dev/input (container or remote session?)");
    }
    // evdev can't swallow, and there is nothing to mask.
    let mut machine = Machine::new(vk, false, min_ms).with_mask(false);
    let vetoing: HashSet<u16> = MODIFIERS.iter().filter(|&&m| machine.vetoes(m)).filter_map(|&m| native(m)).collect();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();
    let epoch = Instant::now();
    std::thread::Builder::new().name("bro-voice-evdev".into()).spawn(move || {
        let mut held: HashSet<u16> = HashSet::new();
        let mut last_scan = Instant::now();
        let mut buf = vec![0u8; std::mem::size_of::<InputEvent>() * 64];
        while !stop_flag.load(Ordering::Acquire) {
            if last_scan.elapsed() >= RESCAN {
                scan(&mut devices);
                last_scan = Instant::now();
            }
            let paths: Vec<PathBuf> = devices.open.keys().cloned().collect();
            let mut fds: Vec<libc::pollfd> = paths
                .iter()
                .map(|p| libc::pollfd { fd: devices.open[p].as_raw_fd(), events: libc::POLLIN, revents: 0 })
                .collect();
            // SAFETY: `fds` is a valid pollfd array for the call.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 250) };
            if n <= 0 {
                continue;
            }
            for (path, pfd) in paths.iter().zip(&fds) {
                if pfd.revents == 0 {
                    continue;
                }
                if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    devices.open.remove(path); // unplugged
                    continue;
                }
                let Some(file) = devices.open.get_mut(path) else { continue };
                let mut gone = false;
                loop {
                    let read = match file.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            gone = true;
                            break;
                        }
                    };
                    for chunk in buf[..read].chunks_exact(std::mem::size_of::<InputEvent>()) {
                        // SAFETY: the kernel writes whole `input_event` records.
                        let ev: InputEvent = unsafe { std::ptr::read_unaligned(chunk.as_ptr() as *const InputEvent) };
                        if ev.kind != EV_KEY || ev.code >= BTN_MISC {
                            continue;
                        }
                        let down = ev.value != 0; // 1 press, 2 autorepeat, 0 release
                        let is_hotkey = ev.code == hotkey;
                        let modifier_down = is_hotkey && down && held.iter().any(|c| vetoing.contains(c));
                        if down {
                            held.insert(ev.code);
                        } else {
                            held.remove(&ev.code);
                        }
                        let vk = if is_hotkey { vk } else { NATIVE | ev.code as u32 };
                        let time = epoch.elapsed().as_millis() as u32;
                        let out = machine.on_key(
                            KeyEvent { vk, scan: ev.code as u32, extended: false, down, time },
                            KeyState { modifier_down, hotkey_down: false },
                        );
                        if let Some(s) = out.signal {
                            signal(s);
                        }
                    }
                    if read < buf.len() {
                        break;
                    }
                }
                if gone {
                    devices.open.remove(path);
                }
            }
        }
    })?;
    Ok(Listener::new(move || stop.store(true, Ordering::Release)))
}
