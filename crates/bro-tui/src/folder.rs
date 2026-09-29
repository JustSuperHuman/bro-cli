//! "Open folder": pick a project to add to the sidebar. Suggestions are the folders your earlier sessions
//! ran in (newest first) that aren't open yet; typing a path ("~/code/x", "F:\x") offers that folder.

use crate::fuzzy;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Frame, layout::Rect, text::Span};
use std::path::{Path, PathBuf};

pub struct FolderPicker {
    pub filter: String,
    pub sel: usize,
    /// suggestions, best first
    pub dirs: Vec<PathBuf>,
}

pub enum Outcome {
    None,
    Close,
    Open(PathBuf),
}

impl FolderPicker {
    pub fn new(dirs: Vec<PathBuf>) -> FolderPicker {
        FolderPicker { filter: String::new(), sel: 0, dirs }
    }

    /// (path, typed by you)
    pub fn view(&self) -> Vec<(PathBuf, bool)> {
        let f = self.filter.trim();
        let mut out = vec![];
        if looks_like_path(f) {
            out.push((expand(f), true));
        }
        let q = if looks_like_path(f) { "" } else { f };
        out.extend(fuzzy::filter(q, &self.dirs, |p| p.to_string_lossy().to_string()).into_iter().map(|i| (self.dirs[i].clone(), false)));
        out
    }

    pub fn paste(&mut self, s: &str) {
        self.filter.push_str(s.trim());
        self.sel = 0;
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.view().len();
        match k.code {
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.sel = 0;
            }
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Enter => {
                if let Some((p, _)) = self.view().get(self.sel) {
                    return Outcome::Open(p.clone());
                }
            }
            KeyCode::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('n' | 'j') if ctrl => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('p' | 'k') if ctrl => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('u') if ctrl => {
                self.filter.clear();
                self.sel = 0;
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.sel = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.sel = 0;
            }
            _ => {}
        }
        Outcome::None
    }
}

pub fn draw(f: &mut Frame, screen: Rect, p: &FolderPicker, t: &Theme) {
    let inner = ui::popup(f, screen, 84, 20, "open folder", t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let mut spans = vec![Span::styled("› ", ui::bold_accent(t))];
    if p.filter.is_empty() {
        spans.push(Span::styled("type to filter, or a path like ~/code/thing or F:\\work", muted(t)));
    } else {
        spans.push(Span::styled(p.filter.clone(), ui::bold()));
    }
    spans.push(Span::styled("▏", ui::accent(t)));
    ui::line(f, Rect { height: 1, ..inner }, spans);
    let rows = inner.height.saturating_sub(3) as usize;
    let view = p.view();
    let start = p.sel.saturating_sub(rows.saturating_sub(1));
    for (k, (path, typed)) in view.iter().enumerate().skip(start).take(rows) {
        let y = inner.y + 2 + (k - start) as u16;
        let on = k == p.sel;
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string());
        let mut s = vec![if on { Span::styled("▌ ", ui::accent(t)) } else { Span::raw("  ") }];
        if *typed {
            s.push(Span::styled(if path.is_dir() { "↳ " } else { "↳ new? " }, fg(t.shine)));
        }
        s.push(Span::styled(format!("{name}  "), if on { ui::bold_accent(t) } else { ui::bold() }));
        s.push(Span::styled(crate::util::short_path(path.parent().unwrap_or(path), 60), muted(t)));
        ui::line(f, Rect { y, height: 1, ..inner }, s);
    }
    if view.is_empty() {
        ui::line(f, Rect { y: inner.y + 2, height: 1, ..inner }, vec![Span::styled("  no suggestions — type a path", muted(t))]);
    }
    ui::hint_line(f, inner, &[("⏎", "open"), ("↑↓", "move"), ("esc", "cancel")], t);
}

/// True for input that is clearly a path ("~/x", "C:\x", "/x", "./x").
pub fn looks_like_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('~') || s.starts_with('/') || s.starts_with('.') || s.starts_with('\\') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// Expand a leading `~`.
pub fn expand(s: &str) -> PathBuf {
    match s.strip_prefix('~') {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest.trim_start_matches(['/', '\\'])),
        None => PathBuf::from(s),
    }
}

/// Deduplicate, keeping the first occurrence (case-insensitive on Windows).
pub fn dedup(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for p in paths {
        if !out.iter().any(|o| same_path(o, &p)) {
            out.push(p);
        }
    }
    out
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_typed_path_and_open() {
        let mut p = FolderPicker::new(vec![PathBuf::from("/code/bro"), PathBuf::from("/code/justgains")]);
        for c in "just".chars() {
            p.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(matches!(p.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Outcome::Open(ref x) if x == Path::new("/code/justgains")));
        p.filter = "~/new".into();
        assert!(p.view()[0].1);
        assert!(looks_like_path("C:\\x") && !looks_like_path("bro"));
        assert_eq!(dedup(vec![PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/a")]), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}
