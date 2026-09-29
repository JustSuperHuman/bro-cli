//! Key routing: overlays → inline editors (rename, sidebar filter) → prefix table → direct chords → sidebar →
//! the focused pane. Terminals get every key that isn't one of ours.

use super::App;
use super::overlays::Overlay;
use crate::alerts::Kind;
use crate::launcher::Outcome;
use crate::palette::Out;
use crate::pane::Event;
use crate::sidebar::Row;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    pub(crate) fn key(&mut self, k: KeyEvent) {
        self.sel = None;
        if self.overlay.is_open() {
            self.overlay_key(k);
            return;
        }
        if self.renaming.is_some() {
            self.rename_key(k);
            return;
        }
        if self.side_filtering {
            self.filter_key(k);
            return;
        }
        // alt+v: paste a clipboard image / copied files into the terminal as paths
        if !self.prefix_armed && !self.side_focus && k.code == KeyCode::Char('v') && k.modifiers == KeyModifiers::ALT
            && let Some(id) = self.focused().filter(|id| self.panes.get(id).is_some_and(|p| p.is_terminal())) {
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let got = crate::clip::grab_image();
                    let _ = tx.send(Event::Clipboard(id, got, k));
                });
                return;
            }
        if self.prefix_armed {
            self.prefix_armed = false;
            if self.keymap.is_prefix(&k) {
                // prefix twice: send it through to the program
                if let Some(id) = self.focused() {
                    self.with_pane(id, |p, cx| p.key(k, cx));
                }
                return;
            }
            match self.keymap.after_prefix(&k) {
                Some(a) => self.run_act(a),
                None if k.code == KeyCode::Esc => {}
                None => self.toast(Kind::Info, format!("{} isn't bound after the prefix — ? lists them", crate::keymap::Chord::from_event(&k).display())),
            }
            return;
        }
        if self.keymap.is_prefix(&k) {
            self.prefix_armed = true;
            return;
        }
        if let Some(a) = self.keymap.direct(&k) {
            self.run_act(a);
            return;
        }
        if self.side_focus {
            self.sidebar_key(k);
            return;
        }
        let Some(id) = self.focused() else {
            self.welcome_key(k);
            return;
        };
        let used = self.with_pane(id, |p, cx| p.key(k, cx)).unwrap_or(false);
        let terminal = self.panes.get(&id).is_some_and(|p| p.is_terminal());
        if !used && !terminal {
            match k.code {
                KeyCode::Char('?') => self.run_act(crate::keymap::Act::Help),
                KeyCode::Esc => {
                    self.side_focus = true;
                    self.select_focused_row();
                }
                _ => {}
            }
        }
    }

    fn welcome_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Enter | KeyCode::Char('n') => self.run_act(crate::keymap::Act::NewSession),
            KeyCode::Char('s') => self.run_act(crate::keymap::Act::NewShell),
            KeyCode::Char('?') => self.run_act(crate::keymap::Act::Help),
            KeyCode::Char('p') => self.run_act(crate::keymap::Act::Palette),
            KeyCode::Char('b') | KeyCode::Tab | KeyCode::Left => {
                self.side_focus = true;
            }
            KeyCode::Char('q') => self.run_act(crate::keymap::Act::Quit),
            _ => {}
        }
    }

    fn overlay_key(&mut self, k: KeyEvent) {
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Launcher(l) => match l.key(k) {
                Outcome::None => {}
                Outcome::Close => self.overlay = Overlay::None,
                Outcome::Launch(spec, place) => {
                    self.overlay = Overlay::None;
                    self.launch_spec(spec, place);
                }
            },
            Overlay::Palette(p) => {
                let out = p.key(k);
                let preview = p.preview();
                let before = p.theme_before.clone();
                match out {
                    Out::None => {
                        let want = preview.unwrap_or(before);
                        if want != self.theme.name {
                            self.set_theme(&want, false);
                        }
                    }
                    Out::Cancel => {
                        self.overlay = Overlay::None;
                        self.set_theme(&before, false);
                    }
                    Out::Run(c) => {
                        self.overlay = Overlay::None;
                        if !matches!(c, crate::palette::Cmd::Theme(_)) && self.theme.name != before {
                            self.set_theme(&before, false);
                        }
                        self.run_palette(c);
                    }
                }
            }
            Overlay::Help(h) => {
                if h.key(k) {
                    self.overlay = Overlay::None;
                }
            }
            Overlay::Resume(_) => self.resume_key(k),
            Overlay::Folder(p) => match p.key(k) {
                crate::folder::Outcome::None => {}
                crate::folder::Outcome::Close => self.overlay = Overlay::None,
                crate::folder::Outcome::Open(dir) => {
                    self.overlay = Overlay::None;
                    self.add_project(dir);
                }
            },
            Overlay::Confirm(_) => self.confirm_key(k),
        }
    }

    fn rename_key(&mut self, k: KeyEvent) {
        let Some((id, mut text)) = self.renaming.take() else { return };
        match k.code {
            KeyCode::Enter => {
                let name = text.trim().to_string();
                if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                    t.meta.name = (!name.is_empty()).then_some(name.clone());
                    let shown = t.display_name();
                    self.svc.bridge_title(&t.meta.sid, &shown);
                }
            }
            KeyCode::Esc => {}
            KeyCode::Backspace => {
                text.pop();
                self.renaming = Some((id, text));
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                if text.chars().count() < 40 {
                    text.push(c);
                }
                self.renaming = Some((id, text));
            }
            _ => self.renaming = Some((id, text)),
        }
    }

    fn filter_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.side.filter.clear();
                self.side_filtering = false;
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::Tab => {
                self.side_filtering = false;
                self.side_sel = 0;
            }
            KeyCode::Backspace => {
                self.side.filter.pop();
                self.side_sel = 0;
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.side.filter.push(c);
                self.side_sel = 0;
            }
            _ => {}
        }
    }

    /// Keys while the sidebar has focus (alt+b).
    fn sidebar_key(&mut self, k: KeyEvent) {
        let rows = self.rows();
        let n = rows.len();
        let sel = self.side_sel.min(n.saturating_sub(1));
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => self.side_sel = (sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => self.side_sel = sel.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.side_sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.side_sel = n.saturating_sub(1),
            KeyCode::PageDown => self.side_sel = (sel + 10).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.side_sel = sel.saturating_sub(10),
            KeyCode::Char('h') | KeyCode::Left => self.side_collapse(),
            KeyCode::Char('l') | KeyCode::Right => {
                if matches!(rows.get(sel), Some(Row::Live { .. })) {
                    self.side_focus = false; // → back into the panes
                } else {
                    self.side_expand();
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_row(sel),
            KeyCode::Char('f') => match rows.get(sel) {
                Some(Row::Past { info }) => self.open_resume(info.idx),
                Some(Row::Live { info, .. }) => self.open_switch(info.pane),
                _ => self.toast(Kind::Info, "f forks an earlier session into another profile"),
            },
            KeyCode::Char('r') => match rows.get(sel) {
                Some(Row::Live { info, .. }) => self.start_rename(info.pane),
                Some(Row::Past { info }) => self.resume(info.idx),
                _ => {}
            },
            KeyCode::Char('x') | KeyCode::Delete => match rows.get(sel) {
                Some(Row::Live { info, .. }) => self.ask_close(info.pane),
                Some(Row::Project { root, live, .. }) => self.close_project(root.clone(), *live),
                _ => {}
            },
            KeyCode::Char('o') => self.open_folder(),
            KeyCode::Char('n') => {
                let dir = self.preferred_dir();
                self.open_launcher(dir, crate::pane::Place::Tab);
            }
            KeyCode::Char('s') => {
                let dir = self.preferred_dir();
                self.open_shell(dir, crate::pane::Place::Tab);
                self.side_focus = false;
            }
            KeyCode::Char('a') => self.archive_row(sel),
            KeyCode::Char('u') => self.undo_archive(),
            KeyCode::Char('A') => self.toggle_show_archived(),
            KeyCode::Char('/') => {
                self.side_filtering = true;
                self.side_sel = 0;
            }
            KeyCode::Char('?') => self.run_act(crate::keymap::Act::Help),
            KeyCode::Esc => {
                if !self.side.filter.is_empty() {
                    self.side.filter.clear();
                } else if !self.tabs.is_empty() {
                    self.side_focus = false;
                }
            }
            KeyCode::Tab
                if !self.tabs.is_empty() => {
                    self.side_focus = false;
                }
            _ => {}
        }
        let rows = self.rows();
        if let Some(root) = rows.get(self.side_sel).and_then(|r| self.row_root(&rows, r)) {
            self.cur_project = Some(root);
        }
    }
}
