//! Holding alt shows what it does, where it does it: each session's card names its jump key and the flip,
//! the focused one adds its pane keys, and the sidebar lights up its session numbers and the keys of the
//! things you can click. Keys show without the `alt+` (you're already holding it), as menu key tips do.
//!
//! Windows reports a bare alt press (`console_nav`); everywhere else a mouse event with alt held counts.

use super::{App, SideHit};
use crate::keymap::Act;
use crate::pane::PaneId;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Position,
    style::{Modifier, Style},
    text::Span,
};
use std::time::{Duration, Instant};

/// Hold alt this long before the hints show, so a quick alt chord doesn't flash them.
pub(crate) const HOLD: Duration = Duration::from_millis(300);

impl App {
    /// Alt went down (`true`) or up, or input said whether it's held.
    pub(crate) fn alt_held(&mut self, down: bool) {
        if down {
            self.alt_down.get_or_insert_with(Instant::now);
        } else {
            self.alt_down = None;
        }
    }

    /// Whether the key hints show now.
    pub(crate) fn alt_hints(&self) -> bool {
        self.alt_down.is_some_and(|t| t.elapsed() >= HOLD) && !self.overlay.is_open()
    }

    /// How long until held alt turns the hints on (None when it won't, or they're on).
    pub(crate) fn alt_wake(&self) -> Option<Duration> {
        let left = HOLD.checked_sub(self.alt_down?.elapsed())?;
        Some(left + Duration::from_millis(5))
    }

    /// `act`'s key with alt held: "i" for alt+i, "I" for alt+shift+i; None when it has no alt chord.
    fn alt_key(&self, act: Act) -> Option<String> {
        self.keymap.chords_for(act).into_iter().find_map(|c| c.strip_prefix("alt+").map(str::to_string))
    }

    pub(super) fn draw_alt_hints(&self, f: &mut Frame) {
        if !self.alt_hints() {
            return;
        }
        let t = &self.theme;
        let key = Style::default().fg(t.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD);
        let label = Style::default().fg(t.accent).bg(t.bg);
        let focus = self.focused().filter(|_| !self.side_focus);
        let order = crate::sidebar::live_order(&self.live_infos(), &self.past_infos(), &self.open_infos());
        let jump = |id: PaneId| order.iter().position(|p| *p == id).filter(|i| *i < 9).and_then(|i| self.alt_key(Act::Jump(i as u8 + 1)));

        // sessions: on the bottom edge, after the status already there, what alt does to this one
        for &(id, r) in &self.outer {
            if r.width < 12 || r.height < 3 || !self.panes.get(&id).is_some_and(|p| p.as_term_ref().is_some()) {
                continue;
            }
            let y = r.bottom() - 1;
            let x_min = past_label(f.buffer_mut(), r.x + 1, r.right().saturating_sub(5), y) + 1;
            // most useful first: the narrowest cards keep only the first
            let mut tips: Vec<(String, &str)> = vec![("dbl-click".into(), "flip")];
            if focus != Some(id)
                && let Some(k) = jump(id)
            {
                tips.push((k, "go"));
            }
            if focus == Some(id) {
                for (act, what) in [(Act::Flip, "flip"), (Act::FlipAll, "flip all"), (Act::Zoom, "zoom"), (Act::Grow, "grow")] {
                    if let Some(k) = self.alt_key(act) {
                        tips.push((k, what));
                    }
                }
            }
            // the close button keeps the last few cells
            let room = r.right().saturating_sub(5).saturating_sub(x_min) as usize;
            while !tips.is_empty() && width(&tips) > room {
                tips.pop();
            }
            if tips.is_empty() {
                continue;
            }
            let w = width(&tips) as u16;
            let x = (r.x + (r.width.saturating_sub(w)) / 2).max(x_min);
            let spans = tips.iter().flat_map(|(k, what)| [Span::styled(format!(" {k} "), key), Span::styled(format!(" {what} "), label)]).collect();
            put(f, x, y, r.right().saturating_sub(5), spans);
        }

        // the sidebar: session numbers light up, clickable spots show their key at their right end
        if self.side_area.width == 0 {
            return;
        }
        let rows = self.rows();
        let mut shown: Vec<Act> = vec![];
        for (r, hit) in &self.side_hits {
            let act = match hit {
                SideHit::Row(i) => {
                    if let Some(crate::sidebar::Row::Live { info, n: Some(_) }) = rows.get(*i)
                        && let Some(k) = jump(info.pane)
                        && k.chars().count() == 1
                    {
                        // the number already drawn after the row's bar
                        put(f, r.x + 1, r.y, r.right(), vec![Span::styled(k, key)]);
                    }
                    continue;
                }
                SideHit::Usage => Act::Usage,
                SideHit::Stats => Act::Stats,
                SideHit::Proxy => Act::Proxy,
                SideHit::Bridge => Act::Bridge,
                SideHit::Settings => Act::Settings,
                SideHit::Launcher => Act::NewSession,
                SideHit::Continue => Act::Continue,
                SideHit::TileAll => Act::TileAll,
                _ => continue,
            };
            if shown.contains(&act) {
                continue;
            }
            let Some(k) = self.alt_key(act) else { continue };
            shown.push(act);
            // the key's own letter in the spot's text lights up where it is ("+ New" for n)
            if let Some(x) = letter_in(f.buffer_mut(), *r, &k) {
                f.buffer_mut()[(x, r.y)].set_style(key);
                continue;
            }
            let text = format!(" {k} ");
            let w = crate::ui::width(&text) as u16;
            let (lo, hi) = (self.side_area.x + 1, self.side_area.right().saturating_sub(1));
            let x = beside(f.buffer_mut(), *r, w, lo, hi);
            put(f, x, r.y, hi, vec![Span::styled(text, key)]);
        }
    }
}

/// The column of `key`'s letter in the text of `r`'s first row: either case for a plain letter, only the
/// capital for a shifted one (alt+T mustn't light the t of "tiled").
fn letter_in(f: &Buffer, r: ratatui::layout::Rect, key: &str) -> Option<u16> {
    let mut chars = key.chars();
    let (Some(k), None) = (chars.next(), chars.next()) else { return None };
    if !k.is_ascii_alphabetic() {
        return None;
    }
    let hit = |s: &str| if k.is_ascii_uppercase() { s == k.to_string() } else { s.eq_ignore_ascii_case(&k.to_string()) };
    (r.x..r.right()).find(|&x| f.cell(Position { x, y: r.y }).is_some_and(|c| hit(c.symbol())))
}

fn blank(f: &Buffer, x: u16, y: u16, w: u16, lo: u16, hi: u16) -> bool {
    x >= lo && x + w <= hi && (x..x + w).all(|x| f.cell(Position { x, y }).is_some_and(|c| c.symbol() == " "))
}

/// Where a `w`-wide key tip for the spot `r` goes on its row, between columns `lo` and `hi`: in blank cells at
/// the end of it, else just after it, else just before it, else over its end.
fn beside(f: &Buffer, r: ratatui::layout::Rect, w: u16, lo: u16, hi: u16) -> u16 {
    let end = r.right().min(hi);
    [end.saturating_sub(w), end, r.x.saturating_sub(w)]
        .into_iter()
        .find(|&x| blank(f, x, r.y, w, lo, hi))
        .unwrap_or_else(|| end.saturating_sub(w).max(lo))
}

/// The column just past the label (a session's status) drawn into the frame's edge from `x`, or `x` when
/// there is none.
fn past_label(f: &Buffer, x: u16, end: u16, y: u16) -> u16 {
    let edge = |x: u16| f.cell(Position { x, y }).is_none_or(|c| matches!(c.symbol(), "─" | "━" | "╌" | "┄"));
    let empty = |x: u16| edge(x) || f.cell(Position { x, y }).is_some_and(|c| c.symbol() == " ");
    let gap = |x: u16| (x..x + 3).all(empty);
    let Some(start) = (x..end).find(|&x| !empty(x)) else { return x };
    // the label ends at the edge line, or where the edge is left blank (three empty cells in a row)
    (start..end).find(|&x| edge(x) || gap(x)).unwrap_or(start)
}

/// Columns the tips take: " k " + " what " each.
fn width(tips: &[(String, &str)]) -> usize {
    tips.iter().map(|(k, what)| crate::ui::width(k) + crate::ui::width(what) + 4).sum()
}

/// Draw `spans` from (`x`, `y`) up to column `end`; returns the columns used.
fn put(f: &mut Frame, x: u16, y: u16, end: u16, spans: Vec<Span<'_>>) -> u16 {
    let area = f.area();
    if x >= end || !area.contains(Position { x, y }) {
        return 0;
    }
    let w = end.min(area.right()) - x;
    let line = ratatui::text::Line::from(spans);
    let used = (line.width() as u16).min(w);
    f.buffer_mut().set_line(x, y, &line, w);
    used
}
