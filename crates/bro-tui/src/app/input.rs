//! Key routing: overlays → inline editors (rename) → prefix table → direct chords → sidebar →
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
        self.link_press = None;
        self.clipboard_request = None;
        let paste_key = (matches!(k.code, KeyCode::Char('v' | 'V'))
            && (k.modifiers == KeyModifiers::CONTROL || k.modifiers == (KeyModifiers::CONTROL | KeyModifiers::SHIFT)))
            || (k.code == KeyCode::Insert && k.modifiers == KeyModifiers::SHIFT);
        if !self.prefix_armed && paste_key {
            let request = std::time::Instant::now();
            self.clipboard_request = Some(request);
            // focus mode's box takes a picture too (a screenshot): text first, else the image
            let focus = self.focused().filter(|id| !self.overlay.is_open() && !self.side_focus && self.renaming.is_none() && self.focus_view(*id));
            if !cfg!(test) {
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let text = crate::clip::grab_text();
                    if text.is_none()
                        && let Some(id) = focus
                        && let Some(paths) = crate::clip::grab_image()
                    {
                        let _ = tx.send(Event::Clipboard(id, Some(paths), k));
                        return;
                    }
                    let _ = tx.send(Event::ClipboardText(request, text));
                });
            }
            return;
        }
        if self.overlay.is_open() {
            self.overlay_key(k);
            return;
        }
        if self.renaming.is_some() {
            self.rename_key(k);
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
            if !self.keymap.is_prefix(&k) && self.menu_key(k) {
                return;
            }
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
            self.open_menu(None);
            return;
        }
        // Codex uses Alt+Up to edit queued input. Keep prefix+k for pane navigation.
        if !self.side_focus && k.code == KeyCode::Up && k.modifiers == KeyModifiers::ALT
            && let Some(id) = self.focused().filter(|id| self.panes.get(id).and_then(|p| p.as_term_ref()).is_some_and(|t| t.meta.harness == Some(bro_core::Harness::Codex))) {
                self.with_pane(id, |p, cx| p.key(k, cx));
                return;
            }
        // focus mode is the window's: in the terminal UI its key stays the program's (alt+f moves a word on)
        if let Some(a) = self.keymap.direct(&k).filter(|a| *a != crate::keymap::Act::Focus || crate::panes::mirror::active()) {
            self.run_act(a);
            return;
        }
        if self.strip_sel.is_some() {
            self.strip_key(k);
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
        // typing into a card that's turned over turns it face up: you see where the keys land
        if self.flips.get(&id).is_some_and(|f| f.back) {
            self.flip_pane(id);
        }
        // focus mode: the keys write a reply (and answer the choice on screen)
        if self.focus_view(id) {
            return self.focus_key(id, k);
        }
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
                Outcome::None => self.remember_launcher_permission(),
                Outcome::Close => self.overlay = Overlay::None,
                Outcome::Profiles => {
                    self.overlay = Overlay::None;
                    self.open_view("profiles");
                }
                Outcome::Project(dir) => self.select_launcher_project(dir),
                Outcome::CreateProject(dir, request) => self.create_project_folder(dir, request),
                Outcome::Launch(spec, place) => {
                    self.overlay = Overlay::None;
                    self.launch_spec(spec, place);
                }
                Outcome::Resume(idx) => {
                    self.overlay = Overlay::None;
                    self.open_resume(idx);
                }
                Outcome::ResumeMany(idx) => {
                    self.overlay = Overlay::None;
                    self.resume_many(&idx);
                }
                Outcome::Archive(id, on) => self.continue_archive(id, on),
                Outcome::UndoArchive => self.undo_archive(),
                Outcome::Install(h) => {
                    self.overlay = Overlay::None;
                    self.install_agent(h);
                }
                Outcome::SignIn => {
                    self.overlay = Overlay::None;
                    self.sign_in_gemini();
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
            Overlay::Confirm(_) => self.confirm_key(k),
            Overlay::File(v) => {
                let o = v.key(k);
                self.file_view_outcome(o);
            }
            Overlay::AgentFiles(v) => {
                let o = v.key(k);
                self.agent_files_outcome(o);
            }
            Overlay::Team(v) => {
                let o = v.key(k);
                self.team_outcome(o);
            }
            Overlay::Watchdog(_) => self.watchdog_key(k),
            Overlay::WatchdogPrompt(_) => self.watchdog_prompt_key(k),
        }
    }

    fn rename_key(&mut self, k: KeyEvent) {
        let Some((id, mut text)) = self.renaming.take() else { return };
        match k.code {
            KeyCode::Enter => {
                let name = text.trim().to_string();
                if let Some(t) = self.panes.get_mut(&id).and_then(|p| p.as_term()) {
                    t.meta.name = (!name.is_empty()).then_some(name.clone());
                    t.meta.renamed = t.meta.name.is_some();
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

    pub(crate) fn sidebar_filter_changed(&mut self) {
        self.side_filtering = true;
        self.side_scroll = 0;
        self.side_manual_scroll = false;
        let rows = self.rows();
        self.side_sel = rows.iter().position(|r| matches!(r, Row::Project { .. })).unwrap_or(0);
        if let Some(root) = rows.get(self.side_sel).and_then(|r| self.row_root(&rows, r)) {
            self.pick_project(root);
        }
    }

    pub(crate) fn clear_sidebar_filter(&mut self) {
        self.side_filtering = false;
        if self.side.filter.is_empty() {
            return;
        }
        let selected = self.rows().get(self.side_sel).cloned();
        self.side.filter.clear();
        self.side_scroll = 0;
        self.side_manual_scroll = false;
        self.side_sel = self.rows().iter().position(|row| match (&selected, row) {
            (Some(Row::Project { key, .. }), Row::Project { key: other, .. }) => key == other,
            (Some(Row::Live { info, .. }), Row::Live { info: other, .. }) => info.pane == other.pane,
            _ => false,
        }).unwrap_or(0);
    }

    pub(crate) fn leave_sidebar(&mut self) {
        self.clear_sidebar_filter();
        self.side_focus = false;
        self.strip_sel = None;
    }

    /// Keys while the sidebar has focus (alt+b).
    fn sidebar_key(&mut self, k: KeyEvent) {
        self.side_manual_scroll = false;
        if let KeyCode::Char(c) = k.code
            && k.modifiers.difference(KeyModifiers::SHIFT).is_empty()
            && !c.is_control()
        {
            if c != '/' || self.side_filtering || !self.side.filter.is_empty() {
                self.side.filter.push(c);
            }
            self.sidebar_filter_changed();
            return;
        }
        let rows = self.rows();
        let n = rows.len();
        let sel = self.side_sel.min(n.saturating_sub(1));
        match k.code {
            KeyCode::Down => self.side_sel = (sel + 1).min(n.saturating_sub(1)),
            KeyCode::Up => self.side_sel = sel.saturating_sub(1),
            KeyCode::Home => self.side_sel = 0,
            KeyCode::End => self.side_sel = n.saturating_sub(1),
            KeyCode::PageDown => self.side_sel = (sel + 10).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.side_sel = sel.saturating_sub(10),
            KeyCode::Left => self.side_parent(),
            KeyCode::Right => {
                if matches!(rows.get(sel), Some(Row::Live { .. })) {
                    self.leave_sidebar(); // → back into the panes
                } else {
                    self.side_step_in();
                }
            }
            KeyCode::Enter if k.modifiers.contains(KeyModifiers::SHIFT) => {
                if let Some(Row::Live { info, .. }) = rows.get(sel) {
                    self.toggle_stack(info.pane);
                    self.side_focus = true;
                }
            }
            KeyCode::Enter => self.activate_row(sel),
            KeyCode::Delete => match rows.get(sel) {
                Some(Row::Live { info, .. }) => self.ask_close(info.pane),
                Some(Row::Project { root, live, .. }) => self.close_project(root.clone(), *live),
                _ => {}
            },
            KeyCode::Backspace => {
                self.side.filter.pop();
                self.sidebar_filter_changed();
            }
            KeyCode::Esc => {
                if self.side_filtering || !self.side.filter.is_empty() {
                    self.clear_sidebar_filter();
                } else if !self.tabs.is_empty() {
                    self.leave_sidebar();
                }
            }
            KeyCode::Tab
                if !self.tabs.is_empty() => {
                    self.leave_sidebar();
                }
            _ => {}
        }
        // a move shows the scrollbar, even one that's stopped at the end
        let nav = matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown);
        if nav || self.side_sel != sel {
            self.side_nav();
        }
        if self.side_focus {
            let rows = self.rows();
            if let Some(root) = rows.get(self.side_sel).and_then(|r| self.row_root(&rows, r)) {
                self.pick_project(root);
            }
        }
    }
}
