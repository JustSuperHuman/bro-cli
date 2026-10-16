//! The mouse's back/forward buttons on Windows. crossterm drops them: the JustTerminal fork sends them as the
//! keyboard's browser back/forward keys (which crossterm can't name), and a classic console window reports them
//! as extra mouse buttons (which crossterm ignores). So bro takes them off the console's input queue before
//! crossterm reads it: [`wait`] stands in for `crossterm::event::poll`. Alt pressed and released on its own
//! (crossterm drops bare modifiers) comes out the same way, for the key hints.

use crate::paste_burst::Raw;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Console::{
    GetNumberOfConsoleInputEvents, GetStdHandle, INPUT_RECORD, KEY_EVENT, LEFT_CTRL_PRESSED, MOUSE_EVENT, PeekConsoleInputW,
    RIGHT_ALT_PRESSED, ReadConsoleInputW, STD_INPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_BROWSER_BACK, VK_BROWSER_FORWARD, VK_MENU};

/// `dwButtonState` bits a console window uses for the fourth and fifth (back/forward) mouse buttons.
const BACK_BUTTON: u32 = 0x08;
const FORWARD_BUTTON: u32 = 0x10;
/// left, right and middle
const OTHER_BUTTONS: u32 = 0x07;

#[derive(Default)]
pub struct Nav {
    /// back/forward mouse buttons held, from the last record taken
    held: u32,
}

/// What the record at the front of the queue means to bro, and whether bro takes it: `(Some(raw), true)` to
/// act on, `(None, true)` to drop (a key release, a button release), `(_, false)` to leave it for crossterm.
fn classify(rec: &INPUT_RECORD, held: &mut u32) -> (Option<Raw>, bool) {
    match rec.EventType as u32 {
        KEY_EVENT => {
            // SAFETY: EventType says which union member is set.
            let key = unsafe { rec.Event.KeyEvent };
            let down = key.bKeyDown != 0;
            let back = match key.wVirtualKeyCode {
                VK_BROWSER_BACK => true,
                VK_BROWSER_FORWARD => false,
                VK_MENU => {
                    // AltGr (right alt with ctrl) types characters: not a hint
                    let state = key.dwControlKeyState;
                    if state & RIGHT_ALT_PRESSED != 0 && state & LEFT_CTRL_PRESSED != 0 {
                        return (None, false);
                    }
                    // a release carrying a character is an alt code (alt+numpad): crossterm types it
                    // SAFETY: as above.
                    let typed = unsafe { key.uChar.UnicodeChar } != 0;
                    return (Some(Raw::Alt(down)), !typed);
                }
                _ => return (None, false),
            };
            (down.then_some(Raw::Nav(back)), true)
        }
        MOUSE_EVENT => {
            // SAFETY: as above.
            let mouse = unsafe { rec.Event.MouseEvent };
            let nav = mouse.dwButtonState & (BACK_BUTTON | FORWARD_BUTTON);
            // a button press or release (not a move or wheel) of only the back/forward buttons
            if mouse.dwEventFlags != 0 || mouse.dwButtonState & OTHER_BUTTONS != 0 || nav == *held {
                return (None, false);
            }
            let pressed = nav & !*held;
            *held = nav;
            let raw = match pressed {
                0 => None,
                p if p & BACK_BUTTON != 0 => Some(Raw::Nav(true)),
                _ => Some(Raw::Nav(false)),
            };
            (raw, true)
        }
        _ => (None, false),
    }
}

impl Nav {
    /// Take bro's records off the front of the queue, calling `on_raw` for each that means something.
    fn take(&mut self, on_raw: &mut dyn FnMut(Raw)) {
        // SAFETY: plain console calls on this process's input handle with properly sized buffers.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            loop {
                let mut rec: INPUT_RECORD = std::mem::zeroed();
                let mut n = 0u32;
                if PeekConsoleInputW(input, &mut rec, 1, &mut n) == 0 || n == 0 {
                    return;
                }
                let (raw, take) = classify(&rec, &mut self.held);
                if !take {
                    // crossterm reads it next; an alt-code release still says alt is up
                    if let Some(raw) = raw {
                        on_raw(raw);
                    }
                    return;
                }
                if ReadConsoleInputW(input, &mut rec, 1, &mut n) == 0 {
                    return;
                }
                if let Some(raw) = raw {
                    on_raw(raw);
                }
            }
        }
    }

    /// Like `crossterm::event::poll(wait)` (`None` waits for good), handing bro's records to `on_raw`.
    pub fn wait(&mut self, wait: Option<Duration>, on_raw: &mut dyn FnMut(Raw)) -> bool {
        let deadline = wait.map(|w| Instant::now() + w);
        loop {
            self.take(on_raw);
            // reads at most one record without waiting: if it wasn't an event, look at the next
            if crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
                return true;
            }
            // SAFETY: as above.
            let pending = unsafe {
                let mut n = 0u32;
                GetNumberOfConsoleInputEvents(GetStdHandle(STD_INPUT_HANDLE), &mut n) != 0 && n > 0
            };
            if pending {
                continue;
            }
            let ms = match deadline {
                Some(d) => match d.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => left.as_millis().clamp(1, 1000) as u32,
                    _ => return false,
                },
                None => 1000,
            };
            // SAFETY: waiting on the console input handle until it has input (or the timeout).
            unsafe { WaitForSingleObject(GetStdHandle(STD_INPUT_HANDLE), ms) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Console::{KEY_EVENT_RECORD, MOUSE_EVENT_RECORD};

    fn key(vk: u16, down: bool) -> INPUT_RECORD {
        key_with(vk, down, 0, 0)
    }

    fn key_with(vk: u16, down: bool, state: u32, ch: u16) -> INPUT_RECORD {
        let mut rec: INPUT_RECORD = unsafe { std::mem::zeroed() };
        rec.EventType = KEY_EVENT as u16;
        let mut k = KEY_EVENT_RECORD { bKeyDown: down as i32, wVirtualKeyCode: vk, dwControlKeyState: state, ..unsafe { std::mem::zeroed() } };
        k.uChar.UnicodeChar = ch;
        rec.Event.KeyEvent = k;
        rec
    }

    fn mouse(buttons: u32, flags: u32) -> INPUT_RECORD {
        let mut rec: INPUT_RECORD = unsafe { std::mem::zeroed() };
        rec.EventType = MOUSE_EVENT as u16;
        rec.Event.MouseEvent = MOUSE_EVENT_RECORD { dwButtonState: buttons, dwEventFlags: flags, ..unsafe { std::mem::zeroed() } };
        rec
    }

    #[test]
    fn browser_keys_and_extra_mouse_buttons_navigate_and_everything_else_is_left_to_crossterm() {
        let mut held = 0;
        assert_eq!(classify(&key(VK_BROWSER_BACK, true), &mut held), (Some(Raw::Nav(true)), true));
        assert_eq!(classify(&key(VK_BROWSER_BACK, false), &mut held), (None, true));
        assert_eq!(classify(&key(VK_BROWSER_FORWARD, true), &mut held), (Some(Raw::Nav(false)), true));
        assert_eq!(classify(&key(0x41, true), &mut held), (None, false));
        assert_eq!(classify(&mouse(FORWARD_BUTTON, 0), &mut held), (Some(Raw::Nav(false)), true));
        assert_eq!(classify(&mouse(FORWARD_BUTTON, 1), &mut held), (None, false), "moves stay crossterm's");
        assert_eq!(classify(&mouse(0, 0), &mut held), (None, true), "the release is dropped");
        assert_eq!(classify(&mouse(0, 0), &mut held), (None, false), "nothing of ours held: crossterm's");
        assert_eq!(classify(&mouse(1, 0), &mut held), (None, false), "left button");
        assert_eq!(classify(&mouse(BACK_BUTTON, 0), &mut held), (Some(Raw::Nav(true)), true));
    }

    #[test]
    fn a_bare_alt_is_bros_but_altgr_and_alt_codes_stay_crossterms() {
        let mut held = 0;
        assert_eq!(classify(&key(VK_MENU, true), &mut held), (Some(Raw::Alt(true)), true));
        assert_eq!(classify(&key(VK_MENU, false), &mut held), (Some(Raw::Alt(false)), true));
        assert_eq!(classify(&key_with(VK_MENU, false, 0, 0xE9), &mut held), (Some(Raw::Alt(false)), false), "alt+0233 types a character");
        assert_eq!(classify(&key_with(VK_MENU, true, RIGHT_ALT_PRESSED | LEFT_CTRL_PRESSED, 0), &mut held), (None, false), "AltGr");
    }
}
