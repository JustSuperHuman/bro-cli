//! Global push-to-talk hotkeys.
//!
//! Hotkeys are named portably and identified internally by their Windows
//! virtual-key code (the canonical id; each backend maps its native key codes to
//! it). [`machine`] is the platform-free hold/tap/chord state machine; the
//! backends feed it and act on its verdicts:
//!
//! - `windows`: `WH_KEYBOARD_LL` hook; swallows the hotkey, replays taps and
//!   chords, masks Win/Alt releases.
//! - `macos`: `CGEventTap` on a CFRunLoop thread; needs Input Monitoring (and
//!   Accessibility to swallow). Modifier hotkeys pass through (a lone Option /
//!   Cmd does nothing on macOS); F13+ keys can be swallowed.
//! - `linux`: evdev (`/dev/input/event*`), which works on X11, Wayland and the
//!   console but needs read access (the `input` group). Never swallows.

pub mod machine;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod linux;

use machine::Signal;

pub const VK_SHIFT_L: u32 = 0xA0;
pub const VK_SHIFT_R: u32 = 0xA1;
pub const VK_CONTROL_L: u32 = 0xA2;
pub const VK_CONTROL_R: u32 = 0xA3;
pub const VK_MENU_L: u32 = 0xA4;
pub const VK_MENU_R: u32 = 0xA5;
pub const VK_WIN_L: u32 = 0x5B;
pub const VK_WIN_R: u32 = 0x5C;
pub const VK_APPS: u32 = 0x5D;
pub const VK_CAPITAL: u32 = 0x14;
pub const VK_SCROLL: u32 = 0x91;
pub const VK_PAUSE: u32 = 0x13;
pub const VK_INSERT: u32 = 0x2D;
pub const VK_F13: u32 = 0x7C;
/// Unassigned VK used to "mask" a Win/Alt release so it doesn't open Start or
/// activate a menu bar (the AutoHotkey trick).
pub const VK_MASK: u32 = 0xE8;

/// Modifiers checked when the hotkey goes down: if any is already held the press
/// belongs to a chord (Ctrl+Win, ...) and is passed through untouched.
pub const MODIFIERS: [u32; 8] = [
    VK_SHIFT_L, VK_SHIFT_R, VK_CONTROL_L, VK_CONTROL_R, VK_MENU_L, VK_MENU_R, VK_WIN_L, VK_WIN_R,
];

/// Parse a hotkey name (`rwin`, `f13`, `vk:0x7C`, ...) to a VK code. Case and
/// surrounding whitespace are ignored; `-`/`_` separators are tolerated
/// (`right-win`, `scroll_lock`).
/// The platform's default hotkey: Right Win on Windows, Right Option on
/// macOS, Right Super on Linux.
pub fn default_hotkey() -> &'static str {
    if cfg!(target_os = "macos") {
        "ropt"
    } else if cfg!(windows) {
        "rwin"
    } else {
        "rsuper"
    }
}

/// Parse a hotkey name (`rwin`, `rsuper`, `ropt`, `f13`, `vk:0x7C`, ...) to its
/// canonical (Windows VK) code. Case and whitespace are ignored; `-`/`_`
/// separators are tolerated (`right-win`, `scroll_lock`).
pub fn parse_hotkey(s: &str) -> Option<u32> {
    let name: String = s.trim().to_ascii_lowercase().chars().filter(|c| !matches!(c, '-' | '_' | ' ')).collect();
    if let Some(hex) = name.strip_prefix("vk:") {
        let hex = hex.strip_prefix("0x").unwrap_or(hex);
        return u32::from_str_radix(hex, 16).ok().filter(|vk| (1..=0xFE).contains(vk));
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return (13..=24).contains(&n).then(|| VK_F13 + (n - 13));
    }
    Some(match name.as_str() {
        "rwin" | "rightwin" | "rsuper" | "rightsuper" | "rmeta" | "rightmeta" | "rcmd" | "rightcmd" | "rcommand"
        | "rightcommand" => VK_WIN_R,
        "lwin" | "leftwin" | "lsuper" | "leftsuper" | "lmeta" | "leftmeta" | "lcmd" | "leftcmd" | "lcommand"
        | "leftcommand" => VK_WIN_L,
        "rctrl" | "rightctrl" | "rcontrol" | "rightcontrol" => VK_CONTROL_R,
        "ralt" | "rightalt" | "altgr" | "ropt" | "roption" | "rightoption" => VK_MENU_R,
        "rshift" | "rightshift" => VK_SHIFT_R,
        "capslock" | "caps" => VK_CAPITAL,
        "scrolllock" | "scroll" => VK_SCROLL,
        "pause" | "break" => VK_PAUSE,
        "insert" | "ins" | "help" => VK_INSERT,
        "apps" | "menu" | "contextmenu" | "compose" => VK_APPS,
        _ => return None,
    })
}

/// The canonical code of a modifier key (either side), for chord detection.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn is_modifier(vk: u32) -> bool {
    MODIFIERS.contains(&vk)
}

/// A running hotkey listener; [`Listener::stop`] unhooks it.
pub struct Listener {
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl Listener {
    #[cfg_attr(not(any(windows, target_os = "macos", target_os = "linux")), allow(dead_code))]
    fn new(stop: impl FnOnce() + Send + 'static) -> Listener {
        Listener { stop: Some(Box::new(stop)) }
    }

    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

pub type SignalFn = Box<dyn Fn(Signal) + Send + 'static>;

/// Start the platform listener for canonical key `vk`. Returns once the hook is
/// installed, or with an error explaining what the OS refused.
pub fn listen(vk: u32, swallow: bool, min_ms: u32, signal: SignalFn) -> anyhow::Result<Listener> {
    #[cfg(windows)]
    return windows::listen(vk, swallow, min_ms, signal);
    #[cfg(target_os = "macos")]
    return macos::listen(vk, swallow, min_ms, signal);
    #[cfg(target_os = "linux")]
    return linux::listen(vk, swallow, min_ms, signal);
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (vk, swallow, min_ms, signal);
        anyhow::bail!("voice input: no global hotkey support on this platform")
    }
}

/// Keys whose scan code carries the E0 prefix: `SendInput` needs
/// `KEYEVENTF_EXTENDEDKEY` for these to be seen as the right-hand/navigation key.
pub fn is_extended(vk: u32) -> bool {
    matches!(
        vk,
        VK_WIN_L | VK_WIN_R | VK_APPS | VK_CONTROL_R | VK_MENU_R | VK_INSERT
            | 0x21..=0x28 // PgUp PgDn End Home arrows
            | 0x2E // Delete
            | 0x2C // PrintScreen
            | 0x90 // NumLock
            | 0x6F // Numpad divide
    )
}

/// Keys whose lone press+release does something (Start menu, menu bar), so a
/// release after a long pass-through hold must be masked.
pub fn needs_mask(vk: u32) -> bool {
    matches!(vk, VK_WIN_L | VK_WIN_R | VK_MENU_L | VK_MENU_R)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(parse_hotkey("rwin"), Some(0x5C));
        assert_eq!(parse_hotkey(" RWin "), Some(0x5C));
        assert_eq!(parse_hotkey("lwin"), Some(0x5B));
        assert_eq!(parse_hotkey("rctrl"), Some(0xA3));
        assert_eq!(parse_hotkey("ralt"), Some(0xA5));
        assert_eq!(parse_hotkey("rshift"), Some(0xA1));
        assert_eq!(parse_hotkey("capslock"), Some(0x14));
        assert_eq!(parse_hotkey("scroll_lock"), Some(0x91));
        assert_eq!(parse_hotkey("pause"), Some(0x13));
        assert_eq!(parse_hotkey("insert"), Some(0x2D));
        assert_eq!(parse_hotkey("apps"), Some(0x5D));
        for alias in ["rsuper", "rcmd", "rmeta", "Right-Command", "rightwin"] {
            assert_eq!(parse_hotkey(alias), Some(VK_WIN_R), "{alias}");
        }
        for alias in ["roption", "ropt", "altgr"] {
            assert_eq!(parse_hotkey(alias), Some(VK_MENU_R), "{alias}");
        }
        assert_eq!(parse_hotkey("lsuper"), Some(VK_WIN_L));
        assert!(parse_hotkey(default_hotkey()).is_some());
        assert_eq!(parse_hotkey("f13"), Some(0x7C));
        assert_eq!(parse_hotkey("F24"), Some(0x87));
    }

    #[test]
    fn vk_codes_and_rejects() {
        assert_eq!(parse_hotkey("vk:0x7C"), Some(0x7C));
        assert_eq!(parse_hotkey("VK:e8"), Some(0xE8));
        assert_eq!(parse_hotkey("vk:0x00"), None);
        assert_eq!(parse_hotkey("vk:0x1FF"), None);
        assert_eq!(parse_hotkey("vk:zz"), None);
        assert_eq!(parse_hotkey("f12"), None);
        assert_eq!(parse_hotkey("f25"), None);
        assert_eq!(parse_hotkey(""), None);
        assert_eq!(parse_hotkey("space"), None);
    }

    #[test]
    fn key_facts() {
        assert!(is_extended(VK_WIN_R) && is_extended(VK_CONTROL_R) && !is_extended(VK_CAPITAL));
        assert!(needs_mask(VK_WIN_R) && needs_mask(VK_MENU_R) && !needs_mask(VK_CONTROL_R));
    }
}
