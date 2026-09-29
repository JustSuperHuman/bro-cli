//! "Open project": pick a folder to add to the sidebar. Suggestions are the folders your earlier sessions
//! ran in (newest first) that aren't open yet. Typing a path ("~/code/x", "F:\x") autocompletes it: the
//! folders inside the typed parent that start with what you've typed are listed, and Tab completes.

use crate::fuzzy;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Frame, layout::Rect, text::Span};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// Folder completions listed at most.
const MAX_COMPLETIONS: usize = 60;

pub struct FolderPicker {
    pub filter: String,
    pub sel: usize,
    /// suggestions, best first
    pub dirs: Vec<PathBuf>,
    /// the current list: (path, kind)
    view: Vec<(PathBuf, Kind)>,
    /// row rects from the last draw: (rect, index into the list)
    pub hits: Vec<(Rect, usize)>,
}

/// Why a row is listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// from your history
    Suggested,
    /// exactly what you typed
    Typed,
    /// a folder inside the typed parent
    Completion,
}

pub enum Outcome {
    None,
    Close,
    Open(PathBuf),
}

impl FolderPicker {
    pub fn new(dirs: Vec<PathBuf>) -> FolderPicker {
        let mut p = FolderPicker { filter: String::new(), sel: 0, dirs, view: vec![], hits: vec![] };
        p.refresh();
        p
    }

    pub fn view(&self) -> &[(PathBuf, Kind)] {
        &self.view
    }

    /// Recompute the list after the filter changed.
    fn refresh(&mut self) {
        self.sel = 0;
        let f = self.filter.trim();
        self.view = if looks_like_path(f) {
            let (parent, partial) = split_typed(f);
            let mut v = vec![];
            let typed = expand(f);
            if typed.is_dir() || !partial.is_empty() {
                v.push((typed.clone(), Kind::Typed));
            }
            v.extend(complete(&parent, &partial).into_iter().filter(|p| *p != typed).map(|p| (p, Kind::Completion)));
            v
        } else {
            fuzzy::filter(f, &self.dirs, |p| p.to_string_lossy().to_string()).into_iter().map(|i| (self.dirs[i].clone(), Kind::Suggested)).collect()
        };
    }

    /// Tab: complete the typed path to the selected folder (or the only one), ready for the next level.
    fn tab_complete(&mut self) {
        let target = match self.view.get(self.sel) {
            Some((p, Kind::Completion)) => Some(p.clone()),
            Some((_, Kind::Typed)) => self.view.iter().find(|(_, k)| *k == Kind::Completion).map(|(p, _)| p.clone()),
            Some((p, Kind::Suggested)) => Some(p.clone()),
            None => None,
        };
        if let Some(p) = target {
            let mut s = shorten_home(&p);
            if !s.ends_with(['/', '\\']) {
                s.push(if s.contains('/') && !s.contains('\\') { '/' } else { MAIN_SEPARATOR });
            }
            self.filter = s;
            self.refresh();
        }
    }

    pub fn paste(&mut self, s: &str) {
        self.filter.push_str(s.trim());
        self.refresh();
    }

    /// Click a row: select it; clicking the selected row opens it.
    pub fn click(&mut self, pos: ratatui::layout::Position) -> Outcome {
        let Some(&(_, i)) = self.hits.iter().find(|(r, _)| r.contains(pos)) else { return Outcome::None };
        if i == self.sel {
            return self.view.get(i).map(|(p, _)| Outcome::Open(p.clone())).unwrap_or(Outcome::None);
        }
        self.sel = i;
        Outcome::None
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.view.len();
        match k.code {
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.refresh();
            }
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Enter => {
                if let Some((p, _)) = self.view.get(self.sel) {
                    return Outcome::Open(p.clone());
                }
            }
            KeyCode::Tab => self.tab_complete(),
            KeyCode::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('n' | 'j') if ctrl => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('p' | 'k') if ctrl => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('u') if ctrl => {
                self.filter.clear();
                self.refresh();
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.refresh();
            }
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.refresh();
            }
            _ => {}
        }
        Outcome::None
    }
}

/// "F:\code\br" → ("F:\code", "br"); "~/" → (home, ""); "F:" → ("F:\", "").
fn split_typed(s: &str) -> (PathBuf, String) {
    let b = s.as_bytes();
    if b.len() == 2 && b[1] == b':' {
        return (PathBuf::from(format!("{s}{MAIN_SEPARATOR}")), String::new());
    }
    match s.rfind(['/', '\\']) {
        Some(i) => {
            let head = &s[..=i];
            (expand(head), s[i + 1..].to_string())
        }
        None => (expand(s), String::new()),
    }
}

/// Folders in `parent` whose name starts with `partial` (case-insensitive), dot-folders only when asked for.
fn complete(parent: &Path, partial: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else { return vec![] };
    let want = partial.to_lowercase();
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let hidden = name.starts_with('.') || name.starts_with('$');
            (name.to_lowercase().starts_with(&want) && (!hidden || want.starts_with('.'))).then(|| e.path())
        })
        .collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    out.truncate(MAX_COMPLETIONS);
    out
}

/// `C:\Users\me\code` → `~\code` so completions stay short to type.
fn shorten_home(p: &Path) -> String {
    if let Some(h) = dirs::home_dir()
        && let Ok(rest) = p.strip_prefix(&h)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~{MAIN_SEPARATOR}{}", rest.display()) };
    }
    p.display().to_string()
}

pub fn draw(f: &mut Frame, screen: Rect, p: &mut FolderPicker, t: &Theme) {
    let mut hits = vec![];
    let inner = ui::popup(f, screen, 84, 22, "open project", t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let mut spans = vec![Span::styled("› ", ui::bold_accent(t))];
    if p.filter.is_empty() {
        spans.push(Span::styled("type to filter your recent folders, or a path like ~/code/ or F:\\", muted(t)));
    } else {
        spans.push(Span::styled(p.filter.clone(), ui::bold()));
    }
    spans.push(Span::styled("▏", ui::accent(t)));
    ui::line(f, Rect { height: 1, ..inner }, spans);
    let rows = inner.height.saturating_sub(3) as usize;
    let view = p.view().to_vec();
    let start = p.sel.saturating_sub(rows.saturating_sub(1));
    for (k, (path, kind)) in view.iter().enumerate().skip(start).take(rows) {
        let y = inner.y + 2 + (k - start) as u16;
        let on = k == p.sel;
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string());
        let mut s = vec![if on { Span::styled("▌ ", ui::accent(t)) } else { Span::raw("  ") }];
        match kind {
            Kind::Typed if !path.is_dir() => s.push(Span::styled("new? ", fg(t.danger))),
            Kind::Typed => s.push(Span::styled("↳ ", fg(t.shine))),
            Kind::Completion => s.push(Span::styled("▸ ", muted(t))),
            Kind::Suggested => {}
        }
        s.push(Span::styled(format!("{name}  "), if on { ui::bold_accent(t) } else { ui::bold() }));
        s.push(Span::styled(crate::util::short_path(path.parent().unwrap_or(path), 60), muted(t)));
        ui::line(f, Rect { y, height: 1, ..inner }, s);
        hits.push((Rect { y, height: 1, ..inner }, k));
    }
    if view.is_empty() {
        ui::line(f, Rect { y: inner.y + 2, height: 1, ..inner }, vec![Span::styled("  nothing here — type a path (~/, C:\\, /…)", muted(t))]);
    }
    p.hits = hits;
    ui::hint_line(f, inner, &[("⏎", "open project"), ("tab", "complete"), ("↑↓", "move"), ("esc", "cancel")], t);
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

    fn key(p: &mut FolderPicker, code: KeyCode) -> Outcome {
        p.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn filter_suggestions_and_open() {
        let mut p = FolderPicker::new(vec![PathBuf::from("/code/bro"), PathBuf::from("/code/justgains")]);
        for c in "just".chars() {
            key(&mut p, KeyCode::Char(c));
        }
        assert!(matches!(key(&mut p, KeyCode::Enter), Outcome::Open(ref x) if x == Path::new("/code/justgains")));
        assert!(looks_like_path("C:\\x") && !looks_like_path("bro"));
        assert_eq!(dedup(vec![PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/a")]), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn typed_paths_autocomplete_and_tab_descends() {
        let root = tempfile::tempdir().unwrap();
        for d in ["alpha", "Alpine", "beta", ".hidden"] {
            std::fs::create_dir_all(root.path().join(d).join("inner")).unwrap();
        }
        std::fs::write(root.path().join("alfile.txt"), "x").unwrap();
        let mut p = FolderPicker::new(vec![]);
        p.paste(&format!("{}{MAIN_SEPARATOR}al", root.path().display()));
        let names: Vec<String> = p.view().iter().filter(|(_, k)| *k == Kind::Completion).map(|(p, _)| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["alpha", "Alpine"], "case-insensitive prefix, folders only");
        // tab completes to the first folder and lists what's inside
        key(&mut p, KeyCode::Tab);
        assert!(p.filter.ends_with(&format!("alpha{MAIN_SEPARATOR}")), "{}", p.filter);
        assert!(p.view().iter().any(|(q, k)| *k == Kind::Completion && q.ends_with("inner")));
        // hidden folders only when asked for
        p.filter.clear();
        p.paste(&format!("{}{MAIN_SEPARATOR}", root.path().display()));
        assert!(!p.view().iter().any(|(q, _)| q.ends_with(".hidden")));
        p.paste(".");
        assert!(p.view().iter().any(|(q, _)| q.ends_with(".hidden")));
        // Enter opens the typed / selected folder
        p.filter.clear();
        p.paste(&root.path().join("beta").display().to_string());
        assert!(matches!(key(&mut p, KeyCode::Enter), Outcome::Open(ref x) if x.ends_with("beta")));
    }
}
