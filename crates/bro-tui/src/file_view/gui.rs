//! The file preview in the desktop app: a large card over the dimmed window. The header names the file (its
//! path in the session with the line, the language, size and full path) and where you are in it; the body is
//! the code in the terminal font with the theme's syntax colours, a line-number gutter and the target line
//! tinted; the footer's buttons are the terminal UI's hints. Keys reach [`FileView`] through the workspace;
//! this draws its state and turns clicks into the [`Outcome`]s the terminal UI's mouse handler gives (a click
//! outside the card closes it).

use super::{Body, Doc, FileView, Media, Outcome, Styled, size_text};
use crate::app::gui::actions::Action;
use crate::app::gui::kit::{self, Style, TextBox, Ui, radius, size, weight, widgets};
use crate::app::gui::surface::Cx;
use crate::theme::Theme;
use bro_gfx::text::{Align, Span, TextOpts};
use bro_gfx::{Color, Fill, MonoMetrics, Point, Quad, Rect};
use crossterm::event::KeyModifiers;
use ratatui::style::{Color as TuiColor, Modifier};
use std::cell::RefCell;
use unicode_width::UnicodeWidthChar;

// click targets
const SCRIM: usize = 0;
const EDITOR: usize = 1;
const SYSTEM: usize = 2;
const COPY: usize = 3;
const MENTION: usize = 4;
const CLOSE: usize = 5;

/// The card: the window less this on every side, at most `MAX_W` wide.
const MARGIN: f32 = 24.0;
const MAX_W: f32 = 1100.0;
const HEAD_H: f32 = 72.0;
const FOOT_H: f32 = 60.0;
/// Card padding left and right.
const PAD: f32 = 20.0;
/// Room above and below the lines, at least.
const BODY_PAD: f32 = 10.0;
/// Between the line numbers and the code.
const GUTTER_GAP: f32 = 18.0;
/// Code lines are this much taller than terminal rows.
const LINE_GAP: f32 = 1.12;
/// How long a scroll glides, and the card's open animation, in seconds.
const GLIDE: f32 = 0.14;
const OPEN: f64 = 0.16;

const ICON_CODE: &str = "\u{F022E}";
const ICON_DOC: &str = "\u{F09EE}";
const ICON_FILE: &str = "\u{F0224}";
const ICON_ALERT: &str = "\u{F0A4C}";
const ICON_IMAGE: &str = "\u{F02E9}";
const ICON_VIDEO: &str = "\u{F0567}";
const ICON_AUDIO: &str = "\u{F075A}";

/// Draw the file preview over the whole window.
pub(crate) fn draw(s: &mut FileView, ui: &mut Ui, area: Rect, _cx: &Cx) -> bool {
    paint(s, ui, area)
}

/// Target `n` was clicked: the same outcome as the terminal UI's hint (or its click outside the popup).
pub(crate) fn click(_s: &mut FileView, n: usize, _p: Point, _mods: KeyModifiers, _count: u32) -> Outcome {
    match n {
        SCRIM | CLOSE => Outcome::Close,
        EDITOR => Outcome::Editor,
        SYSTEM => Outcome::System,
        COPY => Outcome::Copy,
        MENTION => Outcome::Mention,
        _ => Outcome::None,
    }
}

/// The wheel scrolls three lines anywhere, as in the terminal UI.
pub(crate) fn wheel(s: &mut FileView, _n: Option<usize>, down: bool) {
    s.scroll(if down { 3 } else { -3 });
}

// ------------------------------------------------------------------ layout

/// Where the parts go (logical pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    card: Rect,
    head: Rect,
    /// between the header and the footer, the card's full width
    body: Rect,
    /// the lines: exactly `rows` of them, centred in the body
    code: Rect,
    foot: Rect,
    rows: usize,
}

/// The card over `window` with code lines `pitch` tall, `lift` pixels lower while it opens.
fn layout(window: Rect, pitch: f32, lift: f32) -> Layout {
    let w = (window.w - 2.0 * MARGIN).clamp(1.0, MAX_W).round();
    let h = (window.h - 2.0 * MARGIN).max(1.0).round();
    let card = Rect::new((window.x + (window.w - w) / 2.0).round(), (window.y + (window.h - h) / 2.0 + lift).round(), w, h);
    let head = Rect::new(card.x, card.y, card.w, HEAD_H.min(card.h));
    let foot_h = FOOT_H.min(card.h - head.h);
    let foot = Rect::new(card.x, card.bottom() - foot_h, card.w, foot_h);
    let body = Rect::new(card.x, head.bottom(), card.w, (foot.y - head.bottom()).max(0.0));
    let rows = ((body.h - 2.0 * BODY_PAD) / pitch.max(1.0)).floor().max(1.0) as usize;
    let code_h = rows as f32 * pitch;
    let code = Rect::new(body.x, body.y + ((body.h - code_h) / 2.0).max(0.0).round(), body.w, code_h);
    Layout { card, head, body, code, foot, rows }
}

/// The code's columns, in physical pixels: line numbers from `num_x`, text from `text_x` to `text_r`.
#[derive(Clone, Copy, Debug)]
struct Columns {
    digits: usize,
    num_x: f32,
    text_x: f32,
    text_r: f32,
    cols: usize,
}

fn columns(l: &Layout, scale: f32, cell_w: f32, total: usize) -> Columns {
    let digits = total.to_string().len().max(3);
    let num_x = ((l.body.x + PAD) * scale).round();
    let text_x = num_x + digits as f32 * cell_w + (GUTTER_GAP * scale).round();
    let text_r = ((l.body.right() - PAD) * scale).round().max(text_x + cell_w);
    let cols = ((text_r - text_x) / cell_w).floor().max(1.0) as usize;
    Columns { digits, num_x, text_x, text_r, cols }
}

// ------------------------------------------------------------------ motion

/// What moves between frames: the open animation and the scroll position drawn, which glides toward `top` and
/// `left` (in lines and columns).
#[derive(Default)]
struct Motion {
    /// the preview it belongs to (a new one starts over)
    id: u64,
    opened: f64,
    at: f64,
    moving: bool,
    text: bool,
    y: f32,
    x: f32,
}

thread_local! {
    static MOTION: RefCell<Motion> = RefCell::new(Motion::default());
}

/// The open animation (0 → 1) and the scroll position to draw. Jumps of more than a page and a half (g, G, the
/// first centring) don't glide.
fn motion(ui: &mut Ui, s: &FileView, rows: usize, cols: usize) -> (f32, f32, f32) {
    let id = kit::id((s as *const FileView as usize, s.pane, s.line, s.column, s.fallback.as_deref()));
    let text = matches!(s.body, Body::Text(_));
    let (top, left) = (s.top as f32, s.left as f32);
    let now = ui.time;
    let (open, y, x, moving) = MOTION.with(|cell| {
        let mut m = cell.borrow_mut();
        if m.id != id {
            *m = Motion { id, opened: now, at: now, moving: false, text, y: top, x: left };
        }
        // one frame's step after a pause, else the time since the last frame
        let dt = if m.moving { (now - m.at).clamp(0.0, 0.034) as f32 } else { 1.0 / 60.0 };
        m.at = now;
        let k = 1.0 - (-dt * 4.6 / GLIDE).exp();
        let snap = !text || !m.text;
        let glide = |pos: &mut f32, to: f32, page: f32| -> bool {
            if snap || (to - *pos).abs() > page {
                *pos = to;
                return false;
            }
            *pos += (to - *pos) * k;
            if (to - *pos).abs() < 0.01 {
                *pos = to;
                return false;
            }
            true
        };
        let gy = glide(&mut m.y, top, rows as f32 * 1.5);
        let gx = glide(&mut m.x, left, cols as f32);
        m.text = text;
        m.moving = gy || gx;
        let open = ((now - m.opened) / OPEN).clamp(0.0, 1.0) as f32;
        (1.0 - (1.0 - open).powi(3), m.y, m.x, m.moving)
    });
    if moving || open < 1.0 {
        ui.ticking = true;
    }
    (open, y, x)
}

// ------------------------------------------------------------------ drawing

fn paint(s: &mut FileView, ui: &mut Ui, area: Rect) -> bool {
    let Some(m) = ui.text.mono_metrics() else { return false };
    let t = ui.t;
    let pitch_px = (m.cell_h * LINE_GAP).round().max(1.0);
    let pitch = pitch_px / ui.scale;
    let rest = layout(area, pitch, 0.0);
    // paging, the page keys and the first centring need the rows before anything reads `top`
    s.set_rows(rest.rows);
    let total = match &s.body {
        Body::Text(d) => d.total.max(d.end()),
        _ => 1,
    };
    let cols = columns(&rest, ui.scale, m.cell_w, total).cols;
    let (open, y, x) = motion(ui, s, rest.rows, cols);
    let l = layout(area, pitch, ((1.0 - open) * 10.0).round());

    widgets::scrim(ui, area, open, Action::Overlay(SCRIM));
    widgets::card(ui, l.card, radius::OVERLAY);
    // the code sits in a well between the header and the footer
    let hair = ui.hair();
    let well = if t.light { t.sunken } else { t.surface };
    let inner = Rect::new(l.body.x + hair, l.body.y, l.body.w - 2.0 * hair, l.body.h);
    ui.fill(inner, well);
    ui.hline(inner.x, l.body.y, inner.w, t.hairline);
    ui.hline(inner.x, l.foot.y - hair, inner.w, t.hairline);

    header(ui, s, l.head);
    match &s.body {
        Body::Text(d) => code(ui, s, d, &l, &m, pitch_px, y, x, well),
        Body::Loading => loading(ui, l.body),
        Body::Failed { message, path } => failed(ui, l.body, message, path.is_some()),
        Body::Media(media) => media_card(ui, l.body, media),
    }
    footer(ui, s, l.foot);
    true
}

/// `text`, or its end after an ellipsis, in `max_w` (a path keeps its file name).
pub(crate) fn fit_tail(ui: &mut Ui, text: &str, st: Style, max_w: f32) -> String {
    if ui.measure(text, st) <= max_w {
        return text.to_string();
    }
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    // the fewest characters dropped from the front that fit
    let (mut lo, mut hi) = (1usize, starts.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if ui.measure(&format!("…{}", &text[starts[mid]..]), st) <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    match starts.get(lo) {
        Some(&i) => format!("…{}", &text[i..]),
        None => "…".into(),
    }
}

/// [`fit_tail`] for a path: a cut inside a folder's name drops the rest of it ("…/gui/files.rs").
pub(crate) fn fit_path(ui: &mut Ui, path: &str, st: Style, max_w: f32) -> String {
    let fitted = fit_tail(ui, path, st, max_w);
    match fitted.strip_prefix('…') {
        Some(rest) if !rest.starts_with(['/', '\\']) => match rest.find(['/', '\\']) {
            Some(i) => format!("…{}", &rest[i..]),
            None => fitted,
        },
        _ => fitted,
    }
}

/// A paragraph with every line centred, `max_w` wide: draw it at the centre less `max_w / 2`.
pub(crate) fn para_centered(ui: &mut Ui, text: &str, st: Style, max_w: f32, max_lines: usize) -> TextBox {
    let mut o = TextOpts::new(st.size * ui.scale);
    o.line_height = (st.size * 1.42 * ui.scale).round();
    o.max_width = Some((max_w * ui.scale).max(1.0));
    o.wrap = true;
    o.ellipsis = true;
    o.max_lines = Some(max_lines);
    o.align = Align::Center;
    let span = Span { text: text.to_string(), face: st.face, weight: st.weight, italic: st.italic, color: st.color, size: None };
    let layout = ui.text.layout(&[span], &o);
    TextBox { w: layout.width() / ui.scale, h: layout.height() / ui.scale, layout }
}

/// A key in a keycap, the kit's (`widgets::kbd`): 4px corners, a hairline and a lifted bottom edge, `fill` its
/// face (`t.control`, or a tint of the ink on a filled button). Returns its width.
pub(crate) fn keycap(ui: &mut Ui, at: Point, key: &str, ink: Color, fill: Color) -> f32 {
    let t = ui.t;
    let b = ui.line(key, Style::new(size::CAPTION, weight::MEDIUM, ink), None);
    let w = (b.w + 10.0).max(18.0).round();
    let r = Rect::new(at.x, at.y, w, 18.0);
    ui.panel(r, radius::CONTROL, fill, t.stroke);
    let edge = if t.light { Color::rgba(0.0, 0.0, 0.0, 0.18) } else { Color::rgba(0.0, 0.0, 0.0, 0.45) };
    ui.hline(r.x + 2.0, r.bottom() - ui.hair(), (r.w - 4.0).max(0.0), edge);
    ui.draw(&b, Point::new(r.x + ((w - b.w) / 2.0).round(), r.y + (18.0 - b.h) / 2.0 - 0.5));
    w
}

pub(crate) fn keycap_w(ui: &mut Ui, key: &str) -> f32 {
    (ui.measure(key, Style::new(size::CAPTION, weight::MEDIUM, ui.t.text)) + 10.0).max(18.0).round()
}

/// A short line led by its key (`⏎ opens it…`), centred on `cx`.
fn key_line(ui: &mut Ui, cx: f32, y: f32, text: &str) {
    let t = ui.t;
    let (key, rest) = text.split_once(' ').unwrap_or(("", text));
    let b = ui.line(rest, Style::new(size::SMALL, weight::MEDIUM, t.muted), None);
    let kw = if key.is_empty() { 0.0 } else { keycap_w(ui, key) + 8.0 };
    let x = (cx - (kw + b.w) / 2.0).round();
    if !key.is_empty() {
        keycap(ui, Point::new(x, y), key, t.muted, t.control);
    }
    ui.draw(&b, Point::new(x + kw, y + (18.0 - b.h) / 2.0));
}

fn header(ui: &mut Ui, s: &FileView, r: Rect) {
    let t = ui.t;
    // what kind of file, as a plain icon on the card (an alert in the danger colour when it can't be shown)
    let (glyph, tint) = match &s.body {
        Body::Text(d) if matches!(d.language.as_str(), "plaintext" | "markdown") => (ICON_DOC, t.text),
        Body::Text(_) => (ICON_CODE, t.text),
        Body::Media(m) => (media_kind(&m.mime).0, t.text),
        Body::Failed { .. } => (ICON_ALERT, t.danger),
        Body::Loading => (ICON_FILE, t.muted),
    };
    let mark = Rect::new(r.x + PAD - 2.0, r.y + ((r.h - 28.0) / 2.0).round(), 28.0, 28.0);
    ui.icon(mark, glyph, 22.0, tint);
    let cy = mark.y + mark.h / 2.0;
    let right = where_you_are(ui, s, r.right() - PAD, cy);

    // the name in the session and the line
    let x0 = mark.right() + 10.0;
    let room = (right - 24.0 - x0).max(40.0);
    let mono = Style::mono(size::TITLE, t.text);
    let suffix = match (s.line, &s.body) {
        (Some(l), Body::Text(_) | Body::Loading | Body::Failed { .. }) => format!(":{l}"),
        _ => String::new(),
    };
    let fitted = fit_path(ui, &s.title(), mono, room);
    let (base, suffix) = match fitted.strip_suffix(suffix.as_str()) {
        Some(b) if !suffix.is_empty() => (b.to_string(), suffix),
        _ => (fitted, String::new()),
    };
    let cut = base.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    let (dir, name) = base.split_at(cut);
    let spans: Vec<(String, Style)> = [(dir, mono.color(t.muted)), (name, mono.weight(weight::SEMIBOLD)), (suffix.as_str(), mono.color(t.muted))]
        .into_iter()
        .filter(|(text, _)| !text.is_empty())
        .map(|(text, st)| (text.to_string(), st))
        .collect();
    let title = ui.rich(&spans, size::TITLE, None, false, None);

    // what it is, where it lives
    let meta = match &s.body {
        Body::Text(d) => {
            let lang = if d.language == "plaintext" { "text".to_string() } else { d.language.clone() };
            format!("{lang} · {} line{} · {}", d.total, if d.total == 1 { "" } else { "s" }, size_text(d.bytes))
        }
        Body::Media(m) => format!("{} · {}", m.mime, size_text(m.bytes)),
        Body::Failed { .. } => String::new(),
        Body::Loading => "loading…".into(),
    };
    let full = s.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    // a file outside the session's folders is already named in full
    let shown = match &s.body {
        Body::Text(d) => d.relative.as_str(),
        Body::Media(m) => m.relative.as_str(),
        _ => "",
    };
    let full = if full.replace('\\', "/") == shown.replace('\\', "/") { String::new() } else { full };
    let meta_st = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let meta_b = (!meta.is_empty()).then(|| ui.line(&meta, meta_st, Some(room)));
    let line2_h = if meta_b.is_some() || !full.is_empty() { 16.0 } else { 0.0 };
    let top = (cy - (title.h + if line2_h > 0.0 { 3.0 + line2_h } else { 0.0 }) / 2.0).round();
    ui.draw(&title, Point::new(x0, top));
    if line2_h > 0.0 {
        let y = top + title.h + 3.0;
        let mut x = x0;
        if let Some(b) = &meta_b {
            ui.draw(b, Point::new(x, y + (line2_h - b.h) / 2.0));
            x += b.w + 14.0;
        }
        let path_room = x0 + room - x;
        if !full.is_empty() && path_room > 60.0 {
            let st = Style::mono(size::SMALL, t.faint);
            let p = fit_path(ui, &full, st, path_room);
            let b = ui.line(&p, st, None);
            ui.draw(&b, Point::new(x, y + (line2_h - b.h) / 2.0));
        }
    }
}

/// Right of the header: "top–last of total", "→N" when scrolled sideways, a spinner while a page loads.
/// Returns where it starts.
fn where_you_are(ui: &mut Ui, s: &FileView, right: f32, cy: f32) -> f32 {
    let t = ui.t;
    let mut x = right;
    let Body::Text(d) = &s.body else { return x };
    let last = (s.top + s.rows).saturating_sub(1).min(d.total.max(1));
    let st = Style::new(size::SMALL, weight::MEDIUM, t.muted);
    let spans = vec![(format!("{}–{}", s.top, last), st.color(t.text.mix(t.muted, 0.35))), (format!(" of {}", d.total), st)];
    let b = ui.rich(&spans, size::SMALL, None, false, None);
    x -= b.w;
    ui.draw(&b, Point::new(x, cy - b.h / 2.0));
    if s.left > 0 {
        let label = format!("→{}", s.left);
        x -= widgets::pill_width(ui, &label) + 10.0;
        widgets::pill(ui, Point::new(x, cy - 10.0), 20.0, &label, t.muted, t.text.with_alpha(0.07));
    }
    if s.loading {
        let b = ui.line("loading…", st, None);
        x -= b.w + 14.0;
        ui.draw(&b, Point::new(x, cy - b.h / 2.0));
        x -= 18.0;
        ui.spinner(Point::new(x + 7.0, cy), 5.0, 2.0, t.muted);
    }
    x
}

/// The lines in view: numbers, the target line tinted across the body, the code from column `x_pos`.
#[allow(clippy::too_many_arguments)]
fn code(ui: &mut Ui, s: &FileView, d: &Doc, l: &Layout, m: &MonoMetrics, pitch_px: f32, y_pos: f32, x_pos: f32, well: Color) {
    let t = ui.t;
    let sc = ui.scale;
    let total = d.total.max(d.end());
    let c = columns(l, sc, m.cell_w, total);
    let cw = m.cell_w;
    let target = s.line.map(|n| n as usize);
    let map = recolour(&s.theme, ui.theme);
    let band = t.accent.with_alpha(if t.light { 0.1 } else { 0.13 });
    let (bx, bw) = ((l.body.x * sc).round() + 1.0, (l.body.w * sc).round() - 2.0);
    let oy = (l.code.y * sc).round();
    let pad = ((pitch_px - m.cell_h) / 2.0).round();
    let first = y_pos.floor();
    let frac = y_pos - first;
    // (line, top in physical pixels) of every line at least partly in view
    let shown: Vec<(usize, f32)> = (0..=l.rows + 1)
        .filter_map(|i| {
            let n = first as i64 + i as i64;
            (n >= 1 && n as usize <= total).then(|| (n as usize, oy + ((i as f32 - frac) * pitch_px).round()))
        })
        .collect();

    ui.clip(l.code, 0.0);
    let mut buf = [0u8; 4];
    for &(n, ly) in &shown {
        let here = target == Some(n);
        if here {
            ui.scene.rect(Rect::new(bx, ly, bw, pitch_px), band);
        }
        let digits = n.to_string();
        let mut x = c.num_x + c.digits.saturating_sub(digits.len()) as f32 * cw;
        let ink = if here { t.accent } else { t.faint };
        for ch in digits.chars() {
            ui.text.draw_cell(ui.scene, ch.encode_utf8(&mut buf), here, false, x, ly + pad, 1, ink);
            x += cw;
        }
    }
    // the text, cut at its column's edges
    ui.clip(Rect::new(c.text_x / sc, l.code.y, (c.text_r - c.text_x) / sc, l.code.h), 0.0);
    let fade = (18.0 * sc).round();
    for &(n, ly) in &shown {
        if n < d.start || n > d.end() {
            ui.text.draw_cell(ui.scene, "…", false, false, c.text_x, ly + pad, 1, t.muted);
            continue;
        }
        let (before, after) = line(ui, &d.lines[n - d.start], &c, cw, x_pos, ly + pad, m, map.as_ref());
        let bg = if target == Some(n) { band.over(well) } else { well };
        if before {
            ui.scene.top_quad(Quad::new(Rect::new(c.text_x, ly, fade, pitch_px), Fill::Horizontal(bg, bg.with_alpha(0.0))));
        }
        if after {
            ui.scene.top_quad(Quad::new(Rect::new(c.text_r - fade, ly, fade, pitch_px), Fill::Horizontal(bg.with_alpha(0.0), bg)));
        }
    }
    ui.unclip();
    ui.unclip();
    let pitch = pitch_px / sc;
    let track = Rect::new(l.body.x, l.code.y, l.body.w - 3.0, l.code.h);
    let hot = ui.hover_amount("file-view-scroll", l.body);
    widgets::scrollbar(ui, track, total as f32 * pitch, l.rows as f32 * pitch, (y_pos - 1.0) * pitch, 0.6 + 0.4 * hot);
}

/// The seven theme colours the highlighter paints code with (`file_view::Palette`), in its order.
fn slots(t: &Theme) -> [TuiColor; 7] {
    [t.fg, t.muted, t.good, t.inline, t.accent, t.shine, t.danger]
}

/// The code was highlighted with the theme the preview opened with. After a theme change (a custom theme's file
/// edited, say) each of those colours maps to the live theme's, so the code follows the theme like the rest of
/// the card (the terminal UI keeps the old colours until the file is opened again). None while they're the same.
fn recolour(then: &Theme, now: &Theme) -> Option<[(TuiColor, TuiColor); 7]> {
    let (a, b) = (slots(then), slots(now));
    (a != b).then(|| std::array::from_fn(|i| (a[i], b[i])))
}

/// `c` in the live theme (see [`recolour`]).
fn live(c: TuiColor, map: Option<&[(TuiColor, TuiColor); 7]>) -> TuiColor {
    map.and_then(|m| m.iter().find(|(then, _)| *then == c)).map_or(c, |(_, now)| *now)
}

/// One line's cells from column `x_pos` on, at `y` (physical). Says whether text is cut off on the left and on
/// the right.
#[allow(clippy::too_many_arguments)]
fn line(ui: &mut Ui, styled: &Styled, c: &Columns, cw: f32, x_pos: f32, y: f32, m: &MonoMetrics, map: Option<&[(TuiColor, TuiColor); 7]>) -> (bool, bool) {
    let t = ui.t;
    let mut col = 0usize;
    let (mut before, mut after) = (false, false);
    for (st, text) in styled {
        let ink = kit::color(live(st.fg.unwrap_or(TuiColor::Reset), map), t.text, &t.ansi);
        let bold = st.add_modifier.contains(Modifier::BOLD);
        let italic = st.add_modifier.contains(Modifier::ITALIC);
        let under = st.add_modifier.contains(Modifier::UNDERLINED);
        // one cell (two for wide characters) at column `at`; false once past the right edge
        let mut put = |ui: &mut Ui, cluster: &str, at: usize, w: usize| -> bool {
            let x = (c.text_x + (at as f32 - x_pos) * cw).round();
            let wide = w as f32 * cw;
            if x >= c.text_r - 0.5 {
                after = true;
                return false;
            }
            before |= x < c.text_x - 0.5;
            after |= x + wide > c.text_r + 0.5;
            if x + wide > c.text_x + 0.5 {
                if cluster != " " {
                    ui.text.draw_cell(ui.scene, cluster, bold, italic, x, y, w as u8, ink);
                }
                if under {
                    ui.scene.top_quad(Quad::new(Rect::new(x, y + m.underline_y, wide, m.stroke), ink));
                }
            }
            true
        };
        // a character and the zero-width marks after it share a cell
        let mut cluster = String::new();
        let (mut at, mut w) = (0usize, 1usize);
        for ch in text.chars() {
            let width = ch.width().unwrap_or(0);
            if width == 0 {
                if !cluster.is_empty() {
                    cluster.push(ch);
                }
                continue;
            }
            if !cluster.is_empty() && !put(ui, &cluster, at, w) {
                return (before, after);
            }
            cluster.clear();
            cluster.push(ch);
            (at, w) = (col, width);
            col += width;
        }
        if !cluster.is_empty() && !put(ui, &cluster, at, w) {
            return (before, after);
        }
    }
    (before, after)
}

fn loading(ui: &mut Ui, r: Rect) {
    let t = ui.t;
    let c = Point::new(r.x + r.w / 2.0, r.y + r.h * 0.42);
    ui.spinner(Point::new(c.x, c.y - 12.0), 9.0, 2.8, t.muted);
    let b = ui.line("loading…", Style::new(size::BODY, weight::MEDIUM, t.muted), None);
    ui.draw(&b, Point::new((c.x - b.w / 2.0).round(), c.y + 12.0));
}

fn failed(ui: &mut Ui, r: Rect, message: &str, openable: bool) {
    let t = ui.t;
    let w = (r.w - 80.0).clamp(80.0, 560.0);
    let msg = para_centered(ui, message, Style::new(size::TITLE, weight::SEMIBOLD, t.text), w, 4);
    let h = 40.0 + 16.0 + msg.h + if openable { 14.0 + 18.0 } else { 0.0 };
    let mut y = (r.y + ((r.h - h) * 0.42).max(12.0)).round();
    let cx = r.x + r.w / 2.0;
    let mark = Rect::new((cx - 20.0).round(), y, 40.0, 40.0);
    ui.icon(mark, ICON_ALERT, 36.0, t.danger);
    y += 40.0 + 16.0;
    ui.draw(&msg, Point::new((cx - w / 2.0).round(), y));
    if openable {
        key_line(ui, cx, y + msg.h + 14.0, "⏎ opens it with the system's default app");
    }
}

/// Icon, word and how-to line for a media type.
fn media_kind(mime: &str) -> (&'static str, &'static str, &'static str) {
    match mime.split('/').next().unwrap_or_default() {
        "video" => (ICON_VIDEO, "video", "⏎ plays it in your default player · it streams on the phone and web too"),
        "audio" => (ICON_AUDIO, "audio", "⏎ plays it in your default player · it streams on the phone and web too"),
        _ => (ICON_IMAGE, "image", "⏎ opens it in your default viewer · the phone and web show it inline"),
    }
}

fn media_card(ui: &mut Ui, r: Rect, m: &Media) {
    let t = ui.t;
    let (glyph, what, how) = media_kind(&m.mime);
    let name = m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = ui.line(&name, Style::new(size::HEADING, weight::SEMIBOLD, t.text), Some((r.w - 80.0).max(80.0)));
    let detail = ui.line(&format!("{what} · {} · {}", m.mime, size_text(m.bytes)), Style::new(size::SMALL, weight::MEDIUM, t.muted), Some((r.w - 80.0).max(80.0)));
    let h = 56.0 + 16.0 + name.h + 4.0 + detail.h + 18.0 + 18.0;
    let mut y = (r.y + ((r.h - h) * 0.42).max(12.0)).round();
    let cx = r.x + r.w / 2.0;
    let mark = Rect::new((cx - 28.0).round(), y, 56.0, 56.0);
    ui.icon(mark, glyph, 52.0, t.muted);
    y += 56.0 + 16.0;
    ui.draw(&name, Point::new((cx - name.w / 2.0).round(), y));
    y += name.h + 4.0;
    ui.draw(&detail, Point::new((cx - detail.w / 2.0).round(), y));
    key_line(ui, cx, y + detail.h + 18.0, how);
}

// ------------------------------------------------------------------ footer

/// A footer button: its label, then its key.
#[derive(Clone, Copy)]
struct Button {
    key: &'static str,
    label: &'static str,
    target: usize,
}

const CLOSE_BUTTON: Button = Button { key: "esc", label: "close", target: CLOSE };

/// The terminal UI's hints for what's shown; the first is what Enter does.
fn buttons(body: &Body) -> &'static [Button] {
    const TEXT: &[Button] = &[
        Button { key: "e", label: "editor", target: EDITOR },
        Button { key: "o", label: "system app", target: SYSTEM },
        Button { key: "c", label: "copy path", target: COPY },
        Button { key: "@", label: "mention", target: MENTION },
    ];
    const MEDIA: &[Button] = &[Button { key: "⏎", label: "open", target: SYSTEM }, Button { key: "c", label: "copy path", target: COPY }, Button { key: "@", label: "mention", target: MENTION }];
    const FAILED: &[Button] = &[Button { key: "⏎", label: "system app", target: SYSTEM }, Button { key: "c", label: "copy path", target: COPY }];
    match body {
        Body::Text(_) => TEXT,
        Body::Media(_) => MEDIA,
        Body::Failed { path: Some(_), .. } => FAILED,
        _ => &[],
    }
}

/// The scroll keys (text only; a hint, not a button).
const SCROLL_KEYS: [&str; 3] = ["↑↓", "pgup", "pgdn"];

fn button_w(ui: &mut Ui, b: &Button, keys: bool) -> f32 {
    let label = ui.measure(b.label, Style::new(size::BODY, weight::MEDIUM, ui.t.text));
    let key = if keys { 8.0 + keycap_w(ui, b.key) } else { 0.0 };
    (14.0 + label + key + if keys { 9.0 } else { 14.0 }).round()
}

#[derive(Clone, Copy, PartialEq)]
enum Look {
    Primary,
    Secondary,
    Quiet,
}

/// A footer button in the kit's button looks (`widgets::button`): the accent fill, a standard control, or a
/// subtle one with no fill until hovered; its keycap after the label.
fn button(ui: &mut Ui, r: Rect, b: &Button, look: Look, keys: bool) {
    let hot = ui.hover_amount(("file-view-button", b.target), r);
    let down = ui.pressing && hot > 0.5;
    let t = ui.t;
    let (ink, cap_ink, cap_fill) = match look {
        Look::Primary => {
            ui.round(r, radius::CONTROL, if down { t.accent.fade(0.8) } else { t.accent.fade(1.0 - 0.1 * hot) });
            let inner = Rect::new(r.x + radius::CONTROL, r.y, (r.w - 2.0 * radius::CONTROL).max(0.0), r.h);
            ui.hline(inner.x, r.bottom() - ui.hair(), inner.w, Color::rgba(0.0, 0.0, 0.0, 0.18));
            (if down { t.on_accent.fade(0.8) } else { t.on_accent }, t.on_accent.with_alpha(0.8), t.on_accent.with_alpha(0.16))
        }
        Look::Secondary => {
            widgets::control(ui, r, if down { t.control_pressed } else { t.control.mix(t.control_hover, hot) });
            (if down { t.muted } else { t.text }, t.muted, t.control)
        }
        Look::Quiet => {
            if hot > 0.0 || down {
                ui.round(r, radius::CONTROL, if down { t.text.with_alpha(0.04) } else { t.text.with_alpha(0.06 * hot) });
            }
            (t.muted.mix(t.text, hot), t.muted, t.control)
        }
    };
    let label = ui.line(b.label, Style::new(size::BODY, weight::MEDIUM, ink), None);
    ui.draw(&label, Point::new(r.x + 14.0, r.y + (r.h - label.h) / 2.0));
    if keys {
        keycap(ui, Point::new(r.x + 14.0 + label.w + 8.0, r.y + ((r.h - 18.0) / 2.0).round()), b.key, cap_ink, cap_fill);
    }
    ui.hit(r, Action::Overlay(b.target));
}

/// Bottom-left the scroll keys; bottom-right close, then the actions with Enter's rightmost.
fn footer(ui: &mut Ui, s: &FileView, r: Rect) {
    let t = ui.t;
    let acts = buttons(&s.body);
    let avail = r.w - 2.0 * PAD;
    let hint_w = |ui: &mut Ui| -> f32 {
        let keys: f32 = SCROLL_KEYS.iter().map(|k| keycap_w(ui, k) + 4.0).sum();
        keys + 4.0 + ui.measure("scroll", Style::new(size::SMALL, weight::MEDIUM, t.faint)) + 24.0
    };
    let group_w = |ui: &mut Ui, n: usize, keys: bool| -> f32 {
        let mut w = button_w(ui, &CLOSE_BUTTON, keys) + 16.0;
        for b in &acts[..n] {
            w += button_w(ui, b, keys) + 8.0;
        }
        w
    };
    // too narrow: drop the scroll hint, then the keycaps, then the quieter actions
    let (mut hint, mut keys, mut n) = (matches!(s.body, Body::Text(_)), true, acts.len());
    loop {
        let need = group_w(ui, n, keys) + if hint { hint_w(ui) } else { 0.0 };
        if need <= avail {
            break;
        }
        if hint {
            hint = false;
        } else if keys {
            keys = false;
        } else if n > 1 {
            n -= 1;
        } else {
            break;
        }
    }
    let by = r.y + ((r.h - 32.0) / 2.0).round();
    if hint {
        let mut x = r.x + PAD;
        let ky = by + 7.0;
        for k in SCROLL_KEYS {
            x += keycap(ui, Point::new(x, ky), k, t.muted, t.control) + 4.0;
        }
        let b = ui.line("scroll", Style::new(size::SMALL, weight::MEDIUM, t.faint), None);
        ui.draw(&b, Point::new(x + 4.0, by + (32.0 - b.h) / 2.0));
    }
    let mut x = r.right() - PAD;
    for (i, b) in acts[..n].iter().enumerate() {
        let w = button_w(ui, b, keys);
        x -= w;
        button(ui, Rect::new(x, by, w, 32.0), b, if i == 0 { Look::Primary } else { Look::Secondary }, keys);
        x -= 8.0;
    }
    if n > 0 {
        x -= 8.0;
    }
    let w = button_w(ui, &CLOSE_BUTTON, keys);
    button(ui, Rect::new(x - w, by, w, 32.0), &CLOSE_BUTTON, Look::Quiet, keys);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::gui::kit::{Anims, Hit, Tokens};
    use crate::pane::Event;
    use bro_bridge::file_preview::ResolveRoots;
    use bro_gfx::{Scene, Text};
    use std::time::{Duration, Instant};

    thread_local! {
        /// Each frame drawn by a test is a second after the last one (animations settle as they would).
        static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    }

    /// Run `f` with a headless [`Ui`] over a `w`×`h` window (no GPU: layout, text and hits only).
    pub(crate) fn with_ui<R>(theme: &str, w: f32, h: f32, f: impl FnOnce(&mut Ui) -> R) -> (R, Vec<Hit>) {
        let time = CLOCK.with(|c| {
            c.set(c.get() + 1.0);
            c.get()
        });
        let theme = crate::theme::get(theme);
        let tokens = Tokens::from_theme(&theme);
        let mono = crate::fonts::get("jetbrains-mono").map(|f| f.data).unwrap_or(&[]);
        let mut text = Text::new(mono, crate::fonts::SYMBOLS.data, false);
        text.set_mono(14.0, 1.18);
        let mut scene = Scene::new();
        let mut hits = vec![];
        let mut anim = Anims::default();
        let mut ui = Ui {
            scene: &mut scene,
            text: &mut text,
            t: &tokens,
            theme: &theme,
            scale: 1.0,
            pointer: None,
            pressing: false,
            hits: &mut hits,
            top_layer: 0,
            anim: &mut anim,
            now: Instant::now(),
            time,
            ticking: false,
            screen: Rect::new(0.0, 0.0, w, h),
        };
        let r = f(&mut ui);
        (r, hits)
    }

    fn opened(dir: &std::path::Path, name: &str, line: Option<u32>) -> FileView {
        let (tx, rx) = std::sync::mpsc::channel();
        let roots = ResolveRoots { cwd: dir.to_path_buf(), project: None };
        let mut v = FileView::open(None, roots, name.into(), line, None, None, &crate::theme::get("graphite"), tx);
        let Ok(Event::FileView(l)) = rx.recv_timeout(Duration::from_secs(10)) else { panic!("loaded") };
        assert_eq!(v.apply(*l), Outcome::None);
        v
    }

    /// The overlay targets drawn, sorted.
    pub(crate) fn targets(hits: &[Hit]) -> Vec<usize> {
        let mut t: Vec<usize> = hits
            .iter()
            .filter_map(|h| match h.action {
                Action::Overlay(n) => Some(n),
                _ => None,
            })
            .collect();
        t.sort();
        t
    }

    #[test]
    fn the_card_fills_the_window_less_a_margin() {
        let l = layout(Rect::new(0.0, 0.0, 1440.0, 900.0), 18.0, 0.0);
        assert_eq!(l.card, Rect::new(170.0, 24.0, 1100.0, 852.0), "min(window − 48, 1100) × (window − 48), centred");
        assert_eq!((l.head.h, l.foot.h, l.body.y, l.foot.bottom()), (HEAD_H, FOOT_H, l.card.y + HEAD_H, l.card.bottom()));
        assert_eq!(l.rows, ((852.0 - HEAD_H - FOOT_H - 2.0 * BODY_PAD) / 18.0).floor() as usize);
        assert!(l.code.y >= l.body.y + BODY_PAD - 0.5 && l.code.bottom() <= l.foot.y - BODY_PAD + 0.5, "the lines sit inside the body");
        let small = layout(Rect::new(0.0, 0.0, 640.0, 300.0), 18.0, 0.0);
        assert_eq!((small.card.w, small.card.h), (592.0, 252.0));
        assert!(small.rows >= 1);
        // opening lifts the card, not the rows
        assert_eq!(layout(Rect::new(0.0, 0.0, 1440.0, 900.0), 18.0, 10.0).card.y, 34.0);
    }

    #[test]
    fn clicks_give_the_terminal_uis_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut v = opened(dir.path(), "a.txt", None);
        let p = Point::new(0.0, 0.0);
        let m = KeyModifiers::NONE;
        assert_eq!(click(&mut v, SCRIM, p, m, 1), Outcome::Close, "a click outside the card closes it");
        assert_eq!(click(&mut v, CLOSE, p, m, 1), Outcome::Close);
        assert_eq!(click(&mut v, EDITOR, p, m, 1), Outcome::Editor);
        assert_eq!(click(&mut v, SYSTEM, p, m, 1), Outcome::System);
        assert_eq!(click(&mut v, COPY, p, m, 1), Outcome::Copy);
        assert_eq!(click(&mut v, MENTION, p, m, 1), Outcome::Mention);
        assert_eq!(click(&mut v, 99, p, m, 1), Outcome::None);
    }

    /// Draw `v` over a `w`×`h` window; the targets it registered.
    fn drawn(v: &mut FileView, theme: &str, w: f32, h: f32) -> Vec<Hit> {
        with_ui(theme, w, h, |ui| {
            let r = ui.screen;
            assert!(paint(v, ui, r));
        })
        .1
    }

    #[test]
    fn draws_the_text_centred_on_its_line_with_its_buttons() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=400).map(|n| format!("let line_{n} = \"{n}\";\n")).collect();
        std::fs::write(dir.path().join("long.rs"), body).unwrap();
        let mut v = opened(dir.path(), "long.rs", Some(200));
        let hits = drawn(&mut v, "graphite", 1440.0, 900.0);
        let rows = v.rows;
        let pitch = with_ui("graphite", 100.0, 100.0, |ui| ui.text.mono_metrics().map(|m| (m.cell_h * LINE_GAP).round()).unwrap_or(18.0)).0;
        assert_eq!(rows, layout(Rect::new(0.0, 0.0, 1440.0, 900.0), pitch, 0.0).rows, "the page keys use the rows drawn");
        assert!(v.top <= 200 && 200 < v.top + rows, "the target line is in view: top {} rows {rows}", v.top);
        assert_eq!(v.top, 200 - rows / 3, "a third of the way down, as in the terminal UI");
        assert_eq!(targets(&hits), vec![SCRIM, EDITOR, SYSTEM, COPY, MENTION, CLOSE]);
        // Enter's action is the rightmost button, close the leftmost
        let x_of = |n: usize| hits.iter().find(|h| matches!(h.action, Action::Overlay(m) if m == n)).map(|h| h.rect.x).unwrap();
        assert!(x_of(EDITOR) > x_of(SYSTEM) && x_of(SYSTEM) > x_of(COPY) && x_of(COPY) > x_of(MENTION) && x_of(MENTION) > x_of(CLOSE));
        // the wheel scrolls three lines
        let top = v.top;
        wheel(&mut v, None, true);
        assert_eq!(v.top, top + 3);
        wheel(&mut v, Some(EDITOR), false);
        assert_eq!(v.top, top);
    }

    #[test]
    fn each_state_has_the_terminal_uis_hints() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let mut v = opened(dir.path(), "a.txt", None);
        v.body = Body::Failed { message: "This file can't be shown as text.".into(), path: Some(dir.path().join("a.txt")) };
        assert_eq!(targets(&drawn(&mut v, "paper", 1000.0, 700.0)), vec![SCRIM, SYSTEM, COPY, CLOSE]);
        v.body = Body::Failed { message: bro_bridge::file_preview::NOT_FOUND.into(), path: None };
        assert_eq!(targets(&drawn(&mut v, "paper", 1000.0, 700.0)), vec![SCRIM, CLOSE]);
        v.body = Body::Loading;
        assert_eq!(targets(&drawn(&mut v, "paper", 1000.0, 700.0)), vec![SCRIM, CLOSE]);
        v.body = Body::Media(Media { path: dir.path().join("clip.mp4"), relative: "clip.mp4".into(), mention: "@clip.mp4".into(), mime: "video/mp4".into(), bytes: 2048 });
        assert_eq!(targets(&drawn(&mut v, "paper", 1000.0, 700.0)), vec![SCRIM, SYSTEM, COPY, MENTION, CLOSE]);
    }

    #[test]
    fn code_takes_the_live_themes_colours() {
        let (then, now) = (crate::theme::get("graphite"), crate::theme::get("paper"));
        assert!(recolour(&then, &then).is_none(), "the same theme: nothing to map");
        let map = recolour(&then, &now).expect("graphite and paper differ");
        let (a, b) = (slots(&then), slots(&now));
        for c in a {
            // (two slots of one colour map like the first of them)
            let first = a.iter().position(|x| *x == c).unwrap();
            assert_eq!(live(c, Some(&map)), b[first], "{c:?}");
        }
        assert_eq!(live(then.accent, None), then.accent);
        assert_eq!(live(TuiColor::Rgb(1, 2, 3), Some(&map)), TuiColor::Rgb(1, 2, 3), "other colours stay");
        // a preview highlighted in graphite, drawn after a switch to paper, still draws
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "// hi\nfn main() {}\n").unwrap();
        let mut v = opened(dir.path(), "a.rs", Some(2));
        assert_eq!(targets(&drawn(&mut v, "paper", 1000.0, 700.0)), vec![SCRIM, EDITOR, SYSTEM, COPY, MENTION, CLOSE]);
    }

    #[test]
    fn narrow_windows_keep_close_and_enters_action() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
        let mut v = opened(dir.path(), "a.rs", None);
        let hits = drawn(&mut v, "graphite", 300.0, 400.0);
        let t = targets(&hits);
        assert!(t.contains(&CLOSE) && t.contains(&EDITOR), "{t:?}");
        let card = layout(Rect::new(0.0, 0.0, 300.0, 400.0), 18.0, 0.0).card;
        for h in hits.iter().filter(|h| matches!(h.action, Action::Overlay(n) if n != SCRIM)) {
            assert!(h.rect.x >= card.x && h.rect.right() <= card.right() + 0.5, "buttons stay on the card: {:?}", h.rect);
        }
    }

    /// `BRO_SHOTS=<folder> cargo test -p bro-tui --bin bro shots -- --ignored`: the preview and the agent files
    /// list in each of their states, drawn on the GPU into PNGs for looking at the design (needs a graphics
    /// adapter; does nothing without BRO_SHOTS).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn shots() {
        use crate::agent_files::{AgentFilesView, Item, Section};
        use std::path::{Path, PathBuf};
        use std::sync::mpsc::Receiver;
        use winit::application::ApplicationHandler;
        use winit::event::WindowEvent;
        use winit::event_loop::{ActiveEventLoop, EventLoop};
        use winit::platform::windows::EventLoopBuilderExtWindows;
        use winit::window::{Window, WindowId};

        let Some(dir) = std::env::var_os("BRO_SHOTS").map(PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).unwrap();
        type Draw = Box<dyn FnMut(&mut Ui, Rect)>;
        struct Shot {
            name: &'static str,
            theme: &'static str,
            size: (f32, f32),
            pointer: Option<Point>,
            /// physical pixels per logical one
            scale: f32,
            draw: Draw,
        }

        let src = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo = src.parent().and_then(Path::parent).unwrap_or(src).to_path_buf();
        // a preview loaded from disk; later pages land while it's drawn
        let preview = |root: &Path, target: &str, line: Option<u32>, theme: &str| -> (FileView, Receiver<Event>) {
            let (tx, rx) = std::sync::mpsc::channel();
            let roots = ResolveRoots { cwd: root.to_path_buf(), project: None };
            let mut v = FileView::open(Some(1), roots, target.into(), line, None, None, &crate::theme::get(theme), tx);
            if let Ok(Event::FileView(l)) = rx.recv_timeout(Duration::from_secs(10)) {
                v.apply(*l);
            }
            (v, rx)
        };
        let text = |name: &'static str, theme: &'static str, size: (f32, f32), target: &str, line: u32, left: usize, pointer: Option<Point>| -> Shot {
            let (mut v, rx) = preview(src, target, Some(line), theme);
            Shot {
                name,
                theme,
                size,
                pointer,
                scale: 1.0,
                draw: Box::new(move |ui, area| {
                    for _ in 0..20 {
                        while let Ok(Event::FileView(l)) = rx.try_recv() {
                            v.apply(*l);
                        }
                        if !v.loading {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    v.left = left;
                    paint(&mut v, ui, area);
                }),
            }
        };
        let state = |name: &'static str, theme: &'static str, body: fn(&Path) -> Body, line: Option<u32>| -> Shot {
            let (mut v, _rx) = preview(src, "src/file_view.rs", line, theme);
            v.body = body(src);
            v.loading = false;
            Shot { name, theme, size: (1440.0, 900.0), pointer: None, scale: 1.0, draw: Box::new(move |ui, area| { paint(&mut v, ui, area); }) }
        };
        let now = bro_core::util::now_ms();
        let it = |section: Section, kind: &'static str, shown: &str, line: Option<u32>, caption: Option<&str>, ago_s: i64| Item {
            section,
            path: PathBuf::from(shown),
            shown: shown.into(),
            line,
            kind,
            caption: caption.map(String::from),
            at_ms: Some(now - ago_s * 1000),
        };
        let files = vec![
            it(Section::Sent, "sent", "out/usage-chart.png", None, Some("weekly usage by profile, last 30 days"), 40),
            it(Section::Sent, "sent", "docs/gui/DESIGN.md", None, Some("the design rules I followed"), 380),
            it(Section::Touched, "edit", "crates/bro-tui/src/file_view/gui.rs", Some(214), None, 12),
            it(Section::Touched, "create", "crates/bro-tui/src/agent_files/gui.rs", None, None, 95),
            it(Section::Touched, "edit", "crates/bro-tui/src/agent_files.rs", Some(156), None, 130),
            it(Section::Touched, "write", "crates/bro-tui/tests/fixtures/preview.json", None, None, 300),
            it(Section::Touched, "move", "crates/bro-tui/src/app/gui/overlays/files.rs", None, None, 610),
            it(Section::Touched, "delete", "crates/bro-tui/src/app/gui/overlays/legacy_files.rs", None, None, 640),
            it(Section::Touched, "read", "crates/bro-tui/src/file_view.rs", Some(567), None, 900),
            it(Section::Touched, "read", "docs/gui/ARCHITECTURE.md", None, None, 1500),
            it(Section::Touched, "read", "C:/Users/james/AppData/Local/Temp/bro-demo/claude-transcript-2026-10-21.jsonl", Some(4096), None, 4000),
        ];
        let roots = ResolveRoots { cwd: repo.clone(), project: None };
        let list = |name: &'static str, theme: &'static str, size: (f32, f32), items: Vec<Item>, sel: usize, empty: &str, pointer: Option<Point>| -> Shot {
            let mut v = AgentFilesView::new(1, roots.clone(), "work · opus-5".into(), items, empty.into());
            v.sel = sel;
            Shot { name, theme, size, pointer, scale: 1.0, draw: Box::new(move |ui, area| { crate::agent_files::gui::paint(&mut v, ui, area); }) }
        };
        let long: Vec<Item> = (0..40).map(|i| it(Section::Touched, ["edit", "read", "create", "write"][i % 4], &format!("crates/bro-core/src/module_{i:02}.rs"), Some(10 + i as u32 * 7), None, 60 * i as i64)).collect();

        let mut shots = vec![
            text("file-text", "graphite", (1440.0, 900.0), "src/file_view.rs", 567, 0, None),
            text("file-sideways", "graphite", (1440.0, 900.0), "src/file_view.rs", 452, 24, Some(Point::new(1180.0, 846.0))),
            text("file-markdown", "graphite", (1440.0, 900.0), "../../README.md", 20, 0, None),
            text("file-light", "paper", (1440.0, 900.0), "src/file_view.rs", 567, 0, Some(Point::new(1000.0, 846.0))),
            text("file-small", "graphite", (860.0, 540.0), "src/file_view.rs", 120, 0, None),
            state("file-loading", "graphite", |_| Body::Loading, Some(42)),
            state("file-failed", "graphite", |src| Body::Failed { message: "This file can't be shown as text.".into(), path: Some(src.join("assets/bro.png")) }, None),
            state("file-missing", "graphite", |_| Body::Failed { message: bro_bridge::file_preview::NOT_FOUND.into(), path: None }, Some(12)),
            state("file-media", "graphite", |src| Body::Media(Media { path: src.join("demo/launch-walkthrough.mp4"), relative: "demo/launch-walkthrough.mp4".into(), mention: "@demo/launch-walkthrough.mp4".into(), mime: "video/mp4".into(), bytes: 18_874_368 }), None),
            list("agent-files", "graphite", (1440.0, 900.0), files.clone(), 2, "", Some(Point::new(700.0, 470.0))),
            list("agent-files-light", "paper", (1440.0, 900.0), files.clone(), 2, "", None),
            list("agent-files-long", "graphite", (1440.0, 900.0), long, 33, "", None),
            list("agent-files-empty", "graphite", (1440.0, 900.0), vec![], 0, "waiting for the agent's transcript (it appears once the agent has answered)", None),
            list("agent-files-narrow", "graphite", (560.0, 700.0), files.clone(), 0, "", None),
        ];
        // a 150% display: 960 × 600 logical pixels in the same 1440 × 900 window
        for mut s in [text("file-hidpi", "tokyo-night", (960.0, 600.0), "src/file_view.rs", 567, 0, None), list("agent-files-hidpi", "tokyo-night", (960.0, 600.0), files, 4, "", None)] {
            s.scale = 1.5;
            shots.push(s);
        }
        if let Ok(only) = std::env::var("BRO_SHOTS_ONLY") {
            shots.retain(|s| s.name.contains(only.as_str()));
        }

        struct Run {
            shots: Vec<Shot>,
            dir: PathBuf,
            error: Option<String>,
        }
        const W: u32 = 1440;
        const H: u32 = 900;
        /// Something to dim behind the card: a sidebar and two terminals.
        fn backdrop(ui: &mut Ui, area: Rect) {
            let t = ui.t;
            ui.fill(Rect::new(area.x, area.y, 272.0, area.h), t.bg.mix(t.text, 0.02));
            let w = (area.w - 296.0) / 2.0 - 6.0;
            for x in [284.0, 290.0 + w + 6.0] {
                let c = Rect::new(x, 12.0, w, area.h - 24.0);
                ui.panel(c, radius::OVERLAY, t.surface, t.hairline);
                for i in 0..((c.h - 60.0) / 22.0) as usize {
                    let b = ui.line(["PS ~/code/bro-cli-v2> cargo build -p bro-tui", "   Compiling bro-tui v2.0.26", "warning: unused variable: `layout`"][i % 3], Style::mono(13.0, t.muted), Some(c.w - 32.0));
                    ui.draw(&b, Point::new(c.x + 16.0, c.y + 48.0 + i as f32 * 22.0));
                }
            }
        }
        fn save(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
            let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().and_then(|mut w| w.write_image_data(rgba)).map_err(|e| e.to_string())
        }
        impl Run {
            fn shoot(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
                let attrs = Window::default_attributes().with_visible(false).with_inner_size(winit::dpi::PhysicalSize::new(W, H));
                let window = std::sync::Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
                let mut renderer = bro_gfx::Renderer::new(window.clone(), W, H, false)?;
                let mono = crate::fonts::get("jetbrains-mono").map(|f| f.data).unwrap_or(&[]);
                let mut text = Text::new(mono, crate::fonts::SYMBOLS.data, false);
                text.set_mono(14.0, 1.18);
                let mut scene = Scene::new();
                let mut anim = Anims::default();
                let mut time = 1000.0;
                for shot in &mut self.shots {
                    let theme = crate::theme::get(shot.theme);
                    let tokens = Tokens::from_theme(&theme);
                    let area = Rect::new(0.0, 0.0, shot.size.0, shot.size.1);
                    text.set_mono((14.0 * shot.scale).round(), 1.18);
                    // draw until nothing moves (spinners always do: give up after a while)
                    for pass in 0..40 {
                        time += 1.0;
                        scene.clear();
                        scene.background = tokens.bg;
                        text.begin_frame();
                        anim.moving = false;
                        let mut hits = vec![];
                        let mut ui = Ui {
                            scene: &mut scene,
                            text: &mut text,
                            t: &tokens,
                            theme: &theme,
                            scale: shot.scale,
                            pointer: shot.pointer,
                            pressing: false,
                            hits: &mut hits,
                            top_layer: 0,
                            anim: &mut anim,
                            now: Instant::now(),
                            time,
                            ticking: false,
                            screen: Rect::new(0.0, 0.0, W as f32 / shot.scale, H as f32 / shot.scale),
                        };
                        backdrop(&mut ui, area);
                        let prev = ui.layer();
                        (shot.draw)(&mut ui, area);
                        ui.set_layer(prev);
                        let (overflow, ticking) = (ui.text.overflow, ui.ticking || ui.anim.moving);
                        if overflow {
                            text.grow_atlases();
                            continue;
                        }
                        if pass >= 2 && !ticking {
                            break;
                        }
                    }
                    let (w, h, px) = renderer.capture(&scene, &mut text).ok_or("couldn't read the frame back")?;
                    save(&self.dir.join(format!("{}.png", shot.name)), w, h, &px)?;
                }
                Ok(())
            }
        }
        impl ApplicationHandler for Run {
            fn resumed(&mut self, el: &ActiveEventLoop) {
                if let Err(e) = self.shoot(el) {
                    self.error = Some(e);
                }
                el.exit();
            }
            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }
        let mut run = Run { shots, dir, error: None };
        let el = EventLoop::builder().with_any_thread(true).build().expect("event loop");
        el.run_app(&mut run).expect("ran");
        assert!(run.error.is_none(), "{:?}", run.error);
    }
}
