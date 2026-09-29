//! Keys and mouse events → the bytes an xterm would send (ported from z4-oriel, plus shift+enter → ESC CR, the
//! newline chord Claude Code's /terminal-setup configures).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Key → the bytes an xterm would send. `app_cursor` = DECCKM (arrows as `ESC O x`).
pub fn encode_key(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4)
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let csi = |n: u8| -> Vec<u8> { if m == 1 { format!("\x1b[{n}~").into_bytes() } else { format!("\x1b[{n};{m}~").into_bytes() } };
    let arrow = |c: char| -> Vec<u8> {
        if m == 1 {
            if app_cursor { format!("\x1bO{c}").into_bytes() } else { format!("\x1b[{c}").into_bytes() }
        } else {
            format!("\x1b[1;{m}{c}").into_bytes()
        }
    };
    let mut out = match key.code {
        KeyCode::Char(c) if ctrl => {
            let c = c.to_ascii_lowercase();
            match c {
                'a'..='z' => vec![c as u8 - b'a' + 1],
                ' ' | '@' | '2' => vec![0],
                '[' | '3' => vec![27],
                '\\' | '4' => vec![28],
                ']' | '5' => vec![29],
                '^' | '6' => vec![30],
                '_' | '/' | '7' => vec![31],
                '8' | '?' => vec![127],
                _ => c.to_string().into_bytes(),
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter if shift && !alt && !ctrl => return Some(b"\x1b\r".to_vec()),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab if shift => return Some(b"\x1b[Z".to_vec()),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => return Some(b"\x1b[Z".to_vec()),
        KeyCode::Backspace => {
            if ctrl { vec![8] } else { vec![127] }
        }
        KeyCode::Esc => vec![27],
        KeyCode::Up => arrow('A'),
        KeyCode::Down => arrow('B'),
        KeyCode::Right => arrow('C'),
        KeyCode::Left => arrow('D'),
        KeyCode::Home => arrow('H'),
        KeyCode::End => arrow('F'),
        KeyCode::Insert => csi(2),
        KeyCode::Delete => csi(3),
        KeyCode::PageUp => csi(5),
        KeyCode::PageDown => csi(6),
        KeyCode::F(n) => match n {
            1..=4 => {
                let c = b"PQRS"[n as usize - 1] as char;
                if m == 1 { format!("\x1bO{c}").into_bytes() } else { format!("\x1b[1;{m}{c}").into_bytes() }
            }
            5 => csi(15),
            6 => csi(17),
            7 => csi(18),
            8 => csi(19),
            9 => csi(20),
            10 => csi(21),
            11 => csi(23),
            12 => csi(24),
            _ => return None,
        },
        _ => return None,
    };
    // alt+char / alt+enter etc: ESC prefix (arrows and function keys already carry the modifier)
    if alt && matches!(key.code, KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace | KeyCode::Tab | KeyCode::Esc) {
        out.insert(0, 27);
    }
    Some(out)
}

fn btn(b: MouseButton) -> u16 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// Mouse event → xterm report for a program that enabled mouse tracking (`mode` != None). `area` is the pane's
/// inner rect (events are screen coordinates). SGR (1006) or the legacy X10 byte encoding.
pub fn encode_mouse(ev: MouseEvent, area: Rect, mode: vt100::MouseProtocolMode, enc: vt100::MouseProtocolEncoding) -> Option<Vec<u8>> {
    use vt100::MouseProtocolMode as M;
    if mode == M::None {
        return None;
    }
    let col = ev.column.saturating_sub(area.x) + 1;
    let row = ev.row.saturating_sub(area.y) + 1;
    let mut mods = 0u16;
    if ev.modifiers.contains(KeyModifiers::SHIFT) {
        mods += 4;
    }
    if ev.modifiers.contains(KeyModifiers::ALT) {
        mods += 8;
    }
    if ev.modifiers.contains(KeyModifiers::CONTROL) {
        mods += 16;
    }
    let (code, release) = match ev.kind {
        MouseEventKind::Down(b) => (btn(b), false),
        MouseEventKind::Up(b) if mode != M::Press => (btn(b), true),
        MouseEventKind::Drag(b) if matches!(mode, M::ButtonMotion | M::AnyMotion) => (btn(b) + 32, false),
        MouseEventKind::Moved if mode == M::AnyMotion => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        _ => return None,
    };
    let code = code + mods;
    Some(if enc == vt100::MouseProtocolEncoding::Sgr {
        format!("\x1b[<{};{};{}{}", code, col, row, if release { 'm' } else { 'M' }).into_bytes()
    } else {
        let c = if release { 3 + mods } else { code };
        vec![0x1b, b'[', b'M', (32 + c).min(255) as u8, (32 + col.min(223)) as u8, (32 + row.min(223)) as u8]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> Vec<u8> {
        encode_key(KeyEvent::new(code, m), false).unwrap()
    }

    #[test]
    fn plain_and_control_keys() {
        assert_eq!(k(KeyCode::Char('a'), KeyModifiers::NONE), b"a");
        assert_eq!(k(KeyCode::Char('é'), KeyModifiers::NONE), "é".as_bytes());
        assert_eq!(k(KeyCode::Char('c'), KeyModifiers::CONTROL), vec![3]);
        assert_eq!(k(KeyCode::Char(' '), KeyModifiers::CONTROL), vec![0]);
        assert_eq!(k(KeyCode::Char('['), KeyModifiers::CONTROL), vec![27]);
        assert_eq!(k(KeyCode::Enter, KeyModifiers::NONE), b"\r");
        assert_eq!(k(KeyCode::Enter, KeyModifiers::SHIFT), b"\x1b\r");
        assert_eq!(k(KeyCode::Backspace, KeyModifiers::NONE), vec![127]);
        assert_eq!(k(KeyCode::Backspace, KeyModifiers::CONTROL), vec![8]);
        assert_eq!(k(KeyCode::Esc, KeyModifiers::NONE), vec![27]);
        assert_eq!(k(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
        assert_eq!(k(KeyCode::Tab, KeyModifiers::SHIFT), b"\x1b[Z");
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(k(KeyCode::Char('f'), KeyModifiers::ALT), b"\x1bf");
        assert_eq!(k(KeyCode::Backspace, KeyModifiers::ALT), b"\x1b\x7f");
        assert_eq!(k(KeyCode::Char('c'), KeyModifiers::ALT | KeyModifiers::CONTROL), vec![27, 3]);
    }

    #[test]
    fn cursor_and_function_keys() {
        assert_eq!(k(KeyCode::Up, KeyModifiers::NONE), b"\x1b[A");
        assert_eq!(encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), true).unwrap(), b"\x1bOA");
        assert_eq!(k(KeyCode::Right, KeyModifiers::CONTROL), b"\x1b[1;5C");
        assert_eq!(k(KeyCode::Left, KeyModifiers::SHIFT | KeyModifiers::ALT), b"\x1b[1;4D");
        assert_eq!(k(KeyCode::Home, KeyModifiers::NONE), b"\x1b[H");
        assert_eq!(k(KeyCode::Delete, KeyModifiers::NONE), b"\x1b[3~");
        assert_eq!(k(KeyCode::PageUp, KeyModifiers::CONTROL), b"\x1b[5;5~");
        assert_eq!(k(KeyCode::F(1), KeyModifiers::NONE), b"\x1bOP");
        assert_eq!(k(KeyCode::F(5), KeyModifiers::NONE), b"\x1b[15~");
        assert_eq!(k(KeyCode::F(12), KeyModifiers::SHIFT), b"\x1b[24;2~");
        assert!(encode_key(KeyEvent::new(KeyCode::F(20), KeyModifiers::NONE), false).is_none());
    }

    #[test]
    fn mouse_reports() {
        use vt100::{MouseProtocolEncoding as E, MouseProtocolMode as M};
        let area = Rect::new(10, 5, 80, 24);
        let ev = |kind| MouseEvent { kind, column: 12, row: 7, modifiers: KeyModifiers::NONE };
        assert_eq!(encode_mouse(ev(MouseEventKind::Down(MouseButton::Left)), area, M::PressRelease, E::Sgr).unwrap(), b"\x1b[<0;3;3M");
        assert_eq!(encode_mouse(ev(MouseEventKind::Up(MouseButton::Left)), area, M::PressRelease, E::Sgr).unwrap(), b"\x1b[<0;3;3m");
        assert_eq!(encode_mouse(ev(MouseEventKind::ScrollUp), area, M::PressRelease, E::Sgr).unwrap(), b"\x1b[<64;3;3M");
        assert_eq!(encode_mouse(ev(MouseEventKind::Down(MouseButton::Left)), area, M::PressRelease, E::Default).unwrap(), vec![27, b'[', b'M', 32, 35, 35]);
        assert!(encode_mouse(ev(MouseEventKind::Moved), area, M::PressRelease, E::Sgr).is_none());
        assert!(encode_mouse(ev(MouseEventKind::Down(MouseButton::Left)), area, M::None, E::Sgr).is_none());
        assert!(encode_mouse(ev(MouseEventKind::Drag(MouseButton::Left)), area, M::ButtonMotion, E::Sgr).is_some());
    }
}
