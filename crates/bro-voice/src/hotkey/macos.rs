//! macOS backend: a `CGEventTap` on its own CFRunLoop thread.
//!
//! Modifier hotkeys (Right Option, Right Cmd, ...) arrive as `FlagsChanged`
//! events and are never swallowed: a lone modifier does nothing on macOS, and
//! passing it through keeps chords (Opt+E) working natively. Non-modifier
//! hotkeys (F13..F20, Help) are swallowed when `swallow` is set, with taps and
//! chords replayed through `CGEventPost`; that needs an active tap and so the
//! Accessibility permission, while a listen-only tap needs Input Monitoring.
//! Caps Lock is not supported (macOS reports it as a toggle, not a hold).

use super::machine::{KeyEvent, KeyState, Machine, Stroke};
use super::{Listener, MODIFIERS, SignalFn, VK_CONTROL_L, VK_CONTROL_R, VK_INSERT, VK_MENU_L, VK_MENU_R};
use super::{VK_F13, VK_SHIFT_L, VK_SHIFT_R, VK_WIN_L, VK_WIN_R, is_modifier};
use std::ffi::c_void;
use std::sync::mpsc;

type TapCallback = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventSetIntegerValueField(event: *mut c_void, field: u32, value: i64);
    fn CGEventGetFlags(event: *mut c_void) -> u64;
    fn CGEventGetTimestamp(event: *mut c_void) -> u64;
    fn CGEventCreateKeyboardEvent(source: *mut c_void, keycode: u16, keydown: bool) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(allocator: *const c_void, port: *mut c_void, order: isize) -> *mut c_void;
    fn CFMachPortInvalidate(port: *mut c_void);
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRun();
    fn CFRunLoopStop(rl: *mut c_void);
    fn CFRelease(cf: *const c_void);
    static kCFRunLoopCommonModes: *const c_void;
}

const HID_EVENT_TAP: u32 = 0;
const SESSION_EVENT_TAP: u32 = 1;
const HEAD_INSERT: u32 = 0;
const OPTION_DEFAULT: u32 = 0;
const OPTION_LISTEN_ONLY: u32 = 1;
const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
const FIELD_KEYCODE: u32 = 9;
const FIELD_SOURCE_USER_DATA: u32 = 42;
/// `kCGEventSourceUserData` marker on events we post ("BROV").
const SENTINEL: i64 = 0x4252_4F56;
/// Non-hotkey keys get ids above the VK range so they can't collide.
const NATIVE: u32 = 0x1_0000;

/// Canonical (VK) code -> macOS virtual keycode.
fn native(vk: u32) -> Option<u16> {
    Some(match vk {
        VK_WIN_R => 0x36,
        VK_WIN_L => 0x37,
        VK_SHIFT_L => 0x38,
        VK_SHIFT_R => 0x3C,
        VK_MENU_L => 0x3A,
        VK_MENU_R => 0x3D,
        VK_CONTROL_L => 0x3B,
        VK_CONTROL_R => 0x3E,
        VK_INSERT => 0x72, // Help
        v if (VK_F13..VK_F13 + 8).contains(&v) => [0x69, 0x6B, 0x71, 0x6A, 0x40, 0x4F, 0x50, 0x5A][(v - VK_F13) as usize],
        _ => return None,
    })
}

/// Side-specific device flag (NX_DEVICE*KEYMASK) for a modifier keycode.
fn device_bit(keycode: u16) -> Option<u64> {
    Some(match keycode {
        0x36 => 0x10,   // right cmd
        0x37 => 0x08,   // left cmd
        0x38 => 0x02,   // left shift
        0x3C => 0x04,   // right shift
        0x3A => 0x20,   // left option
        0x3D => 0x40,   // right option
        0x3B => 0x01,   // left control
        0x3E => 0x2000, // right control
        _ => return None,
    })
}

struct Ctx {
    machine: Machine,
    signal: SignalFn,
    tap: *mut c_void,
    /// Keycode -> canonical code for the hotkey and the modifiers.
    hotkey: u16,
    vetoing_bits: u64,
}

fn post(strokes: &[Stroke], hotkey_vk: u32, hotkey: u16) {
    for s in strokes {
        let code = if s.vk & NATIVE != 0 { s.scan as u16 } else if s.vk == hotkey_vk { hotkey } else { continue };
        // SAFETY: CoreGraphics calls on an event we create and release here.
        unsafe {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null_mut(), code, !s.up);
            if ev.is_null() {
                continue;
            }
            CGEventSetIntegerValueField(ev, FIELD_SOURCE_USER_DATA, SENTINEL);
            CGEventPost(HID_EVENT_TAP, ev);
            CFRelease(ev);
        }
    }
}

unsafe extern "C" fn tap_callback(_proxy: *mut c_void, ty: u32, event: *mut c_void, user: *mut c_void) -> *mut c_void {
    // SAFETY: `user` is the Ctx owned by the tap thread for the tap's lifetime.
    let ctx = unsafe { &mut *(user as *mut Ctx) };
    if ty == TAP_DISABLED_BY_TIMEOUT || ty == TAP_DISABLED_BY_USER_INPUT {
        unsafe { CGEventTapEnable(ctx.tap, true) };
        return event;
    }
    if !matches!(ty, KEY_DOWN | KEY_UP | FLAGS_CHANGED) {
        return event;
    }
    // SAFETY: `event` is a live CGEvent for the duration of the callback.
    let (code, flags, time, ours) = unsafe {
        (
            CGEventGetIntegerValueField(event, FIELD_KEYCODE) as u16,
            CGEventGetFlags(event),
            (CGEventGetTimestamp(event) / 1_000_000) as u32,
            CGEventGetIntegerValueField(event, FIELD_SOURCE_USER_DATA) == SENTINEL,
        )
    };
    if ours {
        return event;
    }
    let down = match ty {
        KEY_DOWN => true,
        KEY_UP => false,
        _ => match device_bit(code) {
            Some(bit) => flags & bit != 0,
            None => return event, // caps lock / fn toggles
        },
    };
    let is_hotkey = code == ctx.hotkey;
    let vk = if is_hotkey { ctx.machine.hotkey() } else { NATIVE | code as u32 };
    let os = KeyState {
        modifier_down: is_hotkey && down && flags & ctx.vetoing_bits != 0,
        hotkey_down: false,
    };
    let out = ctx.machine.on_key(KeyEvent { vk, scan: code as u32, extended: false, down, time }, os);
    if let Some(signal) = out.signal {
        (ctx.signal)(signal);
    }
    if !out.inject.is_empty() {
        // Posted events queue behind the current one, so a swallowed key is
        // replaced in order.
        post(&out.inject, ctx.machine.hotkey(), ctx.hotkey);
    }
    if out.swallow { std::ptr::null_mut() } else { event }
}

struct SendPtr(*mut c_void);
// SAFETY: a CFRunLoopRef may be stopped from any thread.
unsafe impl Send for SendPtr {}

pub fn listen(vk: u32, swallow: bool, min_ms: u32, signal: SignalFn) -> anyhow::Result<Listener> {
    let hotkey = native(vk).ok_or_else(|| anyhow::anyhow!("that voice hotkey has no macOS key; try ropt, rcmd or f13"))?;
    // A lone modifier does nothing on macOS: never swallow those.
    let swallow = swallow && !is_modifier(vk);
    let machine = Machine::new(vk, swallow, min_ms).with_mask(false);
    let vetoing_bits = MODIFIERS
        .iter()
        .filter(|&&m| machine.vetoes(m))
        .filter_map(|&m| native(m).and_then(device_bit))
        .fold(0u64, |acc, bit| acc | bit);
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<SendPtr, String>>(1);
    std::thread::Builder::new().name("bro-voice-tap".into()).spawn(move || {
        let ctx = Box::into_raw(Box::new(Ctx { machine, signal, tap: std::ptr::null_mut(), hotkey, vetoing_bits }));
        // SAFETY: CoreFoundation/CoreGraphics setup; `ctx` outlives the run loop
        // and is freed after the tap is invalidated.
        unsafe {
            let mask = (1u64 << KEY_DOWN) | (1u64 << KEY_UP) | (1u64 << FLAGS_CHANGED);
            let options = if swallow { OPTION_DEFAULT } else { OPTION_LISTEN_ONLY };
            let tap = CGEventTapCreate(SESSION_EVENT_TAP, HEAD_INSERT, options, mask, tap_callback, ctx as *mut c_void);
            if tap.is_null() {
                if !CGPreflightListenEventAccess() {
                    CGRequestListenEventAccess();
                }
                let need = if swallow { "Input Monitoring and Accessibility" } else { "Input Monitoring" };
                let _ = ready_tx.send(Err(format!(
                    "macOS blocked the voice hotkey: allow your terminal app under System Settings > Privacy & Security > {need}, then restart bro"
                )));
                drop(Box::from_raw(ctx));
                return;
            }
            (*ctx).tap = tap;
            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
            let rl = CFRunLoopGetCurrent();
            CFRunLoopAddSource(rl, source, kCFRunLoopCommonModes);
            CGEventTapEnable(tap, true);
            let _ = ready_tx.send(Ok(SendPtr(rl)));
            CFRunLoopRun();
            CGEventTapEnable(tap, false);
            CFMachPortInvalidate(tap);
            CFRelease(source);
            CFRelease(tap);
            drop(Box::from_raw(ctx));
        }
    })?;
    let rl = match ready_rx.recv() {
        Ok(Ok(rl)) => rl,
        Ok(Err(e)) => anyhow::bail!(e),
        Err(_) => anyhow::bail!("event tap thread exited during setup"),
    };
    Ok(Listener::new(move || {
        let rl = rl;
        // SAFETY: stopping the tap thread's run loop; it cleans up and exits.
        unsafe { CFRunLoopStop(rl.0) };
    }))
}
