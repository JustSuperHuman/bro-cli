//! A small text editor for overlays (the watchdog's milestones, its prompt): soft-wraps to its width at
//! spaces, moves by character, word and visual row, and keeps the cursor in view. No undo. A `selectable`
//! one (the desktop app's reply box) also selects: shift with a move, a drag, a double-click's word; typing,
//! a paste or a delete replace what's selected.

use crate::ui;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Frame, layout::Rect, style::{Modifier, Style}, text::Span};
use std::ops::Range;
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
    /// shift with a move selects (only where the selection is drawn: the desktop app's reply box)
    pub selectable: bool,
    /// the selection's fixed end; the cursor is the end that moves
    anchor: Option<usize>,
}

fn cw(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// What a double-click takes around byte `at`: a word (letters, digits, `_`), a run of spaces, or a run of
/// other marks. Never across a line break.
pub(crate) fn word_range(text: &str, at: usize) -> Range<usize> {
    fn class(c: char) -> u8 {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c == '\n' {
            3
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    }
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    // the character under the press; at a line's end (or the text's), the one before it
    let under = text[at..].chars().next().filter(|c| *c != '\n');
    let (start_at, kind) = match under {
        Some(c) => (at, class(c)),
        None => match text[..at].chars().next_back().filter(|c| *c != '\n') {
            Some(c) => (at - c.len_utf8(), class(c)),
            None => return at..at,
        },
    };
    let start = text[..start_at].char_indices().rev().take_while(|(_, c)| class(*c) == kind).last().map_or(start_at, |(i, _)| i);
    let end = text[start_at..].char_indices().find(|(_, c)| class(*c) != kind).map_or(text.len(), |(i, _)| start_at + i);
    start..end
}

/// The line (between line breaks) holding byte `at`: what a triple-click takes.
pub(crate) fn line_range(text: &str, at: usize) -> Range<usize> {
    let at = at.min(text.len());
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    start..end
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
        self.anchor = None;
    }

    /// Type or paste `s` at the cursor, over the selection when there is one.
    pub fn insert(&mut self, s: &str) {
        let s = clean(s, true);
        self.delete_selection();
        self.text.insert_str(self.cur, &s);
        self.cur += s.len();
        self.goal = None;
    }

    /// The selected bytes, when something is selected.
    pub(crate) fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor.filter(|a| *a != self.cur)?;
        Some(a.min(self.cur)..a.max(self.cur))
    }

    /// The selected text.
    pub(crate) fn selected(&self) -> Option<&str> {
        self.selection().map(|r| &self.text[r])
    }

    /// The end of a selection that stays put (the cursor when nothing is selected): a shift+click grows from it.
    pub(crate) fn anchor(&self) -> usize {
        self.anchor.unwrap_or(self.cur)
    }

    /// Select from `anchor` to `cursor` (bytes, back to character boundaries); the same byte selects nothing.
    pub(crate) fn select(&mut self, anchor: usize, cursor: usize) {
        let anchor = self.boundary(anchor);
        self.cur = self.boundary(cursor);
        self.anchor = (anchor != self.cur).then_some(anchor);
        self.goal = None;
    }

    pub(crate) fn select_all(&mut self) {
        self.select(0, self.text.len());
    }

    pub(crate) fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Take out what's selected (the cursor where it was); false when nothing was.
    pub(crate) fn delete_selection(&mut self) -> bool {
        let Some(r) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(r.clone(), "");
        self.cur = r.start;
        self.anchor = None;
        self.goal = None;
        true
    }

    fn boundary(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
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
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let moves = matches!(k.code, KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown);
        if self.selectable {
            if moves && shift {
                // shift with a move: the selection grows from where it started (or from the cursor)
                let anchor = self.anchor();
                // the move itself, as without shift (nothing selected while it runs)
                self.anchor = None;
                let used = match k.code {
                    // past the first or last row: to the start or the end
                    KeyCode::Up if !alt && !ctrl => self.vertical(-1, width) || { self.cur = 0; true },
                    KeyCode::Down if !alt && !ctrl => self.vertical(1, width) || { self.cur = self.text.len(); true },
                    _ => self.key(KeyEvent::new(k.code, k.modifiers.difference(KeyModifiers::SHIFT)), width),
                };
                self.anchor = (anchor != self.cur).then_some(anchor);
                return used;
            }
            if let Some(r) = self.selection() {
                match k.code {
                    // a plain left or right from a selection lands on its edge
                    KeyCode::Left | KeyCode::Right if !ctrl && !alt => {
                        self.cur = if k.code == KeyCode::Left { r.start } else { r.end };
                        self.anchor = None;
                        self.goal = None;
                        return true;
                    }
                    // a delete takes what's selected, and only that
                    KeyCode::Backspace | KeyCode::Delete => return self.delete_selection(),
                    KeyCode::Char('w' | 'h' | 'u' | 'k') if ctrl => return self.delete_selection(),
                    // a newline goes in its place
                    KeyCode::Enter if self.newline_on_enter || alt || shift => {
                        self.delete_selection();
                    }
                    _ => {}
                }
            }
            if moves || (ctrl && matches!(k.code, KeyCode::Char('a' | 'e'))) {
                self.anchor = None;
            }
        }
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
        self.anchor = None;
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

    /// Put the cursor at byte `at` (a click in the desktop app), back to a character boundary. Nothing stays
    /// selected.
    pub(crate) fn set_cursor(&mut self, at: usize) {
        self.cur = self.boundary(at);
        self.goal = None;
        self.anchor = None;
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

    fn with(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    fn selectable(text: &str) -> TextArea {
        let mut t = TextArea::new(text);
        t.selectable = true;
        t
    }

    #[test]
    fn shift_moves_select_and_plain_moves_land_on_the_selections_edge() {
        let mut t = selectable("hello brave world");
        t.key(with(KeyCode::Left, KeyModifiers::SHIFT), 40);
        t.key(with(KeyCode::Left, KeyModifiers::SHIFT), 40);
        assert_eq!(t.selected(), Some("ld"));
        // ctrl+shift+left grows by a word from the same anchor
        t.key(with(KeyCode::Left, KeyModifiers::SHIFT | KeyModifiers::CONTROL), 40);
        assert_eq!(t.selected(), Some("world"));
        // shift back the other way shrinks it, through the anchor and out the other side
        t.key(with(KeyCode::End, KeyModifiers::SHIFT), 40);
        assert_eq!(t.selection(), None, "back at the anchor nothing is selected");
        t.key(with(KeyCode::Home, KeyModifiers::SHIFT), 40);
        assert_eq!(t.selected(), Some("hello brave world"));
        assert!(t.key(key(KeyCode::Right), 40));
        assert_eq!((t.selection(), t.cursor()), (None, t.text().len()), "right lands on the selection's end");
        t.select(6, 11);
        assert!(t.key(key(KeyCode::Left), 40));
        assert_eq!((t.selection(), t.cursor()), (None, 6), "left lands on its start");
        t.select(6, 11);
        t.key(key(KeyCode::Home), 40);
        assert_eq!((t.selection(), t.cursor()), (None, 0), "other moves go from the cursor and deselect");
    }

    #[test]
    fn shift_up_and_down_select_by_row_and_past_the_ends() {
        let mut t = selectable("first line\nsecond line");
        t.set_cursor(14);
        t.key(with(KeyCode::Up, KeyModifiers::SHIFT), 40);
        assert_eq!(t.selected(), Some("st line\nsec"), "up a row at the same column");
        assert!(t.key(with(KeyCode::Up, KeyModifiers::SHIFT), 40), "shift+up on the first row selects to the start");
        assert_eq!(t.selected(), Some("first line\nsec"));
        t.set_cursor(3);
        assert!(t.key(with(KeyCode::Down, KeyModifiers::SHIFT), 40));
        assert!(t.key(with(KeyCode::Down, KeyModifiers::SHIFT), 40));
        assert_eq!(t.selected(), Some("st line\nsecond line"));
        // without shift, up on the first row still belongs to the owner (the reply box's history)
        t.set_cursor(0);
        assert!(!t.key(key(KeyCode::Up), 40));
    }

    #[test]
    fn typing_pasting_and_deleting_replace_the_selection() {
        let mut t = selectable("ship the offline sync");
        t.select(9, 16);
        assert_eq!(t.selected(), Some("offline"));
        t.key(key(KeyCode::Char('o')), 40);
        t.key(key(KeyCode::Char('n')), 40);
        assert_eq!((t.text(), t.cursor(), t.selection()), ("ship the on sync", 11, None));
        t.select(5, 8);
        t.insert("a\r\nfew");
        assert_eq!(t.text(), "ship a\nfew on sync");
        t.select(0, 5);
        assert!(t.key(key(KeyCode::Backspace), 40));
        assert_eq!(t.text(), "a\nfew on sync", "backspace takes only the selection");
        t.select(1, 2);
        assert!(t.key(key(KeyCode::Delete), 40));
        assert_eq!(t.text(), "afew on sync");
        t.select(0, 4);
        assert!(t.key(with(KeyCode::Char('w'), KeyModifiers::CONTROL), 40));
        assert_eq!(t.text(), " on sync", "ctrl+w deletes the selection, not a word before it");
        t.select(0, 3);
        assert!(t.key(with(KeyCode::Enter, KeyModifiers::SHIFT), 40));
        assert_eq!(t.text(), "\n sync", "shift+enter's newline replaces it too");
        t.select_all();
        assert_eq!(t.selected(), Some("\n sync"));
        assert!(!t.key(key(KeyCode::Enter), 40), "plain enter is still the owner's");
        assert_eq!(t.text(), "\n sync", "and doesn't touch the selection");
        t.set_text("new");
        assert_eq!(t.selection(), None, "replacing the text ends the selection");
    }

    #[test]
    fn a_field_that_doesnt_draw_a_selection_never_makes_one() {
        let mut t = TextArea::new("hello world");
        t.key(with(KeyCode::Left, KeyModifiers::SHIFT), 40);
        assert_eq!(t.selection(), None);
        assert_eq!(t.cursor(), 10, "shift+left still moves");
        t.key(key(KeyCode::Char('!')), 40);
        assert_eq!(t.text(), "hello worl!d");
    }

    #[test]
    fn selection_stays_on_character_boundaries() {
        let mut t = selectable("café 🙂 ok");
        t.select(4, 1000);
        assert_eq!(t.selected(), Some("é 🙂 ok"));
        t.select(5, 8);
        assert_eq!(t.selection(), Some(5..6), "inside the emoji goes back to its start");
        t.select(3, 3);
        assert_eq!(t.selection(), None);
    }

    #[test]
    fn double_and_triple_click_ranges() {
        let s = "let snake_case = foo::bar(1);  next\nline two";
        let w = |at: usize| &s[word_range(s, at)];
        assert_eq!(w(6), "snake_case");
        assert_eq!(w(4), "snake_case", "the first letter");
        assert_eq!(w(3), " ", "a space alone");
        assert_eq!(w(20), "::");
        assert_eq!(w(29), "  ", "a run of spaces");
        assert_eq!(w(35), "next", "at a line's end: the word before it");
        assert_eq!(w(s.len()), "two", "at the end: the last word");
        assert_eq!(w(0), "let");
        assert_eq!(&"é🙂ab"[word_range("é🙂ab", 1)], "é", "never inside a character");
        assert_eq!(word_range("", 0), 0..0);
        assert_eq!(word_range("a\n\nb", 2), 2..2, "an empty line takes nothing");
        assert_eq!(&s[line_range(s, 6)], "let snake_case = foo::bar(1);  next");
        assert_eq!(&s[line_range(s, s.len())], "line two");
    }
}
