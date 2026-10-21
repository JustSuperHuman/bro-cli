//! The agent files overlay (prefix F): what the focused agent sent you (Claude's `SendUserFile`, the shelf) and
//! the files it edited, created, deleted or read, newest first. Enter opens the file preview at the line.

/// Drawn by the desktop app (`bro gui`).
#[cfg(feature = "gui")]
pub(crate) mod gui;

use crate::layout::PaneId;
use crate::panes::insights::Snapshot;
use crate::theme::Theme;
use crate::ui::{self, fg, muted};
use bro_bridge::file_preview::ResolveRoots;
use bro_core::sessions::live::TouchKind;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::Modifier,
    text::Span,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Sent,
    Touched,
}

/// One row.
#[derive(Clone, Debug)]
pub struct Item {
    pub section: Section,
    pub path: PathBuf,
    /// `src/app.rs` inside the session's folders, else the full path
    pub shown: String,
    pub line: Option<u32>,
    /// "edit", "create", "sent"…
    pub kind: &'static str,
    pub caption: Option<String>,
    pub at_ms: Option<i64>,
}

pub enum Outcome {
    None,
    Close,
    /// open item `i` in the file preview
    Open(usize),
}

pub struct AgentFilesView {
    pub pane: PaneId,
    pub roots: ResolveRoots,
    pub title: String,
    pub items: Vec<Item>,
    /// why there's nothing to list
    pub empty: String,
    pub sel: usize,
    scroll: usize,
    /// (row rect, item index) from the last draw
    hits: Vec<(Rect, usize)>,
    /// list rows that fit, from the last draw
    rows: usize,
}

/// `src/app.rs` when `path` is inside the session's cwd or project, else the full path.
pub fn shown_path(path: &Path, roots: &ResolveRoots) -> String {
    let inside = [Some(roots.cwd.as_path()), roots.project.as_deref()].into_iter().flatten().find_map(|root| {
        let root = crate::util::strip_verbatim(root.to_path_buf());
        path.strip_prefix(&root).ok().map(|r| r.to_string_lossy().replace('\\', "/")).filter(|r| !r.is_empty())
    });
    inside.unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn kind_glyph(kind: &str) -> &'static str {
    match kind {
        "sent" => "➤",
        "edit" | "write" => "✎",
        "create" => "+",
        "delete" => "−",
        "move" => "→",
        _ => "·",
    }
}

/// The overlay's rows from a pane's transcript facts: sent first, then touched (each newest first).
pub fn items(snap: Option<&Snapshot>, roots: &ResolveRoots) -> Vec<Item> {
    let Some(f) = snap.and_then(|s| s.facts.as_deref()) else { return vec![] };
    let sent = f.sent.iter().map(|s| Item { section: Section::Sent, shown: shown_path(&s.path, roots), path: s.path.clone(), line: None, kind: "sent", caption: s.caption.clone(), at_ms: s.at_ms });
    let touched = f.touched.iter().map(|t| Item {
        section: Section::Touched,
        shown: shown_path(&t.path, roots),
        path: t.path.clone(),
        line: t.line,
        kind: match t.kind {
            TouchKind::Edit => "edit",
            TouchKind::Write => "write",
            TouchKind::Create => "create",
            TouchKind::Delete => "delete",
            TouchKind::Move => "move",
            TouchKind::Read => "read",
        },
        caption: None,
        at_ms: t.at_ms,
    });
    sent.chain(touched).collect()
}

fn ago(at_ms: Option<i64>) -> String {
    let Some(at) = at_ms else { return String::new() };
    let now = bro_core::util::now_ms();
    crate::util::short_dur(((now - at).max(0) / 1000) as u64)
}

impl AgentFilesView {
    pub fn new(pane: PaneId, roots: ResolveRoots, title: String, items: Vec<Item>, empty: String) -> AgentFilesView {
        AgentFilesView { pane, roots, title, items, empty, sel: 0, scroll: 0, hits: vec![], rows: 10 }
    }

    fn move_by(&mut self, d: isize) {
        if self.items.is_empty() {
            return;
        }
        self.sel = (self.sel as isize + d).clamp(0, self.items.len() as isize - 1) as usize;
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let page = self.rows.max(1) as isize;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Home | KeyCode::Char('g') => self.sel = 0,
            KeyCode::End | KeyCode::Char('G') => self.sel = self.items.len().saturating_sub(1),
            KeyCode::Enter if !self.items.is_empty() => return Outcome::Open(self.sel),
            _ => {}
        }
        Outcome::None
    }

    pub fn mouse(&mut self, m: MouseEvent) -> Outcome {
        let pos = Position { x: m.column, y: m.row };
        match m.kind {
            MouseEventKind::ScrollDown => self.move_by(3),
            MouseEventKind::ScrollUp => self.move_by(-3),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, i)) = self.hits.iter().find(|(r, _)| r.contains(pos)) {
                    return self.activate(i);
                }
            }
            _ => {}
        }
        Outcome::None
    }

    /// Row `i` was clicked: select it and open it at once.
    pub(crate) fn activate(&mut self, i: usize) -> Outcome {
        if i >= self.items.len() {
            return Outcome::None;
        }
        self.sel = i;
        Outcome::Open(i)
    }

    /// How many rows the list shows (PgUp / PgDn move by that many).
    pub(crate) fn set_rows(&mut self, rows: usize) {
        self.rows = rows.max(1);
    }
}

pub fn draw(f: &mut Frame, area: Rect, v: &mut AgentFilesView, t: &Theme) {
    v.hits.clear();
    let w = area.width.saturating_sub(6).clamp(40, 110);
    let h = (v.items.len() as u16 + 9).clamp(10, area.height.saturating_sub(2).max(10));
    let inner = ui::popup(f, area, w, h, &format!("{}agent files · {}", ui::lead("file"), v.title), t);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let list = ui::hint_line(f, inner, &[("⏎", "preview"), ("↑↓", "move"), ("esc", "close")], t);
    if v.items.is_empty() {
        ui::line(f, Rect { y: list.y + 1, height: 1, ..list }, vec![Span::styled(format!("  {}", v.empty), muted(t))]);
        return;
    }
    // rows: a header before each section, then its files
    let mut lines: Vec<Option<usize>> = vec![];
    let mut last = None;
    for (i, it) in v.items.iter().enumerate() {
        if last != Some(it.section) {
            if last.is_some() {
                lines.push(None);
            }
            lines.push(None);
            last = Some(it.section);
        }
        lines.push(Some(i));
    }
    let room = list.height as usize;
    v.rows = room.saturating_sub(2).max(1);
    let sel_line = lines.iter().position(|l| *l == Some(v.sel)).unwrap_or(0);
    if sel_line < v.scroll {
        v.scroll = sel_line.saturating_sub(1);
    } else if sel_line >= v.scroll + room {
        v.scroll = sel_line + 1 - room;
    }
    let width = list.width as usize;
    for (row, l) in lines.iter().enumerate().skip(v.scroll).take(room) {
        let y = list.y + (row - v.scroll) as u16;
        let r = Rect { y, height: 1, ..list };
        match l {
            None => {
                // a header is the row right before its section's first item
                let next = lines.get(row + 1).copied().flatten().map(|i| v.items[i].section);
                let (label, n) = match next {
                    Some(Section::Sent) => ("sent to you", v.items.iter().filter(|i| i.section == Section::Sent).count()),
                    Some(Section::Touched) => ("touched", v.items.iter().filter(|i| i.section == Section::Touched).count()),
                    None => continue,
                };
                ui::line(f, r, vec![Span::styled(label.to_string(), ui::bold()), Span::styled(format!("  {n}"), muted(t))]);
            }
            Some(i) => {
                let it = &v.items[*i];
                let on = *i == v.sel;
                let loc = match it.line {
                    Some(l) => format!("{}:{l}", it.shown),
                    None => it.shown.clone(),
                };
                let right = vec![Span::styled(format!(" {}  {} ", it.kind, ago(it.at_ms)), muted(t))];
                let rw: usize = right.iter().map(|s| ui::width(&s.content)).sum();
                let mut name_style = if on { ui::bold_accent(t) } else { fg(t.fg) };
                if it.kind == "delete" {
                    name_style = name_style.add_modifier(Modifier::CROSSED_OUT);
                }
                let mut left = vec![Span::styled(if on { "▌" } else { " " }, ui::accent(t)), Span::styled(format!("{} ", kind_glyph(it.kind)), fg(t.shine))];
                let room = width.saturating_sub(rw + 4);
                let caption = it.caption.as_deref().map(|c| format!("  {c}")).unwrap_or_default();
                let loc_w = ui::width(&loc).min(room);
                left.push(Span::styled(ui::fit(&loc, room), name_style));
                if !caption.is_empty() && room > loc_w + 4 {
                    left.push(Span::styled(ui::fit(&caption, room - loc_w), muted(t)));
                }
                ui::line_lr(f, r, left, right);
                v.hits.push((r, *i));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_core::Harness;
    use bro_core::sessions::live::AgentFacts;
    use std::sync::Arc;

    fn view(dir: &Path) -> AgentFilesView {
        let mut f = AgentFacts::new(Harness::Claude);
        let p = |s: &str| dir.join(s).to_string_lossy().into_owned();
        f.ingest(&[
            serde_json::json!({"type":"assistant","timestamp":"2026-10-02T10:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":p("src/app.rs"),"old_string":"a","new_string":"b"}}]}}).to_string(),
            serde_json::json!({"type":"user","timestamp":"2026-10-02T10:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"toolUseResult":{"filePath":p("src/app.rs"),"structuredPatch":[{"newStart":12}]}}).to_string(),
            serde_json::json!({"type":"assistant","timestamp":"2026-10-02T10:00:02Z","message":{"id":"m2","content":[{"type":"tool_use","id":"t2","name":"SendUserFile","input":{"files":[p("out/chart.png")],"caption":"the chart"}}]}}).to_string(),
            serde_json::json!({"type":"user","timestamp":"2026-10-02T10:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"1 file delivered"}]}}).to_string(),
        ]);
        let snap = Snapshot { facts: Some(Arc::new(f)), ..Snapshot::default() };
        let roots = ResolveRoots { cwd: dir.to_path_buf(), project: None };
        AgentFilesView::new(1, roots.clone(), "test".into(), items(Some(&snap), &roots), "nothing".into())
    }

    #[test]
    fn sent_files_come_first_then_touched_with_lines() {
        let dir = tempfile::tempdir().unwrap();
        let v = view(dir.path());
        let shown: Vec<(&str, &str, Option<u32>)> = v.items.iter().map(|i| (i.kind, i.shown.as_str(), i.line)).collect();
        assert_eq!(shown, vec![("sent", "out/chart.png", None), ("edit", "src/app.rs", Some(12))]);
        assert_eq!(v.items[0].caption.as_deref(), Some("the chart"));
    }

    #[test]
    fn keys_move_and_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = view(dir.path());
        assert!(matches!(v.key(KeyEvent::from(KeyCode::Char('j'))), Outcome::None));
        assert_eq!(v.sel, 1);
        assert!(matches!(v.key(KeyEvent::from(KeyCode::Char('j'))), Outcome::None));
        assert_eq!(v.sel, 1, "stays on the last row");
        assert!(matches!(v.key(KeyEvent::from(KeyCode::Enter)), Outcome::Open(1)));
        assert!(matches!(v.key(KeyEvent::from(KeyCode::Esc)), Outcome::Close));
        let mut empty = AgentFilesView::new(1, v.roots.clone(), "x".into(), vec![], "nothing yet".into());
        assert!(matches!(empty.key(KeyEvent::from(KeyCode::Enter)), Outcome::None));
    }
}
