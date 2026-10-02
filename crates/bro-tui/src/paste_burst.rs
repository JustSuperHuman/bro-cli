//! Pastes that arrive as keystrokes. The Windows console has no bracketed paste for us to read: when the
//! host pastes (right-click, ctrl+shift+v, a dictation tool typing) we get one key event per character, and
//! every newline is an Enter — which a session would take as "submit", leaving the rest of the text typed
//! but unsent. Text keys that arrive faster than anyone types are folded back into one paste event.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::Duration;

/// Keys closer together than this belong to one burst.
const GAP: Duration = Duration::from_millis(8);
/// Shorter bursts are fast typing (two keys rolled together), not a paste.
const MIN: usize = 3;

/// Where input events come from (crossterm; scripted in tests).
pub trait Source {
    /// Is an event ready within `wait`?
    fn poll(&mut self, wait: Duration) -> bool;
    fn read(&mut self) -> Option<Event>;
}

pub struct Console;

impl Source for Console {
    fn poll(&mut self, wait: Duration) -> bool {
        crossterm::event::poll(wait).unwrap_or(false)
    }

    fn read(&mut self) -> Option<Event> {
        crossterm::event::read().ok()
    }
}

/// The character a key would type, if it is plain text (shift allowed; Enter and Tab count).
fn text_of(ev: &Event) -> Option<char> {
    let Event::Key(KeyEvent { code, modifiers, kind, .. }) = ev else { return None };
    if *kind == KeyEventKind::Release {
        return None;
    }
    match code {
        KeyCode::Char(c) if (*modifiers - KeyModifiers::SHIFT).is_empty() => Some(*c),
        KeyCode::Enter if modifiers.is_empty() => Some('\r'),
        KeyCode::Tab if modifiers.is_empty() => Some('\t'),
        _ => None,
    }
}

/// Forward events from `src` to `emit` until either ends, folding key bursts into `Event::Paste`.
pub fn pump(mut src: impl Source, mut emit: impl FnMut(Event) -> bool) {
    while let Some(ev) = src.read() {
        let Some(c) = text_of(&ev) else {
            if !emit(ev) {
                return;
            }
            continue;
        };
        // nothing right behind it: a typed key, passed on without delay
        if !src.poll(Duration::ZERO) {
            if !emit(ev) {
                return;
            }
            continue;
        }
        let mut keys = vec![ev];
        let mut text = String::from(c);
        let mut tail = None;
        while src.poll(GAP) {
            let Some(next) = src.read() else { break };
            if let Some(c) = text_of(&next) {
                text.push(c);
                keys.push(next);
            } else if !matches!(&next, Event::Key(k) if k.kind == KeyEventKind::Release) {
                tail = Some(next);
                break;
            }
        }
        let out = if keys.len() >= MIN { vec![Event::Paste(text)] } else { keys };
        for e in out.into_iter().chain(tail) {
            if !emit(e) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Events with the pause before each one.
    struct Script(VecDeque<(Duration, Event)>);

    impl Source for Script {
        fn poll(&mut self, wait: Duration) -> bool {
            match self.0.front_mut() {
                Some((gap, _)) if *gap <= wait => true,
                Some((gap, _)) => {
                    *gap -= wait;
                    false
                }
                None => false,
            }
        }

        fn read(&mut self) -> Option<Event> {
            self.0.pop_front().map(|(_, e)| e)
        }
    }

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn release(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Release))
    }

    fn run(script: Vec<(u64, Event)>) -> Vec<Event> {
        let mut out = vec![];
        pump(Script(script.into_iter().map(|(ms, e)| (Duration::from_millis(ms), e)).collect()), |e| {
            out.push(e);
            true
        });
        out
    }

    #[test]
    fn a_burst_of_keys_with_newlines_becomes_one_paste() {
        let mut script = vec![];
        for c in "one\rtwo".chars() {
            let code = if c == '\r' { KeyCode::Enter } else { KeyCode::Char(c) };
            script.push((0, press(code)));
            script.push((0, release(code)));
        }
        // the host hands the rest over a moment later
        script.push((5, press(KeyCode::Enter)));
        script.push((0, press(KeyCode::Char('3'))));
        assert_eq!(run(script), vec![Event::Paste("one\rtwo\r3".into())]);
    }

    #[test]
    fn typing_is_passed_through_key_by_key() {
        let typed = vec![(0, press(KeyCode::Char('y'))), (90, release(KeyCode::Char('y'))), (40, press(KeyCode::Enter)), (80, release(KeyCode::Enter))];
        assert_eq!(run(typed.clone()), typed.into_iter().map(|(_, e)| e).collect::<Vec<_>>());
        // two keys rolled together are still typing
        let rolled = run(vec![(0, press(KeyCode::Char('o'))), (0, press(KeyCode::Char('k'))), (60, press(KeyCode::Enter))]);
        assert_eq!(rolled, vec![press(KeyCode::Char('o')), press(KeyCode::Char('k')), press(KeyCode::Enter)]);
    }

    #[test]
    fn a_chord_ends_the_burst_and_keeps_its_place() {
        let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let out = run(vec![(0, press(KeyCode::Char('a'))), (0, press(KeyCode::Char('b'))), (0, press(KeyCode::Char('c'))), (0, ctrl_c.clone()), (0, press(KeyCode::Char('d')))]);
        assert_eq!(out, vec![Event::Paste("abc".into()), ctrl_c, press(KeyCode::Char('d'))]);
    }
}
