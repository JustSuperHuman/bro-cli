//! The server's copy of a session's screen: kept current with every byte of output so it can answer what the
//! program asks its terminal (see [`crate::answer`]: ConPTY asks where the cursor is at start and waits) and rebuild
//! the terminal for a window that attaches later.

use crate::answer::{Answers, Mode, Profile};
use std::sync::Arc;

/// Scrollback kept per session: what a window that attaches gets back (the terminal UI keeps as much).
const SCROLLBACK: usize = 10_000;

/// What the parser can't keep for us: the window title, and the answers (with the keyboard flags the program set).
struct Cb {
    title: Option<Vec<u8>>,
    answers: Answers,
}

impl vt100::Callbacks for Cb {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(title.to_vec());
    }

    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, i2: Option<u8>, params: &[&[u16]], c: char) {
        self.answers.csi(screen, i1, i2, params, c);
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        self.answers.osc(params);
    }
}

pub(crate) struct Screen {
    parser: vt100::Parser<Cb>,
}

impl Screen {
    pub fn new(rows: u16, cols: u16) -> Screen {
        Screen { parser: vt100::Parser::new_with_callbacks(rows.max(2), cols.max(10), SCROLLBACK, Cb { title: None, answers: Answers::new(Mode::All) }) }
    }

    /// Take in output. Returns the answers to what it asked (empty: nothing), as `profile`'s terminal gives them,
    /// each from the screen as it stood when the program asked.
    pub fn feed(&mut self, data: &[u8], profile: &Arc<Profile>) -> Vec<u8> {
        let cb = self.parser.callbacks_mut();
        if !Arc::ptr_eq(&cb.answers.profile, profile) {
            cb.answers.profile = profile.clone();
        }
        self.parser.process(data);
        self.parser.callbacks_mut().answers.take()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows > 0 && cols > 0 {
            self.parser.screen_mut().set_size(rows, cols);
        }
    }

    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    /// Bytes that rebuild this terminal in a fresh emulator of the same size: the scrollback, the visible
    /// screen, the input modes, the window title and the keyboard protocol.
    pub fn replay(&mut self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let screen = self.parser.screen_mut();
        if screen.alternate_screen() {
            // vt100 exposes only the active grid: the primary screen's history is out of reach while a
            // full-screen program owns the terminal
            out.extend_from_slice(b"\x1b[?1049h");
        } else {
            let (rows, cols) = screen.size();
            screen.set_scrollback(usize::MAX);
            let history = screen.scrollback();
            // with offset k the visible rows are history rows [H-k, H-k+rows), so the first min(k, rows) of
            // them are still above the screen
            let mut offset = history;
            while offset > 0 {
                screen.set_scrollback(offset);
                let take = offset.min(usize::from(rows));
                for row in screen.rows_formatted(0, cols).take(take) {
                    out.extend_from_slice(b"\x1b[m");
                    out.extend_from_slice(&row);
                    out.extend_from_slice(b"\r\n");
                }
                offset -= take;
            }
            screen.set_scrollback(0);
            if history > 0 {
                // contents_formatted() starts by clearing the screen: scroll the last history rows off first
                out.extend(std::iter::repeat_n(b'\n', usize::from(rows.saturating_sub(1))));
            }
        }
        out.extend_from_slice(&screen.contents_formatted());
        out.extend_from_slice(&screen.input_mode_formatted());
        let cb = self.parser.callbacks();
        if let Some(title) = &cb.title {
            out.extend_from_slice(b"\x1b]0;");
            out.extend(title.iter().filter(|b| **b != 0x07 && **b != 0x1b));
            out.push(0x07);
        }
        let flags = cb.answers.kitty_flags();
        if flags != 0 {
            out.extend_from_slice(format!("\x1b[>{flags}u").as_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(s: &mut Screen, data: &[u8]) -> Vec<u8> {
        s.feed(data, &Arc::new(Profile::default()))
    }

    fn rebuilt(s: &mut Screen) -> vt100::Parser {
        let (rows, cols) = s.size();
        let mut p = vt100::Parser::new(rows, cols, 1000);
        p.process(&s.replay());
        p
    }

    #[test]
    fn answers_the_cursor_query_from_the_screen_where_it_was_asked() {
        let mut s = Screen::new(10, 40);
        assert!(feed(&mut s, b"hello").is_empty());
        assert_eq!(feed(&mut s, b"\r\nab\x1b[6n"), b"\x1b[2;3R");
        // where it was when the program asked, not after what followed in the same read
        assert_eq!(feed(&mut s, b"\x1b[6nmore"), b"\x1b[2;3R");
        assert_eq!(feed(&mut s, b"\x1b[c"), b"\x1b[?6c", "and the rest of what a program asks");
    }

    #[test]
    fn narrow_input_repaints_do_not_wrap_when_the_program_disables_wrapping() {
        let mut s = Screen::new(4, 10);
        feed(&mut s, b"\x1b[?7l\x1b[2;1H> abcdefghijk\r> edited\x1b[K");
        assert_eq!(s.parser.screen().contents(), "\n> edited");
        assert_eq!(feed(&mut s, b"\x1b[6n"), b"\x1b[2;9R");
        // A newly attached window must keep the same wrapping rule while typing continues.
        let mut p = rebuilt(&mut s);
        let next = b"\r> 1234567890123\r> final\x1b[K";
        feed(&mut s, next);
        p.process(next);
        assert_eq!(p.screen().contents(), "\n> final");
        assert_eq!(p.screen().contents(), s.parser.screen().contents());
    }

    #[test]
    fn cursor_reports_stay_inside_the_terminal_at_the_right_margin() {
        let mut s = Screen::new(4, 10);
        assert_eq!(feed(&mut s, b"1234567890\x1b[6n"), b"\x1b[1;10R");
        assert_eq!(feed(&mut s, b"x\x1b[6n"), b"\x1b[2;2R");
    }

    #[test]
    fn wrapping_can_be_queried_restored_and_reset_after_a_resize() {
        let mut s = Screen::new(4, 20);
        assert_eq!(feed(&mut s, b"\x1b[?7$p\x1b[?7l\x1b[?7$p"), b"\x1b[?7;1$y\x1b[?7;2$y");
        s.resize(4, 10);
        feed(&mut s, "123456789界\u{301}".as_bytes());
        assert_eq!(s.parser.screen().cursor_position(), (0, 10));
        assert_eq!(feed(&mut s, b"\x1b[6n"), b"\x1b[1;10R");
        let mut p = rebuilt(&mut s);
        assert!(!p.screen().auto_wrap());
        p.process(b"\x1b[?7h\r1234567890x");
        assert_eq!(p.screen().cursor_position(), (1, 1));
        assert_eq!(feed(&mut s, b"\x1bc\x1b[?7$p"), b"\x1b[?7;1$y");
    }

    #[test]
    fn a_replay_rebuilds_screen_scrollback_modes_and_title() {
        let mut s = Screen::new(5, 20);
        for i in 0..12 {
            feed(&mut s, format!("line {i}\r\n").as_bytes());
        }
        feed(&mut s, b"\x1b]0;my task\x07\x1b[?2004h\x1b[31mred\x1b[m");
        let mut p = rebuilt(&mut s);
        assert_eq!(p.screen().contents(), s.parser.screen().contents());
        assert_eq!(p.screen().cursor_position(), s.parser.screen().cursor_position());
        assert!(p.screen().bracketed_paste(), "input modes come back");
        assert_eq!(p.screen().cell(4, 0).unwrap().fgcolor(), vt100::Color::Idx(1));
        p.screen_mut().set_scrollback(usize::MAX);
        assert!(p.screen().scrollback() >= 8, "history comes back: {}", p.screen().scrollback());
        let replay = String::from_utf8_lossy(&s.replay()).to_string();
        assert!(replay.contains("\x1b]0;my task\x07"));
    }

    #[test]
    fn the_kitty_keyboard_flags_follow_pushes_pops_and_sets() {
        let mut s = Screen::new(5, 20);
        feed(&mut s, b"\x1b[>1u\x1b[>3u");
        assert!(String::from_utf8_lossy(&s.replay()).ends_with("\x1b[>3u"));
        feed(&mut s, b"\x1b[<u");
        assert!(String::from_utf8_lossy(&s.replay()).ends_with("\x1b[>1u"));
        feed(&mut s, b"\x1b[=4;2u");
        assert!(String::from_utf8_lossy(&s.replay()).ends_with("\x1b[>5u"));
        feed(&mut s, b"\x1b[<5u");
        assert!(!String::from_utf8_lossy(&s.replay()).contains("\x1b[>"));
    }

    #[test]
    fn a_full_screen_program_is_replayed_on_the_alternate_screen() {
        let mut s = Screen::new(5, 20);
        feed(&mut s, b"shell\r\n\x1b[?1049h\x1b[Hvim here");
        let p = rebuilt(&mut s);
        assert!(p.screen().alternate_screen());
        assert!(p.screen().contents().contains("vim here"));
    }
}
