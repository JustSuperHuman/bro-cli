//! "Continue a session": earlier sessions, searchable, for the current project (tab: every project). Enter
//! resumes (asking which login when you have several); Delete archives; ctrl+a shows archived ones too.
//! Earlier sessions live here instead of cluttering the sidebar. Shift+arrows / shift+click pick a range,
//! ctrl+space / ctrl+click pick one more; Enter then resumes every picked session.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

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
    /// resume these past-session indexes, in list order
    ResumeMany(Vec<usize>),
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
    /// clickable project and all-session filters
    pub scope_hits: Vec<(Rect, bool)>,
    /// picked session ids (multi-select), kept while you search
    pub picked: Vec<String>,
    /// where a shift range starts (a view position) and what was picked before it
    anchor: Option<(usize, Vec<String>)>,
    view: Vec<usize>,
}

impl ContinuePicker {
    /// `entries` newest first.
    pub fn new(entries: Vec<Entry>, project: Option<(String, String)>) -> ContinuePicker {
        let all = project.is_none();
        let mut p = ContinuePicker { filter: String::new(), sel: 0, all, show_archived: false, project, entries, hits: vec![], scope_hits: vec![], picked: vec![], anchor: None, view: vec![] };
        p.refresh();
        p
    }

    /// Replace the entries (after archiving) keeping the cursor near where it was.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let sel = self.sel;
        self.entries = entries;
        self.picked.retain(|id| self.entries.iter().any(|e| &e.id == id));
        self.refresh();
        self.sel = sel.min(self.view.len().saturating_sub(1));
    }

    pub fn set_project(&mut self, project: Option<(String, String)>) {
        self.project = project;
        if self.project.is_none() {
            self.all = true;
        }
        self.refresh();
    }

    pub fn set_all(&mut self, all: bool) {
        self.all = all || self.project.is_none();
        self.refresh();
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
                format!("{} {} {} {} {}", e.title, e.project_name, e.login.as_deref().unwrap_or(""), e.harness.label(), e.id)
            });
            hits.into_iter().map(|k| pool[k]).collect()
        };
        self.sel = 0;
        self.anchor = None;
    }

    pub fn view(&self) -> Vec<&Entry> {
        self.view.iter().map(|&i| &self.entries[i]).collect()
    }

    fn current(&self) -> Option<&Entry> {
        self.view.get(self.sel).map(|&i| &self.entries[i])
    }

    pub fn is_picked(&self, id: &str) -> bool {
        self.picked.iter().any(|p| p == id)
    }

    /// Pick or unpick the row at view position `k`; the next shift range starts there.
    fn toggle(&mut self, k: usize) {
        let Some(id) = self.view.get(k).map(|&i| self.entries[i].id.clone()) else { return };
        match self.picked.iter().position(|p| *p == id) {
            Some(at) => {
                self.picked.remove(at);
            }
            None => self.picked.push(id),
        }
        self.sel = k;
        self.anchor = None;
    }

    /// Move to view position `to`, picking every row between the range's start and there.
    fn extend(&mut self, to: usize) {
        if self.view.is_empty() {
            return;
        }
        let to = to.min(self.view.len() - 1);
        let (from, base) = self.anchor.clone().unwrap_or_else(|| (self.sel, self.picked.clone()));
        let mut picked = base.clone();
        for k in from.min(to)..=from.max(to) {
            let id = &self.entries[self.view[k]].id;
            if !picked.contains(id) {
                picked.push(id.clone());
            }
        }
        self.picked = picked;
        self.sel = to;
        self.anchor = Some((from, base));
    }

    /// Enter: every picked session (list order), else the one under the cursor.
    fn resume_outcome(&self) -> Outcome {
        if self.picked.is_empty() {
            return self.current().map(|e| Outcome::Resume(e.idx)).unwrap_or(Outcome::None);
        }
        let mut idx: Vec<usize> = self.entries.iter().filter(|e| self.is_picked(&e.id)).map(|e| e.idx).collect();
        if idx.len() == 1 {
            return Outcome::Resume(idx.remove(0));
        }
        Outcome::ResumeMany(idx)
    }

    pub fn paste(&mut self, s: &str) {
        self.filter.push_str(s.trim());
        self.refresh();
    }

    /// A click; ctrl picks one more row, shift picks the range up to it.
    pub fn click(&mut self, pos: Position, mods: KeyModifiers) -> Outcome {
        let Some(&(_, k)) = self.hits.iter().find(|(r, _)| r.contains(pos)) else { return Outcome::None };
        if mods.contains(KeyModifiers::CONTROL) {
            if self.picked.is_empty() && k != self.sel {
                self.toggle(self.sel);
            }
            self.toggle(k);
            return Outcome::None;
        }
        if mods.contains(KeyModifiers::SHIFT) {
            self.extend(k);
            return Outcome::None;
        }
        if k == self.sel {
            return self.resume_outcome();
        }
        self.sel = k;
        self.anchor = None;
        Outcome::None
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let n = self.view.len();
        match k.code {
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.refresh();
            }
            KeyCode::Esc if !self.picked.is_empty() => {
                self.picked.clear();
                self.anchor = None;
            }
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Enter => return self.resume_outcome(),
            // some terminals report ctrl+space as ctrl+@ or NUL
            KeyCode::Char(' ' | '@') if ctrl => self.toggle(self.sel),
            KeyCode::Null => self.toggle(self.sel),
            KeyCode::Down if shift => self.extend(self.sel + 1),
            KeyCode::Up if shift => self.extend(self.sel.saturating_sub(1)),
            KeyCode::PageDown if shift => self.extend(self.sel + 10),
            KeyCode::PageUp if shift => self.extend(self.sel.saturating_sub(10)),
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
        // a plain move ends the shift range; the next one starts from the cursor
        if !(shift && matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown)) {
            self.anchor = None;
        }
        Outcome::None
    }
}

pub fn draw_in(f: &mut Frame, inner: Rect, p: &mut ContinuePicker, t: &Theme) {
    // filter + scope
    let mut spans = vec![Span::styled("› ", ui::bold_accent(t))];
    if p.filter.is_empty() {
        spans.push(Span::styled("type to search earlier sessions", muted(t)));
    } else {
        spans.push(Span::styled(p.filter.clone(), ui::bold()));
    }
    spans.push(Span::styled("▏", ui::accent(t)));
    let mut scope = vec![];
    p.scope_hits.clear();
    if let Some((_, name)) = &p.project {
        let on = |b: bool| if b { Style::default().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED) } else { muted(t) };
        let project_label = format!(" {} ", ui::fit(name, 18));
        scope.push(Span::styled(project_label.clone(), on(!p.all)));
        scope.push(Span::raw(" "));
        let all_label = " All sessions ";
        scope.push(Span::styled(all_label, on(p.all)));
        scope.push(Span::styled("  Tab ", muted(t)));
        let scope_width = ui::width(&project_label) + 1 + ui::width(all_label) + ui::width("  Tab ");
        let start = inner.right().saturating_sub(scope_width as u16);
        p.scope_hits.push((Rect { x: start, y: inner.y, width: ui::width(&project_label) as u16, height: 1 }, false));
        p.scope_hits.push((Rect { x: start + ui::width(&project_label) as u16 + 1, y: inner.y, width: ui::width(all_label) as u16, height: 1 }, true));
    }
    ui::line_lr(f, Rect { height: 1, ..inner }, spans, scope);

    let rows = inner.height.saturating_sub(3) as usize;
    let view: Vec<Entry> = p.view().into_iter().cloned().collect();
    let start = p.sel.saturating_sub(rows.saturating_sub(1));
    p.hits.clear();
    let now_w = inner.width as usize;
    for (k, e) in view.iter().enumerate().skip(start).take(rows) {
        let y = inner.y + 2 + (k - start) as u16;
        let r = Rect { y, height: 1, ..inner };
        let on = k == p.sel;
        let picked = p.is_picked(&e.id);
        if on {
            f.buffer_mut().set_style(r, Style::default().bg(crate::theme::mix(t.user, ratatui::style::Color::Rgb(20, 20, 24), 0.35)));
        } else if picked {
            f.buffer_mut().set_style(r, Style::default().bg(crate::theme::mix(t.accent, ratatui::style::Color::Rgb(20, 20, 24), 0.82)));
        }
        let brand = ui::harness_color(Some(e.harness), t);
        let id_tail = e.id.rsplit('-').next().unwrap_or(&e.id);
        let mut right = vec![Span::styled(format!("[{id_tail}]  "), muted(t))];
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
        let mut title_style = if on || picked { ui::bold_accent(t) } else { Style::default() };
        if e.archived {
            title_style = title_style.add_modifier(Modifier::CROSSED_OUT).fg(t.muted);
        }
        let left = vec![
            Span::styled(if on { "▌" } else { " " }, ui::accent(t)),
            Span::styled(if picked { "✓" } else { " " }, ui::bold_accent(t)),
            Span::styled(format!("{} ", ui::harness_glyph(Some(e.harness))), fg(brand)),
            Span::styled(ui::fit(&e.title, now_w.saturating_sub(right_w + 6)), title_style),
        ];
        ui::line_lr(f, r, left, right);
        p.hits.push((r, k));
    }
    if view.is_empty() {
        let msg = if p.entries.is_empty() { "  No earlier sessions yet" } else if p.all { "  Nothing matches" } else { "  No sessions here. Press Tab for all sessions." };
        ui::line(f, Rect { y: inner.y + 2, height: 1, ..inner }, vec![Span::styled(msg, muted(t))]);
    }
    let archived = if p.show_archived { "hide archived" } else { "show archived" };
    if p.picked.is_empty() {
        ui::hint_line(f, inner, &[("⏎", "resume"), ("↑↓", "move"), ("⇧↑↓ ^space", "pick several"), ("del", "archive"), ("^z", "undo"), ("^a", archived), ("esc", "close")], t);
    } else {
        let n = p.picked.len();
        let resume = if n == 1 { "resume 1 session".to_string() } else { format!("resume all {n}, one at a time") };
        ui::hint_line(f, inner, &[("⏎", &resume), ("⇧↑↓", "range"), ("^space ^click", "pick / unpick"), ("esc", "clear picks")], t);
    }
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
        // An empty project still stays scoped until the user chooses all sessions.
        let p = ContinuePicker::new(entries, Some(("zzz".into(), "zzz".into())));
        assert!(!p.all);
        assert!(p.view().is_empty());
    }

    #[test]
    fn shift_ranges_and_ctrl_space_pick_several() {
        let entries: Vec<Entry> = (0..5).map(|i| e(i, "a", &format!("task {i}"), false)).collect();
        let mut p = ContinuePicker::new(entries, None);
        let shift = |p: &mut ContinuePicker, code| p.key(KeyEvent::new(code, KeyModifiers::SHIFT));
        shift(&mut p, KeyCode::Down);
        shift(&mut p, KeyCode::Down);
        assert_eq!(p.picked, ["id0", "id1", "id2"]);
        // back up inside the range shrinks it
        shift(&mut p, KeyCode::Up);
        assert_eq!(p.picked, ["id0", "id1"]);
        // a plain move, then ctrl+space adds one more; the picks survive a search
        key(&mut p, KeyCode::Down);
        key(&mut p, KeyCode::Down);
        p.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert_eq!(p.picked, ["id0", "id1", "id3"]);
        key(&mut p, KeyCode::Char('4'));
        assert_eq!(p.picked.len(), 3);
        assert!(matches!(key(&mut p, KeyCode::Enter), Outcome::ResumeMany(ref v) if v == &[0, 1, 3]));
        // esc clears the search, then the picks, then closes
        key(&mut p, KeyCode::Esc);
        key(&mut p, KeyCode::Esc);
        assert!(p.picked.is_empty());
        assert!(matches!(key(&mut p, KeyCode::Esc), Outcome::Close));
    }

    #[test]
    fn ctrl_and_shift_clicks_pick() {
        let entries: Vec<Entry> = (0..4).map(|i| e(i, "a", &format!("task {i}"), false)).collect();
        let mut p = ContinuePicker::new(entries, None);
        p.hits = (0..4).map(|k| (Rect { x: 0, y: k as u16, width: 20, height: 1 }, k)).collect();
        let at = |k: u16| Position { x: 1, y: k };
        // ctrl+click picks the cursor row too, so two rows end up picked
        assert!(matches!(p.click(at(2), KeyModifiers::CONTROL), Outcome::None));
        assert_eq!(p.picked, ["id0", "id2"]);
        p.click(at(2), KeyModifiers::CONTROL);
        assert_eq!(p.picked, ["id0"]);
        // shift+click picks from the last clicked row
        p.click(at(3), KeyModifiers::SHIFT);
        assert_eq!(p.picked, ["id0", "id2", "id3"]);
        // a plain click on the cursor row resumes every pick
        assert!(matches!(p.click(at(3), KeyModifiers::NONE), Outcome::ResumeMany(ref v) if v == &[0, 2, 3]));
    }
}
