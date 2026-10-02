//! The help overlay (F1 / `?` / prefix ?): generated from the live keymap, plus the sidebar, launcher and
//! mouse cheat-sheets.

use crate::keymap::{Act, Group, Keymap};
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect, style::Modifier, text::Span};

/// One line of help.
#[derive(Clone, Debug)]
pub enum Line {
    Head(String),
    Key(String, String),
    Gap,
}

/// Build the help text from the keymap.
pub fn lines(km: &Keymap) -> Vec<Line> {
    let mut v = vec![];
    for g in Group::ALL {
        v.push(Line::Head(g.label().to_string()));
        for a in Act::all().into_iter().filter(|a| a.group() == g) {
            if let Act::Jump(n) = a {
                if n == 1 {
                    let k = km.chords_for(a).first().cloned().unwrap_or_default().replace('1', "1–9");
                    v.push(Line::Key(k, "jump to live session 1–9 (sidebar order)".into()));
                }
                continue;
            }
            let chords = km.chords_for(a);
            if chords.is_empty() {
                continue;
            }
            v.push(Line::Key(chords.join("  "), a.describe()));
        }
        v.push(Line::Gap);
    }
    let sheet: &[(&str, &[(&str, &str)])] = &[
        ("sidebar (alt+b)", &[("j k ↑ ↓", "move"), ("h l ← →", "to the project / into it"), ("⏎", "open a session · on a project: tile only its sessions, again: tile all (click works too)"), ("c / r", "continue an earlier session (alt+r anywhere)"), ("f", "move a running session to another login"), ("n", "new session in this project"), ("o", "open a project"), ("r", "rename a live session"), ("shift+⏎ / +", "show alongside (stack) · ctrl+click does it too"), ("x", "close a session · on a project: close the project (× works too)"), ("/", "filter"), ("esc", "back to the pane")]),
        ("launcher (alt+n)", &[("type", "filter the column"), ("tab ← →", "next column"), ("↑ ↓", "pick"), ("ctrl+e", "permission: default → auto → skip"), ("ctrl+b", "browser"), ("ctrl+s", "new tab / split"), ("⏎", "launch")]),
        ("clipboard", &[("ctrl+v / shift+insert", "paste text into the focused field or terminal"), ("alt+v", "paste a screenshot / copied files as paths")]),
        ("usage", &[("click agent / account", "new session in the current project"), ("click usage heading", "expand / collapse accounts"), ("click Claude arrow", "show / hide Fable allowance"), ("alt+u", "full usage details")]),
        ("terminal", &[("alt+↑ in Codex", "edit queued input (prefix ↑ still moves pane focus)"), ("click a link", "open in your default browser or application"), ("ctrl+click", "open a link even when the program uses the mouse"), ("drag", "select text — copied when you let go"), ("wheel", "scroll back (when the program doesn't use the mouse)"), ("shift+drag", "select even when the program uses the mouse")]),
        ("mouse", &[("click", "sidebar rows, panes"), ("drag a divider", "resize"), ("drag sidebar edge ↔", "resize sidebar (saved for next launch)"), ("double-click edge", "reset sidebar to automatic width"), ("× on a frame", "close the pane")]),
    ];
    for (head, keys) in sheet {
        v.push(Line::Head(head.to_string()));
        for (k, d) in keys.iter() {
            v.push(Line::Key(k.to_string(), d.to_string()));
        }
        v.push(Line::Gap);
    }
    v
}

/// Overlay state (scroll offset).
#[derive(Default)]
pub struct Help {
    pub scroll: usize,
}

impl Help {
    /// True = close.
    pub fn key(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter => return true,
            KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll += 10,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            _ => {}
        }
        false
    }

    pub fn draw(&mut self, f: &mut Frame, screen: Rect, km: &Keymap, t: &Theme, time: f64) {
        let r = ui::centered(screen, 118, screen.height.saturating_sub(4).max(10));
        f.render_widget(ratatui::widgets::Clear, r);
        let mut title = vec![Span::raw(" "), Span::styled(ui::lead("help"), ui::bold_accent(t))];
        title.extend(ui::title_spans("bro · keys", t, time));
        title.push(Span::raw(" "));
        let status = ratatui::text::Line::from(Span::styled(format!(" prefix {} ", km.prefix.display()), muted(t)));
        let inner = ui::frame_ex(f, r, ratatui::text::Line::from(title), Some(status), Some("esc close · j/k scroll"), true, t);
        let inner = Rect { x: inner.x + 2, width: inner.width.saturating_sub(4), y: inner.y + 1, height: inner.height.saturating_sub(1) };
        let all = lines(km);
        // two columns when there's room
        let two = inner.width >= 100;
        let col_w = if two { inner.width / 2 - 1 } else { inner.width };
        let per_col = inner.height as usize;
        let max_scroll = if two { all.len().saturating_sub(per_col * 2) } else { all.len().saturating_sub(per_col) };
        self.scroll = self.scroll.min(max_scroll);
        let shown = &all[self.scroll.min(all.len())..];
        for (i, l) in shown.iter().enumerate() {
            let (col, row) = if two { (i / per_col, i % per_col) } else { (0, i) };
            if col > 1 || (!two && col > 0) || row >= per_col {
                break;
            }
            let rr = Rect { x: inner.x + col as u16 * (col_w + 2), y: inner.y + row as u16, width: col_w, height: 1 };
            match l {
                Line::Head(h) => ui::line(f, rr, vec![Span::styled(h.to_uppercase(), fg(t.accent).add_modifier(Modifier::BOLD))]),
                Line::Key(k, d) => {
                    let kw = 30usize.min(col_w as usize / 2);
                    ui::line(f, rr, vec![Span::styled(ui::pad(&format!("  {k}"), kw), fg(t.shine).add_modifier(Modifier::BOLD)), Span::styled(d.clone(), muted(t))]);
                }
                Line::Gap => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_follows_the_keymap() {
        let mut o = std::collections::BTreeMap::new();
        o.insert("palette".to_string(), "alt+k".to_string());
        let km = Keymap::new("ctrl+space", &o);
        let text: Vec<String> = lines(&km).iter().filter_map(|l| if let Line::Key(k, d) = l { Some(format!("{k} {d}")) } else { None }).collect();
        assert!(text.iter().any(|l| l.starts_with("alt+k") && l.contains("command palette")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("alt+1–9")));
        assert!(!text.iter().any(|l| l.starts_with("alt+p ")));
    }
}
