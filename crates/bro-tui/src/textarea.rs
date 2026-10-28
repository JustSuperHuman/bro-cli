//! A small text editor for overlays (the watchdog's milestones, its prompt): soft-wraps to its width at
//! spaces, moves by character, word and visual row, and keeps the cursor in view. No undo.

use crate::ui;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Frame, layout::Rect, style::{Modifier, Style}, text::Span};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Debug, Default)]
pub struct TextArea {
    text: String,
    /// byte offset of the cursor
    cur: usize,
    /// Enter types a newline (the prompt editor); otherwise only shift / alt+enter do and plain Enter is
    /// left to the owner (a milestone field adds the next one).
    pub newline_on_enter: bool,
    /// first visual row shown
    scroll: usize,
    /// the column up / down aims for
    goal: Option<usize>,
}

fn cw(c: char) -> usize {
    c.width().unwrap_or(0)
}

impl TextArea {
    pub fn new(text: &str) -> TextArea {
        let text = clean(text, true);
        TextArea { cur: text.len(), text, ..TextArea::default() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = clean(text, true);
        self.cur = self.text.len();
        self.scroll = 0;
        self.goal = None;
    }

    pub fn insert(&mut self, s: &str) {
        let s = clean(s, true);
        self.text.insert_str(self.cur, &s);
        self.cur += s.len();
        self.goal = None;
    }

    /// Visual rows at `width`: byte ranges into the text, soft-wrapped at spaces.
    pub fn rows(&self, width: usize) -> Vec<(usize, usize)> {
        let width = width.max(4);
        let mut rows = vec![];
        let mut line_start = 0;
        for line in self.text.split('\n') {
            let mut start = line_start;
            let mut used = 0;
            let mut last_space: Option<usize> = None;
            for (i, c) in line.char_indices() {
                let at = line_start + i;
                let w = cw(c);
                if used + w > width && at > start {
                    // break after the last space on this row, else mid-word
                    let end = match last_space {
                        Some(sp) if sp > start => sp,
                        _ => at,
                    };
                    rows.push((start, end));
                    start = end;
                    used = ui::width(&self.text[start..at]);
                    last_space = None;
                }
                used += w;
                if c == ' ' {
                    last_space = Some(at + 1);
                }
            }
            rows.push((start, line_start + line.len()));
            line_start += line.len() + 1;
        }
        rows
    }

    /// (visual row, display column) of the cursor.
    fn cursor_at(&self, rows: &[(usize, usize)]) -> (usize, usize) {
        let row = rows.iter().rposition(|&(s, e)| self.cur >= s && self.cur <= e).unwrap_or(0);
        let (s, _) = rows[row];
        (row, ui::width(&self.text[s..self.cur]))
    }

    /// Rows this text needs at `width`.
    pub fn height(&self, width: usize) -> usize {
        self.rows(width).len().max(1)
    }

    fn prev(&self) -> usize {
        self.text[..self.cur].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cur..].chars().next().map_or(self.cur, |c| self.cur + c.len_utf8())
    }

    fn word_left(&self) -> usize {
        let head = &self.text[..self.cur];
        let trimmed = head.trim_end_matches(|c: char| c.is_whitespace());
        trimmed.rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + 1)
    }

    fn word_right(&self) -> usize {
        let tail = &self.text[self.cur..];
        let skip = tail.len() - tail.trim_start_matches(|c: char| c.is_whitespace()).len();
        let rest = &tail[skip..];
        self.cur + skip + rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len())
    }

    /// Move one visual row up (-1) or down (+1); false at the edge (the owner may move focus instead).
    fn vertical(&mut self, dir: isize, width: usize) -> bool {
        let rows = self.rows(width);
        let (row, col) = self.cursor_at(&rows);
        let goal = *self.goal.get_or_insert(col);
        let target = row as isize + dir;
        if target < 0 || target as usize >= rows.len() {
            return false;
        }
        let (s, e) = rows[target as usize];
        let mut at = s;
        let mut used = 0;
        for (i, c) in self.text[s..e].char_indices() {
            if used + cw(c) > goal {
                break;
            }
            used += cw(c);
            at = s + i + c.len_utf8();
        }
        // a soft-wrapped row ends where the next begins: stay on this one
        if at == e && target as usize + 1 < rows.len() && rows[target as usize + 1].0 == e && e > s {
            at = self.text[..e].char_indices().next_back().map_or(s, |(i, _)| i).max(s);
        }
        self.cur = at;
        true
    }

    /// Handle a key; false when it wasn't for the editor (Enter without newline_on_enter, Up on the first row,
    /// Down on the last, Tab, Esc…).
    pub fn key(&mut self, k: KeyEvent, width: usize) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let keep_goal = matches!(k.code, KeyCode::Up | KeyCode::Down);
        let used = match k.code {
            KeyCode::Char(c) if ctrl => match c {
                'a' => {
                    self.cur = self.rows(width).iter().rev().find(|&&(s, _)| s <= self.cur).map_or(0, |r| r.0);
                    true
                }
                'e' => {
                    let rows = self.rows(width);
                    let (row, _) = self.cursor_at(&rows);
                    self.cur = rows[row].1;
                    true
                }
                'w' | 'h' => {
                    let to = if c == 'w' { self.word_left() } else { self.prev() };
                    self.text.replace_range(to..self.cur, "");
                    self.cur = to;
                    true
                }
                'u' => {
                    let start = self.text[..self.cur].rfind('\n').map_or(0, |i| i + 1);
                    self.text.replace_range(start..self.cur, "");
                    self.cur = start;
                    true
                }
                'k' => {
                    let end = self.text[self.cur..].find('\n').map_or(self.text.len(), |i| self.cur + i);
                    self.text.replace_range(self.cur..end, "");
                    true
                }
                _ => false,
            },
            KeyCode::Char(c) => {
                let mut b = [0u8; 4];
                self.insert(c.encode_utf8(&mut b));
                true
            }
            KeyCode::Enter if self.newline_on_enter || alt || k.modifiers.contains(KeyModifiers::SHIFT) => {
                self.text.insert(self.cur, '\n');
                self.cur += 1;
                true
            }
            KeyCode::Backspace if alt || ctrl => {
                let to = self.word_left();
                self.text.replace_range(to..self.cur, "");
                self.cur = to;
                true
            }
            KeyCode::Backspace => {
                let to = self.prev();
                self.text.replace_range(to..self.cur, "");
                self.cur = to;
                true
            }
            KeyCode::Delete => {
                let to = self.next();
                self.text.replace_range(self.cur..to, "");
                true
            }
            KeyCode::Left if ctrl || alt => {
                self.cur = self.word_left();
                true
            }
            KeyCode::Right if ctrl || alt => {
                self.cur = self.word_right();
                true
            }
            KeyCode::Left => {
                self.cur = self.prev();
                true
            }
            KeyCode::Right => {
                self.cur = self.next();
                true
            }
            KeyCode::Home if ctrl => {
                self.cur = 0;
                true
            }
            KeyCode::End if ctrl => {
                self.cur = self.text.len();
                true
            }
            KeyCode::Home => {
                let rows = self.rows(width);
                self.cur = rows[self.cursor_at(&rows).0].0;
                true
            }
            KeyCode::End => {
                let rows = self.rows(width);
                self.cur = rows[self.cursor_at(&rows).0].1;
                true
            }
            KeyCode::Up if !alt && !ctrl => self.vertical(-1, width),
            KeyCode::Down if !alt && !ctrl => self.vertical(1, width),
            KeyCode::PageUp => {
                for _ in 0..10 {
                    self.vertical(-1, width);
                }
                true
            }
            KeyCode::PageDown => {
                for _ in 0..10 {
                    self.vertical(1, width);
                }
                true
            }
            _ => false,
        };
        if used && !keep_goal {
            self.goal = None;
        }
        used
    }

    /// Cursor to the start (true) or end of the text: entering the field from above or below.
    pub fn cursor_to(&mut self, start: bool) {
        self.cur = if start { 0 } else { self.text.len() };
        self.goal = None;
    }

    /// Byte offset of the cursor (the desktop app draws its caret).
    pub(crate) fn cursor(&self) -> usize {
        self.cur
    }

    /// Replace the text, keeping the cursor at byte `at` (back to a character boundary).
    pub(crate) fn set_text_at(&mut self, text: &str, at: usize) {
        self.text = clean(text, true);
        self.goal = None;
        self.set_cursor(at);
    }

    /// Put the cursor at byte `at` (a click in the desktop app), back to a character boundary.
    pub(crate) fn set_cursor(&mut self, at: usize) {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        self.cur = at;
        self.goal = None;
    }

    /// Draw into `area`; a focused field shows its cursor. Empty text shows `placeholder`.
    pub fn render(&mut self, f: &mut Frame, area: Rect, style: Style, focused: bool, placeholder: &str, t: &crate::theme::Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let rows = self.rows(width.saturating_sub(1).max(1));
        let (crow, _) = self.cursor_at(&rows);
        let room = area.height as usize;
        if crow < self.scroll {
            self.scroll = crow;
        } else if crow >= self.scroll + room {
            self.scroll = crow + 1 - room;
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(room));
        let cursor = Style::default().add_modifier(Modifier::REVERSED);
        if self.text.is_empty() {
            let mut spans = vec![];
            if focused {
                spans.push(Span::styled(" ", cursor));
            }
            spans.push(Span::styled(placeholder.to_string(), ui::muted(t).add_modifier(Modifier::ITALIC)));
            ui::line(f, Rect { height: 1, ..area }, spans);
            return;
        }
        for (n, &(s, e)) in rows.iter().enumerate().skip(self.scroll).take(room) {
            let r = Rect { y: area.y + (n - self.scroll) as u16, height: 1, ..area };
            let text = &self.text[s..e];
            if focused && n == crow {
                let at = self.cur - s;
                let (head, tail) = text.split_at(at.min(text.len()));
                let mut chars = tail.chars();
                let under = chars.next();
                let mut spans = vec![Span::styled(head.to_string(), style)];
                spans.push(Span::styled(under.map_or(" ".to_string(), |c| c.to_string()), cursor));
                spans.push(Span::styled(chars.as_str().to_string(), style));
                ui::line(f, r, spans);
            } else {
                ui::line(f, r, vec![Span::styled(text.to_string(), style)]);
            }
        }
    }
}

/// Pasted / typed text made safe for the field: CRLF to LF, tabs to spaces, other control characters out.
fn clean(s: &str, multiline: bool) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ").chars().filter(|c| !c.is_control() || (multiline && *c == '\n')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn wraps_at_spaces_and_keeps_byte_ranges() {
        let t = TextArea::new("ship the offline sync\nthen docs");
        let rows: Vec<&str> = t.rows(10).iter().map(|&(s, e)| &t.text()[s..e]).collect();
        assert_eq!(rows, ["ship the ", "offline ", "sync", "then docs"]);
        let long = TextArea::new("abcdefghijkl");
        assert_eq!(long.rows(5).len(), 3, "a word longer than the row breaks mid-word");
        assert_eq!(TextArea::new("").rows(10), vec![(0, 0)]);
    }

    #[test]
    fn edits_and_moves() {
        let mut t = TextArea::new("hello world");
        assert!(t.key(key(KeyCode::Backspace), 40));
        assert_eq!(t.text(), "hello worl");
        t.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL), 40);
        assert_eq!(t.text(), "hello ");
        t.insert("there\r\nfriend\t!");
        assert_eq!(t.text(), "hello there\nfriend    !");
        assert!(!t.key(key(KeyCode::Enter), 40), "plain enter belongs to the owner");
        assert!(t.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), 40));
        assert!(t.text().ends_with("!\n"));
        // up from the last row lands on the row above; the first row's up is the owner's
        assert!(t.key(key(KeyCode::Up), 40));
        assert!(t.key(key(KeyCode::Up), 40));
        assert!(!t.key(key(KeyCode::Up), 40));
        t.key(key(KeyCode::Home), 40);
        t.key(key(KeyCode::Delete), 40);
        assert!(t.text().starts_with("ello"));
        let mut p = TextArea::new("a");
        p.newline_on_enter = true;
        assert!(p.key(key(KeyCode::Enter), 40));
        assert_eq!(p.text(), "a\n");
    }
}
