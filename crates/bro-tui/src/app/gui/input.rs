//! Keyboard input from winit as the crossterm key events the workspace already understands, so every key
//! binding, the prefix table and each pane's own handling work unchanged.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use winit::event::{ElementState, KeyEvent as WinitKey};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// winit modifiers → crossterm's.
pub fn mods(m: ModifiersState) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    if m.shift_key() {
        out |= KeyModifiers::SHIFT;
    }
    if m.control_key() {
        out |= KeyModifiers::CONTROL;
    }
    if m.alt_key() {
        out |= KeyModifiers::ALT;
    }
    if m.super_key() {
        out |= KeyModifiers::SUPER;
    }
    out
}

/// A key press (or repeat) as crossterm's event; None for releases and keys bro has no use for.
pub fn key(ev: &WinitKey, m: ModifiersState) -> Option<KeyEvent> {
    if ev.state == ElementState::Released {
        return None;
    }
    // Super (Windows key, Cmd) chords are the system's: no binding can use them (the keymap drops the modifier),
    // and a terminal never sees them, so they don't type their plain key into a session either
    if m.super_key() {
        return None;
    }
    let mut modifiers = mods(m);
    let code = match &ev.logical_key {
        Key::Named(n) => match n {
            NamedKey::Enter => KeyCode::Enter,
            NamedKey::Tab if modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
            NamedKey::Tab => KeyCode::Tab,
            NamedKey::Backspace => KeyCode::Backspace,
            NamedKey::Escape => KeyCode::Esc,
            NamedKey::ArrowUp => KeyCode::Up,
            NamedKey::ArrowDown => KeyCode::Down,
            NamedKey::ArrowLeft => KeyCode::Left,
            NamedKey::ArrowRight => KeyCode::Right,
            NamedKey::Home => KeyCode::Home,
            NamedKey::End => KeyCode::End,
            NamedKey::PageUp => KeyCode::PageUp,
            NamedKey::PageDown => KeyCode::PageDown,
            NamedKey::Insert => KeyCode::Insert,
            NamedKey::Delete => KeyCode::Delete,
            NamedKey::Space => KeyCode::Char(' '),
            NamedKey::F1 => KeyCode::F(1),
            NamedKey::F2 => KeyCode::F(2),
            NamedKey::F3 => KeyCode::F(3),
            NamedKey::F4 => KeyCode::F(4),
            NamedKey::F5 => KeyCode::F(5),
            NamedKey::F6 => KeyCode::F(6),
            NamedKey::F7 => KeyCode::F(7),
            NamedKey::F8 => KeyCode::F(8),
            NamedKey::F9 => KeyCode::F(9),
            NamedKey::F10 => KeyCode::F(10),
            NamedKey::F11 => KeyCode::F(11),
            NamedKey::F12 => KeyCode::F(12),
            NamedKey::F13 => KeyCode::F(13),
            NamedKey::F14 => KeyCode::F(14),
            NamedKey::F15 => KeyCode::F(15),
            NamedKey::F16 => KeyCode::F(16),
            NamedKey::F17 => KeyCode::F(17),
            NamedKey::F18 => KeyCode::F(18),
            NamedKey::F19 => KeyCode::F(19),
            NamedKey::F20 => KeyCode::F(20),
            NamedKey::F21 => KeyCode::F(21),
            NamedKey::F22 => KeyCode::F(22),
            NamedKey::F23 => KeyCode::F(23),
            NamedKey::F24 => KeyCode::F(24),
            _ => return None,
        },
        Key::Character(s) => {
            let typed = ev.text.as_deref().and_then(|t| t.chars().next()).filter(|c| !c.is_control());
            // AltGr (ctrl+alt on Windows) typing a character: the character, not a chord
            if modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::ALT)
                && let Some(c) = typed
                && !s.chars().next().is_some_and(|k| k.eq_ignore_ascii_case(&c))
            {
                modifiers.remove(KeyModifiers::CONTROL | KeyModifiers::ALT);
                KeyCode::Char(c)
            } else {
                let c = s.chars().next()?;
                // with shift the logical key is already the shifted character
                KeyCode::Char(c)
            }
        }
        // a chord the layout has no character for (ctrl+alt+a on a layout without AltGr): the key's own
        // character, as a terminal reports it; a dead key on its own is still composing and stays out
        Key::Unidentified(_) | Key::Dead(_) if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            let c = unmodified(ev)?;
            KeyCode::Char(if modifiers.contains(KeyModifiers::SHIFT) { c.to_ascii_uppercase() } else { c })
        }
        _ => return None,
    };
    // shift on a character key is part of the character (crossterm reports 'A' with SHIFT; keymap normalises)
    let kind = if ev.repeat { KeyEventKind::Repeat } else { KeyEventKind::Press };
    Some(KeyEvent { code, modifiers, kind, state: KeyEventState::NONE })
}

/// The character on the key with no modifiers applied (None for keys that don't type one).
#[cfg(any(windows, target_os = "macos", target_os = "linux", target_os = "dragonfly", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
fn unmodified(ev: &WinitKey) -> Option<char> {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    match ev.key_without_modifiers() {
        Key::Character(s) => s.chars().next().filter(|c| !c.is_control()),
        _ => None,
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux", target_os = "dragonfly", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd")))]
fn unmodified(_ev: &WinitKey) -> Option<char> {
    None
}

/// The bare modifier being pressed or released, if this event is one (alt hints and the like).
pub fn modifier_key(ev: &WinitKey) -> Option<(NamedKey, bool)> {
    match &ev.logical_key {
        Key::Named(n @ (NamedKey::Alt | NamedKey::Control | NamedKey::Shift | NamedKey::Super)) => Some((*n, ev.state == ElementState::Pressed)),
        _ => None,
    }
}
