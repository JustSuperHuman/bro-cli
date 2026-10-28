//! Answers to what a program asks its terminal: device attributes, status and cursor reports, mode queries,
//! colours, the text area size and the kitty keyboard flags, given as the desktop app's emulator
//! (alacritty_terminal) gives them. The session server answers for every session it holds, once, whatever windows
//! watch (and when none does); the terminal UI answers the same way for a session it runs itself.
//!
//! Feed it from a vt100 parser's unhandled callbacks: [`Answers::csi`] and [`Answers::osc`] see each query in
//! order, with the screen as it stands at that point of the output, and [`Answers::take`] hands over the replies.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// What a window's terminal looks like, for the answers that depend on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub cursor: [u8; 3],
    /// The 16 ANSI colours.
    pub palette: Vec<[u8; 3]>,
    /// A cell in pixels (width, height), when the window knows it (the desktop app does, a terminal doesn't).
    pub cell: Option<(u16, u16)>,
    /// Keys reach the program in the kitty keyboard protocol when it asks for it.
    pub kitty: bool,
}

impl Default for Profile {
    fn default() -> Profile {
        Profile { fg: [228, 230, 235], bg: [20, 22, 28], cursor: [200, 200, 210], palette: XTERM.to_vec(), cell: None, kitty: false }
    }
}

/// xterm's 16 colours.
const XTERM: [[u8; 3]; 16] = [
    [0, 0, 0],
    [205, 0, 0],
    [0, 205, 0],
    [205, 205, 0],
    [0, 0, 238],
    [205, 0, 205],
    [0, 205, 205],
    [229, 229, 229],
    [127, 127, 127],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [92, 92, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
];

/// The desktop app's emulator reports itself as alacritty_terminal 0.26 (`CSI > c`).
const DA2: &str = "\x1b[>0;2600;1c";

/// Which queries get answers here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// Someone else answers (the session server, for a session it holds), or nobody's asked yet.
    #[default]
    Off,
    /// Only where the cursor is (the desktop app's emulator answers the rest for a session bro runs itself).
    CursorOnly,
    /// Everything.
    All,
}

/// Replies collected while a parser runs, and the keyboard flags the program set (kept even with answers off: a
/// replay restores them).
#[derive(Default)]
pub struct Answers {
    pub mode: Mode,
    pub profile: Arc<Profile>,
    out: Vec<u8>,
    /// pushed kitty keyboard flags, newest last (`CSI > flags u` / `CSI < n u` / `CSI = flags ; mode u`)
    kitty: Vec<u16>,
}

impl Answers {
    pub fn new(mode: Mode) -> Answers {
        Answers { mode, ..Answers::default() }
    }

    /// The replies so far, to write to the program.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// The kitty keyboard flags in force (0: legacy keys).
    pub fn kitty_flags(&self) -> u16 {
        self.kitty.last().copied().unwrap_or(0)
    }

    /// A CSI sequence the parser didn't handle itself.
    pub fn csi(&mut self, screen: &vt100::Screen, i1: Option<u8>, i2: Option<u8>, params: &[&[u16]], c: char) {
        let p = |i: usize| params.get(i).and_then(|p| p.first()).copied();
        if c == 'u' {
            self.keyboard(i1, p(0), p(1));
        }
        if self.mode == Mode::Off {
            return;
        }
        let cursor = |prefix: &str| {
            let (r, c) = screen.cursor_position();
            format!("\x1b[{prefix}{};{}R", r + 1, c + 1)
        };
        let reply = match (i1, i2, c) {
            (None, None, 'n') if p(0) == Some(6) => Some(cursor("")),
            (Some(b'?'), None, 'n') if p(0) == Some(6) => Some(cursor("?")),
            _ if self.mode == Mode::CursorOnly => None,
            (None, None, 'n') if p(0) == Some(5) => Some("\x1b[0n".to_string()),
            (None, None, 'c') if p(0).unwrap_or(0) == 0 => Some("\x1b[?6c".to_string()),
            (Some(b'>'), None, 'c') if p(0).unwrap_or(0) == 0 => Some(DA2.to_string()),
            (Some(b'?'), None, 'u') if self.profile.kitty => Some(format!("\x1b[?{}u", self.kitty_flags())),
            (Some(b'?'), Some(b'$'), 'p') => p(0).map(|m| format!("\x1b[?{m};{}$y", private_mode(screen, m))),
            // insert mode and automatic newline: off (bro's screens don't keep them)
            (Some(b'$'), None, 'p') => p(0).map(|m| format!("\x1b[{m};{}$y", if m == 4 || m == 20 { 2 } else { 0 })),
            (None, None, 't') => {
                let (rows, cols) = screen.size();
                match p(0) {
                    Some(18) => Some(format!("\x1b[8;{rows};{cols}t")),
                    Some(14) => self.profile.cell.map(|(w, h)| format!("\x1b[4;{};{}t", u32::from(rows) * u32::from(h), u32::from(cols) * u32::from(w))),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(r) = reply {
            self.out.extend_from_slice(r.as_bytes());
        }
    }

    /// An OSC sequence the parser didn't handle itself: colour queries (`OSC 10/11/12 ; ?`, `OSC 4 ; n ; ?`).
    pub fn osc(&mut self, params: &[&[u8]]) {
        if self.mode != Mode::All {
            return;
        }
        let Some(&first) = params.first() else { return };
        let rgb = |prefix: &str, c: [u8; 3]| format!("\x1b]{prefix};rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}\x1b\\", c[0], c[1], c[2]);
        match first {
            b"4" => {
                // pairs of index ; spec
                for pair in params[1..].chunks(2) {
                    if let [index, b"?"] = pair
                        && let Some(i) = std::str::from_utf8(index).ok().and_then(|s| s.parse::<usize>().ok())
                        && let Some(c) = self.profile.palette.get(i).copied()
                    {
                        self.out.extend_from_slice(rgb(&format!("4;{i}"), c).as_bytes());
                    }
                }
            }
            b"10" | b"11" | b"12" => {
                // `OSC 10 ; ? ; ?` asks for the foreground and the background
                let start: u8 = std::str::from_utf8(first).ok().and_then(|s| s.parse().ok()).unwrap_or(10);
                for (n, spec) in params[1..].iter().enumerate() {
                    let code = start as usize + n;
                    if *spec != b"?" || code > 12 {
                        continue;
                    }
                    let c = match code {
                        10 => self.profile.fg,
                        11 => self.profile.bg,
                        _ => self.profile.cursor,
                    };
                    self.out.extend_from_slice(rgb(&code.to_string(), c).as_bytes());
                }
            }
            _ => {}
        }
    }

    fn keyboard(&mut self, i1: Option<u8>, first: Option<u16>, second: Option<u16>) {
        match i1 {
            Some(b'>') => {
                self.kitty.push(first.unwrap_or(0));
                // terminals cap the stack; so do we
                if self.kitty.len() > 16 {
                    self.kitty.remove(0);
                }
            }
            Some(b'<') => {
                let n = first.unwrap_or(1).max(1) as usize;
                let keep = self.kitty.len().saturating_sub(n);
                self.kitty.truncate(keep);
            }
            Some(b'=') => {
                let flags = first.unwrap_or(0);
                let top = self.kitty_flags();
                let new = match second.unwrap_or(1) {
                    2 => top | flags,
                    3 => top & !flags,
                    _ => flags,
                };
                match self.kitty.last_mut() {
                    Some(t) => *t = new,
                    None => self.kitty.push(new),
                }
            }
            _ => {}
        }
    }
}

/// DECRQM for a private mode: 1 set, 2 reset, 0 not recognised (as alacritty reports what it keeps).
fn private_mode(screen: &vt100::Screen, mode: u16) -> u8 {
    use vt100::{MouseProtocolEncoding as Enc, MouseProtocolMode as Mouse};
    let on = |b: bool| if b { 1 } else { 2 };
    match mode {
        1 => on(screen.application_cursor()),
        25 => on(!screen.hide_cursor()),
        1000 => on(screen.mouse_protocol_mode() == Mouse::PressRelease),
        1002 => on(screen.mouse_protocol_mode() == Mouse::ButtonMotion),
        1003 => on(screen.mouse_protocol_mode() == Mouse::AnyMotion),
        1005 => on(screen.mouse_protocol_encoding() == Enc::Utf8),
        1006 => on(screen.mouse_protocol_encoding() == Enc::Sgr),
        1049 => on(screen.alternate_screen()),
        2004 => on(screen.bracketed_paste()),
        // synchronized output: understood, never left on
        2026 => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Cb(Answers);

    impl vt100::Callbacks for Cb {
        fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, i2: Option<u8>, params: &[&[u16]], c: char) {
            self.0.csi(screen, i1, i2, params, c);
        }
        fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
            self.0.osc(params);
        }
    }

    fn run(mode: Mode, profile: Profile, input: &[u8]) -> String {
        let mut p = vt100::Parser::new_with_callbacks(24, 80, 0, Cb(Answers { mode, profile: Arc::new(profile), ..Answers::default() }));
        p.process(input);
        String::from_utf8(p.callbacks_mut().0.take()).unwrap()
    }

    #[test]
    fn answers_what_the_desktop_apps_emulator_answers() {
        let all = |s: &[u8]| run(Mode::All, Profile::default(), s);
        assert_eq!(all(b"\x1b[c"), "\x1b[?6c");
        assert_eq!(all(b"\x1b[>c"), DA2);
        assert_eq!(all(b"\x1b[5n"), "\x1b[0n");
        assert_eq!(all(b"ab\x1b[6n"), "\x1b[1;3R", "where the cursor is at that point");
        assert_eq!(all(b"\x1b[?2004h\x1b[?2004$p\x1b[?2026$p\x1b[?7777$p"), "\x1b[?2004;1$y\x1b[?2026;2$y\x1b[?7777;0$y");
        assert_eq!(all(b"\x1b[18t"), "\x1b[8;24;80t");
        assert_eq!(all(b"\x1b[4$p"), "\x1b[4;2$y");
        assert_eq!(all(b"\x1b[14t"), "", "a terminal window doesn't know its cell size");
        assert_eq!(all(b"\x1b]11;?\x07"), "\x1b]11;rgb:1414/1616/1c1c\x1b\\");
        assert_eq!(all(b"\x1b]4;1;?\x07"), "\x1b]4;1;rgb:cdcd/0000/0000\x1b\\");
    }

    #[test]
    fn the_kitty_flags_are_reported_only_where_keys_follow_them() {
        let kitty = Profile { kitty: true, cell: Some((9, 19)), ..Profile::default() };
        assert_eq!(run(Mode::All, kitty.clone(), b"\x1b[>1u\x1b[>3u\x1b[?u"), "\x1b[?3u");
        assert_eq!(run(Mode::All, kitty, b"\x1b[14t"), format!("\x1b[4;{};{}t", 24 * 19, 80 * 9));
        // a terminal UI that sends legacy keys says nothing: the device attributes that follow tell the program
        assert_eq!(run(Mode::All, Profile::default(), b"\x1b[?u\x1b[c"), "\x1b[?6c");
    }

    #[test]
    fn answers_only_the_cursor_or_nothing_when_someone_else_answers() {
        assert_eq!(run(Mode::CursorOnly, Profile::default(), b"\x1b[c\x1b[6n\x1b]10;?\x07"), "\x1b[1;1R");
        assert_eq!(run(Mode::Off, Profile::default(), b"\x1b[c\x1b[6n"), "");
    }

    #[test]
    fn keeps_the_keyboard_flags_with_answers_off() {
        let mut a = Answers::new(Mode::Off);
        let s = vt100::Parser::new(24, 80, 0);
        a.csi(s.screen(), Some(b'>'), None, &[&[1]], 'u');
        a.csi(s.screen(), Some(b'='), None, &[&[4], &[2]], 'u');
        assert_eq!(a.kitty_flags(), 5);
        a.csi(s.screen(), Some(b'<'), None, &[], 'u');
        assert_eq!(a.kitty_flags(), 0);
    }
}
