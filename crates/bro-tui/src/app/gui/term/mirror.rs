//! Each session's screen as alacritty_terminal keeps it: fed the same bytes as the session's vt100 model (see
//! [`crate::panes::mirror`]), it is what the desktop app draws, scrolls and selects in, and it answers the
//! program's queries (device attributes, colours, the kitty keyboard protocol, the text area size).

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Processor, Rgb, StdSyncHandler};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use parking_lot::{Mutex, MutexGuard};
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Lines kept above the screen.
pub const SCROLLBACK: usize = 10_000;

/// What alacritty asked of us while parsing, collected for the reader thread.
#[derive(Default)]
pub struct Requests {
    /// bytes to write back to the program
    pub replies: Vec<u8>,
    pub title: Option<Option<String>>,
    pub bell: bool,
    /// OSC 52: the program put text on the clipboard
    pub clipboard: Option<String>,
}

#[derive(Clone)]
pub struct Listener {
    requests: Arc<Mutex<Requests>>,
    /// colours and cell size the program may ask for
    env: Arc<Mutex<Env>>,
}

/// What the app tells the screens so they can answer queries.
#[derive(Clone, Copy)]
pub struct Env {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub cursor: [u8; 3],
    pub palette: [[u8; 3]; 16],
    pub cell_w: u16,
    pub cell_h: u16,
}

impl Default for Env {
    fn default() -> Env {
        Env { fg: [228, 230, 235], bg: [20, 22, 28], cursor: [200, 200, 210], palette: [[128, 128, 128]; 16], cell_w: 9, cell_h: 19 }
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let mut r = self.requests.lock();
        match event {
            TermEvent::PtyWrite(text) => {
                // the session answers the cursor position report itself
                if !is_cursor_report(&text) {
                    r.replies.extend_from_slice(text.as_bytes());
                }
            }
            TermEvent::Title(t) => r.title = Some(Some(t)),
            TermEvent::ResetTitle => r.title = Some(None),
            TermEvent::Bell => r.bell = true,
            TermEvent::ClipboardStore(_, text) => r.clipboard = Some(text),
            TermEvent::ColorRequest(index, format) => {
                let env = *self.env.lock();
                let rgb = match index {
                    0..=15 => env.palette[index],
                    256 => env.fg,
                    257 => env.bg,
                    258 => env.cursor,
                    _ => env.fg,
                };
                r.replies.extend_from_slice(format(Rgb { r: rgb[0], g: rgb[1], b: rgb[2] }).as_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let env = *self.env.lock();
                // the program wants pixels; rows/cols come from the grid itself, so this only matters for the cell size
                let size = WindowSize { num_lines: 0, num_cols: 0, cell_width: env.cell_w, cell_height: env.cell_h };
                r.replies.extend_from_slice(format(size).as_bytes());
            }
            // a paste the program asked for (OSC 52 read) is never answered: it would leak the clipboard
            TermEvent::ClipboardLoad(..) => {}
            _ => {}
        }
    }
}

/// `ESC [ <row> ; <col> R`
fn is_cursor_report(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("\x1b[").and_then(|r| r.strip_suffix('R')) else { return false };
    let mut parts = rest.split(';');
    matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if !a.is_empty() && !b.is_empty() && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()))
}

/// Rows × columns for alacritty.
#[derive(Clone, Copy)]
pub struct Size {
    pub rows: usize,
    pub cols: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// The screen and its parser.
pub struct Screen {
    pub term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
}

impl Screen {
    /// Finish a synchronized update the program never ended (DEC 2026 times out after 150 ms).
    pub fn flush_sync(&mut self) {
        if self.parser.sync_timeout().sync_timeout().is_some_and(|t| t <= std::time::Instant::now()) {
            self.parser.stop_sync(&mut self.term);
        }
    }
    /// A synchronized update is being held back: when it times out.
    #[allow(dead_code)]
    pub fn sync_deadline(&self) -> Option<std::time::Instant> {
        self.parser.sync_timeout().sync_timeout()
    }
}

/// One session's mirror.
pub struct Mirror {
    screen: Mutex<Screen>,
    requests: Arc<Mutex<Requests>>,
    /// bumps with every chunk of output (what changed since the last frame)
    pub seq: AtomicU64,
}

impl Mirror {
    /// A screen answering queries from `env` (shared by every screen, so a theme change reaches them all).
    pub fn new(env: Arc<Mutex<Env>>) -> Mirror {
        let requests = Arc::new(Mutex::new(Requests::default()));
        let listener = Listener { requests: requests.clone(), env: env.clone() };
        let config = Config { scrolling_history: SCROLLBACK, kitty_keyboard: true, ..Config::default() };
        let term = Term::new(config, &Size { rows: 24, cols: 80 }, listener);
        Mirror { screen: Mutex::new(Screen { term, parser: Processor::new() }), requests, seq: AtomicU64::new(0) }
    }

    pub fn lock(&self) -> MutexGuard<'_, Screen> {
        self.screen.lock()
    }

    /// The screen, unless the reader thread is busy with it (housekeeping that can wait a frame).
    pub fn try_lock(&self) -> Option<MutexGuard<'_, Screen>> {
        self.screen.try_lock()
    }

    #[cfg(test)]
    fn standalone() -> Mirror {
        Mirror::new(Arc::new(Mutex::new(Env::default())))
    }

    /// Requests collected since the last call (title, bell, clipboard).
    pub fn take_requests(&self) -> Requests {
        let mut r = self.requests.lock();
        Requests { replies: vec![], title: r.title.take(), bell: std::mem::take(&mut r.bell), clipboard: r.clipboard.take() }
    }

    /// Down from a `ScreenMirror` to the desktop app's own type.
    pub fn of(m: &dyn crate::panes::mirror::ScreenMirror) -> Option<&Mirror> {
        m.as_any().downcast_ref::<Mirror>()
    }
}

impl crate::panes::mirror::ScreenMirror for Mirror {
    fn feed(&self, bytes: &[u8]) -> Vec<u8> {
        {
            let mut s = self.screen.lock();
            let Screen { term, parser } = &mut *s;
            parser.advance(term, bytes);
            s.flush_sync();
        }
        self.seq.fetch_add(1, Ordering::Relaxed);
        std::mem::take(&mut self.requests.lock().replies)
    }

    fn resize(&self, rows: u16, cols: u16) {
        let mut s = self.screen.lock();
        s.term.resize(Size { rows: rows.max(1) as usize, cols: cols.max(2) as usize });
        self.seq.fetch_add(1, Ordering::Relaxed);
    }

    fn encode_key(&self, key: &KeyEvent) -> Option<Vec<u8>> {
        let mode = *self.screen.lock().term.mode();
        kitty_key(key, mode)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The kitty keyboard protocol's bytes for `key`, when the program turned it on (else None: the usual xterm
/// bytes apply). Only presses are reported (bro doesn't send releases); keys whose legacy encoding is
/// unambiguous keep it, as the protocol allows.
pub fn kitty_key(key: &KeyEvent, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        return None;
    }
    let all_as_esc = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let sup = key.modifiers.contains(KeyModifiers::SUPER);
    let mods = 1 + shift as u32 + 2 * alt as u32 + 4 * ctrl as u32 + 8 * sup as u32;
    let csi_u = |code: u32| -> Vec<u8> { if mods == 1 { format!("\x1b[{code}u").into_bytes() } else { format!("\x1b[{code};{mods}u").into_bytes() } };
    match key.code {
        KeyCode::Char(c) => {
            // plain text (shift only changes the character) stays text
            if !alt && !ctrl && !sup && !all_as_esc {
                return Some(c.to_string().into_bytes());
            }
            // the key's own code is its unshifted form
            let base = c.to_lowercase().next().unwrap_or(c) as u32;
            Some(csi_u(base))
        }
        KeyCode::Enter => Some(if mods == 1 && !all_as_esc { b"\r".to_vec() } else { csi_u(13) }),
        KeyCode::Tab => Some(if mods == 1 && !all_as_esc { b"\t".to_vec() } else { csi_u(9) }),
        // shift+tab: shift alone isn't one of the chords the protocol re-encodes, so it keeps xterm's `CSI Z`;
        // with alt / ctrl (or every key as an escape) it's tab with the shift bit set
        KeyCode::BackTab if !alt && !ctrl && !sup && !all_as_esc => Some(b"\x1b[Z".to_vec()),
        KeyCode::BackTab => Some(format!("\x1b[9;{}u", ((mods - 1) | 1) + 1).into_bytes()),
        KeyCode::Backspace => Some(if mods == 1 && !all_as_esc { b"\x7f".to_vec() } else { csi_u(127) }),
        KeyCode::Esc => Some(csi_u(27)),
        // arrows, home/end, function keys: xterm's encoding already carries the modifiers
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::mirror::ScreenMirror;

    fn screen_text(m: &Mirror) -> String {
        let s = m.lock();
        let grid = s.term.grid();
        let mut out = String::new();
        for line in 0..grid.screen_lines() {
            let row = &grid[alacritty_terminal::index::Line(line as i32)];
            let text: String = (0..grid.columns()).map(|c| row[alacritty_terminal::index::Column(c)].c).collect();
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out
    }

    #[test]
    fn mirrors_output_and_answers_queries_but_not_the_cursor_report() {
        let m = Mirror::standalone();
        m.resize(5, 20);
        let replies = m.feed(b"hello\r\nworld\x1b[6n\x1b[c");
        let text = screen_text(&m);
        assert!(text.starts_with("hello\nworld\n"), "{text:?}");
        let replies = String::from_utf8_lossy(&replies).to_string();
        assert!(!replies.contains('R'), "the cursor report is the session's: {replies:?}");
        assert!(replies.contains("\x1b[?6c"), "device attributes answered: {replies:?}");
        // a title
        m.feed(b"\x1b]0;Fix the timer\x07");
        assert_eq!(m.take_requests().title, Some(Some("Fix the timer".into())));
    }

    #[test]
    fn kitty_keys_only_when_the_program_asks() {
        let m = Mirror::standalone();
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(m.encode_key(&shift_enter), None, "xterm bytes until the program opts in");
        m.feed(b"\x1b[>1u"); // push: disambiguate escape codes
        assert_eq!(m.encode_key(&shift_enter), Some(b"\x1b[13;2u".to_vec()));
        assert_eq!(m.encode_key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)), Some(b"a".to_vec()));
        assert_eq!(m.encode_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(b"\x1b[99;5u".to_vec()));
        assert_eq!(m.encode_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), Some(b"\x1b[27u".to_vec()));
        assert_eq!(m.encode_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)), None, "arrows keep xterm's encoding");
        m.feed(b"\x1b[<u"); // pop
        assert_eq!(m.encode_key(&shift_enter), None);
    }

    #[test]
    fn shift_tab_stays_shift_tab_under_the_kitty_protocol() {
        let disambiguate = TermMode::DISAMBIGUATE_ESC_CODES;
        let back = |m: KeyModifiers| kitty_key(&KeyEvent::new(KeyCode::BackTab, m), disambiguate);
        // what the window sends for shift+tab: BackTab with SHIFT (never read as alt+tab)
        assert_eq!(back(KeyModifiers::SHIFT), Some(b"\x1b[Z".to_vec()));
        assert_eq!(back(KeyModifiers::NONE), Some(b"\x1b[Z".to_vec()));
        assert_eq!(back(KeyModifiers::SHIFT | KeyModifiers::CONTROL), Some(b"\x1b[9;6u".to_vec()), "ctrl+shift+tab");
        assert_eq!(back(KeyModifiers::SHIFT | KeyModifiers::ALT), Some(b"\x1b[9;4u".to_vec()), "alt+shift+tab");
        let all = disambiguate | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(kitty_key(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), all), Some(b"\x1b[9;2u".to_vec()));
    }

    #[test]
    fn cursor_reports_are_recognised() {
        assert!(is_cursor_report("\x1b[12;40R"));
        assert!(!is_cursor_report("\x1b[?6c"));
        assert!(!is_cursor_report("\x1b[;R"));
    }
}
