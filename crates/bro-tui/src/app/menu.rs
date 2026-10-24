//! The session context menu, shared by right-click and the prefix key.

use super::App;
use crate::keymap::Act;
use crate::ui;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect, text::Span};

#[derive(Default)]
pub(crate) struct Menu {
    pub sel: usize,
    pub top: usize,
    /// Pointer position in the frontend's coordinates; None anchors near the focused session.
    pub anchor: Option<(f32, f32)>,
    pub hits: Vec<(Rect, Act)>,
}

pub(crate) fn label(act: Act) -> String {
    match act {
        Act::NewSession => "New session…".into(),
        Act::QuickAgent => "Quick agent (home folder)".into(),
        Act::Reload => "Reload session with a profile…".into(),
        Act::Continue => "Continue an earlier session…".into(),
        Act::SwitchLogin => "Move session to another login…".into(),
        Act::OpenedFile => "Last opened file".into(),
        Act::Jump(n) => format!("Jump to session {n}"),
        Act::ShowTab(0) => "All projects".into(),
        Act::ShowTab(n) => format!("Project {n}"),
        _ => {
            let mut text = act.name().replace('_', " ");
            if let Some(first) = text.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            text
        }
    }
}

impl Menu {
    pub fn keep_visible(&mut self, rows: usize, count: usize) {
        self.sel = self.sel.min(count.saturating_sub(1));
        self.top = self.top.min(self.sel);
        if self.sel >= self.top + rows {
            self.top = self.sel + 1 - rows;
        }
        self.top = self.top.min(count.saturating_sub(rows));
    }
}

impl App {
    pub(crate) fn open_menu(&mut self, anchor: Option<(f32, f32)>) {
        self.prefix_armed = true;
        self.menu = Menu { anchor, ..Menu::default() };
        self.sel = None;
        self.link_press = None;
        self.clipboard_request = None;
    }

    pub(crate) fn menu_move(&mut self, delta: isize) {
        let n = self.keymap.prefix_actions().len();
        self.menu.sel = self.menu.sel.saturating_add_signed(delta).min(n.saturating_sub(1));
    }

    /// Navigation stays in the menu; the other prefix bindings still run their actions.
    pub(super) fn menu_key(&mut self, k: KeyEvent) -> bool {
        if !k.modifiers.is_empty() || self.keymap.after_prefix(&k).is_some() {
            return false;
        }
        match k.code {
            KeyCode::Down => self.menu_move(1),
            KeyCode::Up => self.menu_move(-1),
            KeyCode::PageDown => self.menu_move(10),
            KeyCode::PageUp => self.menu_move(-10),
            KeyCode::Home => self.menu.sel = 0,
            KeyCode::End => self.menu.sel = self.keymap.prefix_actions().len().saturating_sub(1),
            KeyCode::Enter => {
                self.prefix_armed = false;
                if let Some((_, act)) = self.keymap.prefix_actions().get(self.menu.sel) {
                    self.run_act(*act);
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn draw_menu(&mut self, f: &mut Frame, area: Rect) {
        self.menu.hits.clear();
        if area.width < 8 || area.height < 5 {
            return;
        }
        let items = self.keymap.prefix_actions();
        let rows = items.len().min(area.height.saturating_sub(5) as usize).min(20);
        self.menu.keep_visible(rows, items.len());
        let w = area.width.saturating_sub(2).min(82);
        let h = rows as u16 + 4;
        let (x, y) = self.menu.anchor.unwrap_or((area.right().saturating_sub(w + 1) as f32, area.y as f32 + 1.0));
        let r = Rect::new((x as u16).clamp(area.x, area.right() - w), (y as u16).clamp(area.y, area.bottom() - h), w, h);
        let t = &self.theme;
        ui::clear(f, r, t);
        let inner = ui::frame(f, r, &format!("session menu · {}", self.keymap.prefix.display()), None, true, t);
        for (row, (index, (key, act))) in items.iter().enumerate().skip(self.menu.top).take(rows).enumerate() {
            let rr = Rect { y: inner.y + row as u16, height: 1, ..inner };
            let style = if index == self.menu.sel { ui::bold_accent(t) } else { ui::fg(t.fg) };
            ui::line_lr(f, rr, vec![Span::styled(format!(" {}", ui::fit(&label(*act), inner.width.saturating_sub(12) as usize)), style)], vec![Span::styled(format!(" {key} "), ui::accent(t))]);
            self.menu.hits.push((rr, *act));
        }
        ui::hint_line(f, inner, &[("↑↓", "select"), ("⏎", "run"), ("esc", "cancel")], t);
    }
}
