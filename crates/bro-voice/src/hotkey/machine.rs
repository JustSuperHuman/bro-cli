//! The hold-to-talk key state machine, kept free of Win32 so it is testable.
//!
//! The low-level hook feeds every non-self-injected key event through
//! [`Machine::on_key`] and gets back whether to swallow it, what to tell the
//! worker, and which keystrokes to re-inject (sent from the hook thread's
//! message loop, never from inside the hook proc).
//!
//! With `swallow` the hotkey's down/up never reach Windows while talking, so a
//! Win hotkey can't open Start or get "stuck". A quick tap re-injects down+up
//! (Start still opens on a tap); a chord (Win+E) cancels talking and re-injects
//! the hotkey down *followed by* the chord key, so the chord works and the
//! physical hotkey up then passes through normally. Without `swallow` a long
//! Win/Alt hold is masked on release with a dummy key so Start / the menu bar
//! don't activate.

use super::{VK_CONTROL_L, VK_MASK, VK_MENU_R, is_extended, needs_mask};

/// One key event as seen by the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u32,
    pub scan: u32,
    pub extended: bool,
    pub down: bool,
    /// Event timestamp in ms (`KBDLLHOOKSTRUCT::time`, wraps).
    pub time: u32,
}

/// Snapshot of OS key state taken in the hook (before the event is applied).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyState {
    /// Some modifier other than the hotkey is already down.
    pub modifier_down: bool,
    /// Windows believes the hotkey is down (it saw a press we meant to swallow,
    /// e.g. after a hook timeout).
    pub hotkey_down: bool,
}

/// What the worker should hear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Down,
    Up { held_ms: u32 },
    Combo,
}

/// A keystroke to synthesise with `SendInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub vk: u32,
    /// 0 = let the injector map it from `vk`.
    pub scan: u32,
    pub extended: bool,
    pub up: bool,
}

impl Stroke {
    pub fn key(vk: u32, up: bool) -> Stroke {
        Stroke { vk, scan: 0, extended: is_extended(vk), up }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub swallow: bool,
    pub signal: Option<Signal>,
    pub inject: Vec<Stroke>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    /// Talking; `since` is the down timestamp.
    Held { since: u32 },
    /// The hotkey is part of a chord; everything passes until it is released.
    Passthrough,
}

#[derive(Debug)]
pub struct Machine {
    hotkey: u32,
    swallow: bool,
    /// Mask Win/Alt releases after a pass-through hold (Windows only).
    mask: bool,
    min_ms: u32,
    state: State,
}

impl Machine {
    pub fn new(hotkey: u32, swallow: bool, min_ms: u32) -> Machine {
        Machine { hotkey, swallow, mask: cfg!(windows), min_ms, state: State::Idle }
    }

    pub fn with_mask(mut self, mask: bool) -> Machine {
        self.mask = mask;
        self
    }

    pub fn hotkey(&self) -> u32 {
        self.hotkey
    }

    /// The modifiers that should veto a press of this hotkey. AltGr is sent as
    /// LCtrl+RAlt, so LCtrl never vetoes a Right Alt hotkey.
    pub fn vetoes(&self, modifier_vk: u32) -> bool {
        modifier_vk != self.hotkey && !(self.hotkey == VK_MENU_R && modifier_vk == VK_CONTROL_L)
    }

    pub fn on_key(&mut self, ev: KeyEvent, os: KeyState) -> Outcome {
        let mut out = Outcome::default();
        if ev.vk == self.hotkey {
            match (self.state, ev.down) {
                (State::Idle, true) => {
                    if os.modifier_down {
                        self.state = State::Passthrough;
                    } else {
                        self.state = State::Held { since: ev.time };
                        out.signal = Some(Signal::Down);
                        out.swallow = self.swallow;
                    }
                }
                // Autorepeat while talking.
                (State::Held { .. }, true) => out.swallow = self.swallow,
                (State::Held { since }, false) => {
                    self.state = State::Idle;
                    let held_ms = ev.time.wrapping_sub(since);
                    let long = held_ms >= self.min_ms;
                    out.signal = Some(Signal::Up { held_ms });
                    if self.swallow && !os.hotkey_down {
                        out.swallow = true;
                        if !long {
                            out.inject = vec![Stroke::key(self.hotkey, false), Stroke::key(self.hotkey, true)];
                        }
                    } else if long && self.mask && needs_mask(self.hotkey) {
                        // Windows saw the down: mask the release so Start / the
                        // menu bar doesn't open, then deliver the up ourselves.
                        out.swallow = true;
                        out.inject = vec![
                            Stroke::key(VK_MASK, false),
                            Stroke::key(VK_MASK, true),
                            Stroke::key(self.hotkey, true),
                        ];
                    }
                }
                (State::Passthrough, false) => self.state = State::Idle,
                (State::Passthrough, true) | (State::Idle, false) => {}
            }
            return out;
        }
        if ev.down && matches!(self.state, State::Held { .. }) {
            self.state = State::Passthrough;
            out.signal = Some(Signal::Combo);
            if self.swallow && !os.hotkey_down {
                out.swallow = true;
                out.inject = vec![
                    Stroke::key(self.hotkey, false),
                    Stroke { vk: ev.vk, scan: ev.scan, extended: ev.extended, up: false },
                ];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::VK_WIN_R;


    const E: u32 = 0x45;

    fn key(vk: u32, down: bool, time: u32) -> KeyEvent {
        KeyEvent { vk, scan: 0x12, extended: false, down, time }
    }

    fn free() -> KeyState {
        KeyState::default()
    }

    #[test]
    fn long_hold_is_swallowed_without_injection() {
        let mut m = Machine::new(VK_WIN_R, true, 300);
        let o = m.on_key(key(VK_WIN_R, true, 1000), free());
        assert_eq!(o, Outcome { swallow: true, signal: Some(Signal::Down), inject: vec![] });
        // autorepeat
        let o = m.on_key(key(VK_WIN_R, true, 1500), free());
        assert_eq!(o, Outcome { swallow: true, signal: None, inject: vec![] });
        let o = m.on_key(key(VK_WIN_R, false, 3000), free());
        assert_eq!(o, Outcome { swallow: true, signal: Some(Signal::Up { held_ms: 2000 }), inject: vec![] });
    }

    #[test]
    fn tap_reinjects_the_key() {
        let mut m = Machine::new(VK_WIN_R, true, 300);
        m.on_key(key(VK_WIN_R, true, u32::MAX - 50), free());
        let o = m.on_key(key(VK_WIN_R, false, 50), free()); // timestamp wrapped
        assert_eq!(o.signal, Some(Signal::Up { held_ms: 101 }));
        assert!(o.swallow);
        assert_eq!(o.inject, vec![Stroke::key(VK_WIN_R, false), Stroke::key(VK_WIN_R, true)]);
        assert!(o.inject[0].extended);
    }

    #[test]
    fn combo_cancels_and_replays_hotkey_then_key() {
        let mut m = Machine::new(VK_WIN_R, true, 300);
        m.on_key(key(VK_WIN_R, true, 0), free());
        let o = m.on_key(key(E, true, 100), free());
        assert_eq!(o.signal, Some(Signal::Combo));
        assert!(o.swallow);
        assert_eq!(
            o.inject,
            vec![Stroke::key(VK_WIN_R, false), Stroke { vk: E, scan: 0x12, extended: false, up: false }]
        );
        // E repeat / up and the hotkey up all pass through untouched.
        assert_eq!(m.on_key(key(E, true, 150), free()), Outcome::default());
        assert_eq!(m.on_key(key(E, false, 200), free()), Outcome::default());
        assert_eq!(m.on_key(key(VK_WIN_R, false, 900), free()), Outcome::default());
        // Back to idle: next press talks again.
        assert_eq!(m.on_key(key(VK_WIN_R, true, 1000), free()).signal, Some(Signal::Down));
    }

    #[test]
    fn modifier_held_first_passes_through() {
        let mut m = Machine::new(VK_WIN_R, true, 300);
        let held = KeyState { modifier_down: true, hotkey_down: false };
        assert_eq!(m.on_key(key(VK_WIN_R, true, 0), held), Outcome::default());
        assert_eq!(m.on_key(key(VK_WIN_R, false, 900), free()), Outcome::default());
    }

    #[test]
    fn no_swallow_masks_long_win_release() {
        let mut m = Machine::new(VK_WIN_R, false, 300).with_mask(true);
        let o = m.on_key(key(VK_WIN_R, true, 0), free());
        assert_eq!(o, Outcome { swallow: false, signal: Some(Signal::Down), inject: vec![] });
        let o = m.on_key(key(VK_WIN_R, false, 1000), KeyState { modifier_down: false, hotkey_down: true });
        assert!(o.swallow);
        assert_eq!(o.inject.iter().map(|s| (s.vk, s.up)).collect::<Vec<_>>(), vec![
            (VK_MASK, false),
            (VK_MASK, true),
            (VK_WIN_R, true)
        ]);
        // a short no-swallow tap is left alone
        m.on_key(key(VK_WIN_R, true, 2000), free());
        let o = m.on_key(key(VK_WIN_R, false, 2100), free());
        assert!(!o.swallow && o.inject.is_empty());
    }

    #[test]
    fn no_mask_backends_leave_the_release_alone() {
        let mut m = Machine::new(VK_WIN_R, false, 300).with_mask(false);
        m.on_key(key(VK_WIN_R, true, 0), free());
        let o = m.on_key(key(VK_WIN_R, false, 1000), KeyState { modifier_down: false, hotkey_down: true });
        assert_eq!(o, Outcome { swallow: false, signal: Some(Signal::Up { held_ms: 1000 }), inject: vec![] });
    }

    #[test]
    fn os_saw_the_down_so_up_is_not_swallowed() {
        let mut m = Machine::new(0x7C, true, 300); // F13: no mask needed
        m.on_key(key(0x7C, true, 0), free());
        let o = m.on_key(key(0x7C, false, 1000), KeyState { modifier_down: false, hotkey_down: true });
        assert!(!o.swallow && o.inject.is_empty());
    }

    #[test]
    fn altgr_lctrl_does_not_veto_ralt() {
        let m = Machine::new(VK_MENU_R, true, 300);
        assert!(!m.vetoes(VK_CONTROL_L));
        assert!(!m.vetoes(VK_MENU_R));
        assert!(m.vetoes(crate::hotkey::VK_SHIFT_L));
        assert!(Machine::new(VK_WIN_R, true, 300).vetoes(VK_CONTROL_L));
    }
}
