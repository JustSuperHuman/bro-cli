//! The mouse's back/forward buttons on Windows. crossterm drops them: the JustTerminal fork sends them as the
//! keyboard's browser back/forward keys (which crossterm can't name), and a classic console window reports them
//! as extra mouse buttons (which crossterm ignores). So bro takes them off the console's input queue before
//! crossterm reads it: [`wait`] stands in for `crossterm::event::poll`.

use std::time::{Duration, Instant};
use windows_sys::Win32::System::Console::{
    GetNumberOfConsoleInputEvents, GetStdHandle, INPUT_RECORD, KEY_EVENT, MOUSE_EVENT, PeekConsoleInputW, ReadConsoleInputW,
    STD_INPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_BROWSER_BACK, VK_BROWSER_FORWARD};

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

/// What the record at the front of the queue means for back/forward: `Some(Some(back))` to act on,
/// `Some(None)` to drop (a key release, a button release), `None` when it belongs to crossterm.
fn classify(rec: &INPUT_RECORD, held: &mut u32) -> Option<Option<bool>> {
    match rec.EventType as u32 {
        KEY_EVENT => {
            // SAFETY: EventType says which union member is set.
            let key = unsafe { rec.Event.KeyEvent };
            let back = match key.wVirtualKeyCode {
                VK_BROWSER_BACK => true,
                VK_BROWSER_FORWARD => false,
                _ => return None,
            };
            Some((key.bKeyDown != 0).then_some(back))
        }
        MOUSE_EVENT => {
            // SAFETY: as above.
            let mouse = unsafe { rec.Event.MouseEvent };
            let nav = mouse.dwButtonState & (BACK_BUTTON | FORWARD_BUTTON);
            // a button press or release (not a move or wheel) of only the back/forward buttons
            if mouse.dwEventFlags != 0 || mouse.dwButtonState & OTHER_BUTTONS != 0 || nav == *held {
                return None;
            }
            let pressed = nav & !*held;
            *held = nav;
            Some(match pressed {
                0 => None,
                p if p & BACK_BUTTON != 0 => Some(true),
                _ => Some(false),
            })
        }
        _ => None,
    }
}

impl Nav {
    /// Take back/forward records off the front of the queue, calling `on_nav` for each press.
    fn take(&mut self, on_nav: &mut dyn FnMut(bool)) {
        // SAFETY: plain console calls on this process's input handle with properly sized buffers.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            loop {
                let mut rec: INPUT_RECORD = std::mem::zeroed();
                let mut n = 0u32;
                if PeekConsoleInputW(input, &mut rec, 1, &mut n) == 0 || n == 0 {
                    return;
                }
                let Some(action) = classify(&rec, &mut self.held) else { return };
                if ReadConsoleInputW(input, &mut rec, 1, &mut n) == 0 {
                    return;
                }
                if let Some(back) = action {
                    on_nav(back);
                }
            }
        }
    }

    /// Like `crossterm::event::poll(wait)` (`None` waits for good), handing back/forward to `on_nav`.
    pub fn wait(&mut self, wait: Option<Duration>, on_nav: &mut dyn FnMut(bool)) -> bool {
        let deadline = wait.map(|w| Instant::now() + w);
        loop {
            self.take(on_nav);
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
        let mut rec: INPUT_RECORD = unsafe { std::mem::zeroed() };
        rec.EventType = KEY_EVENT as u16;
        rec.Event.KeyEvent = KEY_EVENT_RECORD { bKeyDown: down as i32, wVirtualKeyCode: vk, ..unsafe { std::mem::zeroed() } };
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
        assert_eq!(classify(&key(VK_BROWSER_BACK, true), &mut held), Some(Some(true)));
        assert_eq!(classify(&key(VK_BROWSER_BACK, false), &mut held), Some(None));
        assert_eq!(classify(&key(VK_BROWSER_FORWARD, true), &mut held), Some(Some(false)));
        assert_eq!(classify(&key(0x41, true), &mut held), None);
        assert_eq!(classify(&mouse(FORWARD_BUTTON, 0), &mut held), Some(Some(false)));
        assert_eq!(classify(&mouse(FORWARD_BUTTON, 1), &mut held), None, "moves stay crossterm's");
        assert_eq!(classify(&mouse(0, 0), &mut held), Some(None), "the release is dropped");
        assert_eq!(classify(&mouse(0, 0), &mut held), None, "nothing of ours held: crossterm's");
        assert_eq!(classify(&mouse(1, 0), &mut held), None, "left button");
        assert_eq!(classify(&mouse(BACK_BUTTON, 0), &mut held), Some(Some(true)));
    }
}
