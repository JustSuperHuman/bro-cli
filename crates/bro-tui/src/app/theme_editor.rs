use super::App;
use crate::alerts::Kind;
use crate::theme::editor::{Editor, Field, Reply};

impl App {
    pub(crate) fn open_theme_editor(&mut self) {
        if !crate::fonts::GUI.load(std::sync::atomic::Ordering::Relaxed) {
            self.toast(Kind::Info,"The visual theme editor is available in the desktop window. JSON and TOML themes still work here.");
            return;
        }
        if let Some(editor) = &mut self.theme_editor {
            editor.minimized = false;
            return;
        }
        self.theme_editor = Some(Editor::new(&self.theme, self.tx.clone()));
    }
    pub(crate) fn open_aurora_editor(&mut self) {
        if self.theme_editor.as_ref().is_some_and(|e| e.saving) {
            return;
        }
        self.cancel_theme_editor();
        let mut editor = Editor::new(&crate::theme::get("aurora-studio"), self.tx.clone());
        editor.before = self.theme.clone();
        editor.name.set_text("Aurora Studio");
        editor.advanced = false;
        editor.preview();
        self.theme_editor = Some(editor);
    }

    pub(crate) fn cancel_theme_editor(&mut self) {
        if self.theme_editor.as_ref().is_some_and(|e| e.saving) {
            return;
        }
        if let Some(editor) = self.theme_editor.take() {
            self.theme = editor.before;
            crate::icons::retint(self.theme.is_light());
        }
    }
    pub(crate) fn theme_editor_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Draft {
                owner,
                revision,
                result,
            } => {
                let Some(editor) = self
                    .theme_editor
                    .as_mut()
                    .filter(|e| e.owner == owner && e.revision == revision)
                else {
                    return;
                };
                editor.pending = false;
                match result {
                    Ok(mut palette) => {
                        if let Some(source) = &editor.before.imported {
                            palette.ansi = source.ansi;
                        }
                        editor.file = palette.file.clone();
                        editor.error = None;
                        self.theme = palette.theme("__theme_preview");
                        crate::icons::retint(self.theme.is_light());
                    }
                    Err(error) => editor.error = Some(error),
                }
            }
            Reply::Saved { owner, result } => {
                if !self.theme_editor.as_ref().is_some_and(|e| e.owner == owner) {
                    return;
                }
                match result {
                    Ok(name) => {
                        self.theme_editor = None;
                        self.set_theme(&name, true);
                        self.toast(
                            Kind::Info,
                            format!("Saved {}", crate::theme::label(&self.theme)),
                        );
                    }
                    Err(error) => {
                        let editor = self.theme_editor.as_mut().unwrap();
                        editor.saving = false;
                        editor.save_error = Some(error);
                    }
                }
            }
        }
    }
    pub(crate) fn theme_editor_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::KeyCode;
        if self.theme_editor.is_none() {
            return false;
        }
        if key.code == KeyCode::Esc {
            if let Some(e) = &mut self.theme_editor {
                if e.inspect {
                    e.inspect = false;
                    return true;
                }
                if e.field == Field::Color {
                    e.cancel_color();
                    return true;
                }
                if e.field != Field::None {
                    e.field = Field::None;
                    return true;
                }
            }
            self.cancel_theme_editor();
            return true;
        }
        self.theme_editor
            .as_mut()
            .is_some_and(|e| e.inspect || (!e.minimized && e.input(key)))
    }
}
