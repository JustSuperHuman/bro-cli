//! The keys overlay (F1) in the desktop app: every binding of the live keymap by group (`help::lines`: sessions,
//! navigate, panes, views, app), then the terminal UI's cheat sheets, in two columns when there's room.
//!
//! Keys move `Help::scroll` through the workspace (j/k, pgdn/space, pgup, home/g; esc, F1, q, ?, ⏎ close); this
//! draws by that scroll (clamped here, as the terminal UI's drawing does), the wheel moves it, and the × or a
//! click outside closes it.

use super::super::actions::Action;
use super::super::kit::{Style, TextBox, Ui, radius, size, weight, widgets};
use super::palette::{CAP_H, appear, cap, glide, hints, just_opened, kbd_width, sentence};
use crate::app::{App, Overlay};
use crate::help::Line;
use crate::keymap::Group;
use bro_gfx::text::Align;
use bro_gfx::{Corners, Fill, Point, Quad, Rect};
use crossterm::event::KeyModifiers;
use std::cell::Cell;

const CLOSE: usize = 1;
/// One scroll step (a line of the terminal UI's help), logical px.
const STEP: f32 = 28.0;
const HEADER_H: f32 = 64.0;
const PAD_X: f32 = 28.0;
const COL_GAP: f32 = 44.0;
/// A section's heading, the space between sections, a row's padding and a line of key caps.
const SECTION_H: f32 = 30.0;
const SECTION_GAP: f32 = 22.0;
const ROW_PAD: f32 = 5.0;
const CAPS_LINE: f32 = 22.0;

thread_local! {
    /// The scroll shown last frame (big jumps skip the glide).
    static SHOWN: Cell<f32> = const { Cell::new(0.0) };
}

/// Draw the keys overlay over the window; register targets with `ui.hit(r, Action::Overlay(n))`. False = use the
/// terminal UI's drawing.
pub fn draw(app: &mut App, ui: &mut Ui, window: Rect, _time: f64) -> bool {
    if !matches!(app.overlay, Overlay::Help(_)) {
        return false;
    }
    let t = ui.t;
    let sections = sections(window_lines(&app.keymap));
    let prefix = app.keymap.prefix.display();
    let fresh = just_opened(ui);
    let k = appear(ui, "help", fresh);
    widgets::scrim(ui, window, k, Action::CloseOverlay);
    let w = (window.w - 80.0).min(1120.0).max((window.w - 16.0).min(360.0));
    let h = (window.h - 72.0).max((window.h - 16.0).min(260.0));
    let r = Rect::new((window.x + (window.w - w) / 2.0).round(), (window.y + (window.h - h) / 2.0).round() + (1.0 - k) * 10.0, w, h);
    widgets::card(ui, r, radius::OVERLAY);

    // header: what this is, the prefix, how to leave
    let mark = Rect::new(r.x + PAD_X - 2.0, r.y + (HEADER_H - 24.0) / 2.0, 24.0, 24.0);
    ui.icon(mark, "\u{F030C}", 20.0, t.text);
    let title = ui.label_in(Rect::new(mark.right() + 10.0, r.y, 240.0, HEADER_H), "bro · keys", Style::new(size::HEADING, weight::SEMIBOLD, t.text), Align::Left);
    let close = Rect::new(r.right() - 18.0 - 28.0, r.y + (HEADER_H - 28.0) / 2.0, 28.0, 28.0);
    if widgets::icon_button(ui, close, "\u{F0156}", t.muted, Action::Overlay(CLOSE)) {
        widgets::tooltip(ui, close, "esc close");
    }
    let kw = kbd_width(ui, &prefix);
    let kx = close.x - 18.0 - kw;
    let ky = r.y + (HEADER_H - CAP_H) / 2.0;
    widgets::kbd(ui, Point::new(kx, ky), &prefix);
    let pl = ui.line("prefix", Style::new(size::SMALL, weight::MEDIUM, t.muted), None);
    let px = kx - 8.0 - pl.w;
    ui.draw(&pl, Point::new(px, r.y + (HEADER_H - pl.h) / 2.0));
    let room = px - 28.0 - (mark.right() + 10.0 + title.w + 24.0);
    if room > 60.0 {
        let pairs = [("esc", "close"), ("j/k", "scroll")];
        let hw = hints(ui, None, &pairs, room);
        if hw > 0.0 {
            hints(ui, Some(Point::new(px - 28.0 - hw, ky)), &pairs, room);
        }
    }

    // the sheet: sections flowed into columns, scrolled by `Help::scroll`
    let view = Rect::new(r.x + 1.0, r.y + HEADER_H, r.w - 2.0, r.h - HEADER_H - 1.0);
    let inner_w = view.w - 2.0 * PAD_X;
    let cols = if inner_w >= 760.0 { 2 } else { 1 };
    let col_w = ((inner_w - COL_GAP * (cols - 1) as f32) / cols as f32).floor();
    let laid: Vec<Laid> = sections.iter().map(|s| lay_out(ui, s, col_w, &prefix)).collect();
    let mut col_h = vec![0.0f32; cols];
    let mut at = vec![];
    for s in &laid {
        // each section goes where the columns are shortest, so they end about level
        let c = (0..cols).min_by(|a, b| col_h[*a].total_cmp(&col_h[*b])).unwrap_or(0);
        at.push((c, col_h[c]));
        col_h[c] += s.h + SECTION_GAP;
    }
    let top_pad = 16.0;
    let content = col_h.iter().copied().fold(0.0, f32::max) - SECTION_GAP + top_pad + 24.0;
    let max = (content - view.h).max(0.0);
    let steps = (max / STEP).ceil() as usize;
    let target = if let Overlay::Help(help) = &mut app.overlay {
        help.scroll = help.scroll.min(steps);
        (help.scroll as f32 * STEP).min(max)
    } else {
        0.0
    };
    let jump = (SHOWN.get() - target).abs() > view.h;
    let shown = glide(ui, "help", target, fresh || jump);
    SHOWN.set(shown);

    let inner_r = radius::OVERLAY - 1.0;
    ui.clip(view, inner_r);
    for (s, &(c, y)) in laid.iter().zip(&at) {
        let x = view.x + PAD_X + c as f32 * (col_w + COL_GAP);
        let y = view.y + top_pad + y - shown;
        if y > view.bottom() || y + s.h < view.y {
            continue;
        }
        draw_section(ui, s, x, y, col_w, view);
    }
    // the edges of what's out of view fade away
    if shown < max - 0.5 {
        let fade = Rect::new(view.x, view.bottom() - 40.0, view.w, 40.0);
        let q = Quad::new(ui.px(fade), Fill::Vertical(t.raised.with_alpha(0.0), t.raised)).corners(Corners::bottom(inner_r * ui.scale));
        ui.scene.top_quad(q);
    }
    ui.unclip();
    if shown > 0.5 {
        ui.hline(r.x, view.y, r.w, t.hairline.fade((shown / 24.0).min(1.0)));
    }
    let vis = 0.45 + 0.55 * ui.hover_amount("help-sheet", view);
    widgets::scrollbar(ui, Rect::new(view.x, view.y + 4.0, view.w - 2.0, view.h - 8.0), content, view.h, shown, vis);
    true
}

/// Target `n` was clicked.
pub fn click(app: &mut App, n: usize, _p: Point, _m: KeyModifiers, _count: u32) {
    if n == CLOSE {
        app.overlay = Overlay::None;
    }
}

/// The wheel scrolls the sheet (three lines a notch; the drawing clamps it).
pub fn wheel(app: &mut App, _n: Option<usize>, down: bool) {
    if let Overlay::Help(h) = &mut app.overlay {
        h.scroll = if down { h.scroll + 3 } else { h.scroll.saturating_sub(3) };
    }
}

/// The heading of the window's own keys (font size, copy, full screen), after the terminal UI's sheets.
const WINDOW: &str = "window";
/// The window's own keys (`Gui::gui_key`), written as keymap chords (alternatives two spaces apart).
const WINDOW_KEYS: &[(&str, &str)] = &[
    ("ctrl+=  ctrl+-  ctrl+0", "terminal font bigger / smaller / default size"),
    ("ctrl+shift+c", "copy the selection (ctrl+c too while something is selected)"),
    ("f11", "full screen"),
];

/// The help as the window has it (DD-012): the terminal UI's lines from the live keymap (`help::lines`), with the
/// rows that describe the terminal rewritten for the window (the launcher's arrows and tab as they work now, the
/// mouse as the window takes it), then the window's own keys.
fn window_lines(km: &crate::keymap::Keymap) -> Vec<Line> {
    let key = |(k, d): &(&str, &str)| Line::Key(k.to_string(), d.to_string());
    let mut out = vec![];
    let mut head = String::new();
    for l in crate::help::lines(km) {
        match &l {
            Line::Head(h) => head = h.clone(),
            Line::Key(keys, _) => {
                if let Some(rows) = rewritten(&head, keys) {
                    out.extend(rows.iter().map(key));
                    continue;
                }
            }
            // what the window adds to a sheet goes at its end
            Line::Gap => out.extend(added(&head).iter().map(key)),
        }
        out.push(l);
    }
    out.push(Line::Head(WINDOW.to_string()));
    out.extend(WINDOW_KEYS.iter().map(key));
    out.push(Line::Gap);
    out
}

/// A cheat-sheet row that says something else in the window (`head` the sheet's heading, `keys` the row's keys).
fn rewritten(head: &str, keys: &str) -> Option<&'static [(&'static str, &'static str)]> {
    let sheet = head.split(' ').next().unwrap_or_default();
    Some(match (sheet, keys) {
        // ← → switch the agent; tab moves between the run-on list and the models (launcher::Launcher::key)
        ("launcher", "tab ← →") => &[("← →", "switch agent"), ("shift+← →", "on ★: what it runs on"), ("tab", "list ⇄ models")],
        ("launcher", "ctrl+e") => &[("ctrl+e", "permission: ask → auto → skip all")],
        // inside a terminal a double-click selects a word; the session's title bar is where it zooms
        ("mouse", "double-click a session") => &[("double-click a session's title", "full screen · again: back to the tiles")],
        ("mouse", "double-click edge") => &[("double-click the sidebar edge", "reset the sidebar's width")],
        _ => return None,
    })
}

/// Rows the window adds at the end of a cheat sheet: what its mouse does beyond the terminal UI's.
fn added(head: &str) -> &'static [(&'static str, &'static str)] {
    match head {
        "terminal" => &[("double-click / triple-click", "select a word / a line")],
        "mouse" => &[
            ("middle-click a session's title", "close the session"),
            ("back / forward buttons", "step back and forth through what you looked at"),
            ("drop a file", "paste its path where you type · pictures and videos also show beside your sessions"),
        ],
        _ => &[],
    }
}

/// One group of the help: its heading and (keys, what they do) rows. `chords`: the rows' keys come from the
/// keymap (alternatives two spaces apart, the prefix and its key one space apart), not a cheat sheet's words.
struct Section {
    head: String,
    rows: Vec<(String, String)>,
    chords: bool,
}

fn sections(lines: Vec<Line>) -> Vec<Section> {
    let mut v: Vec<Section> = vec![];
    for l in lines {
        match l {
            Line::Head(head) => {
                // the keymap's groups come first, then the cheat sheets; the window's own keys are chords too
                let chords = v.len() < Group::ALL.len() || head == WINDOW;
                v.push(Section { head, rows: vec![], chords });
            }
            Line::Key(keys, what) => {
                if let Some(s) = v.last_mut() {
                    s.rows.push((keys, what));
                }
            }
            Line::Gap => {}
        }
    }
    v
}

/// A piece of a row's keys.
#[derive(Clone)]
enum Tok {
    /// a key cap; true: the prefix of a sequence (drawn quieter so the key after it reads first)
    Cap(String, bool),
    /// words around keys ("click a link", "in Codex")
    Word(String),
    /// between alternatives
    Sep(&'static str),
}

/// The keys of a row as caps, words and separators.
fn tokens(keys: &str, chords: bool, prefix: &str) -> Vec<Tok> {
    let mut v = vec![];
    if chords {
        for (i, alt) in keys.split("  ").map(str::trim).filter(|s| !s.is_empty()).enumerate() {
            if i > 0 {
                v.push(Tok::Sep("/"));
            }
            let caps: Vec<&str> = alt.split(' ').filter(|s| !s.is_empty()).collect();
            let seq = caps.len() > 1;
            v.extend(caps.iter().enumerate().map(|(j, k)| Tok::Cap(k.to_string(), seq && j == 0 && *k == prefix)));
        }
        return v;
    }
    let words: Vec<&str> = keys.split(' ').filter(|s| !s.is_empty()).collect();
    for (j, w) in words.iter().copied().enumerate() {
        match w {
            "/" => v.push(Tok::Sep("/")),
            "·" => v.push(Tok::Sep("·")),
            w if is_key(w) => v.push(Tok::Cap(w.to_string(), w == "prefix" && words.get(j + 1).is_some_and(|n| is_key(n)))),
            w => match v.last_mut() {
                Some(Tok::Word(s)) => {
                    s.push(' ');
                    s.push_str(w);
                }
                _ => v.push(Tok::Word(w.to_string())),
            },
        }
    }
    v
}

/// A cheat sheet's word that is a key: a chord ("alt+r", "ctrl+click"), a key's name, an arrow, ⏎, a digit range
/// or a punctuation key ("prefix ,").
fn is_key(w: &str) -> bool {
    const NAMES: &[&str] = &["esc", "tab", "delete", "backspace", "enter", "space", "home", "end", "insert", "pgup", "pgdn", "prefix", "alt", "ctrl", "shift", "0–9"];
    let mut chars = w.chars();
    let one = matches!((chars.next(), chars.next()), (Some(_), None));
    (w.len() > 1 && w.contains('+')) || NAMES.contains(&w) || (one && "⏎↑↓←→,.;:?[]".contains(w))
}

/// A section laid out for a column `w` wide.
struct Laid {
    head: String,
    rows: Vec<RowLaid>,
    h: f32,
}

struct RowLaid {
    what: TextBox,
    /// the keys, line by line: (pieces with their widths, the line's width)
    caps: Vec<(Vec<(Tok, f32)>, f32)>,
    h: f32,
}

fn lay_out(ui: &mut Ui, s: &Section, w: f32, prefix: &str) -> Laid {
    let t = ui.t;
    let mut rows = vec![];
    let mut h = SECTION_H;
    for (keys, what) in &s.rows {
        let caps = caps_lines(ui, &tokens(keys, s.chords, prefix), (w * 0.58).floor());
        let caps_w = caps.iter().map(|l| l.1).fold(0.0, f32::max);
        let what_w = (w - caps_w - 18.0).max(w * 0.3);
        // (a paragraph needs its line limit: without one it's cut to a single line)
        let what = ui.para(what, Style::new(size::BODY, weight::REGULAR, t.text), what_w, Some(6));
        let rh = what.h.max(caps.len() as f32 * CAPS_LINE - 4.0) + 2.0 * ROW_PAD;
        h += rh;
        rows.push(RowLaid { what, caps, h: rh });
    }
    Laid { head: s.head.clone(), rows, h }
}

/// The room between two pieces of keys (a chord's own keycaps sit 3px apart; the steps of a sequence, like the
/// prefix and the key after it, a little further).
fn gap(a: &Tok, b: &Tok) -> f32 {
    match (a, b) {
        (Tok::Cap(..), Tok::Cap(..)) => 6.0,
        (Tok::Sep(_), _) | (_, Tok::Sep(_)) => 6.0,
        _ => 5.0,
    }
}

fn piece_style(ui: &Ui, tok: &Tok) -> Style {
    match tok {
        Tok::Sep(_) => Style::new(size::SMALL, weight::REGULAR, ui.t.faint),
        _ => Style::new(size::SMALL, weight::MEDIUM, ui.t.muted),
    }
}

/// A piece's text.
fn piece_text(tok: &Tok) -> &str {
    match tok {
        Tok::Cap(s, _) | Tok::Word(s) => s,
        Tok::Sep(s) => s,
    }
}

fn piece_width(ui: &mut Ui, tok: &Tok) -> f32 {
    match tok {
        Tok::Cap(c, _) => kbd_width(ui, c),
        _ => {
            let st = piece_style(ui, tok);
            ui.measure(piece_text(tok), st)
        }
    }
}

/// How wide a line of pieces is.
fn line_width(pieces: &[(Tok, f32)]) -> f32 {
    pieces.iter().map(|p| p.1).sum::<f32>() + pieces.windows(2).map(|p| gap(&p[0].0, &p[1].0)).sum::<f32>()
}

/// Keys flowed into lines at most `max` wide, breaking between alternatives (a separator at a break goes).
fn caps_lines(ui: &mut Ui, toks: &[Tok], max: f32) -> Vec<(Vec<(Tok, f32)>, f32)> {
    // the pieces between separators stay together; each remembers the separator before it
    let mut units: Vec<(Option<(Tok, f32)>, Vec<(Tok, f32)>)> = vec![(None, vec![])];
    for tok in toks {
        let piece = (tok.clone(), piece_width(ui, tok));
        if matches!(tok, Tok::Sep(_)) {
            units.push((Some(piece), vec![]));
        } else if let Some(u) = units.last_mut() {
            u.1.push(piece);
        }
    }
    let mut lines: Vec<Vec<(Tok, f32)>> = vec![];
    for (sep, unit) in units {
        if unit.is_empty() {
            continue;
        }
        if let Some(last) = lines.last_mut() {
            let mut joined = last.clone();
            joined.extend(sep);
            joined.extend(unit.iter().cloned());
            if line_width(&joined) <= max {
                *last = joined;
                continue;
            }
        }
        lines.push(unit);
    }
    lines
        .into_iter()
        .map(|l| {
            let w = line_width(&l);
            (l, w)
        })
        .collect()
}

fn draw_section(ui: &mut Ui, s: &Laid, x: f32, y: f32, w: f32, view: Rect) {
    let t = ui.t;
    widgets::section(ui, Rect::new(x, y, w, 20.0), &sentence(&s.head));
    let mut y = y + SECTION_H;
    for row in &s.rows {
        let rr = Rect::new(x - 8.0, y, w + 16.0, row.h);
        if rr.bottom() < view.y || rr.y > view.bottom() {
            y += row.h;
            continue;
        }
        if ui.hovered(rr) {
            ui.round(rr, radius::CONTROL, t.hover);
        }
        ui.draw(&row.what, Point::new(x, y + ROW_PAD));
        for (i, (pieces, lw)) in row.caps.iter().enumerate() {
            let mut cx = x + w - lw;
            let cy = y + ROW_PAD + i as f32 * CAPS_LINE + ((row.what.h.min(CAPS_LINE) - CAP_H) / 2.0).max(0.0);
            let mut prev: Option<&Tok> = None;
            for (tok, pw) in pieces {
                if let Some(p) = prev {
                    cx += gap(p, tok);
                }
                if let Tok::Cap(c, quiet) = tok {
                    cap(ui, Point::new(cx, cy), c, *quiet);
                } else {
                    let st = piece_style(ui, tok);
                    let b = ui.line(piece_text(tok), st, None);
                    ui.draw(&b, Point::new(cx, cy + (CAP_H - b.h) / 2.0));
                }
                cx += pw;
                prev = Some(tok);
            }
        }
        y += row.h;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(lines: &[Line]) -> Vec<(String, String, String)> {
        let mut head = String::new();
        let mut out = vec![];
        for l in lines {
            match l {
                Line::Head(h) => head = h.clone(),
                Line::Key(k, d) => out.push((head.clone(), k.clone(), d.clone())),
                Line::Gap => {}
            }
        }
        out
    }

    #[test]
    fn the_terminal_sheets_are_rewritten_for_the_window() {
        let lines = window_lines(&crate::keymap::Keymap::default());
        let rows = flat(&lines);
        let has = |sheet: &str, keys: &str| rows.iter().any(|(h, k, _)| h.starts_with(sheet) && k == keys);
        // every stale row the rewrite looks for still exists in the terminal UI's sheets, and is replaced
        for (sheet, keys) in [("launcher", "tab ← →"), ("launcher", "ctrl+e"), ("mouse", "double-click a session"), ("mouse", "double-click edge")] {
            assert!(flat(&crate::help::lines(&crate::keymap::Keymap::default())).iter().any(|(h, k, _)| h.starts_with(sheet) && k == keys), "{sheet}: {keys}");
            assert!(rewritten(sheet, keys).is_some());
            assert!(!has(sheet, keys) || keys == "ctrl+e", "{sheet}: {keys} is still there");
        }
        assert!(has("launcher", "← →") && has("launcher", "tab") && has("mouse", "double-click a session's title"));
        assert!(rows.iter().any(|(h, k, d)| h.starts_with("launcher") && k == "ctrl+e" && d.contains("ask → auto → skip all")));
        // the window's additions close their sheets, and the window's own keys come last, as chords
        assert!(has("terminal", "double-click / triple-click") && has("mouse", "back / forward buttons"));
        assert!(matches!(lines.last(), Some(Line::Gap)));
        let secs = sections(lines);
        let window = secs.last().unwrap();
        assert_eq!(window.head, WINDOW);
        assert!(window.chords && window.rows.iter().any(|(k, _)| k == "f11"));
        // the keymap's own groups are untouched
        assert!(secs.iter().take(Group::ALL.len()).all(|s| s.chords));
    }
}
