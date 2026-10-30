//! In the native terminal only the colon query is held locally. Selected Quips are attached to the next
//! message; the harness retains its editor, cursor, history, attachments, and normal key handling.

use crate::app::App;
use crate::alerts::Kind;
use crate::layout::PaneId;
use crate::pane::Activity;
use crate::quips::{self, Quip};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    pub(crate) fn terminal_quips_enabled(&self, id: PaneId) -> bool {
        (cfg!(test) || crate::panes::mirror::active()) && !self.focus_view(id)
            && self.panes.get(&id).and_then(|p| p.as_term_ref()).is_some_and(|t| t.meta.harness.is_some() && t.queued.is_none() && t.spawn_error().is_none())
    }

    pub(crate) fn terminal_quip_rows(&self, id: PaneId) -> Vec<usize> {
        self.focus.drafts.get(&id).and_then(|d| d.terminal_quips.query.as_deref()).map_or_else(Vec::new, |q| self.focus.quips.matching(q))
    }

    pub(crate) fn terminal_add_quip(&mut self, id: PaneId, index: usize) {
        let Some(quip) = self.focus.quips.items.get(index).cloned() else { return };
        let mut pending = self.focus.draft(id).terminal_quips.pending.clone();
        if !pending.iter().any(|q| q.trigger == quip.trigger) { pending.push(quip); }
        if let Err(error) = quips::expand_selected(&pending) { self.toast(Kind::Info, error); return; }
        let state = &mut self.focus.draft(id).terminal_quips;
        state.pending = pending;
        state.query = None;
        state.selected = 0;
    }

    pub(crate) fn terminal_remove_quip(&mut self, id: PaneId, index: usize) {
        let state = &mut self.focus.draft(id).terminal_quips;
        if index < state.pending.len() { state.pending.remove(index); }
    }

    pub(crate) fn terminal_flush_quip_query(&mut self, id: PaneId) {
        if let Some(query) = self.focus.draft(id).terminal_quips.query.take() {
            let literal = format!(":{query}");
            self.focus.draft(id).terminal_quips.before = literal.chars().next_back();
            self.with_pane(id, |p, cx| p.paste(&literal, cx));
        }
    }

    pub(crate) fn terminal_quip_key(&mut self, id: PaneId, key: KeyEvent) -> bool {
        if !self.terminal_quips_enabled(id) { return false; }
        if self.panes.get(&id).and_then(|p| p.activity()) == Some(Activity::Blocked) {
            self.focus.draft(id).terminal_quips.query = None;
            return false;
        }
        let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let submit = key.code == KeyCode::Enter && key.modifiers.is_empty();
        let cancel = matches!(key.code, KeyCode::Char('c' | 'u')) && key.modifiers.contains(KeyModifiers::CONTROL);
        if cancel {
            self.focus.draft(id).terminal_quips = quips::Terminal::default();
            return false;
        }
        if self.focus.draft(id).terminal_quips.query.is_some() {
            let rows = self.terminal_quip_rows(id);
            let state = &mut self.focus.draft(id).terminal_quips;
            let selected = state.selected.min(rows.len().saturating_sub(1));
            match key.code {
                KeyCode::Char(c) if plain && (c.is_ascii_alphanumeric() || c == '-') && state.query.as_ref().is_some_and(|q| q.len() < 48) => {
                    state.query.as_mut().unwrap().push(c);
                    state.selected = 0;
                    return true;
                }
                KeyCode::Backspace if plain => {
                    if state.query.as_mut().unwrap().pop().is_none() { state.query = None; }
                    state.selected = 0;
                    return true;
                }
                KeyCode::Up | KeyCode::Down if plain && !rows.is_empty() => {
                    state.selected = if key.code == KeyCode::Up { (selected + rows.len() - 1) % rows.len() } else { (selected + 1) % rows.len() };
                    return true;
                }
                KeyCode::Tab | KeyCode::Enter if key.modifiers.is_empty() && !rows.is_empty() => {
                    self.terminal_add_quip(id, rows[selected]);
                    return true;
                }
                KeyCode::Char(' ') if plain => {
                    let query = state.query.clone().unwrap_or_default();
                    if let Some(index) = rows.iter().copied().find(|i| self.focus.quips.items[*i].trigger.eq_ignore_ascii_case(&query)) {
                        self.terminal_add_quip(id, index);
                        return true;
                    }
                }
                KeyCode::Esc => { self.terminal_flush_quip_query(id); return true; }
                _ => {}
            }
            // A non-Quip (time, URL, code, etc.) reaches the native editor exactly as typed.
            self.terminal_flush_quip_query(id);
        }
        if submit {
            let state = &mut self.focus.draft(id).terminal_quips;
            let is_command = state.command == Some(true);
            state.command = None;
            state.before = None;
            state.code_ticks = 0;
            state.tick_run = 0;
            if !is_command && !state.pending.is_empty() {
                let instructions = match quips::expand_selected(&state.pending) {
                    Ok(text) => text,
                    Err(error) => { self.toast(Kind::Info, error); return true; }
                };
                // End moves to a line boundary without replacing any native text or pasted pictures.
                self.with_pane(id, |p, cx| p.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE), cx));
                if let Some(term) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                    term.send_message(&format!("\n\n{instructions}"));
                    self.focus.draft(id).terminal_quips.pending.clear();
                }
                return true;
            }
        } else if key.code == KeyCode::Esc {
            self.focus.draft(id).terminal_quips = quips::Terminal::default();
        } else if let KeyCode::Char(c) = key.code {
            if plain {
                let before = self.focus.drafts.get(&id).and_then(|d| d.terminal_quips.before)
                    .or_else(|| self.panes.get(&id).and_then(|p| p.as_term_ref()).and_then(|t| t.input_before_cursor()));
                let state = &mut self.focus.draft(id).terminal_quips;
                state.observe_text(c.encode_utf8(&mut [0; 4]));
                if c == ':' && state.code_ticks == 0 && state.command != Some(true) && before.is_none_or(|c| c.is_whitespace() || "([{\"'".contains(c)) {
                    state.query = Some(String::new());
                    state.selected = 0;
                    return true;
                }
                if state.command.is_none() && !c.is_whitespace() { state.command = Some(c == '/' || c == '!'); }
                state.before = Some(c);
            }
        } else if matches!(key.code, KeyCode::Backspace | KeyCode::Delete | KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End) {
            self.focus.draft(id).terminal_quips.before = None;
        }
        false
    }

    /// Pasted explicit tokens become local tags too; code and ordinary punctuation keep their text.
    pub(crate) fn terminal_quip_paste(&mut self, id: PaneId, text: &str) -> bool {
        if !self.terminal_quips_enabled(id) || self.panes.get(&id).and_then(|p| p.activity()) == Some(Activity::Blocked) { return false; }
        if self.focus.draft(id).terminal_quips.query.is_some() {
            if text.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
                self.focus.draft(id).terminal_quips.query.as_mut().unwrap().push_str(text);
                self.focus.draft(id).terminal_quips.selected = 0;
                return true;
            }
            self.terminal_flush_quip_query(id);
        }
        let state = &mut self.focus.draft(id).terminal_quips;
        state.finish_ticks();
        if state.command == Some(true) || (state.command.is_none() && text.trim_start().starts_with(|c| c == '/' || c == '!')) {
            let state = &mut self.focus.draft(id).terminal_quips;
            state.command = Some(true);
            state.before = text.chars().next_back();
            return false;
        }
        let before = state.before.filter(|c| *c != '`').map(|c| c.to_string()).unwrap_or_default();
        let prefix = if state.code_ticks > 0 { format!("{} {before}", "`".repeat(state.code_ticks)) } else { before };
        let joined = format!("{prefix}{text}");
        let found: Vec<_> = self.focus.quips.matches(&joined).into_iter().filter(|(r, _)| r.start >= prefix.len()).map(|(r, q)| ((r.start - prefix.len())..(r.end - prefix.len()), q.clone())).collect();
        let mut pending = self.focus.draft(id).terminal_quips.pending.clone();
        let mut out = String::new();
        let mut at = 0;
        for (range, quip) in found {
            out.push_str(&text[at..range.start]);
            at = range.end;
            if !pending.iter().any(|q: &Quip| q.trigger == quip.trigger) { pending.push(quip); }
        }
        if let Err(error) = quips::expand_selected(&pending) { self.toast(Kind::Info, error); return true; }
        out.push_str(&text[at..]);
        let state = &mut self.focus.draft(id).terminal_quips;
        state.pending = pending;
        state.observe_text(&out);
        state.before = out.chars().next_back();
        if state.command.is_none() { state.command = out.chars().find(|c| !c.is_whitespace()).map(|c| c == '/' || c == '!'); }
        if at == 0 { return false; }
        if !out.is_empty() { self.with_pane(id, |p, cx| p.paste(&out, cx)); }
        true
    }
}
