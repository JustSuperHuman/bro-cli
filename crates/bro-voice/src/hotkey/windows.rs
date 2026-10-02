//! The global `WH_KEYBOARD_LL` hook on its own message-loop thread.
//!
//! The hook proc only runs the [`Machine`] (a few comparisons), forwards signals
//! to the worker over a channel and queues re-injections; the queued keystrokes
//! are sent by the message loop after the proc has returned, so ordering against
//! the swallowed event is deterministic. Our own `SendInput` events carry
//! [`SENTINEL`] in `dwExtraInfo` and are ignored by the proc.

use super::machine::{KeyEvent, KeyState, Machine, Stroke};
use super::{Listener, MODIFIERS, SignalFn};
use std::cell::RefCell;
use std::sync::mpsc;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC,
    MapVirtualKeyW, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

/// `dwExtraInfo` marker on keystrokes we inject ("BROV").
pub const SENTINEL: usize = 0x4252_4F56;
const WM_INJECT: u32 = WM_APP + 0x42;

struct Ctx {
    machine: Machine,
    signal: SignalFn,
    pending: Vec<Stroke>,
    tid: u32,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

fn is_down(vk: u32) -> bool {
    // SAFETY: plain Win32 query.
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, lparam points at a KBDLLHOOKSTRUCT.
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let ours = kb.flags & LLKHF_INJECTED != 0 && kb.dwExtraInfo == SENTINEL;
        if !ours {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let ev = KeyEvent { vk: kb.vkCode, scan: kb.scanCode, extended: kb.flags & LLKHF_EXTENDED != 0, down, time: kb.time };
            let swallow = CTX.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else { return false };
                let Some(ctx) = guard.as_mut() else { return false };
                let hotkey_event = ev.vk == ctx.machine.hotkey();
                let os = KeyState {
                    modifier_down: hotkey_event && down && MODIFIERS.iter().any(|&m| ctx.machine.vetoes(m) && is_down(m)),
                    hotkey_down: hotkey_event && is_down(ev.vk),
                };
                let out = ctx.machine.on_key(ev, os);
                if let Some(signal) = out.signal {
                    (ctx.signal)(signal);
                }
                if !out.inject.is_empty() {
                    ctx.pending.extend(out.inject);
                    // SAFETY: posting to our own thread's queue.
                    unsafe { PostThreadMessageW(ctx.tid, WM_INJECT, 0, 0) };
                }
                out.swallow
            });
            if swallow {
                return 1;
            }
        }
    }
    // SAFETY: forwarding the hook chain unchanged.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

fn send(strokes: &[Stroke]) {
    let inputs: Vec<INPUT> = strokes
        .iter()
        .map(|s| {
            // SAFETY: plain Win32 query.
            let scan = if s.scan != 0 { s.scan } else { unsafe { MapVirtualKeyW(s.vk, MAPVK_VK_TO_VSC) } };
            let mut flags = 0;
            if s.extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if s.up {
                flags |= KEYEVENTF_KEYUP;
            }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT { wVk: s.vk as u16, wScan: scan as u16, dwFlags: flags, time: 0, dwExtraInfo: SENTINEL },
                },
            }
        })
        .collect();
    if !inputs.is_empty() {
        // SAFETY: `inputs` is a valid array of INPUT for the call's duration.
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    }
}

/// Install the hook on a new thread; returns once it is installed (or failed).
pub fn listen(vk: u32, swallow: bool, min_ms: u32, signal: SignalFn) -> anyhow::Result<Listener> {
    let machine = Machine::new(vk, swallow, min_ms).with_mask(true);
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(1);
    let thread = std::thread::Builder::new().name("bro-voice-hook".into()).spawn(move || {
        // SAFETY: standard message-loop thread setup; all handles are owned here.
        unsafe {
            let tid = GetCurrentThreadId();
            let mut msg: MSG = std::mem::zeroed();
            // Create the thread's queue so PostThreadMessageW works immediately.
            PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
            CTX.with(|c| *c.borrow_mut() = Some(Ctx { machine, signal, pending: Vec::new(), tid }));
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(std::ptr::null()), 0);
            if hook.is_null() {
                let _ = ready_tx.send(Err(format!("SetWindowsHookExW failed: {}", std::io::Error::last_os_error())));
                return;
            }
            let _ = ready_tx.send(Ok(tid));
            loop {
                let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
                if r == 0 || r == -1 || msg.message == WM_QUIT {
                    break;
                }
                if msg.message == WM_INJECT {
                    let strokes = CTX.with(|c| c.borrow_mut().as_mut().map(|ctx| std::mem::take(&mut ctx.pending)).unwrap_or_default());
                    send(&strokes);
                }
            }
            UnhookWindowsHookEx(hook);
            CTX.with(|c| *c.borrow_mut() = None);
        }
    })?;
    let tid = match ready_rx.recv() {
        Ok(Ok(tid)) => tid,
        Ok(Err(e)) => anyhow::bail!(e),
        Err(_) => anyhow::bail!("keyboard hook thread exited during setup"),
    };
    Ok(Listener::new(move || {
        // SAFETY: posting WM_QUIT to the hook thread we created.
        unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
        if thread.thread().id() != std::thread::current().id() {
            let _ = thread.join();
        }
    }))
}
