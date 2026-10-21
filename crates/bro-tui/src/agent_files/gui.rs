//! The agent files list in the desktop app (prefix F): what the focused agent sent you and the files it
//! touched, newest first, on a card over the dimmed window. Keys move the selection through the workspace; a
//! click on a row opens it in the file preview at once, as in the terminal UI.

use super::{AgentFilesView, Item, Outcome, Section, ago};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{self, Style, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use crate::file_view::gui::{fit_path, keycap, keycap_w, para_centered};
use bro_gfx::{Point, Rect};
use crossterm::event::KeyModifiers;
use std::cell::RefCell;

// click targets: the hints, then one per row
const PREVIEW: usize = 1;
const CLOSE: usize = 2;
const ROW: usize = 100;

/// The card: the window less this on each side, kept between `MIN_W` and `MAX_W`.
const MARGIN: f32 = 48.0;
const MIN_W: f32 = 520.0;
const MAX_W: f32 = 880.0;
const TITLE_H: f32 = 64.0;
const FOOT_H: f32 = 52.0;
/// The title and the section labels start here; rows' fills `INSET` from the card's edge.
const PAD: f32 = 24.0;
const INSET: f32 = 12.0;
const HEAD_H: f32 = 30.0;
const GAP_H: f32 = 10.0;
const ROW_H: f32 = 34.0;
/// A row with a caption under its path.
const ROW2_H: f32 = 50.0;
const LIST_PAD: f32 = 4.0;
const EMPTY_H: f32 = 184.0;
/// The column the ages line up in.
const AGE_W: f32 = 34.0;
/// How long a scroll glides, and the card's open animation, in seconds.
const GLIDE: f32 = 0.14;
const OPEN: f64 = 0.16;

const ICON_FILE: &str = "\u{F0214}";
const ICON_EMPTY: &str = "\u{F1518}";

/// The terminal UI's kind marks (➤ ✎ + − → ·) as icons.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "sent" => "\u{F048A}",
        "edit" | "write" => "\u{F03EB}",
        "create" => "\u{F0415}",
        "delete" => "\u{F0374}",
        "move" => "\u{F0054}",
        _ => "\u{F09DF}",
    }
}

/// Draw the agent files list over the whole window.
pub(crate) fn draw(s: &mut AgentFilesView, ui: &mut Ui, area: Rect, _cx: &Cx) -> bool {
    paint(s, ui, area)
}

/// Target `n` was clicked: a row opens at once (as in the terminal UI); the hints preview or close.
pub(crate) fn click(s: &mut AgentFilesView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32) -> Outcome {
    match n {
        CLOSE => Outcome::Close,
        PREVIEW if !s.items.is_empty() => Outcome::Open(s.sel),
        n if n >= ROW => s.activate(n - ROW),
        _ => Outcome::None,
    }
}

/// The wheel moves the selection three rows, as in the terminal UI.
pub(crate) fn wheel(s: &mut AgentFilesView, _n: Option<usize>, down: bool) {
    s.move_by(if down { 3 } else { -3 });
}

// ------------------------------------------------------------------ layout

/// What the list shows, top to bottom (the terminal UI's rows: a header before each section, a gap between).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    Gap,
    Head(Section),
    Item(usize),
}

fn lines(items: &[Item]) -> Vec<Line> {
    let mut out = vec![];
    let mut last = None;
    for (i, it) in items.iter().enumerate() {
        if last != Some(it.section) {
            if last.is_some() {
                out.push(Line::Gap);
            }
            out.push(Line::Head(it.section));
            last = Some(it.section);
        }
        out.push(Line::Item(i));
    }
    out
}

fn captioned(it: &Item) -> bool {
    it.caption.as_deref().is_some_and(|c| !c.trim().is_empty())
}

fn line_h(line: Line, items: &[Item]) -> f32 {
    match line {
        Line::Gap => GAP_H,
        Line::Head(_) => HEAD_H,
        Line::Item(i) if captioned(&items[i]) => ROW2_H,
        Line::Item(_) => ROW_H,
    }
}

/// Where the parts go (logical pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    card: Rect,
    title: Rect,
    list: Rect,
    foot: Rect,
}

/// The card over `window` for a list `content` pixels tall (None: the empty message), `lift` lower while it
/// opens.
fn layout(window: Rect, content: Option<f32>, lift: f32) -> Layout {
    let w = (window.w - 2.0 * MARGIN).clamp(MIN_W, MAX_W).min(window.w - 24.0).max(1.0).round();
    let room = (window.h - 2.0 * MARGIN - TITLE_H - FOOT_H).max(ROW_H * 3.0);
    let list_h = content.unwrap_or(EMPTY_H).min(room).round();
    let h = TITLE_H + list_h + FOOT_H;
    let card = Rect::new((window.x + (window.w - w) / 2.0).round(), (window.y + ((window.h - h) * 0.4).max(0.0) + lift).round(), w, h);
    let title = Rect::new(card.x, card.y, card.w, TITLE_H);
    let list = Rect::new(card.x, title.bottom(), card.w, list_h);
    let foot = Rect::new(card.x, list.bottom(), card.w, FOOT_H);
    Layout { card, title, list, foot }
}

/// The offset that keeps line `sel` in view, moving as little as the terminal UI does: up to show the line
/// above it (its section's header), down until it fits with the top line whole.
fn keep(off: f32, view: f32, content: f32, tops: &[f32], heights: &[f32], sel: Option<usize>) -> f32 {
    let mut off = off;
    if let Some(i) = sel {
        let (top, bottom) = (tops[i], tops[i] + heights[i]);
        if top < off {
            off = if i <= 1 { 0.0 } else { tops[i - 1] };
        }
        if bottom > off + view {
            off = if i + 1 == tops.len() {
                content - view
            } else {
                let want = bottom - view;
                tops.iter().copied().find(|&t| t >= want - 0.5).unwrap_or(want).min(top)
            };
        }
    }
    off.clamp(0.0, (content - view).max(0.0))
}

// ------------------------------------------------------------------ motion

/// What moves between frames: the open animation and the list's offset, which glides toward the one that
/// keeps the selection in view.
#[derive(Default)]
struct Motion {
    /// the list it belongs to (a new one starts over)
    id: u64,
    opened: f64,
    at: f64,
    moving: bool,
    off: f32,
    shown: f32,
}

thread_local! {
    static MOTION: RefCell<Motion> = RefCell::new(Motion::default());
}

/// The open animation (0 → 1) and the offset to draw the list at.
fn motion(ui: &mut Ui, s: &AgentFilesView, view: f32, content: f32, tops: &[f32], heights: &[f32], sel: Option<usize>) -> (f32, f32) {
    let id = kit::id((s as *const AgentFilesView as usize, s.pane, s.title.as_str(), s.items.len()));
    let now = ui.time;
    let (open, shown, moving) = MOTION.with(|cell| {
        let mut m = cell.borrow_mut();
        if m.id != id {
            *m = Motion { id, opened: now, at: now, ..Motion::default() };
        }
        m.off = keep(m.off, view, content, tops, heights, sel);
        let dt = if m.moving { (now - m.at).clamp(0.0, 0.034) as f32 } else { 1.0 / 60.0 };
        m.at = now;
        let to = m.off;
        if (to - m.shown).abs() > view * 1.5 || (to - m.shown).abs() < 0.3 {
            m.shown = to;
        } else {
            m.shown += (to - m.shown) * (1.0 - (-dt * 4.6 / GLIDE).exp());
        }
        m.moving = m.shown != to;
        let open = ((now - m.opened) / OPEN).clamp(0.0, 1.0) as f32;
        (1.0 - (1.0 - open).powi(3), m.shown, m.moving)
    });
    if moving || open < 1.0 {
        ui.ticking = true;
    }
    (open, shown)
}

// ------------------------------------------------------------------ drawing

/// [`draw`] without the app's context (the screenshot test draws it too).
pub(crate) fn paint(s: &mut AgentFilesView, ui: &mut Ui, area: Rect) -> bool {
    let lines = lines(&s.items);
    let heights: Vec<f32> = lines.iter().map(|l| line_h(*l, &s.items)).collect();
    let mut tops = Vec::with_capacity(lines.len());
    let mut y = LIST_PAD;
    for h in &heights {
        tops.push(y);
        y += h;
    }
    let content = (!s.items.is_empty()).then_some(y + LIST_PAD);
    let rest = layout(area, content, 0.0);
    // PgUp / PgDn move by the rows in view
    s.set_rows(((rest.list.h / ROW_H).floor() as usize).saturating_sub(1));
    let sel = lines.iter().position(|l| *l == Line::Item(s.sel));
    let (open, off) = motion(ui, s, rest.list.h, content.unwrap_or(0.0), &tops, &heights, sel);
    let l = layout(area, content, ((1.0 - open) * 10.0).round());

    widgets::scrim(ui, area, open, Action::None);
    widgets::card(ui, l.card, radius::OVERLAY);
    title(ui, s, l.title);
    match content {
        None => empty(ui, &s.empty, l.list),
        Some(content) => {
            let t = ui.t;
            let hair = ui.hair();
            ui.clip(l.list, 0.0);
            for (i, line) in lines.iter().enumerate() {
                let r = Rect::new(l.list.x + INSET, l.list.y + tops[i] - off, l.list.w - 2.0 * INSET, heights[i]);
                if r.bottom() < l.list.y || r.y > l.list.bottom() {
                    continue;
                }
                match *line {
                    Line::Gap => {}
                    Line::Head(section) => {
                        let n = s.items.iter().filter(|it| it.section == section).count();
                        let label = if section == Section::Sent { "sent to you" } else { "touched" };
                        header(ui, Rect::new(l.list.x + PAD, r.y, l.list.w - 2.0 * PAD, r.h), label, n);
                    }
                    Line::Item(i) => row(ui, r, &s.items[i], i, i == s.sel),
                }
            }
            ui.unclip();
            // an edge where rows are hidden gets a hairline
            if off > 0.5 {
                ui.hline(l.list.x + hair, l.list.y, l.list.w - 2.0 * hair, t.hairline);
            }
            if off + l.list.h < content - 0.5 {
                ui.hline(l.list.x + hair, l.list.bottom() - hair, l.list.w - 2.0 * hair, t.hairline);
            }
            let hot = ui.hover_amount("agent-files-scroll", l.list);
            widgets::scrollbar(ui, Rect::new(l.list.x, l.list.y, l.list.w - 3.0, l.list.h), content, l.list.h, off, 0.6 + 0.4 * hot);
        }
    }
    hints(ui, s, l.foot);
    true
}

/// "agent files · {session}" beside the file icon.
fn title(ui: &mut Ui, s: &AgentFilesView, r: Rect) {
    let t = ui.t;
    let mark = Rect::new(r.x + PAD - 2.0, r.y + ((r.h - 24.0) / 2.0).round() + 2.0, 24.0, 24.0);
    ui.icon(mark, ICON_FILE, 20.0, t.text);
    let x = mark.right() + 10.0;
    let st = Style::new(size::HEADING, weight::SEMIBOLD, t.text);
    let spans = vec![("agent files".to_string(), st), (" · ".to_string(), st.weight(weight::REGULAR).color(t.faint)), (s.title.clone(), st.weight(weight::MEDIUM).color(t.muted))];
    let b = ui.rich(&spans, size::HEADING, Some((r.right() - PAD - x).max(40.0)), false, None);
    ui.draw(&b, Point::new(x, mark.y + (mark.h - b.h) / 2.0));
}

/// A section's heading as WinUI titles a group (sentence case, semibold, no rule) and its count after it,
/// quietly, sitting on its first row.
fn header(ui: &mut Ui, r: Rect, label: &str, n: usize) {
    let t = ui.t;
    // `widgets::section`'s style
    let st = Style::new(size::SMALL, weight::SEMIBOLD, t.text.mix(t.muted, 0.35));
    let b = ui.line(&sentence(label), st, None);
    let y = (r.bottom() - 7.0 - b.h).round();
    ui.draw(&b, Point::new(r.x, y));
    let c = ui.line(&n.to_string(), Style::new(size::CAPTION, weight::MEDIUM, t.faint), None);
    ui.draw(&c, Point::new(r.x + b.w + 6.0, y + ((b.h - c.h) / 2.0).round()));
}

/// "sent to you" → "Sent to you".
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// One file: its kind (a plain icon), path and line, caption, and on the right what happened how long ago.
fn row(ui: &mut Ui, r: Rect, it: &Item, i: usize, selected: bool) {
    let t = ui.t;
    widgets::row(ui, r, ("agent-files-row", i), selected, Action::Overlay(ROW + i));
    let mark = Rect::new(r.x + PAD - INSET - 2.0, r.y + ((r.h - 24.0) / 2.0).round(), 24.0, 24.0);
    ui.icon(mark, kind_icon(it.kind), 16.0, if selected { t.text } else { t.muted });

    // right: the kind, then the age in a column of its own
    let small = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let age = ui.line(&ago(it.at_ms), small.color(t.faint), None);
    let kind = ui.line(it.kind, small, None);
    let right = r.right() - 12.0;
    let cy = r.y + r.h / 2.0;
    ui.draw(&age, Point::new(right - age.w, cy - age.h / 2.0));
    let kind_x = right - AGE_W - 10.0 - kind.w;
    ui.draw(&kind, Point::new(kind_x, cy - kind.h / 2.0));

    // left: path[:line] (struck through when deleted), the caption under it
    let x0 = mark.right() + 12.0;
    let room = (kind_x - 16.0 - x0).max(30.0);
    let gone = it.kind == "delete";
    let mono = Style::mono(size::BODY, if gone { t.muted } else { t.text });
    let suffix = it.line.map(|l| format!(":{l}")).unwrap_or_default();
    let fitted = fit_path(ui, &format!("{}{suffix}", it.shown), mono, room);
    let (base, suffix) = match fitted.strip_suffix(suffix.as_str()) {
        Some(b) if !suffix.is_empty() => (b.to_string(), suffix),
        _ => (fitted, String::new()),
    };
    let cut = base.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    let (dir, name) = base.split_at(cut);
    // the selected row reads a little brighter than the rest (monospace keeps its width at any weight)
    let quiet = mono.color(match (gone, selected) {
        (true, _) => t.faint,
        (false, true) => t.text.mix(t.muted, 0.4),
        (false, false) => t.muted,
    });
    let name_st = if gone { mono } else { mono.weight(if selected { weight::SEMIBOLD } else { weight::MEDIUM }) };
    let spans: Vec<(String, Style)> = [(dir, quiet), (name, name_st), (suffix.as_str(), quiet)]
        .into_iter()
        .filter(|(text, _)| !text.is_empty())
        .map(|(text, st)| (text.to_string(), st))
        .collect();
    let path = ui.rich(&spans, size::BODY, None, false, None);
    let caption = it.caption.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(|c| ui.line(c, small, Some(room)));
    let py = match &caption {
        Some(c) => (r.y + (r.h - path.h - 2.0 - c.h) / 2.0).round(),
        None => (cy - path.h / 2.0).round(),
    };
    ui.draw(&path, Point::new(x0, py));
    if gone {
        ui.top_fill(Rect::new(x0, (py + path.h * 0.54).round(), path.w, ui.hair().max(1.0)), 0.0, t.muted);
    }
    if let Some(c) = &caption {
        ui.draw(c, Point::new(x0, py + path.h + 2.0));
    }
}

/// Why there's nothing to list, in the middle of the card.
fn empty(ui: &mut Ui, why: &str, r: Rect) {
    let t = ui.t;
    let w = (r.w - 96.0).clamp(80.0, 560.0);
    let msg = para_centered(ui, why, Style::new(size::BODY, weight::MEDIUM, t.muted), w, 3);
    let h = 44.0 + 14.0 + msg.h;
    let y = (r.y + (r.h - h) / 2.0).round();
    let cx = r.x + r.w / 2.0;
    let mark = Rect::new((cx - 22.0).round(), y, 44.0, 44.0);
    ui.icon(mark, ICON_EMPTY, 36.0, t.faint);
    ui.draw(&msg, Point::new((cx - w / 2.0).round(), y + 44.0 + 14.0));
}

/// "⏎ preview · ↑↓ move · esc close" bottom-left; preview and close take clicks too.
fn hints(ui: &mut Ui, s: &AgentFilesView, r: Rect) {
    let t = ui.t;
    let hints = [("⏎", "preview", (!s.items.is_empty()).then_some(PREVIEW)), ("↑↓", "move", None), ("esc", "close", Some(CLOSE))];
    let mut x = r.x + PAD - 8.0;
    for (key, label, target) in hints {
        let kw = keycap_w(ui, key);
        let b = ui.line(label, Style::new(size::SMALL, weight::MEDIUM, t.muted), None);
        let hr = Rect::new(x, r.y + ((r.h - 28.0) / 2.0).round(), (8.0 + kw + 7.0 + b.w + 9.0).round(), 28.0);
        if let Some(n) = target {
            let hot = ui.hover_amount(("agent-files-hint", n), hr);
            if hot > 0.0 {
                ui.round(hr, radius::CONTROL, t.hover.fade(hot));
            }
            ui.hit(hr, Action::Overlay(n));
        }
        keycap(ui, Point::new(hr.x + 8.0, hr.y + 5.0), key, t.muted, t.control);
        ui.draw(&b, Point::new(hr.x + 8.0 + kw + 7.0, hr.y + (hr.h - b.h) / 2.0));
        x = hr.right() + 6.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_view::gui::tests::{targets, with_ui};
    use bro_bridge::file_preview::ResolveRoots;
    use std::path::PathBuf;

    fn item(section: Section, kind: &'static str, shown: &str, line: Option<u32>, caption: Option<&str>) -> Item {
        Item { section, path: PathBuf::from(shown), shown: shown.into(), line, kind, caption: caption.map(String::from), at_ms: Some(bro_core::util::now_ms() - 90_000) }
    }

    fn view(items: Vec<Item>) -> AgentFilesView {
        AgentFilesView::new(1, ResolveRoots { cwd: PathBuf::from("."), project: None }, "work · opus-5".into(), items, "nothing sent or touched yet".into())
    }

    fn many(n: usize) -> Vec<Item> {
        let mut v = vec![item(Section::Sent, "sent", "out/chart.png", None, Some("the chart"))];
        v.extend((0..n).map(|i| item(Section::Touched, ["edit", "create", "delete", "move", "read", "write"][i % 6], &format!("src/file_{i}.rs"), Some(i as u32 + 1), None)));
        v
    }

    #[test]
    fn the_card_is_clamped_and_sized_to_its_list() {
        let l = layout(Rect::new(0.0, 0.0, 1440.0, 900.0), Some(200.0), 0.0);
        assert_eq!((l.card.w, l.card.h), (880.0, TITLE_H + 200.0 + FOOT_H));
        assert_eq!(l.card.x, 280.0);
        assert_eq!(layout(Rect::new(0.0, 0.0, 700.0, 900.0), Some(200.0), 0.0).card.w, 604.0);
        assert_eq!(layout(Rect::new(0.0, 0.0, 560.0, 900.0), Some(200.0), 0.0).card.w, 520.0, "never under 520");
        assert_eq!(layout(Rect::new(0.0, 0.0, 400.0, 900.0), Some(200.0), 0.0).card.w, 376.0, "unless the window is narrower");
        let tall = layout(Rect::new(0.0, 0.0, 1440.0, 900.0), Some(5000.0), 0.0);
        assert_eq!(tall.list.h, 900.0 - 2.0 * MARGIN - TITLE_H - FOOT_H, "a long list scrolls inside the card");
        assert_eq!(layout(Rect::new(0.0, 0.0, 1440.0, 900.0), None, 0.0).list.h, EMPTY_H);
    }

    #[test]
    fn sections_come_with_a_header_and_a_gap_between() {
        let items = many(2);
        assert_eq!(lines(&items), vec![Line::Head(Section::Sent), Line::Item(0), Line::Gap, Line::Head(Section::Touched), Line::Item(1), Line::Item(2)]);
        assert_eq!(line_h(Line::Item(0), &items), ROW2_H, "a caption takes a second line");
        assert_eq!(line_h(Line::Item(1), &items), ROW_H);
    }

    #[test]
    fn the_selection_stays_in_view() {
        let tops = [0.0, 30.0, 64.0, 98.0, 132.0, 166.0];
        let heights = [30.0, 34.0, 34.0, 34.0, 34.0, 34.0];
        let content = 200.0;
        assert_eq!(keep(0.0, 100.0, content, &tops, &heights, Some(1)), 0.0);
        assert_eq!(keep(0.0, 100.0, content, &tops, &heights, Some(3)), 64.0, "down until it fits, the top row whole");
        assert_eq!(keep(64.0, 100.0, content, &tops, &heights, Some(4)), 98.0, "a row at a time");
        assert_eq!(keep(0.0, 100.0, content, &tops, &heights, Some(5)), 100.0, "the last row shows the end");
        assert_eq!(keep(100.0, 100.0, content, &tops, &heights, Some(3)), 64.0, "up to the row above");
        assert_eq!(keep(100.0, 100.0, content, &tops, &heights, Some(1)), 0.0, "the first row shows its header");
        assert_eq!(keep(500.0, 100.0, content, &tops, &heights, None), 100.0);
    }

    #[test]
    fn rows_open_at_once_and_the_wheel_moves_three() {
        let mut v = view(many(20));
        let (_, hits) = with_ui("graphite", 1440.0, 900.0, |ui| {
            let r = ui.screen;
            assert!(paint(&mut v, ui, r));
        });
        let t = targets(&hits);
        assert!(t.contains(&PREVIEW) && t.contains(&CLOSE) && t.contains(&ROW) && t.contains(&(ROW + 10)), "{t:?}");
        assert!(!t.contains(&(ROW + 20)), "rows past the card's height aren't drawn: {t:?}");
        assert!(v.rows > 5, "page keys move by the rows in view: {}", v.rows);
        let p = Point::new(0.0, 0.0);
        assert!(matches!(click(&mut v, ROW + 4, p, KeyModifiers::NONE, 1), Outcome::Open(4)));
        assert_eq!(v.sel, 4, "a click selects too");
        assert!(matches!(click(&mut v, PREVIEW, p, KeyModifiers::NONE, 1), Outcome::Open(4)));
        assert!(matches!(click(&mut v, CLOSE, p, KeyModifiers::NONE, 1), Outcome::Close));
        assert!(matches!(click(&mut v, ROW + 99, p, KeyModifiers::NONE, 1), Outcome::None));
        wheel(&mut v, None, true);
        assert_eq!(v.sel, 7);
        wheel(&mut v, Some(ROW), false);
        assert_eq!(v.sel, 4);
        // the selection at the end scrolls the list (gliding there): its row ends up inside the list
        v.sel = 20;
        let mut hits = vec![];
        for _ in 0..40 {
            hits = with_ui("graphite", 1440.0, 600.0, |ui| {
                let r = ui.screen;
                assert!(paint(&mut v, ui, r));
            })
            .1;
        }
        let list = layout(Rect::new(0.0, 0.0, 1440.0, 600.0), Some(10_000.0), 0.0).list;
        let last = hits.iter().find(|h| matches!(h.action, Action::Overlay(n) if n == ROW + 20)).expect("the selected row is drawn");
        assert!(last.rect.bottom() <= list.bottom() + 0.5 && last.rect.h >= ROW_H - 0.5, "fully in view: {:?} in {list:?}", last.rect);
    }

    #[test]
    fn an_empty_list_only_closes() {
        let mut v = view(vec![]);
        let (_, hits) = with_ui("paper", 1000.0, 700.0, |ui| {
            let r = ui.screen;
            assert!(paint(&mut v, ui, r));
        });
        assert_eq!(targets(&hits), vec![CLOSE]);
        assert!(matches!(click(&mut v, PREVIEW, Point::new(0.0, 0.0), KeyModifiers::NONE, 1), Outcome::None));
    }
}
