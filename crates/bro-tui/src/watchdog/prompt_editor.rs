//! Editing the watchdog reviewer's standing instructions (settings → Watchdog prompt). Saved to
//! `[watchdog] prompt` in `~/.bro/v2.toml`; the text bro ships with is `bro_bridge::watchdog::DEFAULT_PROMPT`.

use crate::textarea::TextArea;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_bridge::watchdog::DEFAULT_PROMPT;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Frame, layout::Rect, style::Style, text::Span};

#[derive(Debug, PartialEq)]
pub enum Out {
    None,
    Close,
    /// Save this prompt; None = go back to bro's own.
    Save(Option<String>),
}

pub struct PromptEditor {
    area: TextArea,
    /// what it was when the editor opened
    saved: String,
    width: usize,
    height: usize,
    /// esc was pressed once with unsaved changes
    discard_armed: bool,
}

impl PromptEditor {
    pub fn new(current: Option<&str>) -> PromptEditor {
        let text = current.filter(|p| !p.trim().is_empty()).unwrap_or(DEFAULT_PROMPT);
        let mut area = TextArea::new(text);
        area.newline_on_enter = true;
        area.cursor_to(true);
        PromptEditor { area, saved: text.to_string(), width: 80, height: 20, discard_armed: false }
    }

    fn dirty(&self) -> bool {
        self.area.text() != self.saved
    }

    pub fn paste(&mut self, text: &str) {
        self.area.insert(text);
        self.discard_armed = false;
    }

    pub fn key(&mut self, k: KeyEvent) -> Out {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('s') if ctrl => {
                let text = self.area.text().trim();
                Out::Save((!text.is_empty() && text != DEFAULT_PROMPT.trim()).then(|| text.to_string()))
            }
            KeyCode::Char('r') if ctrl => {
                self.area.set_text(DEFAULT_PROMPT);
                self.area.cursor_to(true);
                Out::None
            }
            KeyCode::Esc if self.dirty() && !self.discard_armed => {
                self.discard_armed = true;
                Out::None
            }
            KeyCode::Esc => Out::Close,
            KeyCode::PageUp | KeyCode::PageDown => {
                let page = self.height.saturating_sub(2).max(1);
                for _ in 0..page {
                    self.area.key(KeyEvent::new(if k.code == KeyCode::PageUp { KeyCode::Up } else { KeyCode::Down }, KeyModifiers::NONE), self.width);
                }
                Out::None
            }
            _ => {
                self.discard_armed = false;
                self.area.key(k, self.width);
                Out::None
            }
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let w = area.width.saturating_sub(4).min(110);
        let h = area.height.saturating_sub(2);
        let inner = ui::popup(f, area, w, h, &format!("{}watchdog prompt", ui::lead("watchdog")), t);
        let inner = Rect { x: inner.x + 2, width: inner.width.saturating_sub(4), ..inner };
        if inner.height < 6 {
            return;
        }
        let body = ui::hint_line(f, inner, &[("ctrl+s", "save"), ("ctrl+r", "bro's default"), ("esc", "close")], t);
        let custom = self.area.text().trim() != DEFAULT_PROMPT.trim();
        ui::line_lr(f, Rect { height: 1, ..body }, vec![Span::styled("Every reviewer starts with these instructions, then gets a brief: the session, its milestones and the log.", muted(t))], vec![Span::styled(if custom { "customized" } else { "bro's default" }, fg(if custom { t.shine } else { t.inline }))]);
        let status = if self.discard_armed {
            Span::styled("unsaved changes · esc again discards them", fg(t.danger))
        } else if self.dirty() {
            Span::styled("unsaved changes", fg(t.inline))
        } else {
            Span::styled("applies from the next wake-up", muted(t))
        };
        ui::line(f, Rect { y: body.y + 1, height: 1, ..body }, vec![status]);
        let text = Rect { y: body.y + 3, height: body.height.saturating_sub(3), ..body };
        ui::rule(f, Rect { y: body.y + 2, height: 1, ..body }, "", t);
        self.width = text.width.saturating_sub(1) as usize;
        self.height = text.height as usize;
        self.area.render(f, text, Style::default(), true, "empty: bro's own instructions are used", t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_edits_and_resets_to_the_default() {
        let mut e = PromptEditor::new(None);
        assert_eq!(e.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)), Out::Save(None), "the default saves as none");
        e.paste("Always run the tests first.\n");
        assert!(matches!(e.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)), Out::Save(Some(p)) if p.starts_with("Always run the tests first.")));
        assert_eq!(e.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), Out::None, "first esc warns");
        assert_eq!(e.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), Out::Close);
        let mut e = PromptEditor::new(Some("mine"));
        e.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(e.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)), Out::Save(None));
    }
}
