//! The Quips picker and editor share the reply box's keyboard and paste routing.

use crate::app::App;
use crate::layout::PaneId;
use crate::quips::{self, Editor};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    pub(crate) fn open_quips(&mut self) {
        if !cfg!(test) && !crate::panes::mirror::active() {
            self.toast(crate::alerts::Kind::Info, "Quips are in the desktop app: bro gui");
            return;
        }
        let id = self.focused().unwrap_or(0);
        self.leave_sidebar();
        self.focus.draft(id).quips.open = false;
        self.focus_toggle_quips(id);
        self.overlay = crate::app::Overlay::Quips(id);
    }

    pub(crate) fn quip_can_insert(&self, id: PaneId) -> bool {
        !matches!(self.overlay, crate::app::Overlay::Quips(_)) && self.panes.get(&id).and_then(|p| p.as_term_ref()).is_some_and(super::focusable)
    }

    pub(crate) fn close_quips(&mut self, id: PaneId) {
        if self.terminal_quips_enabled(id) && self.focus.drafts.get(&id).is_some_and(|d| d.terminal_quips.query.is_some()) && !matches!(self.overlay, crate::app::Overlay::Quips(_)) {
            self.terminal_flush_quip_query(id);
            return;
        }
        let d = self.focus.draft(id);
        d.quips = quips::Picker { dismissed: Some((d.area.text().into(), d.area.cursor())), ..Default::default() };
        if matches!(self.overlay, crate::app::Overlay::Quips(owner) if owner == id) { self.overlay = crate::app::Overlay::None; }
    }

    pub(crate) fn focus_quip_rows(&self, id: PaneId) -> Vec<usize> {
        if !matches!(self.overlay, crate::app::Overlay::Quips(_)) && self.terminal_quips_enabled(id) { return self.terminal_quip_rows(id); }
        let Some(d) = self.focus.drafts.get(&id) else { return vec![] };
        if d.quips.editor.is_some() || d.menu.is_some() { return vec![]; }
        let query = if d.quips.open {
            d.quips.search.clone()
        } else {
            if d.quips.dismissed.as_ref().is_some_and(|(text, cursor)| text == d.area.text() && *cursor == d.area.cursor()) { return vec![]; }
            let Some((_, query)) = quips::query(d.area.text(), d.area.cursor()) else { return vec![] };
            query
        };
        self.focus.quips.matching(&query)
    }

    pub(crate) fn focus_toggle_quips(&mut self, id: PaneId) {
        let d = self.focus.draft(id);
        let open = !d.quips.open;
        d.quips = quips::Picker { open, ..Default::default() };
        if !open { d.quips.dismissed = Some((d.area.text().into(), d.area.cursor())); }
        d.menu = None;
    }

    pub(crate) fn focus_insert_quip(&mut self, id: PaneId, index: usize) {
        if !matches!(self.overlay, crate::app::Overlay::Quips(_)) && self.terminal_quips_enabled(id) { return self.terminal_add_quip(id, index); }
        if !self.quip_can_insert(id) { return self.focus_edit_quip(id, Some(index)); }
        let Some(token) = self.focus.quips.items.get(index).map(|q| q.token()) else { return };
        let d = self.focus.draft(id);
        let text = d.area.text();
        let range = quips::query(text, d.area.cursor()).map_or(d.area.cursor()..d.area.cursor(), |(range, _)| range);
        let before = &text[..range.start];
        let after = &text[range.end..];
        let left = if before.chars().next_back().is_some_and(|c| !c.is_whitespace() && !"([{\"'".contains(c)) { " " } else { "" };
        let right = if after.starts_with(char::is_whitespace) { "" } else { " " };
        let out = format!("{before}{left}{token}{right}{after}");
        let cursor = before.len() + left.len() + token.len() + right.len();
        d.area.set_text_at(&out, cursor);
        d.keep_tokens();
        d.recall = None;
        d.quips = quips::Picker::default();
    }

    pub(crate) fn focus_edit_quip(&mut self, id: PaneId, original: Option<usize>) {
        let editor = Editor::new(original, original.and_then(|i| self.focus.quips.items.get(i)));
        let p = &mut self.focus.draft(id).quips;
        p.open = true;
        p.editor = Some(editor);
        p.error = None;
        p.delete_armed = false;
    }

    pub(crate) fn focus_save_quip(&mut self, id: PaneId) {
        let Some(e) = self.focus.drafts.get(&id).and_then(|d| d.quips.editor.as_ref()) else { return };
        let mut items = self.focus.quips.items.clone();
        let value = e.value();
        if let Some(index) = e.original {
            items[index] = value;
        } else { items.push(value); }
        match self.focus.quips.save_items(items) {
            Ok(()) => {
                let p = &mut self.focus.draft(id).quips;
                p.editor = None;
                p.search.clear();
                p.error = None;
                p.selected = 0;
            }
            Err(e) => self.focus.draft(id).quips.error = Some(e),
        }
    }

    pub(crate) fn focus_delete_quip(&mut self, id: PaneId) {
        let rows = self.focus_quip_rows(id);
        let p = &mut self.focus.draft(id).quips;
        let Some(index) = rows.get(p.selected.min(rows.len().saturating_sub(1))).copied() else { return };
        if !p.delete_armed { p.delete_armed = true; return; }
        let mut items = self.focus.quips.items.clone();
        items.remove(index);
        match self.focus.quips.save_items(items) {
            Ok(()) => { let p = &mut self.focus.draft(id).quips; p.delete_armed = false; p.selected = p.selected.saturating_sub(1); }
            Err(e) => self.focus.draft(id).quips.error = Some(e),
        }
    }

    pub(crate) fn focus_quips_key(&mut self, id: PaneId, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let d = self.focus.draft(id);
        if let Some(e) = &mut d.quips.editor {
            match k.code {
                KeyCode::Esc => { d.quips.editor = None; d.quips.error = None; }
                KeyCode::Char('s') if ctrl => self.focus_save_quip(id),
                KeyCode::Tab | KeyCode::BackTab => e.field = (e.field + if shift || k.code == KeyCode::BackTab { 2 } else { 1 }) % 3,
                KeyCode::Enter if e.field < 2 => e.field += 1,
                _ => { e.fields[e.field].key(k, e.cols); }
            }
            return true;
        }
        let manual = d.quips.open;
        let rows = self.focus_quip_rows(id);
        if !manual && (rows.is_empty() || ctrl || k.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) || !matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::Enter | KeyCode::Esc)) { return false; }
        let p = &mut self.focus.draft(id).quips;
        let sel = p.selected.min(rows.len().saturating_sub(1));
        if !matches!(k.code, KeyCode::Char('d') if ctrl) { p.delete_armed = false; }
        match k.code {
            KeyCode::Esc => {
                self.close_quips(id);
            }
            KeyCode::Up => p.selected = (sel + rows.len().saturating_sub(1)) % rows.len().max(1),
            KeyCode::Down => p.selected = (sel + 1) % rows.len().max(1),
            KeyCode::Tab | KeyCode::Enter => if let Some(index) = rows.get(sel) { self.focus_insert_quip(id, *index); },
            KeyCode::Char('n') if ctrl => self.focus_edit_quip(id, None),
            KeyCode::Char('e') if ctrl => if let Some(index) = rows.get(sel) { self.focus_edit_quip(id, Some(*index)); },
            KeyCode::Char('d') if ctrl => self.focus_delete_quip(id),
            KeyCode::Backspace => { p.search.pop(); p.selected = 0; },
            KeyCode::Char('u') if ctrl => { p.search.clear(); p.selected = 0; },
            KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => { p.search.push(c); p.selected = 0; },
            _ => {}
        }
        true
    }
}
