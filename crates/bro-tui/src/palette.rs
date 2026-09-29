//! The command palette (alt+p): every action with its current key, themes with live preview, recent launches.

use crate::fuzzy;
use crate::keymap::{Act, Keymap};
use crate::recents::Recent;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};

/// What a palette entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Act(Act),
    Theme(String),
    Recent(usize),
}

/// One entry.
#[derive(Clone, Debug)]
pub struct Item {
    pub icon: &'static str,
    pub label: String,
    pub hint: String,
    pub cmd: Cmd,
}

/// What a key did.
pub enum Out {
    None,
    /// closed; restore the theme it was opened with
    Cancel,
    Run(Cmd),
}

/// The palette state.
pub struct Palette {
    pub query: String,
    pub sel: usize,
    pub items: Vec<Item>,
    /// theme to restore on Esc (live preview)
    pub theme_before: String,
}

fn icon_for(a: Act) -> &'static str {
    use Act::*;
    match a {
        NewSession | NewShell => "rocket",
        Usage | RefreshUsage => "gauge",
        Profiles => "account",
        Proxy => "proxy",
        Bridge => "bridge",
        Themes => "theme",
        Help => "help",
        Quit => "quit",
        Close => "close",
        Split | SplitRight | SplitDown | Zoom => "split",
        _ => "window",
    }
}

impl Palette {
    /// Every action (with its live key), recent launches, then every theme.
    pub fn new(km: &Keymap, recents: &[Recent], themes: &[String], current_theme: &str, query: &str) -> Palette {
        let mut items = vec![];
        for (i, r) in recents.iter().take(5).enumerate() {
            let who = r.provider_id.clone().or_else(|| r.profile_id.clone()).unwrap_or_default();
            items.push(Item {
                icon: "history",
                label: format!("launch {} · {who} · {} — {}", r.harness.label(), r.model.as_deref().map(crate::services::launch::short_model).unwrap_or_else(|| "default".into()), crate::util::short_path(&r.cwd, 30)),
                hint: "recent".into(),
                cmd: Cmd::Recent(i),
            });
        }
        for a in Act::all() {
            if matches!(a, Act::Jump(n) if n > 3) {
                continue; // jump 1–3 is enough to show the pattern
            }
            items.push(Item { icon: icon_for(a), label: a.describe(), hint: km.primary(a), cmd: Cmd::Act(a) });
        }
        for t in themes {
            let mine = if crate::theme::is_custom(t) { " · yours" } else { "" };
            let now = if t == current_theme { " · current" } else { "" };
            items.push(Item { icon: "theme", label: format!("theme {t}{mine}{now}"), hint: String::new(), cmd: Cmd::Theme(t.clone()) });
        }
        Palette { query: query.to_string(), sel: 0, items, theme_before: current_theme.to_string() }
    }

    /// Indices of matching items, best first.
    pub fn matches(&self) -> Vec<usize> {
        fuzzy::filter(&self.query, &self.items, |i| format!("{} {}", i.label, i.hint))
    }

    /// The highlighted theme, for live preview.
    pub fn preview(&self) -> Option<String> {
        let m = self.matches();
        match m.get(self.sel).map(|&i| &self.items[i].cmd) {
            Some(Cmd::Theme(t)) => Some(t.clone()),
            _ => None,
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Out {
        let m = self.matches();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return Out::Cancel,
            KeyCode::Enter => return m.get(self.sel).map(|&i| Out::Run(self.items[i].cmd.clone())).unwrap_or(Out::None),
            KeyCode::Down | KeyCode::Tab => self.sel = (self.sel + 1).min(m.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::BackTab => self.sel = self.sel.saturating_sub(1),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(m.len().saturating_sub(1)),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::Char('n' | 'j') if ctrl => self.sel = (self.sel + 1).min(m.len().saturating_sub(1)),
            KeyCode::Char('p' | 'k') if ctrl => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.sel = 0;
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.sel = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.sel = 0;
            }
            _ => {}
        }
        Out::None
    }

    pub fn draw(&self, f: &mut Frame, screen: Rect, t: &Theme) {
        let m = self.matches();
        let inner = ui::popup(f, screen, 76, 22, &format!("{}palette", ui::lead("search")), t);
        let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
        let mut q = vec![Span::styled("› ", ui::bold_accent(t))];
        if self.query.is_empty() {
            q.push(Span::styled("type a command, a theme, a recent launch…", muted(t)));
        } else {
            q.push(Span::styled(self.query.clone(), ui::bold()));
        }
        q.push(Span::styled("▏", ui::accent(t)));
        ui::line_lr(f, Rect { height: 1, ..inner }, q, vec![Span::styled(format!("{} ", m.len()), muted(t))]);
        let rows = inner.height.saturating_sub(2) as usize;
        let start = self.sel.saturating_sub(rows.saturating_sub(1));
        for (row, &i) in m.iter().skip(start).take(rows).enumerate() {
            let it = &self.items[i];
            let on = start + row == self.sel;
            let style = if on { Style::default().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::default() };
            let left = vec![Span::styled(if on { "▌ " } else { "  " }, ui::accent(t)), Span::styled(ui::lead(it.icon), if on { ui::accent(t) } else { muted(t) }), Span::styled(it.label.clone(), style)];
            let right = vec![Span::styled(format!("{} ", it.hint), if on { fg(t.shine).add_modifier(Modifier::BOLD) } else { muted(t) })];
            ui::line_lr(f, Rect { y: inner.y + 2 + row as u16, height: 1, ..inner }, left, right);
        }
        if m.is_empty() {
            ui::line(f, Rect { y: inner.y + 2, height: 1, ..inner }, vec![Span::styled("  nothing matches", muted(t))]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_actions_with_live_keys_and_previews_themes() {
        let km = Keymap::default();
        let mut p = Palette::new(&km, &[], &["ultra".into(), "ocean".into()], "ultra", "");
        let usage = p.items.iter().find(|i| i.cmd == Cmd::Act(Act::Usage)).unwrap();
        assert_eq!(usage.hint, "alt+u");
        for c in "ocean".chars() {
            p.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(p.preview().as_deref(), Some("ocean"));
        assert!(matches!(p.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Out::Run(Cmd::Theme(t)) if t == "ocean"));
        assert!(matches!(p.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), Out::Cancel));
    }
}
