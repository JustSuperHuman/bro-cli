//! The command palette (alt+p) in the desktop app: a search field over every action with its live key, the
//! recent launches, Claude presets, caveman levels and themes (with their own colours).
//!
//! Keys reach `Palette::key` through the workspace, with the App's live theme preview; this draws that state and
//! adds the mouse: a click selects a row and a click on the selected row runs it like Enter, the wheel moves the
//! selection, resting on a theme row shows that theme until the pointer leaves, and a click outside closes it
//! and restores the theme (`Action::CloseOverlay`, see `gui/mouse.rs`).
//!
//! The bottom of the file holds small pieces the keys overlay, the resume picker and the confirmation share.

use super::super::actions::Action;
use super::super::kit::brand::{self, Logo, Mark};
use super::super::kit::{self, Style, Ui, radius, size, weight, widgets};
use crate::app::{App, Overlay};
use crate::palette::{Cmd, Item};
use crate::recents::Recent;
use bro_gfx::text::Align;
use bro_gfx::{Color, Point, Rect};
use crossterm::event::KeyModifiers;
use ratatui::style::Color as TuiColor;
use std::ops::Range;
use std::time::{Duration, Instant};

/// Card width, the search row's height (the box and the room around it), the box's height, a row's height and
/// the card's padding (logical px).
const WIDTH: f32 = 640.0;
const SEARCH_H: f32 = 56.0;
const BOX_H: f32 = 40.0;
const ROW_H: f32 = 36.0;
const PAD: f32 = 8.0;
/// At most this many rows at once (fewer when the window is short).
const MAX_ROWS: usize = 11;
/// How long the pointer rests on a theme row before the window shows that theme.
const DWELL: Duration = Duration::from_millis(260);
/// Targets: `ROW + i` is `items[i]`.
const ROW: usize = 1_000;

/// What the palette keeps between frames besides its state machine: the list's scroll and the hover preview.
struct View {
    /// which palette this is about (its address; a new one is a new palette)
    palette: usize,
    /// the first row in view
    top: usize,
    /// the pointer, selection and query last frame (what moved since)
    pointer: Option<Point>,
    sel: usize,
    query: String,
    /// the row under the pointer (`items` index) and since when
    hovered: Option<usize>,
    since: Instant,
    /// the pointer moved since the keyboard (or a click or the wheel) last moved the selection
    armed: bool,
    /// (the theme shown because the pointer rests on its row, the theme to put back when it leaves)
    previewing: Option<(String, String)>,
}

impl Default for View {
    fn default() -> View {
        View { palette: 0, top: 0, pointer: None, sel: 0, query: String::new(), hovered: None, since: Instant::now(), armed: false, previewing: None }
    }
}

thread_local! {
    static VIEW: std::cell::RefCell<View> = std::cell::RefCell::new(View::default());
}

/// Draw the palette over the window; register targets with `ui.hit(r, Action::Overlay(n))`. False = use the
/// terminal UI's drawing.
pub fn draw(app: &mut App, ui: &mut Ui, window: Rect, _time: f64) -> bool {
    let Overlay::Palette(p) = &app.overlay else { return false };
    let prefix = app.keymap.prefix.display();
    let recents = &app.recents;
    let t = ui.t;
    let now = ui.now;
    let m = p.matches();
    let sel = p.sel.min(m.len().saturating_sub(1));
    let mut v = VIEW.take();
    // a palette opened since last frame starts from scratch (a new one, or the pointer was elsewhere last frame)
    let id = &**p as *const crate::palette::Palette as usize;
    let opened = just_opened(ui);
    let fresh = v.palette != id || opened;
    if fresh {
        v = View { palette: id, pointer: ui.pointer, sel: p.sel, query: p.query.clone(), since: now, ..View::default() };
    }

    // (one palette replacing another, like "pick a theme", doesn't fade in again)
    let k = appear(ui, "palette", opened);
    widgets::scrim(ui, window, k, Action::CloseOverlay);
    let w = WIDTH.min(window.w - 32.0).max(280.0);
    let top_y = window.y + (window.h * 0.18).round().max(16.0);
    let room = window.bottom() - 24.0 - top_y - SEARCH_H - PAD;
    let fit = ((room / ROW_H).floor().max(3.0) as usize).min(MAX_ROWS);
    let rows = m.len().min(fit);
    let list_h = if m.is_empty() { 64.0 } else { rows as f32 * ROW_H + PAD };
    let r = Rect::new((window.x + (window.w - w) / 2.0).round(), top_y + (1.0 - k) * 10.0, w, SEARCH_H + list_h);
    widgets::card(ui, r, radius::OVERLAY);

    // the search box (it has the keyboard): what's typed, the caret, how many rows match
    let sr = Rect::new(r.x, r.y, r.w, SEARCH_H);
    let sb = Rect::new(r.x + PAD, r.y + PAD, r.w - 2.0 * PAD, BOX_H);
    ui.panel(sb, radius::CONTROL, t.field_focus, t.accent.with_alpha(0.8));
    ui.icon(Rect::new(sb.x + 8.0, sb.y, 24.0, sb.h), "\u{F0349}", 16.0, if p.query.is_empty() { t.faint } else { t.muted });
    let count = ui.line(&m.len().to_string(), Style::new(size::CAPTION, weight::MEDIUM, t.faint), None);
    ui.draw(&count, Point::new(sb.right() - 12.0 - count.w, sb.y + (sb.h - count.h) / 2.0));
    let field = Rect::new(sb.x + 40.0, sb.y, sb.right() - 12.0 - count.w - 12.0 - (sb.x + 40.0), sb.h);
    query(ui, field, &p.query);

    // the rows, the selection kept in view
    let list = Rect::new(r.x + PAD, sr.bottom(), r.w - 2.0 * PAD, rows as f32 * ROW_H);
    if m.is_empty() {
        ui.label_in(Rect::new(r.x, sr.bottom(), r.w, list_h), "nothing matches", Style::new(size::BODY, weight::REGULAR, t.muted), Align::Center);
    }
    if sel < v.top {
        v.top = sel;
    }
    if rows > 0 && sel >= v.top + rows {
        v.top = sel + 1 - rows;
    }
    v.top = v.top.min(m.len().saturating_sub(rows));
    let shown = glide(ui, "palette", v.top as f32, fresh);
    let mut hovered = None;
    ui.clip(list, 0.0);
    for (pos, &i) in m.iter().enumerate().skip(shown.floor().max(0.0) as usize).take(rows + 1) {
        let rr = Rect::new(list.x, list.y + (pos as f32 - shown) * ROW_H, list.w, ROW_H);
        if ui.hovered(rr) {
            hovered = Some(i);
        }
        row(ui, rr, &p.items[i], i, pos == sel, &p.query, &prefix, recents);
    }
    ui.unclip();
    if m.len() > rows {
        let vis = 0.45 + 0.55 * ui.hover_amount("palette-list", list);
        widgets::scrollbar(ui, Rect::new(list.x, list.y, list.w + PAD - 1.0, list.h), m.len() as f32 * ROW_H, rows as f32 * ROW_H, shown * ROW_H, vis);
    }

    // resting on a theme row shows it; leaving puts back what was there
    let hovered_theme = hovered.and_then(|i| match &p.items[i].cmd {
        Cmd::Theme(name) => Some(name.clone()),
        _ => None,
    });
    let (now_sel, now_query) = (p.sel, p.query.clone());
    if now_sel != v.sel || now_query != v.query {
        // the keyboard (or a click, or the wheel) moved the selection: the App previewed what it wants
        v.armed = false;
        v.previewing = None;
    }
    if v.previewing.as_ref().is_some_and(|(shown, _)| *shown != app.theme.name) {
        // something else changed the theme meanwhile
        v.armed = false;
        v.previewing = None;
    }
    if ui.pointer != v.pointer {
        v.armed = ui.pointer.is_some();
        v.pointer = ui.pointer;
    }
    if hovered != v.hovered {
        v.hovered = hovered;
        v.since = now;
    }
    match hovered_theme.filter(|_| v.armed) {
        Some(want) if now.saturating_duration_since(v.since) >= DWELL => {
            if want != app.theme.name {
                let restore = v.previewing.take().map(|(_, r)| r).unwrap_or_else(|| app.theme.name.clone());
                app.set_theme(&want, false);
                v.previewing = Some((want, restore));
            }
        }
        // settling on another theme row: keep the one shown until this one counts
        Some(_) => ui.ticking = true,
        None => {
            if let Some((shown, restore)) = v.previewing.take()
                && app.theme.name == shown
            {
                app.set_theme(&restore, false);
            }
        }
    }
    v.sel = now_sel;
    v.query = now_query;
    VIEW.set(v);
    true
}

/// Target `n` was clicked: a row selects it (previewing a theme like the keys do); the selected row runs like
/// Enter.
pub fn click(app: &mut App, n: usize, _p: Point, _m: KeyModifiers, _count: u32) {
    let Some(item) = n.checked_sub(ROW) else { return };
    let Overlay::Palette(p) = &mut app.overlay else { return };
    let Some(pos) = p.matches().iter().position(|&i| i == item) else { return };
    if pos != p.sel {
        p.sel = pos;
        preview(app);
        return;
    }
    // exactly what Enter does (app/input.rs)
    let c = p.items[item].cmd.clone();
    let before = p.theme_before.clone();
    app.overlay = Overlay::None;
    if !matches!(c, Cmd::Theme(_)) && app.theme.name != before {
        app.set_theme(&before, false);
    }
    app.run_palette(c);
}

/// The wheel moves the selection (and previews a theme like the keys do).
pub fn wheel(app: &mut App, _n: Option<usize>, down: bool) {
    let Overlay::Palette(p) = &mut app.overlay else { return };
    let last = p.matches().len().saturating_sub(1);
    p.sel = if down { (p.sel + 1).min(last) } else { p.sel.saturating_sub(1) };
    preview(app);
}

/// The App's live preview after a move (app/input.rs): the selected theme, else the one it opened with.
fn preview(app: &mut App) {
    let Overlay::Palette(p) = &app.overlay else { return };
    let want = p.preview().unwrap_or_else(|| p.theme_before.clone());
    if want != app.theme.name {
        app.set_theme(&want, false);
    }
}

/// The query with its caret (kept in view), or the placeholder.
fn query(ui: &mut Ui, r: Rect, q: &str) {
    let t = ui.t;
    let st = Style::new(size::HEADING, weight::REGULAR, t.text);
    let caret_h = 20.0;
    let caret_y = r.y + (r.h - caret_h) / 2.0;
    ui.clip(r, 0.0);
    if q.is_empty() {
        let b = ui.line("type a command, a theme, a recent launch…", st.color(t.faint), Some(r.w - 4.0));
        ui.draw(&b, Point::new(r.x + 2.0, r.y + (r.h - b.h) / 2.0));
        widgets::draw_caret(ui, Point::new(r.x, caret_y), caret_h);
    } else {
        let b = ui.line(q, st, None);
        let end = b.layout.caret_x(q.len()) / ui.scale;
        let shift = (end + 4.0 - r.w).max(0.0);
        ui.draw(&b, Point::new(r.x - shift, r.y + (r.h - b.h) / 2.0));
        widgets::draw_caret(ui, Point::new(r.x - shift + end + 1.0, caret_y), caret_h);
    }
    ui.unclip();
}

/// One entry: its icon (a recent launch its agent's logo, a Claude preset Claude's), label (matched characters
/// highlighted, tags as pills), then its key, "recent" or the theme's swatch on the right.
#[allow(clippy::too_many_arguments)]
fn row(ui: &mut Ui, r: Rect, it: &Item, i: usize, selected: bool, query: &str, prefix: &str, recents: &[Recent]) {
    let t = ui.t;
    widgets::row(ui, r, ("palette", i), selected, Action::Overlay(ROW + i));
    let cy = r.y + r.h / 2.0;
    let ir = Rect::new(r.x + 6.0, r.y, 28.0, r.h);
    let logo = match &it.cmd {
        Cmd::Recent(n) => recents.get(*n).map(|rc| brand::harness(rc.harness)),
        Cmd::ClaudePreset(_) => Some(Mark::Logo(Logo::Claude)),
        _ => None,
    };
    match logo {
        Some(mark) => ui.mark(mark, ir, 16.0, None),
        None => ui.icon(ir, glyph(it.icon), 16.0, if selected { t.text } else { t.muted }),
    }
    let mut right = r.right() - 10.0;
    if let Some((bg, dots)) = it.swatch {
        right = swatch(ui, right, cy, bg, &dots) - 14.0;
    } else if matches!(it.cmd, Cmd::Recent(_)) {
        let pw = widgets::pill_width(ui, &it.hint);
        widgets::pill(ui, Point::new(right - pw, cy - 10.0), 20.0, &it.hint, t.muted, t.text.with_alpha(0.07));
        right -= pw + 14.0;
    } else if !it.hint.is_empty() {
        right = keys(ui, right, cy - 9.0, &it.hint, prefix) - 14.0;
    }
    label(ui, Rect::new(r.x + 40.0, r.y, (right - r.x - 40.0).max(0.0), r.h), it, query);
}

/// A label as the terminal UI words it, styled: what it does, its tags (light / dark, yours as quiet pills,
/// current as a check), then the part after " — " quieter.
fn label(ui: &mut Ui, r: Rect, it: &Item, query: &str) {
    let t = ui.t;
    let (head, tags, desc) = parts(it);
    let hits = matched(query, &it.label);
    let st = Style::new(size::BODY, weight::MEDIUM, t.text);
    let hit = st.color(t.accent).weight(weight::BOLD);
    let hb = ui.rich(&runs(&it.label, head, &hits, st, hit), size::BODY, Some(r.w.max(1.0)), false, Some(1));
    ui.draw(&hb, Point::new(r.x, r.y + (r.h - hb.h) / 2.0));
    let mut x = r.x + hb.w;
    let cy = r.y + r.h / 2.0;
    for tag in tags {
        if tag == "current" {
            let cw = current(ui, None);
            if x + 10.0 + cw > r.right() {
                return;
            }
            current(ui, Some(Point::new(x + 10.0, cy)));
            x += 10.0 + cw;
            continue;
        }
        let pw = widgets::pill_width(ui, tag);
        if x + 8.0 + pw > r.right() {
            return;
        }
        widgets::pill(ui, Point::new(x + 8.0, cy - 10.0), 20.0, tag, t.muted, t.text.with_alpha(0.07));
        x += 8.0 + pw;
    }
    if let Some(desc) = desc {
        let x = x + 6.0;
        if r.right() - x < 28.0 {
            return;
        }
        let st = Style::new(size::BODY, weight::REGULAR, t.muted);
        let mut spans = vec![("— ".to_string(), st)];
        spans.extend(runs(&it.label, desc, &hits, st, st.color(t.accent).weight(weight::SEMIBOLD)));
        let db = ui.rich(&spans, size::BODY, Some(r.right() - x), false, Some(1));
        ui.draw(&db, Point::new(x, r.y + (r.h - db.h) / 2.0));
    }
}

/// A label's head, its tags and what follows " — " (byte ranges into the label).
fn parts(it: &Item) -> (Range<usize>, Vec<&str>, Option<Range<usize>>) {
    let l = it.label.as_str();
    let (mut head, desc) = match l.find(" — ") {
        Some(at) => (at, Some(at + " — ".len()..l.len())),
        None => (l.len(), None),
    };
    let mut tags = vec![];
    if let Cmd::Theme(name) = &it.cmd {
        // "theme {name}[ · light| · dark][ · yours][ · current]"
        let base = "theme ".len() + name.len();
        if base <= head && l.is_char_boundary(base) && l.starts_with("theme ") && &l["theme ".len()..base] == name {
            tags = l[base..head].split(" · ").filter(|s| !s.is_empty()).collect();
            head = base;
        }
    } else if l[..head].ends_with(" · current") {
        head -= " · current".len();
        tags.push("current");
    }
    (0..head, tags, desc)
}

/// Where the filter matched the query in `label` (byte offsets): the same greedy, in-order, case-insensitive
/// walk as `fuzzy::score` (which runs over "{label} {hint}", so the label's part comes first).
fn matched(query: &str, label: &str) -> Vec<usize> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = vec![];
    let mut qi = 0;
    for (b, c) in label.char_indices() {
        if qi == q.len() {
            break;
        }
        if c.to_lowercase().next() == Some(q[qi]) {
            out.push(b);
            qi += 1;
        }
    }
    out
}

/// `label[range]` as styled runs: matched characters in `hit`, the rest in `st`.
fn runs(label: &str, range: Range<usize>, hits: &[usize], st: Style, hit: Style) -> Vec<(String, Style)> {
    let mut out: Vec<(String, Style)> = vec![];
    let mut cur = String::new();
    let mut on = false;
    for (b, c) in label[range.clone()].char_indices() {
        let h = hits.binary_search(&(range.start + b)).is_ok();
        if h != on && !cur.is_empty() {
            out.push((std::mem::take(&mut cur), if on { hit } else { st }));
        }
        on = h;
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push((cur, if on { hit } else { st }));
    }
    out
}

/// A chord ("alt+n", "ctrl+space S") as keycaps ending at `right`, a step of a sequence a little apart from the
/// next; returns where they start.
fn keys(ui: &mut Ui, right: f32, y: f32, chord: &str, prefix: &str) -> f32 {
    const STEP_GAP: f32 = 6.0;
    let steps: Vec<&str> = chord.split_whitespace().collect();
    let widths: Vec<f32> = steps.iter().map(|c| kbd_width(ui, c)).collect();
    let mut x = right - widths.iter().sum::<f32>() - STEP_GAP * steps.len().saturating_sub(1) as f32;
    let start = x;
    for (i, (c, w)) in steps.iter().zip(widths).enumerate() {
        cap(ui, Point::new(x, y), c, i == 0 && steps.len() > 1 && *c == prefix);
        x += w + STEP_GAP;
    }
    start
}

/// A theme's own colours (accent, shine, good, inline, danger) on its own background, ending at `right`;
/// returns where it starts.
fn swatch(ui: &mut Ui, right: f32, cy: f32, bg: TuiColor, dots: &[TuiColor; 5]) -> f32 {
    let t = ui.t;
    let r = Rect::new(right - 74.0, cy - 10.0, 74.0, 20.0);
    // a theme that leaves the background to the terminal gets the window's default dark one
    let base = Color::hex(0x0e1015);
    ui.panel(r, radius::CONTROL, kit::color(bg, base, &t.ansi), t.border);
    for (i, c) in dots.iter().enumerate() {
        let s = Rect::new(r.x + 9.0 + i as f32 * 12.0, cy - 4.0, 8.0, 8.0);
        ui.round(s, 2.0, kit::color(*c, t.text, &t.ansi));
    }
    r.x
}

/// The palette's icon names as Nerd Font glyphs.
fn glyph(name: &str) -> &'static str {
    match name {
        "rocket" => "\u{F0463}",
        "gauge" => "\u{F029A}",
        "account" => "\u{F0004}",
        "proxy" => "\u{F04E1}",
        "bridge" => "\u{F05A9}",
        "settings" => "\u{F0493}",
        "theme" => "\u{F03D8}",
        "help" => "\u{F02D7}",
        "quit" => "\u{F0206}",
        "close" => "\u{F0156}",
        "split" => "\u{F0BCC}",
        "history" => "\u{F02DA}",
        "key" => "\u{F0306}",
        "caveman" => "\u{F08B7}",
        "search" => "\u{F0349}",
        _ => "\u{F05AF}",
    }
}

// ------------------------------------------------------------------ shared with the keys overlay, the resume picker and the confirmation

/// How far a dialog has appeared, 0..1: the scrim fades in and the card lifts into place over ~140 ms after
/// `fresh`.
pub(super) fn appear(ui: &mut Ui, key: &str, fresh: bool) -> f32 {
    if fresh {
        ui.ease(("appear", key), 0.0, 0.0001);
    }
    ui.ease(("appear", key), 1.0, 140.0)
}

/// True on the first frame a dialog is drawn: last frame the pointer was over something on a lower layer (from
/// then on it's over the dialog's own scrim). Without the pointer in the window it can't tell, and says no.
pub(super) fn just_opened(ui: &Ui) -> bool {
    ui.pointer.is_some() && ui.top_layer < ui.scene.layer_index()
}

/// An eased scroll position for `key` heading to `target`; straight there when `snap`.
pub(super) fn glide(ui: &mut Ui, key: &str, target: f32, snap: bool) -> f32 {
    if snap {
        ui.ease(("glide", key), target, 0.0001);
    }
    ui.ease(("glide", key), target, 120.0)
}

/// A keycap's height (as `widgets::kbd` draws them).
pub(super) const CAP_H: f32 = 18.0;
/// Between the keycaps of one chord.
const CAP_GAP: f32 = 3.0;

/// The width `widgets::kbd` (and [`cap`]) draws `chord` at: a keycap per key, 3px apart.
pub(super) fn kbd_width(ui: &mut Ui, chord: &str) -> f32 {
    let st = Style::new(size::CAPTION, weight::MEDIUM, ui.t.muted);
    let mut w = 0.0;
    for (i, k) in widgets::chord_keys(chord).iter().enumerate() {
        if i > 0 {
            w += CAP_GAP;
        }
        w += (ui.measure(k, st) + 10.0).max(CAP_H);
    }
    w
}

/// A chord as keycaps at `at`; `quiet` draws the prefix of a two-key sequence ("ctrl+space" in "ctrl+space c")
/// as outlined keys so the key after it reads first. Returns its width.
pub(super) fn cap(ui: &mut Ui, at: Point, chord: &str, quiet: bool) -> f32 {
    if !quiet {
        return widgets::kbd(ui, at, chord);
    }
    let t = ui.t;
    let st = Style::new(size::CAPTION, weight::MEDIUM, t.faint);
    let mut x = at.x;
    for (i, k) in widgets::chord_keys(chord).iter().enumerate() {
        if i > 0 {
            x += CAP_GAP;
        }
        let b = ui.line(k, st, None);
        let w = (b.w + 10.0).max(CAP_H);
        let r = Rect::new(x, at.y, w, CAP_H);
        ui.outline(r, radius::CONTROL, 1.0, t.border);
        ui.draw(&b, Point::new(r.x + (w - b.w) / 2.0, r.y + (CAP_H - b.h) / 2.0 - 0.5));
        x += w;
    }
    x - at.x
}

/// "current", the way WinUI marks the chosen one: a check and the word in the accent, no badge. Drawn with its
/// left edge at `at.x`, centred on `at.y`, when given; returns its width.
pub(super) fn current(ui: &mut Ui, at: Option<Point>) -> f32 {
    let t = ui.t;
    let b = ui.line("current", Style::new(size::SMALL, weight::MEDIUM, t.accent), None);
    if let Some(p) = at {
        ui.icon(Rect::new(p.x, p.y - 8.0, 14.0, 16.0), "\u{F012C}", 14.0, t.accent);
        ui.draw(&b, Point::new(p.x + 17.0, p.y - b.h / 2.0));
    }
    17.0 + b.w
}

/// A hint's keys as keycaps (true) and the words between them: "j/k" is j / k, "↑↓" two arrows, "alt+n" alt n.
fn hint_keys(keys: &str) -> Vec<(String, bool)> {
    let mut out = vec![];
    for (i, alt) in keys.split('/').enumerate() {
        if i > 0 {
            out.push(("/".to_string(), false));
        }
        let arrows = alt.chars().count() > 1 && alt.chars().all(|c| "↑↓←→".contains(c));
        if arrows {
            out.extend(alt.chars().map(|c| (c.to_string(), true)));
        } else {
            out.extend(widgets::chord_keys(alt).into_iter().map(|k| (k, true)));
        }
    }
    out
}

/// Key hints like the terminal UI's ("⏎ resume · esc cancel"): each key a keycap, then what it does, quietly.
/// Keeps to `max_w` (hints that don't fit are left out); draws with the keycaps' top-left at `at` when given.
/// Returns the width.
pub(super) fn hints(ui: &mut Ui, at: Option<Point>, pairs: &[(&str, &str)], max_w: f32) -> f32 {
    let t = ui.t;
    let what = Style::new(size::SMALL, weight::REGULAR, t.muted);
    let sep = Style::new(size::SMALL, weight::REGULAR, t.faint);
    let mut end = 0.0f32;
    for (i, (keys, label)) in pairs.iter().enumerate() {
        let pieces = hint_keys(keys);
        let mut widths = Vec::with_capacity(pieces.len());
        for (text, key) in &pieces {
            widths.push(if *key { kbd_width(ui, text) } else { ui.measure(text, sep) + 6.0 });
        }
        let label_b = ui.line(label, what, None);
        let keys_w = widths.iter().sum::<f32>() + CAP_GAP * pieces.len().saturating_sub(1) as f32;
        let start = if i == 0 { 0.0 } else { end + 16.0 };
        let w = keys_w + 6.0 + label_b.w;
        if start + w > max_w {
            break;
        }
        if let Some(p) = at {
            let mut x = p.x + start;
            for ((text, key), pw) in pieces.iter().zip(&widths) {
                if *key {
                    widgets::kbd(ui, Point::new(x, p.y), text);
                } else {
                    let b = ui.line(text, sep, None);
                    ui.draw(&b, Point::new(x + 3.0, p.y + (CAP_H - b.h) / 2.0));
                }
                x += pw + CAP_GAP;
            }
            ui.draw(&label_b, Point::new(p.x + start + keys_w + 6.0, p.y + (CAP_H - label_b.h) / 2.0));
        }
        end = start + w;
    }
    end
}

/// A heading in sentence case ("resume in" → "Resume in"): the words as they are, the first letter capital.
pub(super) fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The band along a dialog's bottom where its buttons sit, as WinUI's dialogs have it: a step down from the
/// card with a hairline above, inside the card's border and corners.
pub(super) fn button_band(ui: &mut Ui, card: Rect, h: f32) -> Rect {
    let t = ui.t;
    let hair = ui.hair();
    let band = Rect::new(card.x + hair, card.bottom() - h, card.w - 2.0 * hair, h - hair);
    let fill = if t.light { t.bg } else { t.surface };
    ui.round_corners(band, bro_gfx::Corners::bottom(radius::OVERLAY - hair), fill);
    ui.hline(band.x, band.y, band.w, t.hairline);
    band
}
