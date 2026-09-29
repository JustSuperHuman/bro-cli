//! "Continue a session": earlier sessions, searchable, for the current project (tab: every project). Enter
//! resumes (asking which login when you have several); Delete archives; ctrl+a shows archived ones too.
//! Earlier sessions live here instead of cluttering the sidebar.

use crate::fuzzy;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_core::Harness;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::Span,
};

/// One earlier session as the picker shows it.
#[derive(Clone, Debug)]
pub struct Entry {
    /// index into the services' past-session list
    pub idx: usize,
    pub id: String,
    pub harness: Harness,
    pub title: String,
    /// login name ("work"), if any
    pub login: Option<String>,
    pub age_secs: u64,
    pub project_key: String,
    pub project_name: String,
    pub archived: bool,
}

pub enum Outcome {
    None,
    Close,
    /// resume this past-session index
    Resume(usize),
    /// archive (true) / restore (false) this session id
    Archive(String, bool),
    /// undo the last archive
    Undo,
}

pub struct ContinuePicker {
    pub filter: String,
    pub sel: usize,
    /// every project instead of just the current one
    pub all: bool,
    pub show_archived: bool,
    /// the current project (key, name); None = always all
    pub project: Option<(String, String)>,
    pub entries: Vec<Entry>,
    /// row rects from the last draw: (rect, position in the view)
    pub hits: Vec<(Rect, usize)>,
    view: Vec<usize>,
}

impl ContinuePicker {
    /// `entries` newest first.
    pub fn new(entries: Vec<Entry>, project: Option<(String, String)>) -> ContinuePicker {
        // nothing earlier in this project? start on every project
        let all = project.as_ref().is_none_or(|(k, _)| !entries.iter().any(|e| &e.project_key == k && !e.archived));
        let mut p = ContinuePicker { filter: String::new(), sel: 0, all, show_archived: false, project, entries, hits: vec![], view: vec![] };
        p.refresh();
        p
    }

    /// Replace the entries (after archiving) keeping the cursor near where it was.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let sel = self.sel;
        self.entries = entries;
        self.refresh();
        self.sel = sel.min(self.view.len().saturating_sub(1));
    }

    fn refresh(&mut self) {
        let proj = self.project.as_ref().map(|(k, _)| k.clone());
        let pool: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                (self.show_archived || !e.archived) && (self.all || proj.as_deref().is_none_or(|k| e.project_key == k))
            })
            .collect();
        let q = self.filter.trim();
        self.view = if q.is_empty() {
            pool
        } else {
            let hits = fuzzy::filter(q, &pool, |&i| {
                let e = &self.entries[i];
                format!("{} {} {} {}", e.title, e.project_name, e.login.as_deref().unwrap_or(""), e.harness.label())
            });
            hits.into_iter().map(|k| pool[k]).collect()
        };
        self.sel = 0;
    }

    pub fn view(&self) -> Vec<&Entry> {
        self.view.iter().map(|&i| &self.entries[i]).collect()
    }

    fn current(&self) -> Option<&Entry> {
        self.view.get(self.sel).map(|&i| &self.entries[i])
    }

    pub fn paste(&mut self, s: &str) {
        self.filter.push_str(s.trim());
        self.refresh();
    }

    pub fn click(&mut self, pos: Position) -> Outcome {
        let Some(&(_, k)) = self.hits.iter().find(|(r, _)| r.contains(pos)) else { return Outcome::None };
        if k == self.sel {
            return self.current().map(|e| Outcome::Resume(e.idx)).unwrap_or(Outcome::None);
        }
        self.sel = k;
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
            KeyCode::Enter => return self.current().map(|e| Outcome::Resume(e.idx)).unwrap_or(Outcome::None),
            KeyCode::Tab | KeyCode::BackTab if self.project.is_some() => {
                self.all = !self.all;
                self.refresh();
            }
            KeyCode::Delete => {
                if let Some(e) = self.current() {
                    return Outcome::Archive(e.id.clone(), !e.archived);
                }
            }
            KeyCode::Char('z') if ctrl => return Outcome::Undo,
            KeyCode::Char('a') if ctrl => {
                self.show_archived = !self.show_archived;
                self.refresh();
            }
            KeyCode::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
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

pub fn draw(f: &mut Frame, screen: Rect, p: &mut ContinuePicker, t: &Theme) {
    let inner = ui::popup(f, screen, 100, 26, &format!("{}continue a session", ui::lead("history")), t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    // filter + scope
    let mut spans = vec![Span::styled("› ", ui::bold_accent(t))];
    if p.filter.is_empty() {
        spans.push(Span::styled("type to search earlier sessions", muted(t)));
    } else {
        spans.push(Span::styled(p.filter.clone(), ui::bold()));
    }
    spans.push(Span::styled("▏", ui::accent(t)));
    let mut scope = vec![];
    if let Some((_, name)) = &p.project {
        let on = |b: bool| if b { Style::default().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED) } else { muted(t) };
        scope.push(Span::styled(format!(" {name} "), on(!p.all)));
        scope.push(Span::raw(" "));
        scope.push(Span::styled(" all projects ", on(p.all)));
        scope.push(Span::styled("  tab ", muted(t)));
    }
    ui::line_lr(f, Rect { height: 1, ..inner }, spans, scope);

    let rows = inner.height.saturating_sub(4) as usize;
    let view: Vec<Entry> = p.view().into_iter().cloned().collect();
    let start = p.sel.saturating_sub(rows.saturating_sub(1));
    p.hits.clear();
    let now_w = inner.width as usize;
    for (k, e) in view.iter().enumerate().skip(start).take(rows) {
        let y = inner.y + 2 + (k - start) as u16;
        let r = Rect { y, height: 1, ..inner };
        let on = k == p.sel;
        if on {
            f.buffer_mut().set_style(r, Style::default().bg(crate::theme::mix(t.user, ratatui::style::Color::Rgb(20, 20, 24), 0.35)));
        }
        let brand = ui::harness_color(Some(e.harness), t);
        let mut right = vec![];
        if p.all {
            right.push(Span::styled(format!("{}  ", ui::fit(&e.project_name, 16)), fg(crate::theme::mix(t.shine, t.muted, 0.4))));
        }
        if let Some(l) = &e.login {
            right.push(Span::styled(format!("{}  ", ui::fit(l, 12)), muted(t)));
        }
        if e.archived {
            right.push(Span::styled("archived  ", fg(t.frame)));
        }
        right.push(Span::styled(format!("{:>4} ", crate::util::short_dur(e.age_secs)), muted(t)));
        let right_w: usize = right.iter().map(|s| ui::width(&s.content)).sum();
        let mut title_style = if on { ui::bold_accent(t) } else { Style::default() };
        if e.archived {
            title_style = title_style.add_modifier(Modifier::CROSSED_OUT).fg(t.muted);
        }
        let left = vec![
            Span::styled(if on { "▌ " } else { "  " }, ui::accent(t)),
            Span::styled(format!("{} ", ui::harness_glyph(Some(e.harness))), fg(brand)),
            Span::styled(ui::fit(&e.title, now_w.saturating_sub(right_w + 6)), title_style),
        ];
        ui::line_lr(f, r, left, right);
        p.hits.push((r, k));
    }
    if view.is_empty() {
        let msg = if p.entries.is_empty() { "  no earlier sessions yet" } else if p.all { "  nothing matches" } else { "  nothing here — tab shows every project" };
        ui::line(f, Rect { y: inner.y + 2, height: 1, ..inner }, vec![Span::styled(msg, muted(t))]);
    }
    let archived = if p.show_archived { "hide archived" } else { "show archived" };
    ui::hint_line(f, inner, &[("⏎", "resume"), ("↑↓", "move"), ("del", "archive"), ("^z", "undo"), ("^a", archived), ("esc", "close")], t);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(idx: usize, project: &str, title: &str, archived: bool) -> Entry {
        Entry { idx, id: format!("id{idx}"), harness: Harness::Claude, title: title.into(), login: Some("work".into()), age_secs: 60, project_key: project.into(), project_name: project.into(), archived }
    }

    fn key(p: &mut ContinuePicker, code: KeyCode) -> Outcome {
        p.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn scoped_to_the_project_tab_for_all_filter_archive() {
        let entries = vec![e(0, "a", "fix the timer", false), e(1, "b", "qr pairing", false), e(2, "a", "old thing", true)];
        let mut p = ContinuePicker::new(entries.clone(), Some(("a".into(), "a".into())));
        assert!(!p.all);
        assert_eq!(p.view().len(), 1, "this project, archived hidden");
        key(&mut p, KeyCode::Tab);
        assert_eq!(p.view().len(), 2, "every project");
        p.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(p.view().len(), 3, "archived too");
        for c in "qr".chars() {
            key(&mut p, KeyCode::Char(c));
        }
        assert!(matches!(key(&mut p, KeyCode::Enter), Outcome::Resume(1)));
        assert!(matches!(key(&mut p, KeyCode::Delete), Outcome::Archive(ref id, true) if id == "id1"));
        // a project with nothing earlier starts on every project
        let p = ContinuePicker::new(entries, Some(("zzz".into(), "zzz".into())));
        assert!(p.all);
    }
}
