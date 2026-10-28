//! The hold-to-talk key state machine, kept free of Win32 so it is testable.
//!
//! The low-level hook feeds every non-self-injected key event through
//! [`Machine::on_key`] and gets back whether to swallow it, what to tell the
//! worker, and which keystrokes to re-inject (sent from the hook thread's
//! message loop, never from inside the hook proc). While the hotkey is held the
//! hook thread also calls [`Machine::tick`], which ends a press whose release
//! never reached the hook, so talking can't stay on.
//!
//! With `swallow`, a solo Windows-key press belongs to voice even when it is
//! shorter than `min_ms`: its release is masked instead of replayed to Start.
//! Other hotkeys still replay a quick tap. A chord (Win+E) cancels talking and
//! re-injects the hotkey down *followed by* the chord key, so the chord works
//! and the physical hotkey up then passes through normally. Without `swallow`,
//! a long Win/Alt hold is masked on release with a dummy key.

use super::{VK_CONTROL_L, VK_MASK, VK_MENU_R, VK_WIN_L, VK_WIN_R, is_extended, needs_mask};

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

/// A held hotkey that autorepeated and then went quiet this long has been let go
/// without the hook seeing it (Windows drops a hook that answers late, and an
/// elevated window's keys never reach one). Repeats come every 33-400 ms.
pub const STALE_MS: u32 = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    /// Talking; `since` is the down timestamp, `last` the latest sign the key is
    /// still down (the down or an autorepeat), `repeats` whether it repeated.
    Held { since: u32, last: u32, repeats: bool },
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
                        self.state = State::Held { since: ev.time, last: ev.time, repeats: false };
                        out.signal = Some(Signal::Down);
                        out.swallow = self.swallow;
                    }
                }
                // Autorepeat while talking.
                (State::Held { since, .. }, true) => {
                    self.state = State::Held { since, last: ev.time, repeats: true };
                    out.swallow = self.swallow;
                }
                (State::Held { since, .. }, false) => {
                    self.state = State::Idle;
                    let held_ms = ev.time.wrapping_sub(since);
                    let long = held_ms >= self.min_ms;
                    out.signal = Some(Signal::Up { held_ms });
                    if self.swallow && self.mask && matches!(self.hotkey, VK_WIN_L | VK_WIN_R) {
                        // A quick Windows-key tap is still a voice press. Always
                        // close it with a masked key-up: if an earlier down
                        // escaped the hook, this releases it without Start.
                        out.swallow = true;
                        out.inject = vec![
                            Stroke::key(VK_MASK, false),
                            Stroke::key(VK_MASK, true),
                            Stroke::key(self.hotkey, true),
                        ];
                    } else if self.swallow && !os.hotkey_down {
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

    /// A press is being talked into.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn held(&self) -> bool {
        matches!(self.state, State::Held { .. })
    }

    /// Called every so often while [`Machine::held`] (`now` on the event clock):
    /// the `Up` of a press whose key autorepeated and then went quiet for
    /// [`STALE_MS`], as its lost release would have said. A key that never
    /// repeated (a remapper's bare down) can't be told from one still held, so
    /// it waits for its release; so does one the OS itself sees down.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn tick(&mut self, now: u32, os: KeyState) -> Option<Signal> {
        let State::Held { since, last, repeats } = self.state else { return None };
        if os.hotkey_down {
            self.state = State::Held { since, last: now, repeats };
            return None;
        }
        // signed: a `now` a little behind `last` is not a wrap-around
        if !repeats || (now.wrapping_sub(last) as i32) < STALE_MS as i32 {
            return None;
        }
        self.state = State::Idle;
        Some(Signal::Up { held_ms: last.wrapping_sub(since) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::{VK_F13, VK_WIN_R};


    const E: u32 = 0x45;

    fn key(vk: u32, down: bool, time: u32) -> KeyEvent {
        KeyEvent { vk, scan: 0x12, extended: false, down, time }
    }

    fn free() -> KeyState {
        KeyState::default()
    }

    #[test]
    fn long_win_hold_is_masked_on_release() {
        let mut m = Machine::new(VK_WIN_R, true, 300);
        let o = m.on_key(key(VK_WIN_R, true, 1000), free());
        assert_eq!(o, Outcome { swallow: true, signal: Some(Signal::Down), inject: vec![] });
        // autorepeat
        let o = m.on_key(key(VK_WIN_R, true, 1500), free());
        assert_eq!(o, Outcome { swallow: true, signal: None, inject: vec![] });
        let o = m.on_key(key(VK_WIN_R, false, 3000), free());
        assert_eq!(o, Outcome { swallow: true, signal: Some(Signal::Up { held_ms: 2000 }), inject: vec![
            Stroke::key(VK_MASK, false), Stroke::key(VK_MASK, true), Stroke::key(VK_WIN_R, true),
        ] });
    }

    #[test]
    fn short_win_tap_does_not_reopen_start_even_if_down_escaped() {
        let mut m = Machine::new(VK_WIN_R, true, 300);
        m.on_key(key(VK_WIN_R, true, u32::MAX - 50), free());
        let o = m.on_key(key(VK_WIN_R, false, 50), KeyState { hotkey_down: true, ..free() }); // timestamp wrapped
        assert_eq!(o.signal, Some(Signal::Up { held_ms: 101 }));
        assert!(o.swallow);
        assert_eq!(o.inject, vec![Stroke::key(VK_MASK, false), Stroke::key(VK_MASK, true), Stroke::key(VK_WIN_R, true)]);
        assert!(o.inject.iter().all(|s| s.vk != VK_WIN_R || s.up));
    }

    #[test]
    fn short_non_win_tap_still_reinjects_the_key() {
        let mut m = Machine::new(VK_F13, true, 300);
        m.on_key(key(VK_F13, true, 100), free());
        let o = m.on_key(key(VK_F13, false, 200), free());
        assert_eq!(o.inject, vec![Stroke::key(VK_F13, false), Stroke::key(VK_F13, true)]);
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

    #[test]
    fn a_held_key_gone_quiet_is_released() {
        let mut m = Machine::new(VK_F13, true, 300);
        assert_eq!(m.on_key(key(VK_F13, true, 1000), free()).signal, Some(Signal::Down));
        // it autorepeats, then its release never reaches the hook
        for t in [1500, 1533, 1566, 2000] {
            assert_eq!(m.on_key(key(VK_F13, true, t), free()), Outcome { swallow: true, signal: None, inject: vec![] });
        }
        assert_eq!(m.tick(2500, free()), None);
        assert_eq!(m.tick(2000 + STALE_MS - 1, free()), None);
        assert!(m.held());
        assert_eq!(m.tick(2000 + STALE_MS, free()), Some(Signal::Up { held_ms: 1000 }));
        assert!(!m.held());
        assert_eq!(m.tick(9000, free()), None);
        // the release turning up late changes nothing, and the next press talks again
        assert_eq!(m.on_key(key(VK_F13, false, 9000), free()), Outcome::default());
        assert_eq!(m.on_key(key(VK_F13, true, 10_000), free()).signal, Some(Signal::Down));
    }

    #[test]
    fn a_key_that_never_repeated_waits_for_its_release() {
        let mut m = Machine::new(VK_F13, true, 300);
        m.on_key(key(VK_F13, true, 0), free());
        assert_eq!(m.tick(60_000, free()), None);
        assert!(m.held());
        assert_eq!(m.on_key(key(VK_F13, false, 61_000), free()).signal, Some(Signal::Up { held_ms: 61_000 }));
    }

    #[test]
    fn a_key_windows_sees_down_is_still_held() {
        // repeats the hook missed went to Windows instead: the key is down
        let mut m = Machine::new(VK_WIN_R, true, 300);
        m.on_key(key(VK_WIN_R, true, 0), free());
        m.on_key(key(VK_WIN_R, true, 500), free());
        assert_eq!(m.tick(5000, KeyState { hotkey_down: true, ..free() }), None);
        // once Windows has it up too, the quiet counts from there
        assert_eq!(m.tick(5000 + STALE_MS - 1, free()), None);
        assert_eq!(m.tick(5000 + STALE_MS, free()), Some(Signal::Up { held_ms: 5000 }));
    }

    #[test]
    fn quiet_is_timed_across_the_clock_wrap_and_never_backwards() {
        let mut m = Machine::new(VK_F13, true, 300);
        m.on_key(key(VK_F13, true, u32::MAX - 600), free());
        m.on_key(key(VK_F13, true, u32::MAX - 100), free());
        // a tick stamped just before the last repeat is no reason to stop
        assert_eq!(m.tick(u32::MAX - 200, free()), None);
        assert_eq!(m.tick(STALE_MS - 102, free()), None);
        assert_eq!(m.tick(STALE_MS - 101, free()), Some(Signal::Up { held_ms: 500 }));
    }

    #[test]
    fn only_a_held_press_is_timed_out() {
        // a chord's hotkey passes through; Windows tracks it, not voice
        let mut m = Machine::new(VK_WIN_R, true, 300);
        m.on_key(key(VK_WIN_R, true, 0), free());
        m.on_key(key(VK_WIN_R, true, 500), free());
        assert_eq!(m.on_key(key(E, true, 600), free()).signal, Some(Signal::Combo));
        assert!(!m.held());
        assert_eq!(m.tick(60_000, free()), None);
    }
}
