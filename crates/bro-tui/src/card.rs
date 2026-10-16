//! The back of a session card: alt+double-click a session and it turns over to show what it's about and where
//! it stands. Everything on it is already in memory (the pane's title and state, its transcript worker's facts,
//! the recap bro picked), so it's ready the instant it turns and keeps up while it's showing.

use crate::theme::Theme;
use crate::ui::{self, fg};
use bro_bridge::AttentionState;
use bro_core::recap::{Recap, Source};
use bro_core::sessions::live::{AgentFacts, StepStatus, TouchKind};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier},
    text::{Line, Span},
};
use std::path::Path;
use std::time::{Duration, Instant};

/// How long a turn takes (edge-on halfway through).
pub(crate) const TURN: Duration = Duration::from_millis(280);
/// The beat between cards when they all turn together.
pub(crate) const CASCADE: Duration = Duration::from_millis(45);

/// Which side of a card faces you, and when it last started turning.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Flip {
    /// Turning to (or showing) the back
    pub back: bool,
    pub at: Instant,
}

impl Flip {
    /// Turn the card over from `prev`. Turning again mid-turn swings back from where it is.
    pub(crate) fn toggled(prev: Option<Flip>, now: Instant) -> Flip {
        let back = !prev.is_some_and(|f| f.back);
        let left = prev.map_or(Duration::ZERO, |f| TURN.saturating_sub(now.saturating_duration_since(f.at)));
        Flip { back, at: now.checked_sub(left).unwrap_or(now) }
    }

    fn progress(&self, now: Instant) -> f32 {
        (now.saturating_duration_since(self.at).as_secs_f32() / TURN.as_secs_f32()).min(1.0)
    }

    pub(crate) fn turning(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }

    /// The face showing now (true = the back) and how much of the card's width it spans: 1 at rest, 0 edge-on.
    pub(crate) fn face(&self, now: Instant) -> (bool, f32) {
        let p = self.progress(now);
        let ease = |x: f32| x * x * (3.0 - 2.0 * x);
        if p < 0.5 { (!self.back, 1.0 - ease(p * 2.0)) } else { (self.back, ease(p * 2.0 - 1.0)) }
    }
}

/// Squeeze what's drawn in `area` to `cover` of its width around the middle, as a card seen turning: the rest
/// is blanked, the card's edges are drawn in `edge`, and the face dims as it goes edge-on.
pub(crate) fn squash(buf: &mut Buffer, area: Rect, cover: f32, edge: Color, t: &Theme) {
    if cover >= 1.0 || area.width == 0 || area.height == 0 {
        return;
    }
    let w = ((area.width as f32 * cover).round() as u16).min(area.width);
    let left = area.x + (area.width - w) / 2;
    let right = left + w;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let Some(c) = buf.cell_mut(Position { x, y }) else { continue };
            if x >= left && x < right {
                if cover < 0.6 {
                    c.modifier.insert(Modifier::DIM);
                }
                continue;
            }
            c.reset();
            if !matches!(t.bg, Color::Reset) {
                c.set_bg(t.bg);
            }
        }
        let mut mark = |x: u16, s: &str| {
            if x >= area.left() && x < area.right()
                && let Some(c) = buf.cell_mut(Position { x, y })
            {
                c.set_symbol(s).set_fg(edge);
            }
        };
        if w == 0 {
            mark(left.min(area.right() - 1), "│");
        } else {
            if left > area.left() {
                mark(left - 1, "▕");
            }
            mark(right, "▏");
        }
    }
}

/// What the back of one card says.
pub(crate) struct Card<'a> {
    /// The session's name: yours, else the agent's own, else the launch label
    pub title: String,
    /// "claude · work · opus"
    pub who: String,
    pub cwd: &'a Path,
    /// How long the pane has been open
    pub open_for: Duration,
    /// None for a plain shell
    pub state: Option<AttentionState>,
    /// Why it waits on you, when it does
    pub reason: Option<String>,
    pub facts: Option<&'a AgentFacts>,
    pub recap: Option<Recap>,
    /// Unix ms now, for "3m ago"
    pub now_ms: i64,
    /// For the working spinner
    pub time: f64,
    /// Web addresses it mentioned or has on screen, newest first
    pub links: Vec<Link>,
}

/// A web address on the back, and for one on this machine whether anything answers there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Link {
    pub url: String,
    pub local: bool,
    /// A local address: something is listening (Some(true)), nothing is (Some(false)), not checked yet (None)
    pub up: Option<bool>,
}

/// A line of the back and what clicking it opens (a URL, or a `file:` link to preview).
pub(crate) type Target = (usize, String);

/// How many rows each section may take.
#[derive(Clone, Copy)]
struct Room {
    asked: usize,
    plan: usize,
    latest: usize,
    files: usize,
    links: usize,
}

const ROOMY: Room = Room { asked: 3, plan: 7, latest: 4, files: 4, links: 4 };
const TIGHT: Room = Room { asked: 2, plan: 3, latest: 2, files: 2, links: 2 };
/// An agent's card before its transcript says anything.
const EMPTY: &str = "Nothing in the transcript yet. This side fills in as the agent works.";
/// The labels' column: "latest" plus two spaces.
const GUTTER: usize = 8;

/// The back of the card as lines `width` columns wide, at most `height` of them, with the lines that open
/// something when clicked.
pub(crate) fn lines(card: &Card, width: u16, height: u16, color: Color, t: &Theme) -> (Vec<Line<'static>>, Vec<Target>) {
    let w = width as usize;
    let (mut out, mut targets) = build(card, w, ROOMY, color, t);
    if out.len() > height as usize {
        (out, targets) = build(card, w, TIGHT, color, t);
    }
    out.truncate(height as usize);
    targets.retain(|(i, _)| *i < out.len());
    (out, targets)
}

fn build(card: &Card, w: usize, room: Room, color: Color, t: &Theme) -> (Vec<Line<'static>>, Vec<Target>) {
    let mut out = vec![];
    let mut targets: Vec<Target> = vec![];
    out.push(Line::from(Span::styled(ui::fit(&card.title, w), fg(color).add_modifier(Modifier::BOLD))));
    // the header already names it after the login when it has no title of its own
    let who = if card.who == card.title { String::new() } else { card.who.clone() };
    let mut about = vec![who, crate::util::short_path(card.cwd, 40)];
    if let Some(b) = card.facts.and_then(|f| f.branch.as_deref()) {
        about.push(format!("⎇ {b}"));
    }
    about.push(format!("open {}", span(card.open_for)));
    let about = about.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    out.push(Line::from(Span::styled(ui::fit(&about, w), ui::muted(t))));
    out.push(Line::from(Span::styled("─".repeat(w), fg(t.frame))));

    // labels on the left when there's room for them, else each section stands alone
    let gutter = if w >= GUTTER + 16 { GUTTER } else { 0 };
    let text_w = w - gutter;
    let section = |out: &mut Vec<Line<'static>>, targets: &mut Vec<Target>, label: &str, rows: Vec<(Vec<Span<'static>>, Option<String>)>| {
        for (i, (row, target)) in rows.into_iter().enumerate() {
            let mut spans = vec![];
            if gutter > 0 {
                let l = if i == 0 { label } else { "" };
                spans.push(Span::styled(format!("{l:>w$}  ", w = gutter - 2), fg(t.muted).add_modifier(Modifier::ITALIC)));
            }
            spans.extend(row);
            if let Some(target) = target {
                targets.push((out.len(), target));
            }
            out.push(Line::from(spans));
        }
    };
    let plain_rows = |rows: Vec<Vec<Span<'static>>>| rows.into_iter().map(|r| (r, None)).collect::<Vec<_>>();
    let links = link_rows(&card.links, text_w, room.links, t);

    let Some(state) = card.state else {
        out.push(Line::from(Span::styled(ui::fit("Plain shell · no agent transcript to read", w), ui::muted(t))));
        if !links.is_empty() {
            out.push(Line::default());
            section(&mut out, &mut targets, "links", links);
        }
        return (out, targets);
    };

    // now: the state, what it's doing or waiting on, and how long ago it last wrote anything
    let (glyph, word, c) = match state {
        AttentionState::Blocked => ("●", "needs you", t.danger),
        AttentionState::Working => (["◐", "◓", "◑", "◒"][(card.time * 6.0) as usize % 4], "working", t.accent),
        AttentionState::Done => ("●", "done", t.good),
        AttentionState::Idle => ("○", "idle", t.muted),
    };
    let doing = match state {
        AttentionState::Working => card.facts.and_then(|f| f.pending.as_ref()).map(|p| p.reason.clone()),
        _ => card.reason.clone(),
    };
    let ago = card.facts.and_then(|f| f.last_activity_ms).filter(|_| state != AttentionState::Working).map(|ms| {
        format!(" · {} ago", span(Duration::from_millis(card.now_ms.saturating_sub(ms).max(0) as u64)))
    });
    let mut now = vec![Span::styled(format!("{glyph} {word}"), fg(c).add_modifier(Modifier::BOLD))];
    let mut used = ui::width(word) + 2;
    if let Some(ago) = &ago {
        used += ui::width(ago);
    }
    if let Some(d) = doing.filter(|d| !d.trim().is_empty()) {
        now.push(Span::styled(ui::fit(&format!(" · {d}"), text_w.saturating_sub(used)), fg(t.fg)));
    }
    if let Some(ago) = ago {
        now.push(Span::styled(ago, ui::muted(t)));
    }
    section(&mut out, &mut targets, "now", vec![(now, None)]);

    let Some(f) = card.facts else {
        out.push(Line::default());
        out.extend(wrap(EMPTY, w, 3).into_iter().map(|l| Line::from(Span::styled(l, ui::muted(t)))));
        if !links.is_empty() {
            out.push(Line::default());
            section(&mut out, &mut targets, "links", links);
        }
        return (out, targets);
    };

    if let Some(p) = &f.prompt {
        let rows = wrap(&p.text, text_w, room.asked).into_iter().map(|l| vec![Span::styled(l, fg(t.user))]).collect();
        section(&mut out, &mut targets, "asked", plain_rows(rows));
    }

    if !f.plan.is_empty() {
        section(&mut out, &mut targets, "plan", plain_rows(plan_rows(f, text_w, room.plan, t)));
    }

    // latest: Claude's own summary or a generated recap, else the agent's last message in full
    let latest = match &card.recap {
        Some(r) if r.source != Source::LastMessage => Some(r.text.clone()),
        _ => f.reply.as_ref().map(|r| r.text.clone()).or_else(|| card.recap.as_ref().map(|r| r.text.clone())),
    };
    if let Some(text) = latest.map(|s| plain(&s)).filter(|s| !s.is_empty()) {
        let rows = wrap(&text, text_w, room.latest).into_iter().map(|l| vec![Span::styled(l, fg(t.fg))]).collect();
        section(&mut out, &mut targets, "latest", plain_rows(rows));
    }

    let changed: Vec<_> = f.touched.iter().filter(|x| x.kind != TouchKind::Read).collect();
    let read = f.touched.len() - changed.len();
    let mut rows: Vec<(Vec<Span<'static>>, Option<String>)> = vec![];
    let show = if changed.len() > room.files { room.files.saturating_sub(1).max(1) } else { changed.len() };
    for x in changed.iter().take(show) {
        let (g, c) = match x.kind {
            TouchKind::Create => ("+", t.good),
            TouchKind::Delete => ("−", t.danger),
            TouchKind::Move => ("→", t.accent),
            _ => ("~", t.accent),
        };
        let rel = x.path.strip_prefix(card.cwd).unwrap_or(&x.path).to_string_lossy().replace('\\', "/");
        let open = (x.kind != TouchKind::Delete).then(|| crate::panes::links::file_uri(&x.path, x.line, None)).flatten();
        rows.push((vec![Span::styled(format!("{g} "), fg(c)), Span::styled(tail(&rel, text_w.saturating_sub(2)), fg(t.fg))], open));
    }
    let more = changed.len() - show;
    let tally = match (more, read) {
        (0, 0) => None,
        (0, r) => Some(format!("read {r}")),
        (m, 0) => Some(format!("+{m} more changed")),
        (m, r) => Some(format!("+{m} more changed · read {r}")),
    };
    if let Some(s) = tally {
        rows.push((vec![Span::styled(ui::fit(&s, text_w), ui::muted(t))], None));
    }
    let nothing_yet = out.len() == 4 && f.prompt.is_none();
    if !links.is_empty() {
        section(&mut out, &mut targets, "links", links);
    }
    if !rows.is_empty() {
        section(&mut out, &mut targets, "files", rows);
    }

    if nothing_yet && out.len() == 4 {
        out.push(Line::default());
        out.extend(wrap(EMPTY, w, 3).into_iter().map(|l| Line::from(Span::styled(l, ui::muted(t)))));
    }
    (out, targets)
}

/// The links in `max` rows: dev servers that answer first, then addresses elsewhere, then local ones not yet
/// checked, then the ones nothing answers on.
fn link_rows(links: &[Link], w: usize, max: usize, t: &Theme) -> Vec<(Vec<Span<'static>>, Option<String>)> {
    let rank = |l: &Link| match (l.local, l.up) {
        (true, Some(true)) => 0,
        (false, _) => 1,
        (true, None) => 2,
        (true, Some(false)) => 3,
    };
    let mut sorted: Vec<&Link> = links.iter().collect();
    sorted.sort_by_key(|l| rank(l));
    let show = if sorted.len() > max { max.saturating_sub(1).max(1) } else { sorted.len() };
    let mut rows = vec![];
    for l in sorted.iter().take(show) {
        let (glyph, gs, note) = match (l.local, l.up) {
            (true, Some(true)) => ("●", fg(t.good), Some(("running", fg(t.good)))),
            (true, Some(false)) => ("○", ui::muted(t), Some(("not running", ui::muted(t)))),
            (true, None) => ("◌", ui::muted(t), None),
            (false, _) => ("↗", fg(t.accent), None),
        };
        let note_w = note.map_or(0, |(n, _)| ui::width(n) + 2);
        let text_style = if l.up == Some(false) { ui::muted(t) } else { fg(t.fg).add_modifier(Modifier::UNDERLINED) };
        let mut row = vec![Span::styled(format!("{glyph} "), gs), Span::styled(ui::fit(&show_url(&l.url), w.saturating_sub(2 + note_w)), text_style)];
        if let Some((n, st)) = note {
            row.push(Span::styled(format!("  {n}"), st));
        }
        rows.push((row, Some(l.url.clone())));
    }
    if sorted.len() > show {
        rows.push((vec![Span::styled(format!("  +{} more", sorted.len() - show), ui::muted(t))], None));
    }
    rows
}

/// An address as it reads best: no scheme, no lone trailing slash.
fn show_url(url: &str) -> String {
    let s = url.split_once("://").map_or(url, |(_, rest)| rest);
    s.strip_suffix('/').filter(|r| !r.contains('/')).unwrap_or(s).to_string()
}

/// The plan in `max` rows: finished steps fold into a count when they don't all fit, the step in progress
/// always shows, and what's left trails off into "+n to go".
fn plan_rows(f: &AgentFacts, w: usize, max: usize, t: &Theme) -> Vec<Vec<Span<'static>>> {
    let done = f.plan.iter().filter(|s| s.status == StepStatus::Done).count();
    let step = |s: &bro_core::sessions::live::PlanStep| -> Vec<Span<'static>> {
        let (g, gs, ts) = match s.status {
            StepStatus::Done => ("✓", fg(t.good), ui::muted(t)),
            StepStatus::Active => ("▸", fg(t.accent).add_modifier(Modifier::BOLD), fg(t.fg).add_modifier(Modifier::BOLD)),
            StepStatus::Pending => ("○", ui::muted(t), fg(t.fg)),
        };
        vec![Span::styled(format!("{g} "), gs), Span::styled(ui::fit(&s.text, w.saturating_sub(2)), ts)]
    };
    let total = f.plan.len();
    let mut rows = vec![];
    if total <= max {
        rows.extend(f.plan.iter().map(step));
        return rows;
    }
    let open: Vec<_> = f.plan.iter().filter(|s| s.status != StepStatus::Done).collect();
    if done > 0 {
        rows.push(vec![Span::styled("✓ ", fg(t.good)), Span::styled(format!("{done} of {total} done"), ui::muted(t))]);
    }
    let room = max.saturating_sub(rows.len());
    let fits = if open.len() > room { room.saturating_sub(1) } else { open.len() };
    rows.extend(open.iter().take(fits).map(|s| step(s)));
    if open.len() > fits {
        rows.push(vec![Span::styled(format!("  +{} to go", open.len() - fits), ui::muted(t))]);
    }
    rows
}

/// Draw the back into `area` on the card's tinted ground; returns the rows that open something when clicked.
pub(crate) fn render(f: &mut Frame, area: Rect, card: &Card, color: Color, t: &Theme) -> Vec<(Rect, String)> {
    if area.width == 0 || area.height == 0 {
        return vec![];
    }
    let ground = crate::theme::project_tint(color, t);
    let buf = f.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = buf.cell_mut(Position { x, y }) {
                c.reset();
                c.set_bg(ground);
            }
        }
    }
    let (pad_x, pad_y) = (if area.width > 30 { 2 } else { 1 }, u16::from(area.height > 8));
    let inner = Rect {
        x: area.x + pad_x,
        y: area.y + pad_y,
        width: area.width.saturating_sub(pad_x * 2),
        height: area.height.saturating_sub(pad_y * 2),
    };
    let (lines, targets) = lines(card, inner.width, inner.height, color, t);
    for (i, line) in lines.into_iter().enumerate() {
        f.buffer_mut().set_line(inner.x, inner.y + i as u16, &line, inner.width);
    }
    targets.into_iter().map(|(i, uri)| (Rect { y: inner.y + i as u16, height: 1, ..inner }, uri)).collect()
}

/// "42s", "12m", "3h 5m", "2d"
fn span(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 if s % 3600 >= 60 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// Markdown read as plain text: emphasis and code marks off, headings and bullets flattened.
fn plain(s: &str) -> String {
    let s = s.replace("**", "").replace("__", "").replace('`', "");
    s.split_whitespace().filter(|w| !matches!(*w, "#" | "##" | "###" | "-" | "*")).collect::<Vec<_>>().join(" ")
}

/// The end of a path in `w` columns: `…/src/app/card.rs`.
fn tail(s: &str, w: usize) -> String {
    if ui::width(s) <= w {
        return s.to_string();
    }
    let mut out: Vec<char> = vec![];
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    std::iter::once('…').chain(out.into_iter().rev()).collect()
}

/// Word-wrap to `w` columns in at most `max` lines, ending in `…` when there's more.
fn wrap(text: &str, w: usize, max: usize) -> Vec<String> {
    if w == 0 || max == 0 {
        return vec![];
    }
    let mut lines: Vec<String> = vec![];
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let sep = usize::from(!cur.is_empty());
        if ui::width(&cur) + sep + ui::width(word) <= w {
            if sep == 1 {
                cur.push(' ');
            }
            cur.push_str(word);
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        // a word longer than the line is cut across lines
        let mut rest = word;
        while ui::width(rest) > w {
            let mut cut = 0;
            let mut used = 0;
            for (i, c) in rest.char_indices() {
                let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if used + cw > w {
                    break;
                }
                used += cw;
                cut = i + c.len_utf8();
            }
            if cut == 0 {
                break;
            }
            lines.push(rest[..cut].to_string());
            rest = &rest[cut..];
        }
        cur = rest.to_string();
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > max {
        lines.truncate(max);
        let last = lines.last_mut().expect("max > 0");
        while !last.is_empty() && ui::width(last) + 1 > w {
            last.pop();
        }
        let trimmed = last.trim_end().to_string();
        *last = format!("{trimmed}…");
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_core::Harness;
    use serde_json::json;
    use std::path::PathBuf;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string()).collect()
    }

    /// An absolute folder on this platform (a file link needs one).
    const ROOT: &str = if cfg!(windows) { r"C:\w\app" } else { "/w/app" };
    const SEP: &str = if cfg!(windows) { "\\" } else { "/" };

    fn facts() -> AgentFacts {
        let mut f = AgentFacts::new(Harness::Claude);
        let lines = [
            json!({"type": "user", "timestamp": "2026-10-15T10:00:00Z", "cwd": ROOT, "gitBranch": "feat/dark", "message": {"content": "add a dark mode toggle to the settings screen"}}),
            json!({"type": "assistant", "timestamp": "2026-10-15T10:00:05Z", "message": {"id": "m1", "content": [{"type": "tool_use", "id": "t1", "name": "TodoWrite", "input": {"todos": [
                {"content": "Add the setting", "status": "completed"}, {"content": "Wire the toggle", "status": "in_progress"}, {"content": "Test it", "status": "pending"}]}}]}}),
            json!({"type": "assistant", "timestamp": "2026-10-15T10:00:06Z", "message": {"id": "m2", "content": [{"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": format!("{ROOT}{SEP}src{SEP}settings.rs")}}]}}),
            json!({"type": "user", "timestamp": "2026-10-15T10:00:07Z", "message": {"content": [{"type": "tool_result", "tool_use_id": "t2", "content": "ok"}]}, "toolUseResult": {}}),
            json!({"type": "assistant", "timestamp": "2026-10-15T10:01:00Z", "message": {"id": "m3", "content": [{"type": "text", "text": "The **toggle** works. Want me to add tests?"}]}}),
        ];
        f.ingest(&lines.iter().map(|l| l.to_string()).collect::<Vec<_>>());
        f
    }

    #[test]
    fn the_back_says_what_its_about_and_where_it_stands() {
        let t = crate::theme::get("default");
        let f = facts();
        let cwd = PathBuf::from(ROOT);
        let card = Card {
            title: "Dark mode toggle".into(),
            who: "claude · work".into(),
            cwd: &cwd,
            open_for: Duration::from_secs(42 * 60),
            state: Some(AttentionState::Done),
            reason: Some("Want me to add tests?".into()),
            facts: Some(&f),
            recap: bro_core::recap::choose(&f, None),
            now_ms: chrono::DateTime::parse_from_rfc3339("2026-10-15T10:04:00Z").unwrap().timestamp_millis(),
            time: 0.0,
            links: vec![],
        };
        let (got, targets) = lines(&card, 60, 40, Color::Cyan, &t);
        let got = text(&got);
        assert_eq!(got[0], "Dark mode toggle");
        assert!(got[1].contains("⎇ feat/dark") && got[1].ends_with("open 42m"), "{}", got[1]);
        assert!(got.iter().any(|l| l.contains("now  ● done · Want me to add tests? · 3m ago")), "{got:#?}");
        assert!(got.iter().any(|l| l.contains("asked  add a dark mode toggle to the settings screen")), "{got:#?}");
        assert!(got.iter().any(|l| l.contains("plan  ✓ Add the setting")));
        assert!(got.iter().any(|l| l.contains("▸ Wire the toggle")));
        assert!(got.iter().any(|l| l.contains("latest  The toggle works. Want me to add tests?")), "markdown off: {got:#?}");
        assert!(got.iter().any(|l| l.contains("files  ~ src/settings.rs")), "relative to the session's folder: {got:#?}");
        let file_row = got.iter().position(|l| l.contains("~ src/settings.rs")).unwrap();
        assert!(targets.iter().any(|(i, u)| *i == file_row && u.starts_with("file:") && u.ends_with("settings.rs")), "files open on click: {targets:?}");
        // short on rows: sections shrink, the header stays
        let tight = text(&lines(&card, 60, 8, Color::Cyan, &t).0);
        assert_eq!(tight.len(), 8);
        assert_eq!(tight[0], "Dark mode toggle");
        // a plain shell says so
        let shell = Card {
            state: None,
            facts: None,
            recap: None,
            links: vec![Link { url: "http://localhost:5173/".into(), local: true, up: Some(true) }],
            ..card
        };
        let (shown, targets) = lines(&shell, 60, 20, Color::Cyan, &t);
        let shown = text(&shown);
        assert!(shown.iter().any(|l| l.starts_with("Plain shell")));
        assert!(shown.iter().any(|l| l.contains("links  ● localhost:5173  running")), "a dev server in a shell: {shown:#?}");
        assert_eq!(targets.last().map(|(_, u)| u.as_str()), Some("http://localhost:5173/"));
    }

    #[test]
    fn running_dev_servers_lead_the_links() {
        let t = crate::theme::get("default");
        let l = |url: &str, local, up| Link { url: url.into(), local, up };
        let links = [
            l("http://localhost:3000/", true, Some(false)),
            l("https://github.com/o/r/pull/12", false, None),
            l("http://localhost:5173/", true, Some(true)),
            l("http://localhost:8080/api", true, None),
            l("https://x.dev/", false, None),
        ];
        let rows: Vec<String> = link_rows(&links, 50, 4, &t).iter().map(|(r, _)| r.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(rows, vec!["● localhost:5173  running", "↗ github.com/o/r/pull/12", "↗ x.dev", "  +2 more"]);
        let all: Vec<String> = link_rows(&links, 50, 9, &t).iter().map(|(r, _)| r.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(all.last().map(String::as_str), Some("○ localhost:3000  not running"), "nothing answering goes last");
        assert_eq!(all[3], "◌ localhost:8080/api");
    }

    #[test]
    fn long_plans_fold_what_is_done() {
        let t = crate::theme::get("default");
        let mut f = AgentFacts::new(Harness::Claude);
        let todos: Vec<_> = (0..10).map(|i| json!({"content": format!("step {i}"), "status": if i < 6 { "completed" } else if i == 6 { "in_progress" } else { "pending" }})).collect();
        f.ingest_line(&json!({"type": "assistant", "message": {"id": "m", "content": [{"type": "tool_use", "id": "t", "name": "TodoWrite", "input": {"todos": todos}}]}}).to_string());
        let rows: Vec<String> = plan_rows(&f, 40, 4, &t).iter().map(|r| r.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(rows, vec!["✓ 6 of 10 done", "▸ step 6", "○ step 7", "  +2 to go"]);
    }

    #[test]
    fn wrapping_and_paths() {
        assert_eq!(wrap("one two three four", 9, 5), vec!["one two", "three", "four"]);
        assert_eq!(wrap("one two three four", 9, 2), vec!["one two", "three…"]);
        assert_eq!(wrap("abcdefghij", 4, 5), vec!["abcd", "efgh", "ij"]);
        assert_eq!(tail("crates/bro-tui/src/card.rs", 12), "…src/card.rs");
        assert_eq!(span(Duration::from_secs(3 * 3600 + 5 * 60)), "3h 5m");
        assert_eq!(plain("## Done\n- **fixed** `x`"), "Done fixed x");
    }

    #[test]
    fn a_turn_goes_edge_on_halfway_and_can_swing_back() {
        let t0 = Instant::now();
        let f = Flip::toggled(None, t0);
        assert!(f.back);
        assert_eq!(f.face(t0), (false, 1.0), "starts on the front, full width");
        let (back, cover) = f.face(t0 + TURN / 2);
        assert!(back && cover < 0.01, "edge-on halfway");
        assert_eq!(f.face(t0 + TURN), (true, 1.0));
        assert!(!f.turning(t0 + TURN));
        // turned again a quarter of the way in: swings back from the same width
        let at = t0 + TURN / 4;
        let (_, before) = f.face(at);
        let g = Flip::toggled(Some(f), at);
        assert!(!g.back);
        let (face, after) = g.face(at);
        assert!(!face && (before - after).abs() < 0.02, "{before} vs {after}");
    }

    #[test]
    fn squash_blanks_the_sides_and_draws_the_edges() {
        let t = crate::theme::get("default");
        let area = Rect { x: 0, y: 0, width: 10, height: 1 };
        let mut buf = Buffer::with_lines(["abcdefghij"]);
        squash(&mut buf, area, 0.4, Color::Red, &t);
        let row: String = (0..10).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(row, "  ▕defg▏  ");
    }
}
